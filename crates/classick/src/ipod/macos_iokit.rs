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
}
