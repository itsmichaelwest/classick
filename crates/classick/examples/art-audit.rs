//! Read-only: list every track on the iPod with its has_artwork flag, so we
//! can see exactly which tracks lost art. Usage: cargo run --example art-audit -- /Volumes/IPOD
use anyhow::{Context, Result};
use classick::ffi;
use classick::ipod::db::OwnedDb;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::PathBuf;

fn s(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() } }
}

fn main() -> Result<()> {
    let mount = std::env::args().nth(1).context("usage: art-audit <mount>")?;
    let db = OwnedDb::open(&PathBuf::from(&mount))?;
    let (mut with, mut without) = (0u32, 0u32);
    unsafe {
        let mut node = (*db.as_ptr()).tracks;
        while !node.is_null() {
            let t = (*node).data as *mut ffi::Itdb_Track;
            let has = (*t).has_artwork; // 1 = yes, 2 = no (libgpod tri-state)
            let art = has == 1;
            if art { with += 1 } else { without += 1 }
            println!(
                "art={} has_artwork={} size={} | {} - {} - {}",
                if art { "YES" } else { "no " },
                has,
                (*t).artwork_size,
                s((*t).artist as *const c_char),
                s((*t).album as *const c_char),
                s((*t).title as *const c_char),
            );
            node = (*node).next;
        }
    }
    eprintln!("=== {with} with art, {without} without ===");
    Ok(())
}
