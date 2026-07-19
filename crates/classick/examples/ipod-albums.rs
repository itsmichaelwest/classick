//! List unique "Artist <tab> Album" pairs on the iPod (read-only DB
//! introspection). Handy for picking music that isn't already synced.
//!
//! Usage: cargo run --example ipod-albums -- /Volumes/IPOD

use anyhow::{Context, Result};
use classick::ffi;
use classick::ipod::db::OwnedDb;
use std::collections::BTreeSet;
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
        .context("usage: ipod-albums <mount>")?;
    let db = OwnedDb::open(&PathBuf::from(&mount))?;
    let mut set = BTreeSet::new();
    unsafe {
        let mut node = (*db.as_ptr()).tracks;
        while !node.is_null() {
            let t = (*node).data as *mut ffi::Itdb_Track;
            set.insert(format!(
                "{}\t{}",
                s((*t).artist as *const c_char),
                s((*t).album as *const c_char)
            ));
            node = (*node).next;
        }
    }
    for line in &set {
        println!("{line}");
    }
    eprintln!("{} unique artist/album pairs", set.len());
    Ok(())
}
