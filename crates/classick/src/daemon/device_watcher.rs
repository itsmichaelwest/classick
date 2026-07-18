//! Device-watcher abstraction. `DeviceWatcher` is the trait the daemon
//! runtime listens on for iPod plug-in / plug-out events. Production
//! impl: `PollingDeviceWatcher` (1.5s scan loop reusing
//! `ipod::device::scan_for_ipods`). The trait exists so M5 polish can
//! swap in a Windows-event-driven impl without touching the runtime.
//!
//! `Debouncer` coalesces multiple Connected events for the same serial
//! inside a 500ms window (Windows fires arrival notifications several
//! times during enumeration / drive-letter assignment). Disconnects
//! pass straight through.

use crate::ipod::device::DetectedIpod;
use crate::daemon::device_registry::canonical_serial_key;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// One observation from a `DeviceWatcher` impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Connected(DetectedIpod),
    Disconnected { serial: String },
}

/// Production-trait for device watchers. `start` consumes the watcher
/// (via `Box<Self>` so the trait is object-safe) and returns a stream
/// of events. Closing the receiver should stop the watcher (impl
/// decides how).
pub trait DeviceWatcher: Send + 'static {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent>;
}

/// Wraps a `DeviceEvent` stream and suppresses duplicate Connected
/// events for the same serial inside `window`. The first event wins;
/// subsequent ones inside the window are dropped silently.
pub struct Debouncer {
    window: Duration,
    last_seen: HashMap<String, Instant>,
}

impl Debouncer {
    pub fn new(window: Duration) -> Self {
        Self { window, last_seen: HashMap::new() }
    }

    /// Returns `Some(event)` if the event should be propagated, `None`
    /// if it should be dropped as a duplicate.
    pub fn admit(&mut self, event: DeviceEvent) -> Option<DeviceEvent> {
        match &event {
            DeviceEvent::Connected(ipod) => {
                let now = Instant::now();
                if let Some(prev) = self.last_seen.get(&ipod.serial) {
                    if now.duration_since(*prev) < self.window {
                        return None;
                    }
                }
                self.last_seen.insert(ipod.serial.clone(), now);
                Some(event)
            }
            DeviceEvent::Disconnected { serial } => {
                self.last_seen.remove(serial);
                Some(event)
            }
        }
    }
}

type ScanFn = Box<dyn FnMut() -> Vec<DetectedIpod> + Send>;

pub(crate) fn diff_inventory(
    previous: &HashMap<String, DetectedIpod>,
    current: Vec<DetectedIpod>,
) -> Vec<DeviceEvent> {
    let previous = previous
        .values()
        .cloned()
        .fold(HashMap::new(), |mut inventory, ipod| {
            inventory.insert(canonical_serial_key(&ipod.serial), ipod);
            inventory
        });
    let current = current.into_iter().fold(HashMap::new(), |mut inventory, ipod| {
        inventory.insert(canonical_serial_key(&ipod.serial), ipod);
        inventory
    });

    let mut removed: Vec<_> = previous
        .iter()
        .filter(|(key, _)| !current.contains_key(*key))
        .map(|(key, ipod)| (key.clone(), ipod.serial.clone()))
        .collect();
    removed.sort_by(|left, right| left.0.cmp(&right.0));

    let mut observed: Vec<_> = current
        .iter()
        .filter(|(key, ipod)| previous.get(*key) != Some(*ipod))
        .collect();
    observed.sort_by(|left, right| left.0.cmp(right.0));

    removed
        .into_iter()
        .map(|(_, serial)| DeviceEvent::Disconnected { serial })
        .chain(
            observed
                .into_iter()
                .map(|(_, ipod)| DeviceEvent::Connected(ipod.clone())),
        )
        .collect()
}

fn inventory(current: Vec<DetectedIpod>) -> HashMap<String, DetectedIpod> {
    current.into_iter().fold(HashMap::new(), |mut inventory, ipod| {
        inventory.insert(canonical_serial_key(&ipod.serial), ipod);
        inventory
    })
}

/// Periodically polls a scan function and emits Connected /
/// Disconnected events. Production wiring uses
/// `ipod::device::scan_for_ipods`; tests inject a scripted closure.
pub struct PollingDeviceWatcher {
    scan: ScanFn,
    interval: Duration,
}

impl PollingDeviceWatcher {
    /// Production constructor: scans every 1.5s using the real volume
    /// enumeration.
    pub fn new_production() -> Self {
        let scan: ScanFn = Box::new(crate::ipod::device::scan_for_ipods);

        Self {
            scan,
            interval: crate::daemon::DEVICE_POLL_INTERVAL,
        }
    }

    #[cfg(test)]
    pub fn new_for_test(scan: ScanFn, interval: Duration) -> Self {
        Self { scan, interval }
    }
}

impl DeviceWatcher for PollingDeviceWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        let (tx, rx) = mpsc::channel::<DeviceEvent>(crate::daemon::DEVICE_EVENT_CHANNEL_CAPACITY);
        let mut me = *self;
        tokio::spawn(async move {
            let mut last = HashMap::new();
            let mut ticker = tokio::time::interval(me.interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let current = (me.scan)();
                let events = diff_inventory(&last, current.clone());
                last = inventory(current);
                for event in events {
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipod(serial: &str) -> DetectedIpod {
        DetectedIpod {
            serial: serial.to_string(),
            model_label: "iPod 7G".to_string(),
            drive: "G:\\".to_string(),
            name: None,
            volume_guid: None,
        }
    }

    #[test]
    fn debouncer_admits_first_connected_event() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let admitted = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(admitted.is_some());
    }

    #[test]
    fn debouncer_drops_duplicate_connected_within_window() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let dup = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(dup.is_none(), "duplicate Connected inside window must be dropped");
    }

    #[test]
    fn debouncer_admits_different_serial_immediately() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let other = d.admit(DeviceEvent::Connected(ipod("0xDEF")));
        assert!(other.is_some(), "different serial must not be debounced");
    }

    #[test]
    fn debouncer_admits_connected_after_window_elapses() {
        let mut d = Debouncer::new(Duration::from_millis(10));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        std::thread::sleep(Duration::from_millis(25));
        let again = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(again.is_some(), "after window, same serial must be admitted again");
    }

    #[test]
    fn debouncer_always_passes_disconnect() {
        let mut d = Debouncer::new(Duration::from_millis(500));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let disc = d.admit(DeviceEvent::Disconnected { serial: "0xABC".to_string() });
        assert!(disc.is_some(), "Disconnect events must never be debounced");
    }

    #[test]
    fn debouncer_disconnect_clears_state_so_reconnect_admits() {
        let mut d = Debouncer::new(Duration::from_secs(60));
        let _ = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        let _ = d.admit(DeviceEvent::Disconnected { serial: "0xABC".to_string() });
        let reconnect = d.admit(DeviceEvent::Connected(ipod("0xABC")));
        assert!(reconnect.is_some(), "after Disconnect, reconnect must admit even within window");
    }

    use crate::ipod::device::DetectedIpod;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Closure-driven scan func, so tests can step through observations.
    fn scripted_scanner(observations: Vec<Vec<DetectedIpod>>) -> impl FnMut() -> Vec<DetectedIpod> {
        let queue = Arc::new(Mutex::new(observations));
        move || {
            let mut q = queue.lock().unwrap();
            if q.is_empty() { Vec::new() } else { q.remove(0) }
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn polling_emits_connected_on_first_appearance() {
        let scanner = scripted_scanner(vec![
            vec![ipod("0xABC")],  // First poll
        ]);
        let watcher = PollingDeviceWatcher::new_for_test(
            Box::new(scanner),
            Duration::from_millis(100),
        );
        let mut rx = Box::new(watcher).start();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let event = rx.recv().await.expect("event");
        match event {
            DeviceEvent::Connected(d) => assert_eq!(d.serial, "0xABC"),
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn polling_emits_disconnected_when_device_disappears() {
        let scanner = scripted_scanner(vec![
            vec![ipod("0xABC")],
            vec![ipod("0xABC")],
            vec![],
        ]);
        let watcher = PollingDeviceWatcher::new_for_test(
            Box::new(scanner),
            Duration::from_millis(100),
        );
        let mut rx = Box::new(watcher).start();
        // Drain Connected
        tokio::time::sleep(Duration::from_millis(150)).await;
        let first = rx.recv().await.unwrap();
        assert!(matches!(first, DeviceEvent::Connected(_)));
        // Advance until disconnect.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let disc = rx.recv().await.unwrap();
        match disc {
            DeviceEvent::Disconnected { serial } => assert_eq!(serial, "0xABC"),
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    fn inventory(ipods: Vec<DetectedIpod>) -> HashMap<String, DetectedIpod> {
        ipods
            .into_iter()
            .map(|ipod| (ipod.serial.clone(), ipod))
            .collect()
    }

    #[test]
    fn diff_inventory_connects_every_initial_device() {
        let events = diff_inventory(&HashMap::new(), vec![ipod("0xA"), ipod("0xB")]);

        assert_eq!(
            events,
            vec![DeviceEvent::Connected(ipod("0xA")), DeviceEvent::Connected(ipod("0xB"))]
        );
    }

    #[test]
    fn diff_inventory_disconnects_only_the_removed_device() {
        let previous = inventory(vec![ipod("0xA"), ipod("0xB")]);

        let events = diff_inventory(&previous, vec![ipod("0xB")]);

        assert_eq!(events, vec![DeviceEvent::Disconnected { serial: "0xA".to_string() }]);
    }

    #[test]
    fn diff_inventory_reconnects_a_device_when_its_metadata_changes() {
        let previous = inventory(vec![ipod("0xA")]);
        let mut updated = ipod("0xA");
        updated.drive = "H:\\".to_string();

        let events = diff_inventory(&previous, vec![updated.clone()]);

        assert_eq!(events, vec![DeviceEvent::Connected(updated)]);
    }

    #[test]
    fn diff_inventory_ignores_an_unrelated_usb_removal() {
        let previous = inventory(vec![ipod("0xA"), ipod("0xB")]);

        let events = diff_inventory(&previous, vec![ipod("0xA"), ipod("0xB")]);

        assert!(events.is_empty());
    }
}
