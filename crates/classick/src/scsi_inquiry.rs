//! SCSI INQUIRY pass-through to read the iPod's `SysInfoExtended` XML
//! directly from the device firmware. This is the same mechanism
//! iTunes uses to identify a connected iPod's exact model, capacity,
//! and color — the firmware always knows (the data lives in NOR
//! flash `SysCfg`) and exposes it via vendor-specific SCSI VPD pages.
//!
//! ## Protocol (per libgpod's `tools/ipod-scsi.c`)
//!
//! 1. Send `INQUIRY EVPD` with VPD page `0xC0` → response payload is a
//!    list of supported page codes (typically `0xC2..0xE8` on iPod
//!    Classic 6G/7G).
//! 2. For each page code in that list, send `INQUIRY EVPD` with
//!    `CDB[2] = page`. Each response contains a UTF-8 XML fragment in
//!    `buf[4..]` (skipping the 4-byte VPD header).
//! 3. Concatenate the fragments in order → the full SysInfoExtended
//!    Apple plist XML.
//!
//! Each INQUIRY uses a 6-byte CDB:
//! ```text
//! CDB[0] = 0x12  // INQUIRY opcode
//! CDB[1] = 0x01  // EVPD bit set
//! CDB[2] = <page code>
//! CDB[3] = 0x00
//! CDB[4] = 0xFC  // allocation length = 252 bytes
//! CDB[5] = 0x00
//! ```
//!
//! ## Permissions (TL;DR: this code path needs admin)
//!
//! `IOCTL_SCSI_PASS_THROUGH_DIRECT`'s control code (`0x4D014`) embeds
//! `FILE_READ_ACCESS | FILE_WRITE_ACCESS` in its low bits. The
//! Windows I/O manager validates that against the open handle's
//! granted access before dispatching, so the volume handle must have
//! both `GENERIC_READ` and `GENERIC_WRITE`. Opening `\\.\<X>:` with
//! read+write access against a raw volume requires administrator
//! elevation on modern Windows.
//!
//! Empirically verified (see `SCSI.md`): every combination tested
//! against a normal user's session returns `ERROR_ACCESS_DENIED` —
//! either at `CreateFile` (when requesting any non-zero access) or
//! at the `DeviceIoControl` call (with zero-access opens). Only an
//! elevated process can execute the full SCSI INQUIRY sequence.
//!
//! Because the failure is universal and not transient, the caller
//! (`crate::ipod::device::recover_ipod_info_from_usb`) caches the
//! per-device result for the daemon's lifetime — the IOCTL is
//! attempted at most ONCE per FirewireGuid per process, then a
//! cached error short-circuits subsequent polls. The
//! USB-PID-+-capacity heuristic in `identify_ipod` is the
//! production-realistic fallback and produces a libgpod-recognised
//! `ModelNumStr` that's sufficient for iTunes acceptance of the
//! signed iTunesDB (proven 2026-05-24 against an iPod Classic 7G).
//!
//! ## What's the SCSI code still here for, then?
//!
//! Three uses:
//!
//! 1. `examples/scsi-probe.rs` — diagnostic CLI for elevated
//!    sessions; dumps the full XML for forensic / debugging work.
//! 2. Future Nano 5G+ support — those devices use hash72 / hashAB,
//!    which derive a per-device crypto key from data inside
//!    `SysInfoExtended` that cannot be reconstructed from USB
//!    descriptors alone. When we add that support we'll also need
//!    a privileged path to invoke this code (LocalSystem helper
//!    service via MSIX `desktop6:Service`, or SDDL grant via a
//!    traditional installer — see `SCSI.md` for the analysis).
//! 3. Belt-and-suspenders for the rare elevated daemon run (e.g.
//!    dev builds, troubleshooting) — when SCSI succeeds it's the
//!    authoritative source for `ModelNumStr` and overrides the
//!    heuristic.

#![cfg(windows)]

use anyhow::{anyhow, bail, Context, Result};
use std::ffi::c_void;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// `IOCTL_SCSI_PASS_THROUGH_DIRECT` — sends a SCSI command to a
/// device, with the data buffer pointed-to directly by the request
/// (no double-buffering through the IOCTL payload). Value defined in
/// `<ntddscsi.h>`; not yet exposed by windows-sys.
const IOCTL_SCSI_PASS_THROUGH_DIRECT: u32 = 0x0004_D014;

/// SCSI data direction: device → host (we're reading).
const SCSI_IOCTL_DATA_IN: u8 = 1;

/// Per-page response buffer size, matching libgpod's `IPOD_BUF_LENGTH`.
/// Each VPD page returns at most this many bytes; we request 0xFC.
const IPOD_BUF_LENGTH: u32 = 252;

/// Index-of-index VPD page. The response lists which content pages
/// the device exposes.
const VPD_INDEX_PAGE: u8 = 0xC0;

/// Hard ceiling on the number of content pages we'll iterate, defends
/// against a malformed/garbage index response that claims hundreds of
/// pages and would otherwise wedge the daemon in a long loop.
const MAX_CONTENT_PAGES: usize = 64;

/// Windows `SCSI_PASS_THROUGH_DIRECT` structure, packed exactly as
/// the driver expects. `Cdb` is fixed at 16 bytes even when the
/// command is shorter (extra bytes are zero).
#[repr(C)]
struct ScsiPassThroughDirect {
    length: u16,
    scsi_status: u8,
    path_id: u8,
    target_id: u8,
    lun: u8,
    cdb_length: u8,
    sense_info_length: u8,
    data_in: u8,
    data_transfer_length: u32,
    timeout_value: u32,
    data_buffer: *mut c_void,
    sense_info_offset: u32,
    cdb: [u8; 16],
}

/// SCSI pass-through + a colocated sense buffer in a single request
/// blob. `DeviceIoControl` writes sense data into the trailing buffer
/// at the `sense_info_offset` we specify in the header.
#[repr(C)]
struct ScsiPassThroughDirectWithSense {
    sptd: ScsiPassThroughDirect,
    /// Required padding so the 32-byte sense buffer starts on an
    /// 8-byte boundary regardless of how the compiler lays out
    /// `ScsiPassThroughDirect` on this platform. Without this, some
    /// Windows builds reject the IOCTL with INVALID_PARAMETER.
    _pad: [u8; 4],
    sense: [u8; 32],
}

/// Open the iPod's volume handle (`\\.\X:`) with the absolute minimum
/// access (`dwDesiredAccess = 0`, query-only). On Windows this opens
/// the handle in "IOCTL-only" mode — no read or write data access is
/// granted, but `DeviceIoControl` calls that don't transfer file data
/// (like our SCSI INQUIRY pass-through) still work, and crucially the
/// OS does NOT require admin elevation.
///
/// We tried both `\\.\PhysicalDriveN` with `GENERIC_READ` and `\\.\X:`
/// with `GENERIC_READ` first — both got `ERROR_ACCESS_DENIED` from a
/// normal user session, confirming that any non-zero access mode
/// against a raw disk/volume triggers Windows' admin requirement on
/// modern builds. Zero-access + IOCTL pass-through is the documented
/// non-elevation path for this specific use case (see Microsoft KB
/// articles on `IOCTL_SCSI_PASS_THROUGH` and the MSDN entry for
/// `CreateFile`'s `dwDesiredAccess` parameter, "Querying Attributes"
/// section).
fn open_volume(drive_letter: char) -> Result<OwnedHandle> {
    let path = format!(r"\\.\{}:", drive_letter.to_ascii_uppercase());
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // IOCTL_SCSI_PASS_THROUGH_DIRECT's control code (0x4D014) embeds
    // FILE_READ_ACCESS | FILE_WRITE_ACCESS in its access bits, so the
    // handle MUST have both read and write access for the IOCTL to
    // dispatch — zero-access opens, GENERIC_READ alone, both return
    // ACCESS_DENIED on the IOCTL call. GENERIC_READ | GENERIC_WRITE
    // works but requires admin elevation on raw volume handles.
    let handle: HANDLE = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        let err = std::io::Error::last_os_error();
        return Err(anyhow!("opening {path}: {err}"));
    }
    // SAFETY: handle is a valid file handle owned by us — CreateFileW
    // returned success, no other code holds a copy. OwnedHandle takes
    // ownership and CloseHandles on drop.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as *mut _) })
}

/// Issue one `INQUIRY EVPD page=<page_code>` against the open device.
/// Returns the full response bytes (header + payload). The caller is
/// responsible for slicing past the 4-byte VPD header to get the XML
/// payload (`response[4..]`).
fn inquiry_vpd(handle: HANDLE, page_code: u8) -> Result<Vec<u8>> {
    let mut buffer = vec![0u8; IPOD_BUF_LENGTH as usize];
    let mut request = ScsiPassThroughDirectWithSense {
        sptd: ScsiPassThroughDirect {
            length: std::mem::size_of::<ScsiPassThroughDirect>() as u16,
            scsi_status: 0,
            path_id: 0,
            target_id: 0,
            lun: 0,
            cdb_length: 6,
            sense_info_length: 32,
            data_in: SCSI_IOCTL_DATA_IN,
            data_transfer_length: IPOD_BUF_LENGTH,
            timeout_value: 10, // seconds
            data_buffer: buffer.as_mut_ptr() as *mut c_void,
            sense_info_offset: (std::mem::size_of::<ScsiPassThroughDirect>()
                + std::mem::size_of::<[u8; 4]>()) as u32,
            cdb: [
                0x12,      // INQUIRY
                0x01,      // EVPD bit
                page_code, // VPD page code
                0x00,
                IPOD_BUF_LENGTH as u8, // allocation length (252)
                0x00,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ],
        },
        _pad: [0; 4],
        sense: [0; 32],
    };

    let mut bytes_returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_SCSI_PASS_THROUGH_DIRECT,
            &mut request as *mut _ as *mut c_void,
            std::mem::size_of::<ScsiPassThroughDirectWithSense>() as u32,
            &mut request as *mut _ as *mut c_void,
            std::mem::size_of::<ScsiPassThroughDirectWithSense>() as u32,
            &mut bytes_returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        let err = std::io::Error::last_os_error();
        return Err(anyhow!(
            "DeviceIoControl (INQUIRY page {:#04x}): {err}",
            page_code
        ));
    }
    if request.sptd.scsi_status != 0 {
        bail!(
            "SCSI INQUIRY page {:#04x} returned status {} (sense key {:#04x})",
            page_code,
            request.sptd.scsi_status,
            request.sense.get(2).copied().unwrap_or(0) & 0x0F,
        );
    }
    Ok(buffer)
}

/// Read the iPod's SysInfoExtended XML by issuing the SCSI INQUIRY
/// VPD sequence against the volume mounted at `<drive_letter>:\`.
///
/// Returns the concatenated UTF-8 XML string. Errors with a
/// context-rich message that callers can use to decide whether to
/// fall back (permission denied, device rejected the page, malformed
/// index response). A failure here is **not fatal** — the caller can
/// still identify the device by USB PID + capacity, just less
/// precisely.
pub fn read_sysinfo_extended(drive_letter: char) -> Result<String> {
    let owned = open_volume(drive_letter)
        .with_context(|| format!("opening volume {drive_letter}: for SCSI INQUIRY"))?;
    let handle: HANDLE = std::os::windows::io::AsRawHandle::as_raw_handle(&owned) as HANDLE;

    // Step 1: Read the index page to learn which content pages exist.
    let index_response =
        inquiry_vpd(handle, VPD_INDEX_PAGE).context("SCSI INQUIRY VPD index page 0xC0")?;
    // VPD response layout (SPC-3 §6.5.1):
    //   byte 0: peripheral device type (low 5 bits) + qualifier
    //   byte 1: page code (echoes our request)
    //   byte 2-3: page length (BE u16) — # of payload bytes following
    //   byte 4..: payload (for page 0xC0: list of supported page codes)
    if index_response.len() < 4 {
        bail!(
            "VPD page 0xC0 response truncated (len {})",
            index_response.len()
        );
    }
    let payload_len = u16::from_be_bytes([index_response[2], index_response[3]]) as usize;
    let payload_end = std::cmp::min(4 + payload_len, index_response.len());
    let page_list: Vec<u8> = index_response[4..payload_end].to_vec();
    if page_list.is_empty() {
        bail!("VPD index page 0xC0 listed no content pages");
    }
    if page_list.len() > MAX_CONTENT_PAGES {
        bail!(
            "VPD index page 0xC0 listed {} pages — exceeds sanity ceiling {}",
            page_list.len(),
            MAX_CONTENT_PAGES,
        );
    }

    // Step 2: Read each content page, concatenating payloads.
    let mut xml = Vec::with_capacity(page_list.len() * IPOD_BUF_LENGTH as usize);
    for &page_code in &page_list {
        let response = inquiry_vpd(handle, page_code)
            .with_context(|| format!("SCSI INQUIRY VPD content page {:#04x}", page_code))?;
        if response.len() < 4 {
            bail!(
                "VPD page {:#04x} response truncated (len {})",
                page_code,
                response.len()
            );
        }
        let chunk_len = u16::from_be_bytes([response[2], response[3]]) as usize;
        let chunk_end = std::cmp::min(4 + chunk_len, response.len());
        xml.extend_from_slice(&response[4..chunk_end]);
    }

    // The device returns UTF-8 XML; trailing zero bytes from the
    // last page's unused buffer space are common — strip them.
    while xml.last() == Some(&0) {
        xml.pop();
    }
    String::from_utf8(xml).map_err(|e| anyhow!("SysInfoExtended payload is not valid UTF-8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Windows IOCTL code is the canonical value from
    /// `<ntddscsi.h>`. A typo here would silently break SCSI
    /// pass-through with INVALID_FUNCTION.
    #[test]
    fn ioctl_code_matches_microsoft_documentation() {
        assert_eq!(IOCTL_SCSI_PASS_THROUGH_DIRECT, 0x0004_D014);
    }

    /// SCSI INQUIRY EVPD CDB layout — guards against accidentally
    /// dropping the EVPD bit or the page-code byte during a
    /// refactor. Same opcode/structure libgpod's ipod-scsi.c uses.
    #[test]
    fn inquiry_cdb_template_is_correct() {
        let request = ScsiPassThroughDirectWithSense {
            sptd: ScsiPassThroughDirect {
                length: 0,
                scsi_status: 0,
                path_id: 0,
                target_id: 0,
                lun: 0,
                cdb_length: 6,
                sense_info_length: 0,
                data_in: SCSI_IOCTL_DATA_IN,
                data_transfer_length: 0,
                timeout_value: 0,
                data_buffer: std::ptr::null_mut(),
                sense_info_offset: 0,
                cdb: [
                    0x12, 0x01, 0xC0, 0x00, 0xFC, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                ],
            },
            _pad: [0; 4],
            sense: [0; 32],
        };
        assert_eq!(request.sptd.cdb[0], 0x12, "INQUIRY opcode");
        assert_eq!(request.sptd.cdb[1] & 0x01, 0x01, "EVPD bit must be set");
        assert_eq!(request.sptd.cdb[2], 0xC0, "VPD page code");
        assert_eq!(request.sptd.cdb[4], 0xFC, "allocation length 252");
    }
}
