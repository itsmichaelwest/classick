//! Event-driven macOS device watcher. Implements `DeviceWatcher` via IOKit USB
//! match/terminate notifications on a dedicated CFRunLoop thread, bridged into
//! the trait's mpsc channel. The daemon runtime selects this on macOS in place
//! of `PollingDeviceWatcher`.
//!
//! The CFRunLoop thread lives for the daemon's lifetime (the process exits on
//! shutdown, taking the thread with it). A USB attach fires before the volume
//! mounts, so the `Added` handler waits briefly for `scan_for_ipod` to see it.

use crate::daemon::device_watcher::{DeviceEvent, DeviceWatcher};
use crate::ipod::device::{self, DetectedIpod};
use crate::ipod::macos_iokit::{run_usb_notifications, UsbChange};
use std::time::Duration;
use tokio::sync::mpsc;

pub struct IokitDeviceWatcher;

impl IokitDeviceWatcher {
    pub fn new_production() -> Self {
        Self
    }
}

impl DeviceWatcher for IokitDeviceWatcher {
    fn start(self: Box<Self>) -> mpsc::Receiver<DeviceEvent> {
        let (tx, rx) = mpsc::channel::<DeviceEvent>(crate::daemon::DEVICE_EVENT_CHANNEL_CAPACITY);
        std::thread::spawn(move || {
            let mut last: Option<DetectedIpod> = None;
            run_usb_notifications(Box::new(move |change| match change {
                UsbChange::Added => {
                    // Attach precedes the volume mount; poll briefly for it.
                    for _ in 0..50 {
                        if let Some(d) = device::scan_for_ipod() {
                            let is_new =
                                last.as_ref().map(|p| p.serial != d.serial).unwrap_or(true);
                            if is_new {
                                if tx.blocking_send(DeviceEvent::Connected(d.clone())).is_err() {
                                    return;
                                }
                                last = Some(d);
                            }
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
                UsbChange::Removed => {
                    if let Some(prev) = last.take() {
                        let _ =
                            tx.blocking_send(DeviceEvent::Disconnected { serial: prev.serial });
                    }
                }
            }));
        });
        rx
    }
}
