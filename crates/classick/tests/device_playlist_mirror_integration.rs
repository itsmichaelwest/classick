use classick::ipod::device_playlists::{adopt_from_ipod, mirror_to_ipod};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

fn scratch(label: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-tmp")
        .join(format!(
            "playlist-mirror-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn device_mirror(mount: &Path) -> PathBuf {
    mount.join("iPod_Control/classick/playlists")
}

#[test]
fn mirror_copies_playlist_files_and_subscriptions() {
    let root = scratch("write");
    let mount = root.join("mount");
    let playlists = root.join("playlists");
    let subscriptions = root.join("subscriptions.json");
    std::fs::create_dir_all(&playlists).unwrap();
    std::fs::write(playlists.join("mix.m3u8"), b"#EXTM3U\n").unwrap();
    std::fs::write(&subscriptions, b"{\"version\":1}").unwrap();

    mirror_to_ipod(&mount, &playlists, &subscriptions);

    assert_eq!(
        std::fs::read(device_mirror(&mount).join("mix.m3u8")).unwrap(),
        b"#EXTM3U\n"
    );
    assert_eq!(
        std::fs::read(device_mirror(&mount).join("subscriptions.json")).unwrap(),
        b"{\"version\":1}"
    );
}

#[test]
fn adopt_copies_a_device_mirror_only_into_an_empty_host() {
    let root = scratch("adopt");
    let mount = root.join("mount");
    let mirror = device_mirror(&mount);
    std::fs::create_dir_all(&mirror).unwrap();
    std::fs::write(mirror.join("mix.m3u8"), b"playlist").unwrap();
    std::fs::write(mirror.join("subscriptions.json"), b"subscriptions").unwrap();
    let playlists = root.join("host/playlists");
    let subscriptions = root.join("host/devices/serial/subscriptions.json");

    assert_eq!(adopt_from_ipod(&mount, &playlists, &subscriptions), 1);
    assert_eq!(
        std::fs::read(playlists.join("mix.m3u8")).unwrap(),
        b"playlist"
    );
    assert_eq!(std::fs::read(subscriptions).unwrap(), b"subscriptions");
}

#[test]
fn adopt_never_merges_over_either_existing_host_artifact() {
    let root = scratch("preserve");
    let mount = root.join("mount");
    let mirror = device_mirror(&mount);
    std::fs::create_dir_all(&mirror).unwrap();
    std::fs::write(mirror.join("mix.m3u8"), b"device").unwrap();
    let playlists = root.join("host/playlists");
    let subscriptions = root.join("host/subscriptions.json");
    std::fs::create_dir_all(subscriptions.parent().unwrap()).unwrap();
    std::fs::write(&subscriptions, b"host").unwrap();

    assert_eq!(adopt_from_ipod(&mount, &playlists, &subscriptions), 0);
    assert!(!playlists.join("mix.m3u8").exists());
    assert_eq!(std::fs::read(subscriptions).unwrap(), b"host");
}
