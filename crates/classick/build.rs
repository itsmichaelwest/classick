use std::env;
use std::path::{Path, PathBuf};

/// Default MSYS2 install location on Windows. Overridable via the `MSYS2_ROOT`
/// env var for users with a non-default install.
const DEFAULT_MSYS2_ROOT: &str = r"C:\msys64";

fn main() {
    println!("cargo:rerun-if-env-changed=MSYS2_ROOT");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Use CARGO_CFG_TARGET_OS (not `cfg!(windows)`) so cross-compilation
    // takes the right path. In a build script, `cfg(windows)` reflects the
    // host, not the target.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "windows" => build_windows(&manifest_dir, &out_dir),
        _ => build_pkg_config(&out_dir),
    }

    // Native macOS device identity + hotplug FFI (src/ipod/macos_iokit.rs).
    if target_os == "macos" {
        build_macos_netfs(&manifest_dir);
        println!("cargo:rustc-link-lib=framework=IOKit");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=NetFS");
    }
}

fn build_macos_netfs(manifest_dir: &Path) {
    let shim = manifest_dir.join("src/daemon/netfs_shim.m");
    println!("cargo:rerun-if-changed={}", shim.display());
    cc::Build::new()
        .file(shim)
        .flag("-fblocks")
        .compile("classick_netfs_shim");
}

// ---------------------------------------------------------------------------
// Windows path: vendored libgpod under crates/classick/vendor/libgpod,
// MSYS2 MinGW64 sysroot for the GLib headers bindgen needs, vendored
// pixbuf loaders + refalac binaries staged into target/.
// ---------------------------------------------------------------------------

fn msys2_root() -> PathBuf {
    PathBuf::from(env::var("MSYS2_ROOT").unwrap_or_else(|_| DEFAULT_MSYS2_ROOT.to_string()))
}

fn build_windows(manifest_dir: &Path, out_dir: &Path) {
    let vendor = manifest_dir.join("vendor").join("libgpod");
    let mingw64 = msys2_root().join("mingw64");

    // Link the vendored import library.
    println!(
        "cargo:rustc-link-search=native={}",
        vendor.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=gpod");
    // GLib symbols (e.g. g_error_free) live in libglib-2.0-0.dll. We generated
    // an MSVC-format import lib (glib.lib) from its export table so Rust can
    // resolve those symbols at link time.
    println!("cargo:rustc-link-lib=dylib=glib");

    // Re-run if the header changes.
    let header = vendor.join("include").join("gpod").join("itdb.h");
    println!("cargo:rerun-if-changed={}", header.display());
    println!(
        "cargo:rerun-if-changed={}",
        vendor.join("lib").join("gpod.lib").display()
    );

    // GLib headers ship with the MSYS2 MinGW64 toolchain used to build libgpod.
    // bindgen needs them because itdb.h includes <glib.h> / <glib-object.h>.
    let glib_include = mingw64.join("include").join("glib-2.0");
    let glib_config_include = mingw64.join("lib").join("glib-2.0").join("include");

    let bindings = bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg(format!("-I{}", vendor.join("include").display()))
        .clang_arg(format!("-I{}", glib_include.display()))
        .clang_arg(format!("-I{}", glib_config_include.display()))
        .pipe(allowlists)
        .layout_tests(false)
        .generate()
        .expect("bindgen failed to generate libgpod bindings");

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
            panic!(
                "failed to create target dir {}: {}",
                target_dir.display(),
                e
            )
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
                    panic!(
                        "failed to copy {} -> {}: {}",
                        path.display(),
                        dest.display(),
                        e
                    )
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
                    panic!(
                        "failed to copy {} -> {}: {}",
                        path.display(),
                        dest.display(),
                        e
                    )
                });
            }
        }
    }
    // Regenerate loaders.cache pointing at the staged loaders dir (not the vendor
    // absolute paths). gdk-pixbuf-query-loaders.exe lives in MSYS2's mingw64 bin.
    let query_exe = mingw64.join("bin").join("gdk-pixbuf-query-loaders.exe");
    if query_exe.exists() {
        // Pass each staged loader DLL as an arg; the tool emits the cache to stdout.
        let loader_dlls: Vec<_> = std::fs::read_dir(&dst_loaders)
            .expect("read staged loaders")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |e| e == "dll"))
            .collect();
        let output = std::process::Command::new(&query_exe)
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

    // -----------------------------------------------------------------------
    // Phase 3 Task 5: copy vendored refalac64.exe + its DLLs alongside the exe
    // when present. Gracefully skips when vendor/refalac/ is empty/missing
    // (refalac is opt-in via --encoder refalac; default encoder is ffmpeg).
    // Users who want refalac either drop the qaac binaries into vendor/refalac/
    // (build.rs picks them up) OR put refalac64.exe on PATH (preflight finds
    // it via Config::refalac_path).
    // -----------------------------------------------------------------------
    let refalac_dir = manifest_dir.join("vendor").join("refalac");
    println!("cargo:rerun-if-changed={}", refalac_dir.display());
    let refalac_exe = refalac_dir.join("refalac64.exe");
    if refalac_exe.exists() {
        let entries = std::fs::read_dir(&refalac_dir).unwrap_or_else(|e| {
            panic!(
                "failed to read vendor refalac dir {}: {}",
                refalac_dir.display(),
                e
            )
        });
        for entry in entries {
            let entry = entry.expect("failed to read directory entry in vendor/refalac");
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str());
            if matches!(ext, Some("exe") | Some("dll")) {
                let dest = target_dir.join(path.file_name().unwrap());
                std::fs::copy(&path, &dest).unwrap_or_else(|e| {
                    panic!(
                        "failed to copy {} -> {}: {}",
                        path.display(),
                        dest.display(),
                        e
                    )
                });
            }
        }
    } else {
        println!(
            "cargo:warning=vendor/refalac/ empty; refalac encoder unavailable unless refalac64.exe is on PATH"
        );
    }
}

// ---------------------------------------------------------------------------
// Non-Windows path: pkg-config for libgpod + glib (the standard upstream
// gtk-pod install path on Linux + macOS). No vendored binaries, no pixbuf
// staging — gdk-pixbuf's system installation handles loader discovery.
//
// Distros that ship libgpod-1.0.pc:
//   Debian/Ubuntu: apt install libgpod-dev libglib2.0-dev
//   Fedora:        dnf install libgpod-devel glib2-devel
//   Arch:          pacman -S libgpod glib2
//   macOS:         no libgpod formula exists — run scripts/build-libgpod-macos.sh
//                  then export PKG_CONFIG_PATH to its prefix (that script prints
//                  the line; see also vendor/libgpod/BUILD-NOTES.md).
//
// pkg-config emits the cargo:rustc-link-search / cargo:rustc-link-lib lines
// itself when `.probe()` is called; we just collect the include paths to
// feed bindgen.
// ---------------------------------------------------------------------------

fn build_pkg_config(out_dir: &Path) {
    let libgpod = pkg_config::Config::new()
        .atleast_version("0.8")
        .probe("libgpod-1.0")
        .unwrap_or_else(|e| {
            panic!(
                "libgpod-1.0 not found via pkg-config: {e}\n\
                 Install libgpod-dev (Debian/Ubuntu), libgpod-devel (Fedora), \
                 libgpod (Arch / Homebrew), or build from source. See \
                 https://gitlab.gnome.org/Archive/libgpod"
            )
        });
    let glib = pkg_config::Config::new()
        .atleast_version("2.0")
        .probe("glib-2.0")
        .unwrap_or_else(|e| {
            panic!(
                "glib-2.0 not found via pkg-config: {e}\n\
                 Install libglib2.0-dev (Debian/Ubuntu), glib2-devel (Fedora), \
                 glib2 (Arch), or glib (Homebrew)."
            )
        });

    // Locate gpod/itdb.h inside one of libgpod's include paths.
    let header = libgpod
        .include_paths
        .iter()
        .map(|p| p.join("gpod").join("itdb.h"))
        .find(|p| p.exists())
        .unwrap_or_else(|| {
            panic!(
                "could not locate gpod/itdb.h in libgpod include paths: {:?}",
                libgpod.include_paths
            )
        });
    println!("cargo:rerun-if-changed={}", header.display());

    let mut builder = bindgen::Builder::default().header(header.to_str().unwrap());
    for inc in libgpod
        .include_paths
        .iter()
        .chain(glib.include_paths.iter())
    {
        builder = builder.clang_arg(format!("-I{}", inc.display()));
    }
    let bindings = builder
        .pipe(allowlists)
        .layout_tests(false)
        .generate()
        .expect("bindgen failed to generate libgpod bindings");

    bindings
        .write_to_file(out_dir.join("libgpod_bindings.rs"))
        .expect("failed to write bindings");

    // No DLL copy, no pixbuf staging, no refalac on non-Windows:
    //   - libgpod + glib are linked from system paths
    //   - gdk-pixbuf's system installation owns loaders.cache (via
    //     $XDG_DATA_DIRS); we don't override it. main.rs's
    //     GDK_PIXBUF_MODULE_FILE setter is cfg(windows) so the system
    //     default wins here.
    //   - refalac is Windows-only (it's a qaac binary that needs
    //     CoreAudioToolbox.dll); the ffmpeg encoder is the universal
    //     codepath.
}

// ---------------------------------------------------------------------------
// Shared bindgen allowlist. Identical for both Windows and non-Windows.
// ---------------------------------------------------------------------------

fn allowlists(b: bindgen::Builder) -> bindgen::Builder {
    b.allowlist_function("itdb_.*")
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
}

/// Pipe combinator so we can apply a function to a builder mid-chain. Stable
/// idiom; lets `allowlists` be shared between both code paths without
/// duplicating the long list.
trait Pipe: Sized {
    fn pipe<F: FnOnce(Self) -> Self>(self, f: F) -> Self {
        f(self)
    }
}
impl Pipe for bindgen::Builder {}
