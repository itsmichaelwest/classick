//! Remove exactly the tracks classick recorded in its manifest (matched by
//! libgpod DBID), plus their on-disk files. Targeted cleanup for a mistaken
//! sync — leaves tracks the manifest doesn't know about untouched.
//!
//! Usage: cargo run --example remove-synced -- /Volumes/IPOD

use anyhow::{Context, Result};
use classick::ffi;
use classick::ipod::db::OwnedDb;
use classick::ipod::device;
use std::collections::HashSet;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::ptr;

fn main() -> Result<()> {
    let mount = std::env::args().nth(1).context("usage: remove-synced <mount>")?;
    let mount_path = PathBuf::from(&mount);

    let manifest_path = dirs::config_dir()
        .context("no config dir")?
        .join("classick")
        .join("manifest.json");
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )?;
    let targets: HashSet<u64> = json["tracks"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t["ipod_dbid"].as_u64()).collect())
        .unwrap_or_default();
    println!("manifest records {} track(s) to remove by DBID", targets.len());
    if targets.is_empty() {
        return Ok(());
    }

    let db = OwnedDb::open(&mount_path)?;
    let guid = device::read_firewire_guid(&mount_path)?;
    unsafe {
        let device_ptr = (*db.as_ptr()).device;
        device::set_firewire_guid(device_ptr, &guid)?;
    }

    // Collect matching track pointers before mutating the GList.
    let mut hits: Vec<*mut ffi::Itdb_Track> = Vec::new();
    unsafe {
        let mut node = (*db.as_ptr()).tracks;
        while !node.is_null() {
            let t = (*node).data as *mut ffi::Itdb_Track;
            if targets.contains(&(*t).dbid) {
                hits.push(t);
            }
            node = (*node).next;
        }
    }
    println!("matched {} track(s) on the iPod by DBID", hits.len());

    let mut deleted = 0usize;
    unsafe {
        for &t in &hits {
            let fname_c = ffi::itdb_filename_on_ipod(t);
            if !fname_c.is_null() {
                let p = CStr::from_ptr(fname_c).to_string_lossy().into_owned();
                if std::fs::remove_file(Path::new(&p)).is_ok() {
                    deleted += 1;
                }
                ffi::g_free(fname_c as *mut std::os::raw::c_void);
            }
            ffi::itdb_playlist_remove_track(ptr::null_mut(), t);
            ffi::itdb_track_remove(t);
        }
    }
    println!("removed {} DB entries, deleted {} files", hits.len(), deleted);

    db.write()?;
    println!("new track count: {}", db.track_count());
    println!("Eject before unplugging.");
    Ok(())
}
