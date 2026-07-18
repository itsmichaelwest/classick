//! Native macOS device identity via IOKit / CoreFoundation. The only module
//! in the crate with `unsafe` above the libgpod FFI layer. Supersedes the
//! earlier `ioreg`/`df` shellout path.
//!
//! Strategy: resolve the mount to its BSD device name via `statfs`
//! (`/Volumes/X` -> `/dev/disk2s1` -> `disk2s1`), match the corresponding
//! `IOMedia` object in the IORegistry, read its `Size` (capacity), then walk
//! up the registry to the parent USB device (vendor `0x05AC`) and read its
//! `USB Serial Number` (= FireWireGuid for USB iPods) and `idProduct` (PID).

use std::os::raw::c_char;
use std::path::Path;

use core_foundation::base::TCFType;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

use io_kit_sys::{
    kIOMasterPortDefault, IOBSDNameMatching, IOObjectRelease,
    IORegistryEntryCreateCFProperty, IORegistryEntryGetParentEntry,
    IOServiceGetMatchingService,
};
use io_kit_sys::types::io_registry_entry_t;

/// USB identity recovered from the IORegistry for a mounted iPod.
#[derive(Debug, Clone)]
pub struct IokitUsbIdentity {
    pub firewire_guid: String,
    pub pid: Option<u16>,
    pub capacity_bytes: Option<u64>,
}

/// Format a USB iSerialNumber string as libgpod's `FirewireGuid`
/// (`0x` prefix, uppercase hex). For USB iPods the USB serial number
/// string is the FireWire GUID.
pub fn format_firewire_guid(usb_serial: &str) -> String {
    format!("0x{}", usb_serial.trim().to_uppercase())
}

fn supported_ipod_added_identity(usb_serial: &str, pid: u16) -> Option<String> {
    let serial = usb_serial.trim();
    if serial.len() != 16
        || !serial.chars().all(|character| character.is_ascii_hexdigit())
        || !matches!(
            pid,
            0x1201
                | 0x1202
                | 0x1203
                | 0x1204
                | 0x1205
                | 0x1206
                | 0x1209
                | 0x1240
                | 0x1260
                | 0x1261
                | 0x1262
                | 0x1263
                | 0x1265
                | 0x1266
                | 0x1267
                | 0x1300
                | 0x1301
                | 0x1302
                | 0x1303
        )
    {
        return None;
    }
    Some(format_firewire_guid(serial))
}

/// Resolve a mounted iPod volume to its USB identity. Returns `None` if the
/// mount can't be resolved to a BSD name, isn't backed by an Apple USB
/// device, or lacks a serial number.
pub fn identity_for_mount(mount: &Path) -> Option<IokitUsbIdentity> {
    let bsd = bsd_name_for_mount(mount)?;
    unsafe { identity_for_bsd_name(&bsd) }
}

/// `/Volumes/Foo` -> `disk2s1` (strips the `/dev/` prefix from the mount's
/// backing device as reported by `statfs`).
fn bsd_name_for_mount(mount: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let c_mount = std::ffi::CString::new(mount.as_os_str().as_bytes()).ok()?;
    let mut sfs: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c_mount.as_ptr(), &mut sfs) } != 0 {
        return None;
    }
    let dev = unsafe { std::ffi::CStr::from_ptr(sfs.f_mntfromname.as_ptr()) }
        .to_str()
        .ok()?;
    let name = dev.strip_prefix("/dev/").unwrap_or(dev);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

unsafe fn identity_for_bsd_name(bsd: &str) -> Option<IokitUsbIdentity> {
    // IOServiceGetMatchingService consumes (releases) the matching dict.
    let c_bsd = std::ffi::CString::new(bsd).ok()?;
    let matching = IOBSDNameMatching(kIOMasterPortDefault, 0, c_bsd.as_ptr());
    if matching.is_null() {
        return None;
    }
    let media = IOServiceGetMatchingService(kIOMasterPortDefault, matching);
    if media == 0 {
        return None;
    }

    let capacity_bytes = read_u64_prop(media, "Size");

    // Walk parents (IOService plane) until we hit the Apple USB device.
    let mut guid: Option<String> = None;
    let mut pid: Option<u16> = None;
    let mut chain: Vec<io_registry_entry_t> = vec![media];
    let mut entry = media;
    // Bounded to avoid any pathological cycle; the USB device is a handful of
    // levels above the IOMedia.
    // The Apple vendor id (0x05AC) appears on several nodes up the chain
    // (USB interfaces, then the device). Only the IOUSBHostDevice carries the
    // serial number, so require BOTH the vendor id and a serial before
    // accepting a node — otherwise we stop at an interface and miss the guid.
    for _ in 0..32 {
        let ser = read_string_prop(entry, "USB Serial Number");
        if read_u64_prop(entry, "idVendor") == Some(0x05AC) && ser.is_some() {
            guid = ser.map(|s| format_firewire_guid(&s));
            pid = read_u64_prop(entry, "idProduct").map(|v| v as u16);
            break;
        }
        let mut parent: io_registry_entry_t = 0;
        let plane = b"IOService\0".as_ptr() as *const c_char;
        if IORegistryEntryGetParentEntry(entry, plane, &mut parent) != 0 || parent == 0 {
            break;
        }
        chain.push(parent);
        entry = parent;
    }

    for e in chain {
        IOObjectRelease(e);
    }

    Some(IokitUsbIdentity {
        firewire_guid: guid?,
        pid,
        capacity_bytes,
    })
}

/// Read an integer IORegistry property (CFNumber) as u64.
unsafe fn read_u64_prop(entry: io_registry_entry_t, key: &str) -> Option<u64> {
    let cfkey = CFString::new(key);
    let raw = IORegistryEntryCreateCFProperty(
        entry,
        cfkey.as_concrete_TypeRef(),
        std::ptr::null(),
        0,
    );
    if raw.is_null() {
        return None;
    }
    let num = CFNumber::wrap_under_create_rule(raw as _);
    num.to_i64().map(|v| v as u64)
}

/// Read a string IORegistry property (CFString).
unsafe fn read_string_prop(entry: io_registry_entry_t, key: &str) -> Option<String> {
    let cfkey = CFString::new(key);
    let raw = IORegistryEntryCreateCFProperty(
        entry,
        cfkey.as_concrete_TypeRef(),
        std::ptr::null(),
        0,
    );
    if raw.is_null() {
        return None;
    }
    let s = CFString::wrap_under_create_rule(raw as _);
    Some(s.to_string())
}

/// A USB attach/detach signal from the IOKit run loop.
#[derive(Debug, Clone)]
pub enum UsbChange {
    Added { serial: String },
    Removed,
}

/// IOKit notification / interest-type strings (see `IOKitKeys.h`).
const KIO_MATCHED_NOTIFICATION: &[u8] = b"IOServiceMatched\0";
const KIO_GENERAL_INTEREST: &[u8] = b"IOGeneralInterest\0";

/// `kIOMessageServiceIsTerminated` = `iokit_common_msg(0x10)` =
/// `err_system(0x38) | err_sub(0) | 0x10` (see `IOKit/IOMessage.h`). Delivered
/// to a `kIOGeneralInterest` interest notification when the watched device is
/// removed. This per-device interest is how removal is detected — a
/// `kIOTerminatedNotification` *matching* notification doesn't fire reliably
/// (the device's property dict, e.g. `idVendor`, isn't matchable as the node
/// tears down), which is why earlier unplugs went unnoticed.
const KIO_MESSAGE_SERVICE_IS_TERMINATED: u32 = 0xE000_0010;

/// Run a `CFRunLoop` that invokes `on_event` whenever a validated, supported
/// iPod USB device is added or removed. Blocks the calling thread indefinitely
/// (the notification source keeps the run loop alive) — intended to run on a
/// dedicated `std::thread` owned by the device watcher. `on_event(Added)` also
/// fires once at startup for each already-connected supported iPod.
///
/// Removal is detected the canonical Apple way: for every matched device we
/// register a per-device `IOServiceAddInterestNotification(kIOGeneralInterest)`
/// and watch for `kIOMessageServiceIsTerminated`. We hold the device handle, so
/// there's no property-match-at-teardown problem.
///
/// `on_event` runs ON the run-loop thread, so it MUST be fast and non-blocking
/// (a channel send). Doing blocking work here (e.g. polling for the volume
/// mount) starves every other IOKit notification — that is what made plug-in
/// and unplug events go missing.
pub fn run_usb_notifications(on_event: Box<dyn FnMut(UsbChange) + Send>) {
    use core_foundation_sys::runloop::{
        kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRun,
    };
    use io_kit_sys::types::{io_iterator_t, io_object_t, io_service_t};
    use io_kit_sys::{
        kIOMasterPortDefault, IONotificationPortCreate, IONotificationPortGetRunLoopSource,
        IONotificationPortRef, IOIteratorNext, IOObjectRelease, IOServiceAddInterestNotification,
        IOServiceAddMatchingNotification, IOServiceMatching,
    };
    use std::os::raw::{c_char, c_void};

    // Shared by the matched callback and every per-device interest callback.
    // All run on the single CFRunLoop thread, so the raw-pointer aliasing is
    // sound (no concurrency). Leaked for the process lifetime (the run loop
    // never returns).
    struct Ctx {
        port: IONotificationPortRef,
        on_event: Box<dyn FnMut(UsbChange) + Send>,
    }
    // Per-device termination registration; refcon for its interest callback.
    // Boxed and leaked into IOKit, reclaimed in `device_terminated`.
    struct Interest {
        ctx: *mut Ctx,
        notification: io_object_t,
        service: io_service_t,
    }

    // A matched Apple USB device appeared. Register a termination interest on
    // it, then signal Added. Stays fast — no blocking here.
    unsafe extern "C" fn device_matched(refcon: *mut c_void, iter: io_iterator_t) {
        let ctx = &mut *(refcon as *mut Ctx);
        loop {
            let service = IOIteratorNext(iter);
            if service == 0 {
                break;
            }
            // Filter to Apple (0x05AC) here — the matching dict can't (see the
            // registration site). Skip + release anything else.
            if read_u64_prop(service, "idVendor") != Some(0x05AC) {
                IOObjectRelease(service);
                continue;
            }
            let Some(pid) = read_u64_prop(service, "idProduct").map(|pid| pid as u16) else {
                IOObjectRelease(service);
                continue;
            };
            let Some(raw_serial) = read_string_prop(service, "USB Serial Number") else {
                IOObjectRelease(service);
                continue;
            };
            let Some(serial) = supported_ipod_added_identity(&raw_serial, pid) else {
                IOObjectRelease(service);
                continue;
            };
            let interest = Box::into_raw(Box::new(Interest {
                ctx: refcon as *mut Ctx,
                notification: 0,
                service,
            }));
            let mut notification: io_object_t = 0;
            let kr = IOServiceAddInterestNotification(
                ctx.port,
                service,
                KIO_GENERAL_INTEREST.as_ptr() as *mut c_char,
                device_terminated,
                interest as *mut c_void,
                &mut notification,
            );
            if kr == 0 {
                (*interest).notification = notification;
                // Keep `service` alive — released in device_terminated.
                (ctx.on_event)(UsbChange::Added { serial });
            } else {
                tracing::warn!("IOServiceAddInterestNotification failed: {kr}");
                drop(Box::from_raw(interest));
                IOObjectRelease(service);
            }
        }
    }

    // A watched device is being torn down. Signal Removed and release the
    // device handle + its interest notification.
    unsafe extern "C" fn device_terminated(
        refcon: *mut c_void,
        _service: io_service_t,
        message_type: u32,
        _arg: *mut c_void,
    ) {
        if message_type != KIO_MESSAGE_SERVICE_IS_TERMINATED {
            return;
        }
        let interest = Box::from_raw(refcon as *mut Interest);
        let ctx = &mut *interest.ctx;
        (ctx.on_event)(UsbChange::Removed);
        IOObjectRelease(interest.notification);
        IOObjectRelease(interest.service);
    }

    unsafe {
        let port = IONotificationPortCreate(kIOMasterPortDefault);
        let src = IONotificationPortGetRunLoopSource(port);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopDefaultMode);

        let ctx = Box::into_raw(Box::new(Ctx { port, on_event }));

        // MATCHED: USB host devices, filtered to Apple (0x05AC) in the callback.
        //
        // Two hard-won details:
        //  * Class MUST be `IOUSBHostDevice`. The legacy `IOUSBDevice` class does
        //    not exist on modern macOS (`ioreg -c IOUSBDevice` returns nothing),
        //    so matching it yields an always-empty iterator.
        //  * The `idVendor` filter must NOT go in the matching dict. Setting it
        //    there makes IOKit's matching engine return an empty iterator (the
        //    CFNumber doesn't compare the way the engine wants), which silently
        //    broke every add/remove notification. Filtering by vendor in
        //    `device_matched` via `read_u64_prop` works reliably.
        let matching = IOServiceMatching(b"IOUSBHostDevice\0".as_ptr() as *const c_char);

        let mut it_add: io_iterator_t = 0;
        let kr_add = IOServiceAddMatchingNotification(
            port,
            KIO_MATCHED_NOTIFICATION.as_ptr() as *mut c_char,
            matching, // consumes this reference
            device_matched,
            ctx as *mut c_void,
            &mut it_add,
        );
        if kr_add != 0 {
            tracing::warn!("IOServiceAddMatchingNotification(matched) failed: {kr_add}");
        }
        device_matched(ctx as *mut c_void, it_add); // arm + process already-connected

        CFRunLoopRun();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_serial_as_uppercase_hex_guid() {
        assert_eq!(format_firewire_guid("000a27002138b0a8"), "0x000A27002138B0A8");
    }

    #[test]
    fn trims_and_uppercases() {
        assert_eq!(format_firewire_guid("  ab12cd  "), "0xAB12CD");
    }

    #[test]
    fn supported_ipod_added_identity_requires_a_guid_and_known_ipod_pid() {
        assert_eq!(
            supported_ipod_added_identity(" 000a27002138b0a8 ", 0x1261),
            Some("0x000A27002138B0A8".to_string())
        );
        assert_eq!(supported_ipod_added_identity("not-a-guid", 0x1261), None);
        assert_eq!(supported_ipod_added_identity("000a27002138b0a8", 0x12AB), None);
    }

    // Regression: pin the IOKit notification/interest constants. The message
    // type is `kIOMessageServiceIsTerminated = err_system(0x38) | 0x10`; a
    // wrong value means the per-device termination interest silently never
    // reports removal.
    #[test]
    fn iokit_notification_constants_are_correct() {
        assert_eq!(KIO_MATCHED_NOTIFICATION, b"IOServiceMatched\0");
        assert_eq!(KIO_GENERAL_INTEREST, b"IOGeneralInterest\0");
        assert_eq!(KIO_MESSAGE_SERVICE_IS_TERMINATED, 0xE000_0010);
    }
}
