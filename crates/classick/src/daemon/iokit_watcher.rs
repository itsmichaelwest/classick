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

        // Raw USB add/remove signals flow from the CFRunLoop thread to this
        // worker thread. The run-loop callbacks MUST stay fast (a channel
        // send), so the blocking volume-mount scan runs HERE, never on the run
        // loop — blocking the run loop starves every other IOKit notification
        // (that's what silently dropped plug-in and unplug events).
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<UsbChange>();

        std::thread::spawn(move || {
            let mut last: Option<DetectedIpod> = None;

            // One-shot startup scan (NOT a poll — runs exactly once). The IOKit
            // matched-notification arming reports already-connected devices, but
            // a daemon restart while the iPod is plugged in is common enough
            // that we don't want detection to hinge on arming-iterator timing.
            if let Some(d) = device::scan_for_ipod() {
                tracing::info!("device watcher: iPod {} already connected at startup", d.serial);
                if tx.blocking_send(DeviceEvent::Connected(d.clone())).is_err() {
                    return;
                }
                last = Some(d);
            }

            while let Ok(change) = raw_rx.recv() {
                tracing::debug!("device watcher: raw USB signal {change:?}");
                match change {
                    UsbChange::Added => {
                        // A USB attach precedes the volume mount, and an iPod
                        // Classic's spinning HDD can take 10s+ to spin up and
                        // mount after re-plug (at startup it's instant only
                        // because the volume was already mounted). Wait up to
                        // 30s for the mount — a bounded post-event settle, NOT
                        // steady-state polling. There's no second USB event to
                        // fall back on, so giving up early leaves it stuck.
                        let mut found = false;
                        for _ in 0..120 {
                            if let Some(d) = device::scan_for_ipod() {
                                found = true;
                                let is_new =
                                    last.as_ref().map(|p| p.serial != d.serial).unwrap_or(true);
                                if is_new {
                                    tracing::info!(
                                        "device watcher: iPod {} attached; sending Connected",
                                        d.serial
                                    );
                                    if tx.blocking_send(DeviceEvent::Connected(d.clone())).is_err()
                                    {
                                        return;
                                    }
                                    last = Some(d);
                                }
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(250));
                        }
                        if !found {
                            tracing::info!(
                                "device watcher: Added signal but no iPod volume mounted within 30s"
                            );
                        }
                    }
                    UsbChange::Removed => {
                        // A per-device termination fired. It's for SOME Apple
                        // USB device (keyboard, trackpad, hub, or the iPod), so
                        // confirm the iPod itself is gone before reporting a
                        // disconnect — an unrelated unplug leaves it detectable.
                        if last.is_some() {
                            let mut gone = false;
                            for _ in 0..30 {
                                if device::scan_for_ipod().is_none() {
                                    gone = true;
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            if gone {
                                if let Some(prev) = last.take() {
                                    tracing::info!(
                                        "device watcher: iPod {} gone; sending Disconnected",
                                        prev.serial
                                    );
                                    if tx
                                        .blocking_send(DeviceEvent::Disconnected { serial: prev.serial })
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                            } else {
                                tracing::debug!(
                                    "device watcher: USB removal but iPod still detectable; ignoring"
                                );
                            }
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
