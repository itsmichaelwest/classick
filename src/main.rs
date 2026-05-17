use anyhow::Result;
use clap::Parser;
use ipod_sync::cli::Cli;
use ipod_sync::config::{self};
use ipod_sync::manifest::{self, Action};
use ipod_sync::source;

fn main() -> Result<()> {
    // Pixbuf loader cache wiring (set up in Phase 1, still required for any
    // libgpod artwork call — harmless if we don't touch artwork in this run).
    //
    // SAFETY: set_var is `unsafe` in Rust 2024 because it races with other
    // threads reading the environment. We're single-threaded here and this is
    // the first statement in main, so there's nothing to race with.
    unsafe { std::env::set_var("GDK_PIXBUF_MODULE_FILE", env!("PIXBUF_LOADERS_CACHE")); }

    let cli = Cli::parse();
    let config = config::resolve(cli)?;

    println!("Source : {}", config.source.display());
    println!("iPod   : {}", config.ipod.as_deref().unwrap_or("(auto-detect deferred to non-dry-run path)"));
    println!("Manifest: {}", config.manifest_path.display());
    println!();

    println!("Walking source...");
    let sources = source::walk(&config.source)?;
    println!("  found {} FLAC file(s)", sources.len());

    let manifest = manifest::load_or_default(&config.manifest_path)?;
    println!("Existing manifest entries: {}", manifest.tracks.len());

    let actions = manifest::diff(&manifest, &sources);

    let mut add = 0usize;
    let mut modify = 0usize;
    let mut remove = 0usize;
    let mut unchanged = 0usize;
    for a in &actions {
        match a {
            Action::Add(_) => add += 1,
            Action::Modify(_, _) => modify += 1,
            Action::Remove(_) => remove += 1,
            Action::Unchanged(_) => unchanged += 1,
        }
    }
    println!();
    println!("Action plan:");
    println!("  Add      : {add}");
    println!("  Modify   : {modify}");
    println!("  Remove   : {remove} {}", if config.no_delete { "(--no-delete; will be skipped)" } else { "" });
    println!("  Unchanged: {unchanged}");

    if config.dry_run {
        println!();
        println!("Dry run; nothing was written.");
        return Ok(());
    }

    // The non-dry-run path lands in Task 10. For now error out loudly.
    eprintln!();
    eprintln!("ERROR: non-dry-run mode not yet implemented (Task 10).");
    eprintln!("Pass --dry-run to preview the action plan.");
    std::process::exit(2);
}
