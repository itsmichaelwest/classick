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
}
