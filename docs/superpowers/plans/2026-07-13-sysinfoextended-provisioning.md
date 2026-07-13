# SysInfoExtended Provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provision a correct per-model `SysInfoExtended` onto the iPod before libgpod opens the DB, so libgpod emits the artwork ithmb format set the firmware actually reads (notably `F1069`) and cover art displays.

**Architecture:** A new pure-Rust module resolves the connected iPod's `ModelNumStr` → an embedded per-model `SysInfoExtended` template (vendored, CC0), injects the device's real FireWireGUID, and writes `iPod_Control/Device/SysInfoExtended` before `OwnedDb::open`. libgpod reads it during device init and generates the correct thumbnails. Non-fatal: any provisioning failure warns and continues.

**Tech Stack:** Rust (`std`, `anyhow`, `tracing`, `include_bytes!`), the vendored libgpod, existing `device::resolve_libgpod_identity`.

## Global Constraints

- **Reuse existing detection:** `device::resolve_libgpod_identity(mount) -> Result<LibgpodIdentity>` where `pub struct LibgpodIdentity { pub firewire_guid: String, pub model_num_str: String }`. Do NOT add new device detection.
- **Provisioning is non-fatal.** Never fail a sync because provisioning failed — worst case is the pre-existing "art may not display" state. Match `ipod/db.rs`'s "add track without art rather than fail" philosophy.
- **Persistent write** to `iPod_Control/Device/SysInfoExtended` (overwrite, idempotent). No cleanup logic. (Validated safe + inert to macOS Music.app this session; Windows/iTunes re-test deferred.)
- **Data is CC0** from `github.com/dstaley/ipod-sysinfo` — free to vendor. Add an attribution note.
- **Correct-template-per-model.** Never substitute a near-model; unknown `ModelNumStr` → skip + `warn!`. A wrong template is worse than none.
- **GUID normalization:** `resolve_libgpod_identity` returns e.g. `0x000A27002138B0A8`; the plist form is bare uppercase hex `000A27002138B0A8` (strip a leading `0x`/`0X`, uppercase).
- **macOS is the build+verify target.** Verify with `cargo test -p classick` and the on-device smoke (Task 6). Windows/Linux: compile-clean only (this is pure cross-platform Rust — no `#[cfg]` needed).
- **`tracing` only, no `println!`** outside `examples/` (stdout is the IPC wire).
- **Commits:** Conventional Commits; scopes `ipod`, `artwork`, `apply-loop`, `docs`, `chore`. Stage named files; never `git add -A`; never amend; never `--no-verify`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/classick/src/apply_loop.rs` | (1) Revert temp `art-debug` tracing. (5) Resolve identity + call `provision` before `OwnedDb::open` at the apply path. |
| `crates/classick/src/ipod/db.rs` | (1) Revert temp `art-debug` tracing in `add_track_with_file`. |
| `crates/classick/src/ipod/sysinfo_provision.rs` | **New.** `inject_guid`, `template_for_model`, `provision`, the `ModelNumStr → template` table. Pure + tested. |
| `crates/classick/src/ipod/mod.rs` (or `lib.rs`) | Declare `pub mod sysinfo_provision;`. |
| `crates/classick/data/sysinfo-extended/*.plist` | **New.** Vendored CC0 per-model templates. |
| `crates/classick/data/sysinfo-extended/ATTRIBUTION.md` | **New.** CC0 source + provenance note. |
| `crates/classick/examples/art-audit.rs` | Keep (verification tool). `art-check.rs` may be removed. |
| `docs/SCSI.md` | Note the reversal of the "don't write SysInfoExtended" decision. |
| `LEARNINGS.md` | One-line gotcha: artwork needs per-model SysInfoExtended (F1069). |

---

## Task 1: Revert temporary art-debug tracing

The debugging session left `art-debug` `tracing::info!` calls in two files. Remove them so the branch is clean before the real feature lands. Keep `examples/art-audit.rs`.

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (the `art-debug: extracted …` / `art-debug: has_embedded_art=false …` lines in `transcode_one`)
- Modify: `crates/classick/src/ipod/db.rs` (the `art-debug: set_thumbnails_from_data …` block in `add_track_with_file`)

- [ ] **Step 1: Remove the tracing in `apply_loop.rs`.** In `transcode_one`, the art block currently reads:

```rust
    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        // TEMP(art-debug): trace where cover art is lost end-to-end.
        tracing::info!("art-debug: extracted {} bytes for {}", bytes.len(), src.path.display());
        Some(bytes)
    } else {
        tracing::info!("art-debug: has_embedded_art=false for {}", src.path.display());
        None
    };
```

Restore it to:

```rust
    let art = if has_embedded_art(&probe) {
        let art_path = transcode::temp_art_path();
        transcode::extract_cover_art(&src.path, &art_path, &config.ffmpeg)?;
        let bytes = std::fs::read(&art_path)?;
        let _ = std::fs::remove_file(&art_path);
        Some(bytes)
    } else {
        None
    };
```

- [ ] **Step 2: Remove the tracing in `ipod/db.rs`.** In `add_track_with_file`, delete the `TEMP(art-debug)` block so it reads:

```rust
            if let Some(bytes) = art {
                let ok = ffi::itdb_track_set_thumbnails_from_data(
                    track,
                    bytes.as_ptr(),
                    bytes.len() as _,
                );
                if ok == 0 {
```

(i.e. remove the `tracing::info!("art-debug: set_thumbnails_from_data(...) -> ok={ok}, has_artwork=..., artwork_size=...")` call between the `set_thumbnails_from_data` call and the `if ok == 0 {`).

- [ ] **Step 3: Verify no art-debug remains**

Run: `grep -rn "art-debug" crates/classick/src`
Expected: no output.

- [ ] **Step 4: Build + test**

Run: `cargo build -p classick && cargo test -p classick`
Expected: clean build; all tests pass (was 210).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/apply_loop.rs crates/classick/src/ipod/db.rs
git commit -m "chore(artwork): remove temporary art-debug tracing"
```

---

## Task 2: `inject_guid` — pure GUID substitution

**Files:**
- Create: `crates/classick/src/ipod/sysinfo_provision.rs`
- Modify: `crates/classick/src/ipod/mod.rs` — add `pub mod sysinfo_provision;` (check whether `ipod` is a `mod.rs` dir module or declared in `lib.rs`; add the declaration wherever the other `ipod::` submodules are declared).
- Test: in `sysinfo_provision.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `pub fn inject_guid(template: &[u8], firewire_guid: &str) -> anyhow::Result<Vec<u8>>` — returns the template bytes with the `<key>FireWireGUID</key>`'s following `<string>…</string>` value replaced by the normalized (bare uppercase hex) GUID.

- [ ] **Step 1: Write the failing test** — create `crates/classick/src/ipod/sysinfo_provision.rs`:

```rust
//! Provision a per-model `SysInfoExtended` onto the iPod so libgpod emits the
//! artwork ithmb format set the firmware reads. See
//! docs/superpowers/specs/2026-07-13-sysinfoextended-provisioning-design.md.

use anyhow::{anyhow, Result};
use std::path::Path;

/// Substitute `firewire_guid` into `template`'s `<key>FireWireGUID</key>` value.
/// The GUID is normalized to the plist form: a leading `0x`/`0X` is stripped and
/// the hex is uppercased (e.g. `0x000a2700…` -> `000A2700…`). The rest of the
/// template — crucially the `ImageSpecifications` — is untouched.
pub fn inject_guid(template: &[u8], firewire_guid: &str) -> Result<Vec<u8>> {
    let xml = std::str::from_utf8(template)
        .map_err(|e| anyhow!("SysInfoExtended template is not UTF-8: {e}"))?;
    let normalized = firewire_guid
        .strip_prefix("0x")
        .or_else(|| firewire_guid.strip_prefix("0X"))
        .unwrap_or(firewire_guid)
        .to_ascii_uppercase();

    let key = "<key>FireWireGUID</key>";
    let key_at = xml
        .find(key)
        .ok_or_else(|| anyhow!("template missing <key>FireWireGUID</key>"))?;
    let open = xml[key_at..]
        .find("<string>")
        .map(|i| key_at + i + "<string>".len())
        .ok_or_else(|| anyhow!("no <string> after FireWireGUID key"))?;
    let close = xml[open..]
        .find("</string>")
        .map(|i| open + i)
        .ok_or_else(|| anyhow!("unterminated <string> for FireWireGUID"))?;

    let mut out = String::with_capacity(xml.len());
    out.push_str(&xml[..open]);
    out.push_str(&normalized);
    out.push_str(&xml[close..]);
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "<plist><dict>\
<key>FireWireGUID</key><string>000A27002150925D</string>\
<key>ImageSpecifications</key><array><key>1069</key><dict><key>FormatId</key><integer>1069</integer></dict></array>\
</dict></plist>";

    #[test]
    fn replaces_guid_and_keeps_image_specs() {
        let out = inject_guid(SAMPLE.as_bytes(), "0x000A27002138B0A8").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<key>FireWireGUID</key><string>000A27002138B0A8</string>"));
        assert!(!s.contains("000A27002150925D"));
        // ImageSpecifications (incl. the F1069 format) must be intact.
        assert!(s.contains("<key>ImageSpecifications</key>"));
        assert!(s.contains("<integer>1069</integer>"));
    }

    #[test]
    fn normalizes_lowercase_and_bare_guid() {
        let lower = inject_guid(SAMPLE.as_bytes(), "0x000a27002138b0a8").unwrap();
        assert!(String::from_utf8(lower).unwrap().contains("<string>000A27002138B0A8</string>"));
        let bare = inject_guid(SAMPLE.as_bytes(), "000A27002138B0A8").unwrap();
        assert!(String::from_utf8(bare).unwrap().contains("<string>000A27002138B0A8</string>"));
    }

    #[test]
    fn errors_when_no_guid_key() {
        assert!(inject_guid(b"<plist><dict></dict></plist>", "000A").is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p classick sysinfo_provision`
Expected: FAIL — module not declared / `inject_guid` not found.

- [ ] **Step 3: Declare the module.** Add `pub mod sysinfo_provision;` alongside the other `ipod` submodule declarations (find with `grep -rn "pub mod db;" crates/classick/src` — declare it in the same file, likely `crates/classick/src/ipod/mod.rs`). The `sysinfo_provision.rs` body from Step 1 is the implementation.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p classick sysinfo_provision`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/ipod/sysinfo_provision.rs crates/classick/src/ipod/mod.rs
git commit -m "feat(ipod): add SysInfoExtended FireWireGUID injection"
```

---

## Task 3: Vendor CC0 templates + `template_for_model` resolution table

**Files:**
- Create: `crates/classick/data/sysinfo-extended/*.plist` (vendored)
- Create: `crates/classick/data/sysinfo-extended/ATTRIBUTION.md`
- Modify: `crates/classick/src/ipod/sysinfo_provision.rs` (table + `template_for_model`)
- Test: in `sysinfo_provision.rs`

**Interfaces:**
- Produces: `pub fn template_for_model(model_num_str: &str) -> Option<&'static [u8]>` — returns the embedded template for a known `ModelNumStr`, else `None`.

**Data provenance (CC0):** files come from `github.com/dstaley/ipod-sysinfo` under `models/`:
- iPod Classic Late-2009 (MC293) — `models/iPod/6th generation/Late_2009_SysInfoExtended.plist` → vendor as `classic-late2009.plist` (**verified on-device this session**).
- iPod Classic 6G/6.5G — `models/iPod/6th generation/SysInfoExtended.plist` → `classic-6g.plist`.
- iPod Video 5G — `models/iPod/5th generation/SysInfoExtended.plist` → `video-5g.plist`.
- iPod Photo/4G — `models/iPod/4th generation/SysInfoExtended.plist` → `photo-4g.plist`.
- iPod nano 1G–4G — `models/iPod nano/{1st,2nd,3rd,4th} generation/SysInfoExtended.plist` → `nano-{1,2,3,4}g.plist`.

Fetch each with: `gh api "repos/dstaley/ipod-sysinfo/contents/<url-encoded-path>" | python3 -c "import sys,json,base64; open('<dest>','wb').write(base64.b64decode(json.load(sys.stdin)['content']))"` (spaces in paths → `%20`). Verify each is non-empty and contains `<key>ImageSpecifications</key>` before committing.

**ModelNumStr → template mapping:** transcribe the `ModelNumStr` values for each generation from libgpod's authoritative `ipod_info_table` in [`src/itdb_device.c`](https://github.com/gtkpod/libgpod/blob/master/src/itdb_device.c) (the same table libgpod uses to pick fallback formats — so our selection can never disagree with it). Each `SysInfo` file in the dstaley model folders also lists that model's `ModelNumStr` and corroborates the folder→generation mapping. The **verified anchor** that a test pins: `MC293 → classic-late2009.plist`.

- [ ] **Step 1: Vendor the plists + attribution.** Fetch the files above into `crates/classick/data/sysinfo-extended/`. Create `ATTRIBUTION.md`:

```markdown
# SysInfoExtended templates

Source: https://github.com/dstaley/ipod-sysinfo (License: CC0-1.0, public domain).
These are per-model iPod device-capability plists. classick writes the matching
one to `iPod_Control/Device/SysInfoExtended` so libgpod generates the correct
artwork thumbnail (`.ithmb`) formats. GUID is injected at write time per device.
```

Verify: `for f in crates/classick/data/sysinfo-extended/*.plist; do grep -q ImageSpecifications "$f" && echo "OK $f" || echo "BAD $f"; done` → all `OK`.

- [ ] **Step 2: Write the failing test** — append to `sysinfo_provision.rs` `#[cfg(test)]`:

```rust
    #[test]
    fn resolves_mc293_to_classic_late2009_template() {
        let t = template_for_model("MC293").expect("MC293 must resolve");
        let xml = std::str::from_utf8(t).unwrap();
        assert!(xml.contains("<key>ImageSpecifications</key>"));
        // Late-2009 Classic exposes the F1069 cover format that the firmware reads.
        assert!(xml.contains("<integer>1069</integer>"));
    }

    #[test]
    fn unknown_model_resolves_to_none() {
        assert!(template_for_model("XPID_9999").is_none());
        assert!(template_for_model("").is_none());
    }

    #[test]
    fn every_embedded_template_has_image_specifications() {
        for (model, bytes) in super::ALL_TEMPLATES {
            let xml = std::str::from_utf8(bytes)
                .unwrap_or_else(|_| panic!("{model}: template not UTF-8"));
            assert!(
                xml.contains("<key>ImageSpecifications</key>"),
                "{model}: template missing ImageSpecifications"
            );
        }
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p classick sysinfo_provision`
Expected: FAIL — `template_for_model` / `ALL_TEMPLATES` not found.

- [ ] **Step 4: Implement the table.** Add to `sysinfo_provision.rs`:

```rust
// Embedded per-model SysInfoExtended templates (CC0, see data/.../ATTRIBUTION.md).
const CLASSIC_LATE2009: &[u8] = include_bytes!("../../data/sysinfo-extended/classic-late2009.plist");
const CLASSIC_6G: &[u8] = include_bytes!("../../data/sysinfo-extended/classic-6g.plist");
const VIDEO_5G: &[u8] = include_bytes!("../../data/sysinfo-extended/video-5g.plist");
const PHOTO_4G: &[u8] = include_bytes!("../../data/sysinfo-extended/photo-4g.plist");
const NANO_1G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-1g.plist");
const NANO_2G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-2g.plist");
const NANO_3G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-3g.plist");
const NANO_4G: &[u8] = include_bytes!("../../data/sysinfo-extended/nano-4g.plist");

/// `(ModelNumStr, template)` — ModelNumStr values transcribed from libgpod's
/// `ipod_info_table` (src/itdb_device.c). MC293 (Classic Late-2009) is the
/// hardware-verified entry; the rest are best-effort (see spec §"Model set").
/// Add rows here to support more models — one line + one vendored plist.
const TABLE: &[(&str, &[u8])] = &[
    // iPod Classic Late-2009 (VERIFIED on-device).
    ("MC293", CLASSIC_LATE2009), // 160GB
    ("MC297", CLASSIC_LATE2009), // 160GB (black)
    // iPod Classic 6G / 6.5G — transcribe remaining ModelNumStr rows from
    // libgpod ipod_info_table (e.g. MA002/MA444/MB029/MB147/MB145/MB565 …),
    // each -> CLASSIC_6G.
    ("MB147", CLASSIC_6G),
    // iPod Video 5G — transcribe (MA002/MA146/MA444/MA446/MA448 …) -> VIDEO_5G.
    // iPod Photo/4G — transcribe -> PHOTO_4G.
    // iPod nano 1G–4G — transcribe -> NANO_{1..4}G.
];

/// All embedded templates, for the validity test.
#[cfg(test)]
pub(super) const ALL_TEMPLATES: &[(&str, &[u8])] = &[
    ("classic-late2009", CLASSIC_LATE2009), ("classic-6g", CLASSIC_6G),
    ("video-5g", VIDEO_5G), ("photo-4g", PHOTO_4G),
    ("nano-1g", NANO_1G), ("nano-2g", NANO_2G), ("nano-3g", NANO_3G), ("nano-4g", NANO_4G),
];

/// Embedded SysInfoExtended template for a resolved `ModelNumStr`, or `None`
/// for a model we don't ship a template for (caller skips + warns; never
/// substitute a near-model — a wrong template is worse than none).
pub fn template_for_model(model_num_str: &str) -> Option<&'static [u8]> {
    TABLE.iter().find(|(m, _)| *m == model_num_str).map(|(_, t)| *t)
}
```

**Implementer note:** the committed deliverable is the Classic family (MC293 verified). Transcribe the remaining generations' ModelNumStr rows from the cited libgpod table to activate the best-effort models; each `include_bytes!` file must exist (Step 1) or the crate won't compile. If a generation's plist isn't vendored, omit its `const` + rows rather than leaving a dangling `include_bytes!`.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p classick sysinfo_provision`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/classick/data/sysinfo-extended crates/classick/src/ipod/sysinfo_provision.rs
git commit -m "feat(ipod): embed per-model SysInfoExtended templates + resolution"
```

---

## Task 4: `provision` — resolve, inject, write (non-fatal)

**Files:**
- Modify: `crates/classick/src/ipod/sysinfo_provision.rs`
- Test: in `sysinfo_provision.rs`

**Interfaces:**
- Consumes: `template_for_model`, `inject_guid`, `crate::ipod::device::LibgpodIdentity`.
- Produces: `pub fn provision(mount: &Path, identity: &crate::ipod::device::LibgpodIdentity) -> anyhow::Result<()>` — resolves the template for `identity.model_num_str`, injects `identity.firewire_guid`, writes `<mount>/iPod_Control/Device/SysInfoExtended`. Returns `Ok(())` when it provisioned OR skipped (unknown model, logged). Returns `Err` only on a genuine write/inject failure (the caller logs + continues — non-fatal).

- [ ] **Step 1: Write the failing test** — append to `sysinfo_provision.rs` `#[cfg(test)]`:

```rust
    use crate::ipod::device::LibgpodIdentity;
    use std::path::PathBuf;

    fn temp_mount() -> PathBuf {
        // Unique per-process temp mount with the Device dir libgpod expects.
        let base = std::env::temp_dir().join(format!("classick-provision-{}", std::process::id()));
        std::fs::create_dir_all(base.join("iPod_Control/Device")).unwrap();
        base
    }

    #[test]
    fn provision_writes_sysinfoextended_for_known_model() {
        let mount = temp_mount();
        let id = LibgpodIdentity { firewire_guid: "0x000A27002138B0A8".into(), model_num_str: "MC293".into() };
        provision(&mount, &id).unwrap();
        let written = std::fs::read_to_string(mount.join("iPod_Control/Device/SysInfoExtended")).unwrap();
        assert!(written.contains("<string>000A27002138B0A8</string>"));
        assert!(written.contains("<integer>1069</integer>"));
        // Idempotent: running again is fine.
        provision(&mount, &id).unwrap();
        let _ = std::fs::remove_dir_all(&mount);
    }

    #[test]
    fn provision_skips_unknown_model_without_writing() {
        let mount = temp_mount();
        let id = LibgpodIdentity { firewire_guid: "0x000A27002138B0A8".into(), model_num_str: "XPID_9999".into() };
        provision(&mount, &id).unwrap(); // Ok, but no file
        assert!(!mount.join("iPod_Control/Device/SysInfoExtended").exists());
        let _ = std::fs::remove_dir_all(&mount);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p classick sysinfo_provision`
Expected: FAIL — `provision` not found.

- [ ] **Step 3: Implement `provision`.** Add to `sysinfo_provision.rs`:

```rust
/// Write the per-model `SysInfoExtended` (with the device's real GUID) to
/// `<mount>/iPod_Control/Device/SysInfoExtended` so libgpod generates correct
/// artwork ithmb formats. MUST run before `OwnedDb::open`. Non-fatal to callers:
/// returns Ok when it provisioned OR skipped an unknown model; returns Err only
/// on a real write/inject failure, which the caller logs and continues past.
pub fn provision(mount: &Path, identity: &crate::ipod::device::LibgpodIdentity) -> Result<()> {
    let Some(template) = template_for_model(&identity.model_num_str) else {
        tracing::warn!(
            "no SysInfoExtended template for ModelNumStr {:?}; artwork may not display. \
             Add a template + table row to support this model.",
            identity.model_num_str
        );
        return Ok(());
    };
    let xml = inject_guid(template, &identity.firewire_guid)?;
    let path = mount.join("iPod_Control").join("Device").join("SysInfoExtended");
    std::fs::write(&path, &xml)
        .map_err(|e| anyhow!("writing {}: {e}", path.display()))?;
    tracing::info!(
        "provisioned SysInfoExtended for {} ({} bytes)",
        identity.model_num_str, xml.len()
    );
    Ok(())
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p classick sysinfo_provision`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/ipod/sysinfo_provision.rs
git commit -m "feat(ipod): SysInfoExtended provision() — resolve, inject, write"
```

---

## Task 5: Wire provisioning into the apply loop + docs

**Files:**
- Modify: `crates/classick/src/apply_loop.rs` (apply path, around lines 303–322)
- Modify: `docs/SCSI.md`, `LEARNINGS.md`
- Test: existing suite (the unit tests cover the module; wiring is verified by build + on-device smoke in Task 6)

**Interfaces:**
- Consumes: `crate::ipod::sysinfo_provision::provision`, existing `device::resolve_libgpod_identity`.

- [ ] **Step 1: Reorder identity resolution + add provisioning.** In `apply_loop.rs`, the apply path currently opens the DB then resolves identity:

```rust
    let sync_result: Result<RunOutcome> = (|| -> Result<RunOutcome> {
        let db = OwnedDb::open(Path::new(&mount))?;
        // ... comment ...
        let identity = device::resolve_libgpod_identity(Path::new(&mount))?;
        progress.log(format!(
            "iPod identity: FirewireGuid={}, ModelNumStr={}",
            identity.firewire_guid, identity.model_num_str,
        ));
        unsafe {
            let device_ptr = (*db.as_ptr()).device;
            device::set_firewire_guid(device_ptr, &identity.firewire_guid)?;
            device::set_model_num(device_ptr, &identity.model_num_str)?;
        }
```

Rewrite so identity is resolved and `SysInfoExtended` provisioned **before** `OwnedDb::open` (libgpod reads the file at open), reusing the same `identity` for `set_model_num`:

```rust
    let sync_result: Result<RunOutcome> = (|| -> Result<RunOutcome> {
        // Resolve identity BEFORE opening the DB: we need ModelNumStr to pick
        // the SysInfoExtended template, and libgpod reads that file during
        // OwnedDb::open. (FirewireGuid + ModelNumStr are also pushed into the
        // device below for DB signing.)
        let identity = device::resolve_libgpod_identity(Path::new(&mount))?;
        progress.log(format!(
            "iPod identity: FirewireGuid={}, ModelNumStr={}",
            identity.firewire_guid, identity.model_num_str,
        ));
        // Provision the per-model SysInfoExtended so libgpod emits the artwork
        // ithmb formats the firmware reads (notably F1069). Non-fatal: art is
        // best-effort, never abort a sync over it.
        if let Err(e) = crate::ipod::sysinfo_provision::provision(Path::new(&mount), &identity) {
            progress.log(format!("SysInfoExtended provisioning failed (art may not display): {e:#}"));
        }
        let db = OwnedDb::open(Path::new(&mount))?;
        unsafe {
            let device_ptr = (*db.as_ptr()).device;
            device::set_firewire_guid(device_ptr, &identity.firewire_guid)?;
            device::set_model_num(device_ptr, &identity.model_num_str)?;
        }
```

(Remove the now-duplicated `resolve_libgpod_identity` + `progress.log("iPod identity…")` that were after the open. Keep the original explanatory comment about why ModelNumStr matters, moved up with the resolve.)

- [ ] **Step 2: Build + test**

Run: `cargo build -p classick && cargo test -p classick`
Expected: clean build; all tests pass.

- [ ] **Step 3: Update `docs/SCSI.md`.** Under "What we built so far" / the decisions list, append a note:

```markdown
### 2026-07-13 update: we now DO write SysInfoExtended (for artwork)

The "we have NOT / decided NOT to write SysInfoExtended to disk" decision above
is reversed for macOS artwork. Root cause: without SysInfoExtended, libgpod's
built-in per-model format guess omits cover-art formats the firmware reads (e.g.
F1069 on the Late-2009 Classic), so art never displays. classick now writes a
per-model SysInfoExtended (embedded CC0 templates, GUID injected) to
`iPod_Control/Device/SysInfoExtended` before DB open. Validated on-device: art
displays, the iPod firmware reads the DB, and macOS Music.app reads the iPod
identically with or without the file (safe + inert). Windows/iTunes re-test
pending. See docs/superpowers/specs/2026-07-13-sysinfoextended-provisioning-design.md.
```

- [ ] **Step 4: Add a `LEARNINGS.md` bullet** (check for duplicates first):

```markdown
- iPod cover art needs a per-model `SysInfoExtended` at `iPod_Control/Device/`
  before libgpod opens the DB. Without it libgpod guesses the artwork ithmb
  format set and omits ones the firmware reads (e.g. `F1069` on Classic
  Late-2009) → valid thumbnails written but never displayed. classick provisions
  it (`ipod::sysinfo_provision`) from embedded CC0 templates.
```

- [ ] **Step 5: Commit**

```bash
git add crates/classick/src/apply_loop.rs docs/SCSI.md LEARNINGS.md
git commit -m "feat(apply-loop): provision SysInfoExtended before DB open for artwork"
```

---

## Task 6: Final verification (build, tests, on-device smoke)

**Files:** none (verification only)

- [ ] **Step 1: Full workspace build + tests**

Run: `cargo build --release -p classick && cargo test -p classick`
Expected: clean; all pass (211+ — the new `sysinfo_provision` tests added).

- [ ] **Step 2: Confirm no art-debug / no dangling includes**

Run: `grep -rn "art-debug" crates/classick/src` → empty.
Run: `cargo build -p classick 2>&1 | grep -i "include_bytes\|No such file"` → empty (every vendored plist referenced exists).

- [ ] **Step 3: On-device smoke (REQUIRED before merge).** With the iPod mounted:

```bash
# Clean slate, then a real sync through the normal apply path (provisioning active):
cargo run --release --example wipe-tracks -- /Volumes/IPOD
rm -f "$HOME/Library/Application Support/classick/manifest.json"
./target/release/classick --apply --source "<an album dir with embedded art>" --ipod /Volumes/IPOD
# Confirm the file was provisioned + correct ithmb formats:
ls /Volumes/IPOD/iPod_Control/Device/SysInfoExtended        # exists
ls /Volumes/IPOD/iPod_Control/Artwork/*.ithmb | grep F1069  # F1069 present
cargo run --release --example art-audit -- /Volumes/IPOD    # has_artwork=1 for all
```

Then **eject and visually confirm the album shows cover art on the iPod screen** (the definitive check — `has_artwork=1` alone is not proof of display, which is how this bug hid).

Expected: `SysInfoExtended` present, `F1069` ithmb present, all tracks `has_artwork=1`, **art visible on-device**.

- [ ] **Step 4: (Optional) remove `art-check.rs`** if not wanted:

```bash
git rm crates/classick/examples/art-check.rs
```

- [ ] **Step 5: Commit any remaining (e.g. art-check removal)**

```bash
git add -u crates/classick/examples
git commit -m "chore(artwork): drop art-check spike example"
```

---

## Self-review notes

- **Spec coverage:** embedded data + attribution (Task 3), model→template resolution from libgpod table (Task 3), GUID injection (Task 2), write-before-open + persistent + idempotent + non-fatal (Tasks 4–5), reuse `resolve_libgpod_identity` (Task 5), revert debug + keep art-audit (Tasks 1, 6), docs reversal (Task 5), on-device verify incl. visual (Task 6), model set with MC293 verified + others best-effort (Task 3). All covered.
- **Type consistency:** `inject_guid(&[u8], &str) -> Result<Vec<u8>>`, `template_for_model(&str) -> Option<&'static [u8]>`, `provision(&Path, &LibgpodIdentity) -> Result<()>` are used consistently across Tasks 2/3/4/5. `LibgpodIdentity { firewire_guid, model_num_str }` matches the existing struct.
- **Known best-effort gap:** non-Classic templates ship unverified on hardware (spec-accepted). Task 3's table leaves those rows as an explicit transcription step from the cited libgpod source; only vendored plists get `const`/rows (no dangling `include_bytes!`).
