//! Probe an iPod's firmware via SCSI INQUIRY VPD pages — independent
//! of any sync logic. Dumps the raw SysInfoExtended XML to stdout
//! and a structured parsed view to stderr.
//!
//! Usage:
//!   cargo run --example scsi-probe -- <drive-letter>
//!
//! Example (iPod mounted at G:):
//!   cargo run --example scsi-probe -- G
//!
//! Exit codes:
//!   0 — SCSI INQUIRY succeeded, XML parsed
//!   1 — IOCTL failed (likely permissions; try elevated shell)
//!   2 — IOCTL succeeded but XML couldn't be parsed
//!   3 — Bad command-line arguments

#[cfg(not(windows))]
fn main() {
    eprintln!("scsi-probe is a Windows-only example (SCSI IOCTL pass-through).");
}

#[cfg(windows)]
use classick::scsi_inquiry;
#[cfg(windows)]
use classick::sysinfo_extended::ParsedSysInfo;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: scsi-probe <drive-letter>");
        eprintln!("example: scsi-probe G");
        std::process::exit(3);
    }
    let letter: char = match args[1].chars().next() {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        _ => {
            eprintln!("invalid drive letter {:?} (expected a single A-Z)", args[1]);
            std::process::exit(3);
        }
    };

    eprintln!("--- Reading SysInfoExtended from volume {letter}: ---");
    let xml = match scsi_inquiry::read_sysinfo_extended(letter) {
        Ok(xml) => {
            eprintln!("✓ SCSI INQUIRY succeeded: {} bytes of XML", xml.len());
            xml
        }
        Err(e) => {
            eprintln!("✗ SCSI INQUIRY failed: {e:#}");
            eprintln!();
            eprintln!("Common causes:");
            eprintln!("  - Permission denied: try running from an elevated shell");
            eprintln!("  - Wrong drive letter: check that {letter}:\\ is actually the iPod");
            eprintln!("  - iPod unplugged or in disk-mode-only state");
            std::process::exit(1);
        }
    };

    eprintln!();
    eprintln!("--- Raw XML (first 1024 bytes) ---");
    let preview = if xml.len() > 1024 { &xml[..1024] } else { &xml };
    eprintln!("{preview}");
    if xml.len() > 1024 {
        eprintln!("... ({} more bytes)", xml.len() - 1024);
    }

    eprintln!();
    eprintln!("--- Parsed structured view ---");
    match ParsedSysInfo::from_xml(&xml) {
        Ok(parsed) => {
            eprintln!("  ModelNumStr   : {:?}", parsed.model_num_str);
            eprintln!("  SerialNumber  : {:?}", parsed.serial_number);
            eprintln!("  FirewireGuid  : {:?}", parsed.firewire_guid);
            eprintln!("  FamilyID      : {:?}", parsed.family_id);
            eprintln!("  BuildID       : {:?}", parsed.build_id);
            eprintln!("  Capacity (GB) : {:?}", parsed.capacity_gb);
        }
        Err(e) => {
            eprintln!("✗ Parse failed: {e:#}");
            eprintln!("(raw XML is above; check it's well-formed)");
            std::process::exit(2);
        }
    }

    // Always also write the FULL XML to stdout so the user can
    // redirect to a file if they want the whole thing:
    //   cargo run --example scsi-probe G > sysinfoextended.xml
    println!("{xml}");
}
