//! Event-driven macOS device watcher. Implements `DeviceWatcher` via IOKit USB
//! match/terminate notifications on a dedicated CFRunLoop thread, bridged into
//! the trait's mpsc channel. The daemon runtime selects this on macOS in place
//! of `PollingDeviceWatcher`.
//!
//! The CFRunLoop thread lives for the daemon's lifetime (the process exits on
//! shutdown, taking the thread with it). A USB attach fires before the volume
//! mounts, so the `Added` handler waits briefly for `scan_for_ipods` to see it.

use crate::daemon::device_registry::canonical_serial_key;
use crate::daemon::device_watcher::{diff_inventory, DeviceEvent, DeviceWatcher};
use crate::ipod::device::{self, DetectedIpod};
use crate::ipod::macos_iokit::{run_usb_notifications, UsbChange};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

pub struct IokitDeviceWatcher;

impl IokitDeviceWatcher {
    pub fn new_production() -> Self {
        Self
    }
}

fn replace_inventory(
    devices: &mut HashMap<String, DetectedIpod>,
    current: Vec<DetectedIpod>,
) -> Vec<DeviceEvent> {
    let events = diff_inventory(devices, current.clone());
    *devices = current.into_iter().fold(HashMap::new(), |mut inventory, ipod| {
        inventory.insert(canonical_serial_key(&ipod.serial), ipod);
        inventory
    });
    events
}

fn settle_added_serial(
    devices: &mut HashMap<String, DetectedIpod>,
    serial: &str,
    attempts: usize,
    scan: &mut dyn FnMut() -> Vec<DetectedIpod>,
    emit: &mut dyn FnMut(DeviceEvent) -> bool,
    wait: &mut dyn FnMut(),
) -> bool {
    let serial_key = canonical_serial_key(serial);
    if devices.contains_key(&serial_key) {
        return true;
    }
    for _ in 0..attempts {
        let events = replace_inventory(devices, scan());
        for event in events {
            if !emit(event) {
                return false;
            }
        }
        if devices.contains_key(&serial_key) {
            return true;
        }
        wait();
    }
    false
}

impl DeviceWatcher for IokitDeviceWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        let (tx, rx) = mpsc::channel::<DeviceEvent>(crate::daemon::DEVICE_EVENT_CHANNEL_CAPACITY);

        // Raw USB add/remove signals flow from the CFRunLoop thread to this
        // worker thread. The run-loop callbacks MUST stay fast (a channel
        // send), so the blocking volume-mount scan runs HERE, never on the run
        // loop — blocking the run loop starves every other IOKit notification
        // (that's what silently dropped plug-in and unplug events).
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<UsbChange>();

        std::thread::spawn(move || {
            let mut devices = HashMap::<String, DetectedIpod>::new();

            let rescan = |devices: &mut HashMap<String, DetectedIpod>| -> Option<bool> {
                let current = device::scan_for_ipods();
                let events = replace_inventory(devices, current);
                let changed = !events.is_empty();
                for event in events {
                    if tx.blocking_send(event).is_err() {
                        return None;
                    }
                }
                Some(changed)
            };

            // One-shot startup scan (NOT a poll — runs exactly once). The IOKit
            // matched-notification arming reports already-connected devices, but
            // a daemon restart while the iPod is plugged in is common enough
            // that we don't want detection to hinge on arming-iterator timing.
            if rescan(&mut devices).is_none() {
                return;
            }

            while let Ok(change) = raw_rx.recv() {
                tracing::debug!("device watcher: raw USB signal {change:?}");
                match change {
                    UsbChange::Added { serial } => {
                        // A USB attach precedes the volume mount, and an iPod
                        // Classic's spinning HDD can take 10s+ to spin up and
                        // mount after re-plug (at startup it's instant only
                        // because the volume was already mounted). Wait up to
                        // 30s for the mount — a bounded post-event settle, NOT
                        // steady-state polling. There's no second USB event to
                        // fall back on, so giving up early leaves it stuck.
                        if devices.contains_key(&canonical_serial_key(&serial)) {
                            tracing::debug!(
                                "device watcher: iPod {serial} already present; ignoring Added"
                            );
                            continue;
                        }
                        let mut scan = device::scan_for_ipods;
                        let mut send_failed = false;
                        let mut emit = |event| match tx.blocking_send(event) {
                            Ok(()) => true,
                            Err(_) => {
                                send_failed = true;
                                false
                            }
                        };
                        let mut wait = || std::thread::sleep(Duration::from_millis(250));
                        let found = settle_added_serial(
                            &mut devices,
                            &serial,
                            120,
                            &mut scan,
                            &mut emit,
                            &mut wait,
                        );
                        if send_failed {
                            return;
                        }
                        if !found {
                            tracing::info!(
                                "device watcher: Added signal but no iPod volume mounted within 30s"
                            );
                        }
                    }
                    UsbChange::Removed => {
                        // A per-device termination can be an unrelated Apple
                        // USB device. Rescanning the full inventory means that
                        // only identities actually absent from the collection
                        // emit a disconnect.
                        let mut changed = false;
                        for _ in 0..30 {
                            let before = devices.clone();
                            match rescan(&mut devices) {
                                Some(_) if before != devices => {
                                    changed = true;
                                    break;
                                }
                                Some(_) => {}
                                None => return,
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        if !changed {
                            tracing::debug!(
                                "device watcher: USB removal did not change iPod inventory; ignoring"
                            );
                        }
                    }
                }
            }
        });

        // CFRunLoop thread: fast callbacks that just forward the raw signal to
        // the worker above. Never blocks the run loop.
        std::thread::spawn(move || {
            run_usb_notifications(Box::new(move |change| {
                let _ = raw_tx.send(change);
            }));
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
            model_label: "iPod Classic".to_string(),
            drive: format!("/Volumes/{serial}"),
            name: None,
            volume_guid: None,
        }
    }

    fn inventory(ipods: Vec<DetectedIpod>) -> HashMap<String, DetectedIpod> {
        ipods
            .into_iter()
            .map(|ipod| (canonical_serial_key(&ipod.serial), ipod))
            .collect()
    }

    #[test]
    fn added_for_absent_serial_waits_until_that_ipod_appears() {
        let mut devices = inventory(vec![ipod("0xB")]);
        let mut scans = vec![vec![ipod("0xB")], vec![ipod("0xA"), ipod("0xB")]];
        let mut emitted = Vec::new();
        let mut waits = 0;
        let mut scan = || scans.remove(0);
        let mut emit = |event| {
            emitted.push(event);
            true
        };
        let mut wait = || waits += 1;

        let settled = settle_added_serial(
            &mut devices,
            "0xA",
            2,
            &mut scan,
            &mut emit,
            &mut wait,
        );

        assert!(settled);
        assert_eq!(waits, 1, "unchanged B must not settle A's Added signal");
        assert_eq!(emitted, vec![DeviceEvent::Connected(ipod("0xA"))]);
    }

    #[test]
    fn added_for_known_serial_finishes_without_scanning_or_waiting() {
        let mut devices = inventory(vec![ipod("0xB")]);
        let mut scans = 0;
        let mut emitted = Vec::new();
        let mut waits = 0;
        let mut scan = || {
            scans += 1;
            vec![ipod("0xB")]
        };
        let mut emit = |event| {
            emitted.push(event);
            true
        };
        let mut wait = || waits += 1;

        let settled = settle_added_serial(
            &mut devices,
            "0xB",
            120,
            &mut scan,
            &mut emit,
            &mut wait,
        );

        assert!(settled);
        assert_eq!(scans, 0);
        assert_eq!(waits, 0);
        assert!(emitted.is_empty());
    }
}
