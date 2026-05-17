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

    // Re-run if the header changes
    let header = vendor.join("include").join("gpod").join("itdb.h");
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed=build.rs");

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
        .allowlist_type("GError")
        .allowlist_type("GList")
        .layout_tests(false)
        .generate()
        .expect("bindgen failed to generate libgpod bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("libgpod_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write bindings");

    // Ensure the DLL is alongside the exe for `cargo run`
    let dll_src = vendor.join("bin").join("gpod.dll");
    let target_dir = manifest_dir
        .join("target")
        .join(env::var("PROFILE").unwrap());
    if dll_src.exists() {
        let _ = std::fs::create_dir_all(&target_dir);
        let _ = std::fs::copy(&dll_src, target_dir.join("gpod.dll"));
        // Copy GLib runtime deps too
        if let Ok(entries) = std::fs::read_dir(vendor.join("bin")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("dll") {
                    let _ = std::fs::copy(&path, target_dir.join(path.file_name().unwrap()));
                }
            }
        }
    }
}
