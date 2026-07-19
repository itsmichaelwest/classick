//! Print each iPod track's title + tracklen (ms) — diagnostic for verifying the
//! iTunesDB duration is set (0 = the iPod shows -0:00). Read-only.
//! Usage: cargo run --example ipod-durations -- /Volumes/IPOD

use anyhow::{Context, Result};
use classick::ffi;
use classick::ipod::db::OwnedDb;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::PathBuf;

fn s(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

fn main() -> Result<()> {
    let mount = std::env::args()
        .nth(1)
        .context("usage: ipod-durations <mount>")?;
    let db = OwnedDb::open(&PathBuf::from(&mount))?;
    let mut zero = 0usize;
    unsafe {
        let mut node = (*db.as_ptr()).tracks;
        while !node.is_null() {
            let t = (*node).data as *mut ffi::Itdb_Track;
            let len = (*t).tracklen;
            if len == 0 {
                zero += 1;
            }
            println!("{:>8} ms  {}", len, s((*t).title as *const c_char));
            node = (*node).next;
        }
    }
    eprintln!("{zero} track(s) with tracklen==0");
    Ok(())
}
