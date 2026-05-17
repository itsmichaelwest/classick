use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest_dir.join("vendor").join("libgpod");

    // Link the import library
    println!(
        "cargo:rustc-link-search=native={}",
        vendor.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=gpod");
    // GLib symbols (e.g. g_error_free) live in libglib-2.0-0.dll. We generated
    // an MSVC-format import lib (glib.lib) from its export table so Rust can
    // resolve those symbols at link time.
    println!("cargo:rustc-link-lib=dylib=glib");

    // Re-run if the header changes
    let header = vendor.join("include").join("gpod").join("itdb.h");
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("lib").join("gpod.lib").display()
    );

    // GLib headers ship with the MSYS2 MinGW64 toolchain used to build libgpod.
    // bindgen needs them because itdb.h includes <glib.h> / <glib-object.h>.
    let glib_include = "C:/msys64/mingw64/include/glib-2.0";
    let glib_config_include = "C:/msys64/mingw64/lib/glib-2.0/include";

    // Generate Rust bindings
    let bindings = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg(format!("-I{}", vendor.join("include").display()))
        .clang_arg(format!("-I{}", glib_include))
        .clang_arg(format!("-I{}", glib_config_include))
        // libgpod surface
        .allowlist_function("itdb_.*")
        .allowlist_type("Itdb_.*")
        .allowlist_var("ITDB_.*")
        // GError handling is needed by Task 6's spike
        .allowlist_function("g_error_.*")
        .allowlist_function("g_strdup")
        .allowlist_function("g_free")
        // Task 11: route libgpod's GLib WARNING/CRITICAL messages through tracing.
        .allowlist_function("g_log_.*")
        .allowlist_type("GLogLevelFlags")
        .allowlist_var("G_LOG_.*")
        .allowlist_type("GError")
        .allowlist_type("GList")
        .layout_tests(false)
        .generate()
        .expect("bindgen failed to generate libgpod bindings");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_path = out_dir.join("libgpod_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write bindings");

    // Ensure the DLLs are alongside the exe for `cargo run`.
    // Derive target_dir from OUT_DIR so CARGO_TARGET_DIR is honored:
    // OUT_DIR = <real_target>/<profile>/build/<pkg>-<hash>/out
    let target_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR should have at least 3 ancestors")
        .to_path_buf();

    let bin_dir = vendor.join("bin");
    if bin_dir.exists() {
        std::fs::create_dir_all(&target_dir).unwrap_or_else(|e| {
            panic!("failed to create target dir {}: {}", target_dir.display(), e)
        });
        let entries = std::fs::read_dir(&bin_dir).unwrap_or_else(|e| {
            panic!("failed to read vendor bin dir {}: {}", bin_dir.display(), e)
        });
        for entry in entries {
            let entry = entry.expect("failed to read directory entry in vendor/libgpod/bin");
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("dll") {
                let dest = target_dir.join(path.file_name().unwrap());
                std::fs::copy(&path, &dest).unwrap_or_else(|e| {
                    panic!("failed to copy {} -> {}: {}", path.display(), dest.display(), e)
                });
            }
        }
    }

    // Copy the gdk-pixbuf loader plugins into a `pixbuf-loaders/` subdir next
    // to the exe, then bake the absolute path to loaders.cache into the binary
    // as PIXBUF_LOADERS_CACHE so main.rs can set GDK_PIXBUF_MODULE_FILE at
    // startup. libgpod's artwork code (itdb_track_set_thumbnails_from_data /
    // ithumb-writer.c) calls gdk_pixbuf_new_from_*; pixbuf will silently
    // return NULL if it can't locate a loader for the input format, which
    // makes libgpod's artwork APIs no-op without any GError.
    let src_loaders = vendor.join("pixbuf-loaders");
    let dst_loaders = target_dir.join("pixbuf-loaders");
    if src_loaders.exists() {
        std::fs::create_dir_all(&dst_loaders).unwrap_or_else(|e| {
            panic!(
                "failed to create pixbuf-loaders dir {}: {}",
                dst_loaders.display(),
                e
            )
        });
        let entries = std::fs::read_dir(&src_loaders).unwrap_or_else(|e| {
            panic!(
                "failed to read vendor pixbuf-loaders dir {}: {}",
                src_loaders.display(),
                e
            )
        });
        for entry in entries {
            let entry = entry.expect("failed to read pixbuf-loaders entry");
            let path = entry.path();
            if path.is_file() {
                let dest = dst_loaders.join(path.file_name().unwrap());
                std::fs::copy(&path, &dest).unwrap_or_else(|e| {
                    panic!("failed to copy {} -> {}: {}", path.display(), dest.display(), e)
                });
            }
        }
    }
    // Regenerate loaders.cache pointing at the staged loaders dir (not the vendor
    // absolute paths). gdk-pixbuf-query-loaders.exe lives in MSYS2's mingw64 bin.
    let query_exe = std::path::Path::new(r"C:\msys64\mingw64\bin\gdk-pixbuf-query-loaders.exe");
    if query_exe.exists() {
        // Pass each staged loader DLL as an arg; the tool emits the cache to stdout.
        let loader_dlls: Vec<_> = std::fs::read_dir(&dst_loaders)
            .expect("read staged loaders")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |e| e == "dll"))
            .collect();
        let output = std::process::Command::new(query_exe)
            .args(&loader_dlls)
            .output()
            .expect("run gdk-pixbuf-query-loaders");
        if !output.status.success() {
            panic!(
                "gdk-pixbuf-query-loaders failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::write(dst_loaders.join("loaders.cache"), &output.stdout)
            .expect("write staged loaders.cache");
    } else {
        // Fall back to the vendored cache (dev-tree paths) if MSYS2's query tool
        // isn't available. This is the previous Phase 1 behavior.
        let src_cache = src_loaders.join("loaders.cache");
        let dst_cache = dst_loaders.join("loaders.cache");
        if src_cache.exists() {
            std::fs::copy(&src_cache, &dst_cache).expect("copy vendor loaders.cache");
        }
    }
    println!(
        "cargo:rustc-env=PIXBUF_LOADERS_CACHE={}",
        dst_loaders.join("loaders.cache").display()
    );
    println!("cargo:rerun-if-changed={}", src_loaders.display());
}
