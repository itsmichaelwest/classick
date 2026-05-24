# Codebase review — findings

**Date:** 2026-05-24
**Reviewer:** single serial pass (Claude Opus 4.7)
**Spec:** `docs/superpowers/specs/2026-05-24-codebase-review-design.md`

Live document — updated as the review pass progresses.

## Summary

**32 findings total.** Distribution:

| Category      | High | Medium | Low | Total |
| ---           | ---  | ---    | --- | ---   |
| bug-pattern   | 6    | 5      | 1   | 12    |
| hardcoded     | 2    | 3      | 2   | 7     |
| size          | 1    | 3      | 0   | 4     |
| boundary      | 0    | 1      | 2   | 3     |
| doc           | 0    | 0      | 4   | 4     |
| coverage      | 0    | 0      | 2   | 2     |

### Top priorities (high severity)

| ID    | Title                                                                          | Category     |
| ---   | ---                                                                            | ---          |
| F-02  | `"ipod-sync"` literal duplicated across 10+ sites (Rust + .NET)                | hardcoded    |
| F-08  | iPod filesystem path components scattered as bare string literals              | hardcoded    |
| F-09  | Two parallel iPod-detection paths use different presence criteria              | bug-pattern  |
| F-16  | `--ffmpeg` configured path silently ignored by the primary transcode path      | bug-pattern  |
| F-20  | `apply_loop::run` is a 740-line function and should be decomposed              | size         |
| F-21  | F-16 extends to `apply_loop::ffmpeg_version` (folds into F-16)                 | bug-pattern  |
| F-26  | Scheduler uses a process-global `static AtomicBool` for first-tick gating      | bug-pattern  |
| F-29  | `App.OnLaunched` uses `Task.Delay(150)` to paper over a startup race           | bug-pattern  |

### Suggested fix sequencing

1. **One-and-done extractions (low risk, high cleanup value):** F-02 (PROJECT_DIR const), F-08 (`ipod::layout` module), F-15 / F-18 / F-28 (magic numbers → consts). One PR per category. Touches many files but every change is mechanical.
2. **Real bugs (correctness):** F-09 (canonical iPod-detection predicate), F-16 + F-21 (thread `--ffmpeg` everywhere), F-26 (scheduler `interval_at`), F-29 + F-30 (drop race-y `Task.Delay`s). One PR per bug.
3. **Structural refactors (larger, dedicated PRs):** F-17 (`transcode/` split), F-19 (`progress/` split), F-20 (decompose `apply_loop::run`), F-25 (extract daemon handlers).
4. **Type-safety + clarity (medium-cost, high payoff):** F-13 (manifest stringly-typed enums), F-31 (.NET stringly-typed states), F-01 (audit `pub mod` declarations).
5. **Defer or batch as cleanup:** F-03, F-04, F-05, F-06, F-07, F-10, F-11, F-12, F-14, F-22, F-23, F-24, F-27, F-32.

No inline fixes were applied during this review pass (per the hybrid policy, F-02 is the only candidate large enough to be worth its own commit, but it touches 10+ files and so was promoted to a finding for a dedicated PR).

## Findings

### F-01 — `lib.rs` exports every module as `pub`

- **Location:** `src/lib.rs:1-19`
- **Category:** boundary
- **Severity:** medium
- **Observation:** Every module in the crate is declared `pub mod X;` in `lib.rs`. This treats internal implementation details (`apply_loop`, `try_with_prompt`, `progress`, `tags`, `manifest`, etc.) as external API. The lib is primarily consumed by `main.rs`, integration tests, and examples — but `pub` makes every type in every module a public API, which constrains future refactoring and makes it hard to tell from outside what's stable.
- **Proposed fix:** Audit each `pub mod` declaration. Modules that are reached only from `main.rs` / examples / tests inside this crate can be `pub(crate)`. The handful that are genuinely cross-cutting (e.g. types referenced by integration tests) stay `pub`. Likely outcome: only `cli`, `config`, `daemon`, `ipod`, `progress` need to be `pub`; the rest become `pub(crate)`.
- **Risks:** Some integration tests may reference types from modules being demoted to `pub(crate)`. Mitigation: run `cargo test` after the change; promote any module a test legitimately needs back to `pub`.

### F-02 — `"ipod-sync"` AppData/identifier literal duplicated across 10+ sites (Rust + .NET)

- **Location (Rust live):** `src/config.rs:155`, `src/config_file.rs:113`, `src/logging.rs:90`, `src/transcode.rs:232/240/368/379`, `src/daemon/runtime.rs:659`, `examples/bench-diff.rs:23`
- **Location (.NET live):** `ui-windows/IpodSync.UI.Core/Ipc/DaemonClient.cs:29` (`PipeName = "ipod-sync"`), `ui-windows/IpodSync.UI.Core/Diag.cs:29` (logs dir), `ui-windows/IpodSync.UI/Notifications/NotificationDecision.cs:41,54` (toast app name), `ui-windows/IpodSync.UI/Views/SettingsAboutPage.xaml.cs:25` (logs dir)
- **Category:** hardcoded
- **Severity:** high
- **Observation:** The string `"ipod-sync"` serves at least three roles — AppData/LocalAppData directory name, named-pipe label, and notification-toast identifier — and is duplicated across both the Rust crate and the .NET solution with no shared source of truth. Changing one side without the other would silently desync the IPC contract (pipe name) or split persistent state across directories.
- **Proposed fix:** Two-sided. Rust: `pub const PROJECT_DIR: &str = "ipod-sync";` in `lib.rs`, all live call sites use it. .NET: `IpodSync.UI.Core` defines `internal static class AppIdentity { internal const string Name = "ipod-sync"; }`. Long-term, the Rust pipe name + the C# pipe name should be generated from a single declaration in `docs/ipc-protocol.md` or a shared `Identity.json` consumed by both build systems. Short-term, both constants exist + a comment cross-references them.
- **Risks:** None for the inline extraction. The shared-source-of-truth proposal is more substantial — defer to a separate task.

### F-03 — `Config::to_persisted` uses field-by-field struct literal

- **Location:** `src/config.rs:37-51`
- **Category:** bug-pattern
- **Severity:** medium
- **Observation:** `to_persisted` constructs `PersistedConfig` by listing every field explicitly. Adding a new field to `PersistedConfig` mechanically breaks this function. `LEARNINGS.md` already documents one Phase 6 M2 blocked task caused by exactly this pattern.
- **Proposed fix:** Switch to `..PersistedConfig::default()` for the trailing fields:
  ```rust
  PersistedConfig {
      source: Some(self.source.clone()),
      ipod: self.ipod.clone(),
      ffmpeg: Some(self.ffmpeg.clone()),
      no_delete: Some(self.no_delete),
      no_tui: Some(!self.use_tui),
      encoder: Some(self.encoder),
      passthrough_wav: Some(self.passthrough_wav),
      refalac_path: Some(self.refalac_path.clone()),
      force_reencode: Some(self.force_reencode),
      ..PersistedConfig::default()
  }
  ```
  `PersistedConfig` already derives `Default`; this trades a brittle break for a quiet "new field defaults to None" semantics. Trade-off: callers no longer get a compile error reminding them to consider the new field — but the existing compile error doesn't usefully prompt that consideration anyway (it just says "missing field").
- **Risks:** None for current callers. The semantic shift is "future fields default to None when projected from Config" — acceptable because that's already what happens for any field Config doesn't track (e.g. `daemon`, `ipod_identity` are explicitly `None` today).

### F-04 — `normalize_drive` silently accepts invalid multi-letter drive strings

- **Location:** `src/config.rs:160-166`
- **Category:** bug-pattern
- **Severity:** low
- **Observation:** `normalize_drive("GG")` returns `"GG"` unchanged. The function appends a colon only for single ASCII letters; anything else passes through. A typoed `--ipod GG` reaches the iPod detection code as a "drive" string that will never match a real drive.
- **Proposed fix:** Either tighten to validate (`Result<String, anyhow::Error>` rejecting non-`X` and non-`X:` forms), or document the function as "best-effort normalization, validation happens downstream." Validation would be cleaner — invalid `--ipod` is a user error that should surface at config-resolve time, not after preflight.
- **Risks:** Breaking existing flows that rely on lax acceptance. Unlikely — only documented forms are `G` and `G:`.

### F-05 — Test fixtures repeat `["ipod-sync", ...]` argv pattern 20+ times

- **Location:** `src/config.rs:178-461` (multiple tests), `src/cli.rs:134-262` (multiple tests)
- **Category:** doc / coverage
- **Severity:** low
- **Observation:** Every test that constructs a `Cli` writes `Cli::try_parse_from(["ipod-sync", ...])` from scratch. The argv[0] literal is repeated 20+ times. A helper would DRY this up and make the binary-name change in F-02 less invasive.
- **Proposed fix:** Add a test helper `fn cli(args: &[&str]) -> Cli { let mut full = vec!["ipod-sync"]; full.extend(args); Cli::try_parse_from(full).unwrap() }` and use it across tests. Move to a shared `tests/common/cli.rs` module or keep file-local.
- **Risks:** None — pure refactor of test code.

### F-06 — `SyncMode::Default` and `NotifyLevel::Default` written by hand instead of `#[derive(Default)]`

- **Location:** `src/config_file.rs:16-30`
- **Category:** doc
- **Severity:** low
- **Observation:** Both enums hand-write `impl Default` to return a specific variant. Modern Rust supports `#[derive(Default)]` with `#[default]` on the chosen variant — fewer lines, harder to get wrong on enum extension.
- **Proposed fix:** Replace each manual impl with the derive form:
  ```rust
  #[derive(Default, ...)]
  pub enum SyncMode {
      #[default]
      Review,
      AutoApply,
  }
  ```
  Same for `NotifyLevel`. Safe inline fix.
- **Risks:** None — equivalent behavior.

### F-07 — Several one-line "default" functions could become trait impls

- **Location:** `src/config_file.rs:74-78`
- **Category:** doc
- **Severity:** low
- **Observation:** `default_true`, `default_review_mode`, `default_auto_apply_mode`, `default_schedule_minutes`, `default_daemon_settings` are one-line helper functions used by `#[serde(default = "...")]` attributes. The `default_daemon_settings` helper exists only because `Option<DaemonSettings>` doesn't automatically default to `Some(DaemonSettings::default())` — that's serde, not avoidable. The others could be replaced by making the underlying types' `Default` impls match (already done via F-06) and using `#[serde(default)]` without a function.
- **Proposed fix:** After F-06 lands, audit which `default = "..."` attributes can drop to bare `#[serde(default)]`. Keep `default_daemon_settings` (needed for `Option<T>` default-to-`Some` semantics).
- **Risks:** None.

### F-08 — iPod filesystem path components are bare string literals scattered across files

- **Location:** `src/ipod/device.rs:36-38, 99, 159-162`, `src/ipod/db.rs:44-46, 121-123`
- **Category:** hardcoded
- **Severity:** high
- **Observation:** The iPod's on-disk layout — `iPod_Control`, `Device`, `iTunes`, `Music`, `Artwork`, `SysInfo`, `iTunesDB`, `Play Counts.bak` — is hardcoded as string literals at every join site. A change to libgpod's expected layout (or even a typo while editing one site) splits behavior across the codebase.
- **Proposed fix:** Add an `ipod::layout` module with constants:
  ```rust
  pub mod layout {
      pub const IPOD_CONTROL: &str = "iPod_Control";
      pub const DEVICE: &str = "Device";
      pub const ITUNES: &str = "iTunes";
      pub const SYSINFO: &str = "SysInfo";
      pub const ITUNES_DB: &str = "iTunesDB";
      pub const PLAY_COUNTS_BAK: &str = "Play Counts.bak";
  }
  ```
  Or thin helper functions: `fn sysinfo_path(mount: &Path) -> PathBuf`. Call sites become `mount.join(layout::IPOD_CONTROL).join(layout::DEVICE).join(layout::SYSINFO)` or just `layout::sysinfo_path(mount)`. Safe inline fix — mechanical replacement.
- **Risks:** None — exact string equivalence.

### F-09 — Two parallel iPod-detection paths use different presence criteria

- **Location:** `src/ipod/device.rs:82-94` (`scan_for_ipod` — checks `iPod_Control/Device/SysInfo`) and `src/ipod/device.rs:141-147` (`detect_ipod_mount` — checks `iPod_Control/iTunes/iTunesDB`)
- **Category:** bug-pattern
- **Severity:** high
- **Observation:** Two iPod-detection functions in the same file disagree on what file proves "this is an iPod". `scan_for_ipod` (used by the daemon to identify devices) checks for SysInfo; `detect_ipod_mount` (used by the CLI sync path) checks for iTunesDB. A real iPod has both, but a freshly-restored or partially-corrupted device may have only one. The two callers can disagree about whether an iPod is plugged in — the daemon shows "connected" while the CLI says "no iPod found", or vice versa.
- **Proposed fix:** Pick one canonical predicate (probably "both files exist" since both are needed for any useful operation) and have one function answer "is this an iPod?". The two detection entry points can then differ only in what they return (DetectedIpod vs mount string), sharing the underlying predicate.
- **Risks:** Changing the predicate could classify a previously-detected device as not-an-iPod (or vice versa). Mitigate by checking against real hardware before merging.

### F-10 — `extract_firewire_guid` and `parse_sysinfo_field` use incompatible key-match strategies

- **Location:** `src/ipod/device.rs:16-31` and `src/ipod/device.rs:117-126`
- **Category:** bug-pattern
- **Severity:** medium
- **Observation:** Both parse `Key: value` lines from SysInfo but with different rules. `extract_firewire_guid` does strict `split_once(':')` + exact-key match. `parse_sysinfo_field` does `strip_prefix(key)` then trims `:` and whitespace — which means a key `Foo` matches a line `FooBar: x`. The latter is used by `scan_drive_for_ipod` to read both `FirewireGuid` and `ModelNumStr`. Test `ignores_lines_starting_with_firewire_guid_prefix_but_not_exact_key` at line 237 documents the strict behavior but only `extract_firewire_guid` actually has that behavior.
- **Proposed fix:** Consolidate into one parser that does the strict variant (`split_once(':')` + exact-key match). Replace both call sites. Add a test verifying `parse_sysinfo_field` no longer matches a strict prefix.
- **Risks:** If the real SysInfo file has trailing whitespace or other artifacts the strict parser doesn't tolerate, scanning may regress. Mitigate by running the existing fixture-based tests + checking against the real `sample-sysinfo.txt`.

### F-11 — GList track-find pattern duplicated three times in `db.rs`

- **Location:** `src/ipod/db.rs:216-225` (`delete_track`), `259-268` (`update_track_metadata`), `295-307` (`list_tracks_for_rebuild`)
- **Category:** boundary
- **Severity:** low
- **Observation:** The same loop — walk `(*db).tracks` GList, extract each `*mut Itdb_Track`, optionally compare dbid — appears three times. A `unsafe fn find_track(&self, dbid: u64) -> Option<*mut ffi::Itdb_Track>` helper would centralize the unsafe walking and let the callers focus on what they do with the found track.
- **Proposed fix:** Extract the walk into a private helper. Both `find_by_dbid` and `for_each_track` flavors would cover all three call sites. Keep `unsafe` since the returned pointer is only valid while the OwnedDb lives.
- **Risks:** None — pure refactor.

### F-13 — Manifest uses stringly-typed enums for `encoder` and `source_format`

- **Location:** `src/manifest.rs:50-65`, `src/manifest.rs:202-207` (`is_encoder_mismatch`)
- **Category:** bug-pattern
- **Severity:** medium
- **Observation:** `ManifestEntry.encoder` is `String` storing one of `"ffmpeg" | "refalac" | "passthrough" | "unknown"`. `source_format` is `String` storing one of `"flac" | "mp3" | "aac" | "alac" | "wav" | "ogg" | "opus" | "aiff"`. These are enums in disguise — but expressed as bare strings, they get no compiler help when adding values, are silently wrong on a typo, and force string-equality checks scattered through the code (e.g. `entry.encoder == "unknown"`). The codebase already has a real `EncoderChoice` enum in `cli.rs` for the user-facing choice; the manifest needs its own enum because it has two extra states (`Passthrough`, `Unknown`) that don't fit `EncoderChoice`.
- **Proposed fix:** Define two real enums with `#[serde(rename_all = "lowercase")]`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "lowercase")]
  pub enum ManifestEncoder { Ffmpeg, Refalac, Passthrough, Unknown }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "lowercase")]
  pub enum SourceFormat { Flac, Mp3, Aac, Alac, Wav, Ogg, Opus, Aiff }
  ```
  Use `#[derive(Default)]` with `#[default] Unknown` / `#[default] Flac` for the `#[serde(default)]` back-compat. `is_encoder_mismatch` becomes a `match` instead of string comparisons.
- **Risks:** Manifest JSON format changes from `"encoder": "ffmpeg"` (which already is what we write) to ... `"encoder": "ffmpeg"`. The serialized representation is identical, so existing manifests load cleanly. The change is purely type-system.

### F-14 — `is_transient` in source.rs has duplicated `matches!` block

- **Location:** `src/source.rs:102-127`
- **Category:** doc
- **Severity:** low
- **Observation:** The same `matches!(io_err.kind(), TimedOut | Interrupted | WouldBlock | UnexpectedEof)` appears twice — once for the direct downcast, once when walking `err.chain()`. The direct downcast is redundant because `err.chain()` includes the top-level error.
- **Proposed fix:** Drop the direct downcast; walk the chain only. Or extract `fn io_kind_is_transient(k: ErrorKind) -> bool`.
- **Risks:** None — semantically equivalent.

### F-16 — `--ffmpeg` configured path is silently ignored by the primary transcode path

- **Location:** `src/transcode.rs:157` (`transcode_to_alac`), `:138` (`probe`), `:458` (`extract_cover_art`), `:212` (`verify_tools_available`)
- **Category:** bug-pattern
- **Severity:** high
- **Observation:** The `--ffmpeg` CLI flag exists, lands in `Config.ffmpeg` (`src/config.rs:82-85`), is described as "Path to ffmpeg.exe. Defaults to 'ffmpeg' on PATH." But `transcode_to_alac` calls `Command::new("ffmpeg")` with the bare literal. `extract_cover_art` does the same. `verify_tools_available` checks `Command::new("ffmpeg")` against PATH directly. Only `transcode_via_refalac` (line 394) takes a `ffmpeg_path: &Path` parameter and uses it. A user who sets `--ffmpeg C:\custom\ffmpeg.exe` sees preflight succeed (if PATH also has ffmpeg) and then silently transcodes with the wrong binary, or fails inscrutably if PATH has no ffmpeg. ffprobe is a separate concern — there's no `--ffprobe` flag at all, so we implicitly assume ffprobe is alongside ffmpeg.
- **Proposed fix:** Thread `&Config` (or just `ffmpeg_path: &Path`) through `transcode_to_alac`, `extract_cover_art`, `probe`, and `verify_tools_available`. For ffprobe, derive its path from the configured ffmpeg's parent directory (`ffmpeg_path.with_file_name("ffprobe.exe")`), falling back to PATH lookup if absent. Add an integration test that exercises `--ffmpeg` against a known-good custom location.
- **Risks:** API change — `transcode_to_alac` and friends gain parameters. Internal-only API (no public consumers outside the crate), so the churn is contained. Behavior change for existing setups: anyone relying on the bug (i.e., setting `--ffmpeg` to a bad path while having a working ffmpeg on PATH) would suddenly fail. Acceptable — that's the bug surfacing.

### F-17 — `transcode.rs` is over the 500 LOC budget; clear split boundaries exist

- **Location:** `src/transcode.rs` (704 LOC total; ~470 implementation)
- **Category:** size
- **Severity:** medium
- **Observation:** transcode.rs mixes four distinct concerns: ffprobe JSON types (~110 LOC including the manual Deserialize), source classification (~95 LOC), ffmpeg/refalac transcoding (~160 LOC), and temp-path helpers + cover-art extraction (~80 LOC). The file works but reads as four modules glued together.
- **Proposed fix:** Split into a `transcode` directory:
  - `transcode/mod.rs` — public API re-exports + temp-path helpers
  - `transcode/probe.rs` — `ProbeOutput`, `ProbeFormat`, `ProbeTags`, `ProbeStream`, `ProbeDisposition`, manual `Deserialize` for `ProbeTags`, `probe()`, `has_embedded_art()`
  - `transcode/classify.rs` — `SourceAction`, `ClassifyConfig`, `classify()`
  - `transcode/encoder.rs` — `ffmpeg_args()`, `transcode_to_alac()`, `transcode_via_refalac()`, `passthrough()`, `extract_cover_art()`, `verify_tools_available()`, `verify_refalac_available()`
  - Each ends up 100–200 LOC implementation. Tests stay co-located with their target.
- **Risks:** Public API surface changes (callers import from new paths). Hold to next dedicated PR; this is not an inline-fix candidate.

### F-18 — Temp-path helpers duplicate the `%TEMP%\ipod-sync\ipod-sync-{pid}.<ext>` pattern four times

- **Location:** `src/transcode.rs:230-243` (`temp_alac_path`, `temp_art_path`), `:366-371` (`temp_wav_path`), `:377-383` (`temp_passthrough_path`)
- **Category:** hardcoded
- **Severity:** medium
- **Observation:** Four temp-path helpers each rebuild the same path scaffolding from scratch:
  ```rust
  let mut p = std::env::temp_dir();
  p.push("ipod-sync");
  p.push(format!("ipod-sync-{}.{ext}", std::process::id()));
  ```
  Each repeats the `"ipod-sync"` directory and `"ipod-sync-"` prefix literals. Widens the F-02 problem.
- **Proposed fix:** Introduce one helper:
  ```rust
  fn temp_path_with_ext(ext: &str) -> PathBuf {
      std::env::temp_dir()
          .join(crate::PROJECT_DIR)
          .join(format!("{}-{}.{ext}", crate::PROJECT_DIR, std::process::id()))
  }
  ```
  All four helpers become one-liners: `temp_path_with_ext("m4a")`, etc. Or expose `temp_path_with_ext` directly and delete the four wrappers.
- **Risks:** None — semantically identical outputs.

### F-19 — `progress.rs` is over the LOC budget; three render backends should split out

- **Location:** `src/progress.rs` (883 LOC total, ~620 implementation)
- **Category:** size
- **Severity:** medium
- **Observation:** progress.rs combines (a) the public `Progress` event-sender struct and its types, (b) `run_plain` (60 LOC), (c) `run_ipc` + `write_ipc_event` (90 LOC), (d) `TuiState` + `run_tui` + key handlers + render functions (~440 LOC). The three backends share only the input event type — they don't share rendering logic. A single file forces the reader to mentally context-switch between three independent renderers.
- **Proposed fix:** Split into a `progress/` directory:
  - `progress/mod.rs` — public `Progress` struct, `start()` dispatch, public types (`ActionPlanSummary`, `Decision`, `ReviewDecision`, `PromptRequest`, `FormRequest`, `ProgressEvent`)
  - `progress/plain.rs` — `run_plain`
  - `progress/ipc.rs` — `run_ipc`, `write_ipc_event`
  - `progress/tui.rs` — `run_tui`, `TuiState`, `ReviewState`, `FormState`, `apply_event`, key handlers, render functions, `format_duration`
- **Risks:** Public API surface stays identical (re-exports through `progress/mod.rs`). Tests stay co-located with their target. Hold to a dedicated PR.

### F-20 — `apply_loop::run` is a 740-line function and should be decomposed

- **Location:** `src/apply_loop.rs:71-810`
- **Category:** size
- **Severity:** high
- **Observation:** The entire orchestration of a sync — preflight checks, manifest load/rebuild, diff, review, apply loop, post-apply cleanup, optional persist — lives in one function. At 740 lines, no reader holds it in head all at once. Bugs (e.g. the LEARNINGS-documented retry/abort gotcha, the metadata-only partial-tag-write Skip case) live in subsections that should each be their own testable function.
- **Proposed fix:** Decompose by phase:
  - `fn run` becomes ~50 lines: validate, dispatch to phase functions, return
  - `fn load_or_rebuild_manifest(config, mount) -> Result<Manifest>`
  - `fn run_review(actions, progress, decision_rx, config) -> Result<ReviewOutcome>` where `ReviewOutcome` is `Apply { no_delete } | DryRun | Quit`
  - `fn apply_actions(actions, db, manifest, progress, ...) -> Result<()>` (the actual per-action loop)
  - `fn save_results(config, manifest)` (post-apply persistence)
  Each phase function is independently testable; bugs localize to a single function. Action-arm logic (`add_one`, `do_metadata_only`, etc.) is already extracted — they keep their current shape but become callees of `apply_actions` only.
- **Risks:** Largest structural change in this review. Requires careful state-threading (the `db`, `manifest`, `progress`, `decision_rx`, action counts). Recommend a dedicated PR with the existing 149 tests as the safety net.

### F-21 — F-16 extends to `apply_loop::ffmpeg_version` (also bypasses `--ffmpeg`)

- **Location:** `src/apply_loop.rs:58`
- **Category:** bug-pattern
- **Severity:** high (folds into F-16)
- **Observation:** `ffmpeg_version()` calls `Command::new("ffmpeg")` with the bare literal, same bug as `transcode_to_alac`. Recorded `encoder_version` in the manifest could be from a different ffmpeg than the one transcoding (if PATH and `--ffmpeg` disagree).
- **Proposed fix:** Pass `&Config` or `ffmpeg_path: &Path` into `ffmpeg_version`. Same fix as F-16. Track together.
- **Risks:** Same as F-16.

### F-22 — Three near-identical retry-prompt loops in preflight.rs could share one helper

- **Location:** `src/preflight.rs:26-50` (verify_ffmpeg), `:58-97` (verify_refalac), `:102-152` (resolve_ipod_mount), `:160-199` (walk_source)
- **Category:** boundary
- **Severity:** low
- **Observation:** Each function has the same shape: `loop { match fallible_op() { Ok(x) => return Ok(x), Err(e) => { let msg = format!(...); let outcome = await_prompt(...); match outcome { Retry => continue, _ => return Err } } } }`. The body of the closure varies (Retry/Abort vs Retry/Change/Abort), and `walk_source` has a side effect on Custom(1), but the loop structure is identical.
- **Proposed fix:** Generic helper `retry_with_prompt<T>(op: impl FnMut() -> Result<T>, on_error: impl Fn(&anyhow::Error) -> Prompt, progress, decision_rx) -> Result<T>`. The Custom(N) branches stay in their original callers since the side-effects differ.
- **Risks:** None — pure refactor; tests stay co-located.

### F-23 — iPod path literals also appear in preflight.rs (folds into F-08)

- **Location:** `src/preflight.rs:111`
- **Category:** hardcoded
- **Severity:** medium (folds into F-08)
- **Observation:** `iPod_Control\iTunes\iTunesDB` path components repeated in preflight too. Same fix as F-08 — use the `ipod::layout` constants/helpers.
- **Proposed fix:** Same as F-08.
- **Risks:** None.

### F-24 — Wizard contains hardcoded project name in user-facing UI text

- **Location:** `src/wizard.rs:20-22`
- **Category:** hardcoded
- **Severity:** low (folds into F-02)
- **Observation:** `"ipod-sync — first-launch setup\n..."` displays the project name verbatim in the wizard label. A rename has to update this string too.
- **Proposed fix:** Use the F-02 PROJECT_DIR constant (or a separate `PROJECT_DISPLAY_NAME` if we want a different display form, e.g. capitalized).
- **Risks:** None.

### F-25 — `daemon/runtime.rs` is at the LOC budget and mixes orchestration with handlers

- **Location:** `src/daemon/runtime.rs` (665 LOC, ~600 implementation)
- **Category:** size
- **Severity:** medium
- **Observation:** runtime.rs combines the daemon's `tokio::select!` main loop with seven distinct handler functions (handle_internal_event, handle_device_event, handle_client_command, start_sync_session, broadcast_status, build_config_update, make_history_entry). Each handler is well-factored individually, but the file is long enough that flipping between the loop and a handler is painful.
- **Proposed fix:** Move handler implementations to a sibling `daemon/handlers.rs` (or three: device, client, internal). runtime.rs keeps `run_daemon`, `run_daemon_with_deps`, `DaemonDeps`, `SpawnFn`, `InternalEvent`, and the `tokio::select!` loop only. ~200 LOC after split.
- **Risks:** Visibility annotations need adjustment for the shared types. Defer to dedicated PR.

### F-26 — Scheduler uses a process-global `static AtomicBool` to gate first-tick skipping

- **Location:** `src/daemon/scheduler.rs:48-56`
- **Category:** bug-pattern
- **Severity:** high
- **Observation:** `SyncScheduler::tick` uses a process-global `static SEEN_FIRST: AtomicBool` to consume the first "immediate" tick of `tokio::time::interval` only on the very first `tick()` call across the entire process. The author's own comment admits: "tests that build multiple schedulers should call tick twice and discard the first." This leaks test concerns into production: any `rearm()` after first tick silently skips the new schedule's grace period, and the second-instance fallback ("call tick twice") is a contract that won't be enforced as the code evolves.
- **Proposed fix:** Use `tokio::time::interval_at(Instant::now() + Duration::from_secs(...), Duration::from_secs(...))` in the constructor — sets the first tick to be at +N, eliminating the "immediate tick" entirely. Drop the static and the comment. `tick()` becomes one line: `i.tick().await`.
- **Risks:** Behavior change for the first tick: under the old code, the first user-observed tick was at +N (after consuming the "free" tick); under the new code, same outcome but via construction rather than runtime gating. Tests should still pass; verify with `cargo test daemon::scheduler`.

### F-27 — Sync subprocess output forwarded to a 256-slot broadcast channel may drop events on large syncs

- **Location:** `src/daemon/runtime.rs:49` (`broadcast::channel::<DaemonEvent>(256)`), `src/daemon/sync_orchestrator.rs:84` (`event_tx.send(DaemonEvent::SyncEvent { line: line.clone() })`)
- **Category:** bug-pattern
- **Severity:** medium
- **Observation:** The orchestrator forwards every IPC line from the sync subprocess as a `DaemonEvent::SyncEvent` on the daemon's 256-slot broadcast channel. A 1,400-track sync generates thousands of `track_start` + `track_done` events plus per-track logs. Any client (the UI) that falls behind by more than 256 events gets `RecvError::Lagged` (handled at `ipc_server.rs:162` with a warn — but the event is lost, and the UI's progress display silently skips). The chance of this happening grows with library size and UI thread contention.
- **Proposed fix:** Either (a) bump channel capacity and accept the memory cost (4× is still tiny), (b) introduce a coalescer that batches consecutive track events before broadcasting, or (c) split into two channels — one for high-frequency progress (small, drop OK), one for state-change events (larger, drop bad). Recommend (a) as the first move; revisit (b)/(c) if 1024 still drops under load.
- **Risks:** Memory cost is trivial (DaemonEvent is small). No semantic risk.

### F-28 — Magic durations scattered across the daemon

- **Location:** `src/daemon/runtime.rs:44` (`schedule_minutes default 30`), `:49` (`broadcast 256`), `:121` (`Duration::from_millis(500)` debounce), `src/daemon/device_watcher.rs:82` (`1500ms` polling), `:94` (`mpsc 32`), `src/daemon/sync_orchestrator.rs:101,122` (`Duration::from_secs(5)` kill grace)
- **Category:** hardcoded
- **Severity:** low
- **Observation:** Eight magic durations and capacities sprinkled across the daemon. Some are tuning parameters (polling interval, debounce window, channel sizes); some are policy (schedule default, kill grace). Today nobody can change them without grepping; tomorrow someone will misadjust one without realizing another depends on it (e.g. debounce window must be < polling interval).
- **Proposed fix:** Module-level `const`s grouped in `daemon/mod.rs` or a `daemon/tuning.rs`. Document the relationships in comments (e.g. "debounce < polling × 2 to absorb at least one duplicate scan").
- **Risks:** None — pure relocation.

### F-29 — `App.OnLaunched` uses `Task.Delay(150)` to paper over a startup race

- **Location:** `ui-windows/IpodSync.UI/App.xaml.cs:88`
- **Category:** bug-pattern
- **Severity:** high
- **Observation:** After connecting to the daemon and starting the router, the app sleeps 150ms with a self-incriminating comment: "give the router time to populate LatestConfig". The next line uses `LatestConfig?.Ipod is null` to decide whether to show the wizard. If the daemon's `ConfigUpdate` event takes longer than 150ms (slow disk, debug build, heavy CPU), the wizard fires for an already-configured user. If it's faster than expected, no harm — but the contract is "racing time."
- **Proposed fix:** Replace with a deterministic await: subscribe a one-shot `TaskCompletionSource<ConfigUpdateEvent>` to the router's `ConfigUpdated` event BEFORE sending `GetConfigCommand`, then `await tcs.Task.WaitAsync(TimeSpan.FromSeconds(2))` after the send. The TCS resolves on the actual event; the timeout is a defensive ceiling, not the primary signal.
- **Risks:** Minor — if the daemon never replies (process died between connect and send), the timeout fires and we proceed as if unconfigured. Same failure mode as today, but bounded by a real signal instead of a guess.

### F-30 — `App.OnLaunched` also uses `Task.Delay(500)` after spawning the daemon

- **Location:** `ui-windows/IpodSync.UI/App.xaml.cs:51`
- **Category:** bug-pattern
- **Severity:** medium
- **Observation:** After `SpawnDaemon()` (which is fire-and-forget), the app sleeps 500ms before calling `DaemonClient.ConnectAsync`. `ConnectAsync` already has its own backoff loop (1s, 2s, 4s = ~7s total). The 500ms pre-sleep is redundant — and if the daemon takes longer than 500ms to start listening on the pipe, the connect retries anyway. If it's faster, we waste up to 500ms of startup latency.
- **Proposed fix:** Drop the `Task.Delay(500)`. Let `ConnectAsync`'s backoff do its job. The first connect attempt will fail-fast (`ConnectAsync(2000, ...)` returns within 2s if pipe doesn't exist), backoff kicks in.
- **Risks:** None — strictly better.

### F-31 — Daemon state values are stringly-typed across the C# UI

- **Location:** `ui-windows/IpodSync.UI/Notifications/NotificationDecision.cs:25-54`, `ui-windows/IpodSync.UI/ViewModels/PopoverViewModel.cs:72,80,88` (and others)
- **Category:** bug-pattern
- **Severity:** medium
- **Observation:** The C# side receives daemon state as `string State` and compares with literals (`"syncing"`, `"idle"`, `"ok"`, `"none"`, `"errors_only"`). On the Rust side these are real enums (`DaemonStateLabel`, `NotifyLevel`, `SyncOutcome`) serialized as snake_case. A typo on the C# side silently misses the state; a new variant added on the Rust side is invisible to the C# side without a code change.
- **Proposed fix:** Mirror the Rust enums on the C# side as proper enums with `[JsonStringEnumConverter(JsonNamingPolicy.SnakeCaseLower)]`. Replace the string comparisons with enum matches. The `DaemonEvent` record fields change from `string State` to `DaemonState State` (and friends).
- **Risks:** Adding a variant on Rust still requires updating the C# enum, but the compiler at least flags unhandled `switch` arms. The serializer-side conversion is fully tested.

### F-32 — Tests rely on `await Task.Delay(50–100)` to wait for async events

- **Location:** `ui-windows/IpodSync.UI.Tests/WizardViewModelTests.cs` (6 occurrences), `ui-windows/IpodSync.UI.Tests/DaemonEventRouterTests.cs` (6 occurrences)
- **Category:** coverage
- **Severity:** low
- **Observation:** Tests sleep 50–100ms to wait for events to propagate through router. Today the timings work; under a loaded CI host or a future ARM CI runner, they may flake.
- **Proposed fix:** Replace each `Task.Delay` with a `TaskCompletionSource<T>` subscribed to the event being awaited, plus `await tcs.Task.WaitAsync(TimeSpan.FromSeconds(5))`. Fires immediately when the event arrives; cap at 5s so a broken test still fails in bounded time. Add a small `WaitForEvent<TEvent>(EventHandler<TEvent> register, ...)` helper to keep callsites tidy.
- **Risks:** None — strictly more reliable.

### F-15 — Magic numbers in source.rs (retry count, backoff base, buffer size)

- **Location:** `src/source.rs:53` (`max_retries = 3`), `:79` (`1u64 << attempt`), `:218` (`vec![0u8; 64 * 1024]`)
- **Category:** hardcoded
- **Severity:** low
- **Observation:** Retry count 3, exponential backoff `1<<attempt`, and a 64 KB hash-buffer size are inline literals. The first two have explanatory comments; the buffer size doesn't.
- **Proposed fix:** Hoist to module-level `const`s alongside the existing `FINGERPRINT_PREFIX_BYTES`:
  ```rust
  const WALKER_MAX_RETRIES: u32 = 3;
  const AUDIO_HASH_CHUNK_BYTES: usize = 64 * 1024;
  ```
  Leave the `1<<attempt` backoff inline — it's clear from context.
- **Risks:** None.

### F-12 — `CString::new("FirewireGuid").unwrap()` panics on a static constant

- **Location:** `src/ipod/device.rs:59`
- **Category:** doc
- **Severity:** low
- **Observation:** `CString::new("FirewireGuid").unwrap()` constructs a CString from a string-literal that obviously contains no NUL, then unwraps. Defensible but ugly. The cleaner pattern is `c"FirewireGuid"` (raw C-string literals, stable since Rust 1.77) or `const KEY: &CStr = CStr::from_bytes_with_nul(b"FirewireGuid\0").unwrap();` (also const since 1.72).
- **Proposed fix:** Replace with `c"FirewireGuid"` (Rust 1.77+ syntax). Compile-time guarantee, no allocation.
- **Risks:** Need Rust 1.77+ in `rust-toolchain.toml` (check current MSRV first).
