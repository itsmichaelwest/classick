//! Diagnostic: resolve a mounted iPod's identity on macOS via the native
//! IOKit path, printing both the low-level IOKit read and the layered
//! `resolve_libgpod_identity` result. macOS-only.
//!
//! Usage: cargo run --example mac-identity -- /Volumes/<name>

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("mac-identity is a macOS-only diagnostic.");
}

#[cfg(target_os = "macos")]
fn main() {
    let mount = match std::env::args().nth(1) {
        Some(m) => m,
        None => {
            eprintln!("usage: mac-identity /Volumes/<name>");
            std::process::exit(2);
        }
    };
    let p = std::path::Path::new(&mount);

    match classick::ipod::macos_iokit::identity_for_mount(p) {
        Some(id) => println!(
            "identity_for_mount: guid={} pid={:?} capacity_bytes={:?}",
            id.firewire_guid, id.pid, id.capacity_bytes
        ),
        None => println!("identity_for_mount: None (not an Apple USB device, or unmounted)"),
    }

    match classick::ipod::device::resolve_libgpod_identity(p) {
        Ok(id) => println!(
            "resolve_libgpod_identity: FirewireGuid={} ModelNumStr={}",
            id.firewire_guid, id.model_num_str
        ),
        Err(e) => println!("resolve_libgpod_identity: ERROR {e:#}"),
    }
}
