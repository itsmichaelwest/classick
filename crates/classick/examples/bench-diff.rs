//! Benchmark for the steady-state walk+diff path (SPEC §6 #2).
//!
//! Runs the same code path as `main.rs` up to the diff, against the real
//! manifest at `%APPDATA%\classick\manifest.json`, but does NOT need the
//! iPod plugged in. Used to verify the Phase 2 optimization
//! (stat-only walker + lazy fingerprint callback) brings the no-op second
//! run under the 5-second SPEC target.
//!
//! Usage:
//!   $env:CLASSICK_SOURCE = "\\HOST\data\media\music"
//!   cargo run --release --example bench-diff

use anyhow::{anyhow, Result};
use classick::manifest;
use classick::source;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let source_root = std::env::var("CLASSICK_SOURCE")
        .map_err(|_| anyhow!("set CLASSICK_SOURCE to the music root"))?;
    let manifest_path: PathBuf = std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join(classick::PROJECT_DIR).join("manifest.json"))
        .ok_or_else(|| anyhow!("APPDATA not set"))?;

    println!("source    = {source_root}");
    println!("manifest  = {}", manifest_path.display());

    let t0 = Instant::now();
    let manifest = manifest::load_or_default(&manifest_path)?;
    let t_load = t0.elapsed();
    println!("load manifest: {:.3}s ({} tracks)", t_load.as_secs_f64(), manifest.tracks.len());

    let t1 = Instant::now();
    let sources = source::walk(std::path::Path::new(&source_root))?;
    let t_walk = t1.elapsed();
    println!("walk source:   {:.3}s ({} files)", t_walk.as_secs_f64(), sources.len());

    let mut fp_calls = 0usize;
    let mut audio_fp_calls = 0usize;
    let t2 = Instant::now();
    // Encoder choice + force_reencode are irrelevant to this benchmark:
    // we're measuring stat-only walk + diff throughput, not the encoder
    // branch. Hardcoding ("ffmpeg", false) keeps the diff deterministic
    // against any local manifest regardless of the user's current --encoder.
    let actions = manifest::diff(
        &manifest,
        &sources,
        |p| {
            fp_calls += 1;
            source::fingerprint(p)
        },
        |p| {
            audio_fp_calls += 1;
            source::audio_fingerprint(p)
        },
        "ffmpeg",
        false,
    )?;
    let t_diff = t2.elapsed();
    println!(
        "diff:          {:.3}s ({} fingerprint reads, {} audio-fp reads)",
        t_diff.as_secs_f64(),
        fp_calls,
        audio_fp_calls,
    );

    let mut add = 0;
    let mut modify = 0;
    let mut metadata_only = 0;
    let mut remove = 0;
    let mut unchanged = 0;
    for a in &actions {
        match a {
            manifest::Action::Add(_) => add += 1,
            manifest::Action::Modify(_, _) => modify += 1,
            manifest::Action::MetadataOnly { .. } => metadata_only += 1,
            manifest::Action::Remove(_) => remove += 1,
            manifest::Action::Unchanged(_) => unchanged += 1,
        }
    }
    println!("plan: Add={add} Modify={modify} MetadataOnly={metadata_only} Remove={remove} Unchanged={unchanged}");

    let total = t0.elapsed();
    println!("total (load+walk+diff): {:.3}s", total.as_secs_f64());
    Ok(())
}
