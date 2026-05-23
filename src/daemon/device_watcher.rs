//! Device-watcher abstraction. `DeviceWatcher` is the trait the daemon
//! runtime listens on for iPod plug-in / plug-out events. Production
//! impl: `PollingDeviceWatcher` (1.5s scan loop reusing
//! `ipod::device::scan_for_ipod`). The trait exists so M5 polish can
//! swap in a Windows-event-driven impl without touching the runtime.
//!
//! `Debouncer` coalesces multiple Connected events for the same serial
//! inside a 500ms window (Windows fires arrival notifications several
//! times during enumeration / drive-letter assignment). Disconnects
//! pass straight through.

use crate::ipod::device::DetectedIpod;
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
/// and returns a stream of events. Closing the receiver should stop
/// the watcher (impl decides how).
pub trait DeviceWatcher: Send + 'static {
    fn start(self) -> mpsc::Receiver<DeviceEvent>;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ipod(serial: &str) -> DetectedIpod {
        DetectedIpod {
            serial: serial.to_string(),
            model_label: "iPod 7G".to_string(),
            drive: "G:\\".to_string(),
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
}
