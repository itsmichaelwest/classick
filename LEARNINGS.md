# Learnings — ipod-sync

Per global AGENTS.md: record discovered conventions, gotchas, debugging insights, and useful commands here as work proceeds. One bullet per learning.

## iTunes will always reject a libgpod-managed iPod (2026-05-24)

- **Fundamental:** libgpod's iTunesDB writes are byte-compatible with what the iPod firmware accepts (Phase 1 gate proved this) but the signature does NOT match Apple's stricter check. iTunes 12.x on Windows will pop a "cannot read contents of the iPod, please Restore" dialog whenever it sees a libgpod-managed device. **This is not a bug we can fix without reverse-engineering Apple's signing algorithm exactly.**
- **Recovery from the panic-Restore loop:** the user hit this on 2026-05-24, factory-restored from iTunes, lost ~30 min of sync work. The right answer for them was "ignore the dialog and close iTunes," not Restore.
- **Mitigations shipped to keep users out of this trap:**
  1. **iTunes-running guard** (`preflight::verify_itunes_not_running`) — refuses to start a sync if `iTunes.exe` is running, with a clear "close iTunes, do NOT click Restore" message and a Retry option. Detects `AppleMobileDeviceService.exe` too but only as advisory (some users keep it for iPhone syncing). PowerShell `Get-Process` shell-out, no new deps.
  2. **Warning copy in wizard's Done page + Settings General page** — explains the iTunes-rejection behavior up-front so the user knows the "cannot read" dialog is expected and how to respond.

## DB-vs-disk reconciliation at sync start (2026-05-24)

- **What it covers:** two pre-existing corruption classes that compounded across crashed syncs:
  - **Orphans on disk** — files under `iPod_Control\Music\F**\` that no DB track references (left by an `itdb_cp_track_to_ipod` call whose process died before `db.write()`).
  - **Dangling DB refs** — tracks in the DB whose `ipod_path` points to files that no longer exist (e.g. user deleted with Explorer, third-party tool, iTunes Restore that wiped the partition).
- **Fix:** `OwnedDb::reconcile_with_disk(mount)` walks both sides into HashSets and: deletes the set-difference orphans from disk; calls `delete_track` for the set-difference dangling DB entries (which also removes them from the in-memory DB). Runs from `apply_loop.rs` after `set_firewire_guid` and before the diff, so the action plan sees a 1:1-consistent baseline. Cheap (~1s on a 1,400-track library — bounded by walkdir over the F-folders).
- **Why both directions matter:** orphan-only cleanup leaves a clean disk but a DB with dangling refs that iTunes/iPod firmware then complains about. DB-cleanup-only leaves orphans wasting space. The single bidirectional sweep is the only state that's actually coherent.

## iTunesDB session-start backup as crash defense (2026-05-24)

- **Concern:** libgpod's `itdb_write` uses MSVCRT `rename` (confirmed via `strings gpod.dll` — symbols include `__imp_rename`, `itdb_rename_files`, `ITDB_FILE_ERROR_RENAME`). On Windows 10+ MSVCRT rename is `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` which IS atomic. But the path isn't 100% audited, and we've already had to patch one Windows-rename quirk for `Play Counts` → `Play Counts.bak`. Worth defense in depth.
- **What we ship:** `crate::ipod::db::backup_itunesdb(mount)` copies the current `iTunesDB` to `iPod_Control\iTunes\iTunesDB.ipod-sync-backup` via a `.tmp` intermediate + atomic rename. Called once per sync session (NOT per checkpoint — one good backup per session is enough). If a sync crash corrupts the live DB, manual recovery is `copy iTunesDB.ipod-sync-backup iTunesDB`. A future `--restore-db-backup` subcommand could automate this; for now the manual command is documented here.
- **Cost:** one ~10 KB–2 MB file copy per sync session. Negligible.
- **What this does NOT cover:** mid-checkpoint corruption (an in-flight write that lands a partial DB). For that we'd need to either (a) checkpoint-write to `iTunesDB.tmp` and rename ourselves rather than trust libgpod, or (b) keep a rolling N-deep backup. Both are larger projects; the session-start backup catches the common "crash before any successful db.write" case which is what bit the user.

## Periodic db.write checkpoints bound the orphan-on-crash window (2026-05-24)

- **Problem (before fix):** `apply_loop.rs` only called `db.write()` + `manifest::save_atomic` at the very end. A daemon crash / USB unplug / power loss / hard-kill mid-sync left every track copied so far via `itdb_cp_track_to_ipod` as an orphan — present under `iPod_Control\Music\F**` but unreferenced by the iTunesDB on disk. The graceful-Shutdown and graceful-Cancel fixes only cover *signalled* exits; everything else still produced orphan piles.
- **Fix:** New `crate::SYNC_CHECKPOINT_EVERY` constant (= 25). Apply loop counts each non-Unchanged action; every Nth, runs the same `db.write()` + `manifest::save_atomic` sequence the post-loop final-flush already uses. Worst-case orphan window shrinks from "entire library" to "≤ N tracks". On a ~1,400-track library that's ~56 checkpoints × ~100ms each ≈ 5.6s overhead on a ~90min sync (<0.2%).
- **Constant location:** `src/lib.rs` next to `PROJECT_DIR` so the trade-off (orphan-window vs checkpoint overhead) is documented at one place — adjust there if real-world failure modes warrant a different N.

## Daemon integration tests need a per-test config + pipe sandbox (2026-05-24)

- **Symptom:** `daemon_runtime_integration` tests fail with `SendError` and/or `Elapsed(())` when (a) the developer's `%APPDATA%\ipod-sync\config.toml` has `subsequent_sync_mode = "review"` — `auto_sync_enabled()` returns false so the auto-sync test paths silently no-op; OR (b) a real daemon is already bound to `\\.\pipe\ipod-sync` — `spawn_server_full_with(.first_pipe_instance(true).)` fails → daemon task exits → `device_rx` is dropped → `tx.send(...).await.unwrap()` panics.
- **Fix:** Added three optional override fields to `DaemonDeps`: `config_path`, `history_path`, `pipe_name`. Production passes `None` for all three. A new `sandbox()` test helper builds (a) a unique tempdir under `target/test-tmp/daemon-int-<pid>-<n>/` (b) a `config.toml` with `subsequent_sync_mode = "auto_apply"` (c) a unique pipe name `\\.\pipe\ipod-sync-test-<pid>-<n>`. The pipe-name plumbing also required a new `spawn_server_full_with(event_tx, pipe_name)` entry in `ipc_server.rs`; the existing `spawn_server_full` is now a thin wrapper passing `PIPE_NAME`.
- **Why this matters:** the previous test setup was "works on my machine"–coupled to whatever sync-mode the developer happened to have set, and unrunnable while the UI was open. Both are now decoupled.

## Graceful Stop sync: poll decision_rx between tracks (2026-05-24)

- **Symptom (before fix):** Click "Stop sync" in the tray; daemon sends `{"type":"cancel"}` to the subprocess; nothing visible happens for 5 seconds; daemon's `bounded_kill` then `TerminateProcess`s the subprocess. Every track copied via `itdb_cp_track_to_ipod` so far becomes an orphan because `db.write()` never ran.
- **Root cause:** `IpcCommand::Cancel` in `src/ipc.rs` maps to `Decision::Review(ReviewDecision::Quit)` and pushes it onto `decision_rx`. But `src/apply_loop.rs`'s `for action in actions` loop never reads from `decision_rx` except inside per-track `try_with_prompt` error-retry paths. On a healthy sync those retries never fire, so the queued Quit just sits there. Documented as "M1 limitation" in the source.
- **Fix:** At the top of each iteration of the apply loop, non-blocking `decision_rx.try_recv()`. If we see `Decision::Review(ReviewDecision::Quit)`, set a `cancelled` flag and `break`. The post-loop code (`db.write()` + `manifest::save_atomic`) still runs, so completed tracks get registered in the iTunesDB and a "Sync cancelled. Completed tracks were saved." log line replaces the normal "Done." Other (stray) decisions are dropped — they have no consumer at this point.
- **Why this is the right fix:** the orchestrator's `cancel_rx` → write-cancel-to-stdin → `bounded_kill` chain is fine; it just needed the subprocess side to ACT on the cancel within the grace window. Anything more invasive (per-action cancellation tokens, async apply loop) would be a much bigger rewrite for the same observable behaviour.

## ffmpeg without `-nostdin` hangs at ~97% of a track (2026-05-24)

- **Symptom:** Sync subprocess starts, ffmpeg writes ~97% of the source FLAC's bytes to the ALAC temp file in under 2 seconds, then **completely stalls** — ffmpeg process stays alive and "responding" but CPU stops accumulating and the temp file's mtime + size never change. Manual reproduction of the exact ffmpeg command in a PowerShell shell completes in <1 second.
- **Root cause:** `transcode.rs` invoked ffmpeg via `Command::new(...).status()` without setting stdin. By Rust default, `.status()` inherits the parent's stdin. The parent (the daemon's sync subprocess) was launched with `Stdio::piped()` for stdin (the daemon's cancel-command channel). So ffmpeg inherited an *open pipe* with no data. Without `-nostdin`, ffmpeg's interactive-keypress reader blocks on that stdin during stream finalization, even though it never actually receives any data.
- **Fix:** Two layers in `src/transcode.rs`:
  1. Added `-nostdin` to `ffmpeg_args()` (covers the main `transcode_to_alac` path) and to the ffmpeg invocations inside `transcode_via_refalac` and `extract_cover_art`.
  2. Added `.stdin(Stdio::null())` to every `Command::new(ffmpeg_path)` call site as defense in depth — if a future ffmpeg ignores `-nostdin` or a new call site forgets the flag, the inherited-pipe stdin scenario becomes structurally impossible.
- **Diagnostic that nailed it:** running the *exact* failing ffmpeg command manually completed in 0.9s vs 3+ minutes wedged when spawned by the sync subprocess. Same args, same source, same output — the only difference is inherited stdio. Always check this when ffmpeg works standalone but hangs when subprocess-launched.

## Daemon Shutdown leaks the sync subprocess (2026-05-24)

- **Symptom:** User closes the UI, daemon receives the `shutdown` IPC command, the daemon process dies — but the spawned `ipod-sync.exe --ipc-mode --apply` keeps running indefinitely, transcoding tracks via SMB + ffmpeg, writing events to a broken stdout pipe. Caught in the wild with PID 57120 still alive 3+ hours after its daemon (PID 48596) had received `shutdown`. The orphan locks the iPod and can collide with the next daemon's sync.
- **Root cause:** `handle_client_command`'s `Shutdown` arm called `std::process::exit(0)`, which skips Drop. Tokio's `Child` Drop (which would run `TerminateProcess` if `kill_on_drop(true)` is set) never fires. On Windows there is no SIGHUP-style parent-death signal, so the child becomes a true orphan.
- **Fix:** Two layers in `runtime.rs` + `sync_orchestrator.rs`:
  1. `build_command()` now sets `.kill_on_drop(true)` so any unexpected drop of the `Child` (panic, runtime teardown) kills the subprocess.
  2. `handle_client_command` returns `bool` ("should exit"); `Shutdown` returns true instead of `std::process::exit`. The main `select!` loop breaks with `ExitReason::Shutdown`, the post-loop code signals cancel + drains for up to `SHUTDOWN_DRAIN_BUDGET` (8s), then returns `Ok(())` so the tokio runtime drops cleanly and `kill_on_drop` is the backstop.
- **Important: do NOT remove the drain.** Hard-killing the subprocess mid-`itdb_write` would corrupt the iPod's iTunesDB. The drain gives the orchestrator's `cancel\n`-then-`bounded_kill(SYNC_KILL_GRACE)` sequence time to complete first.

## Daemon's configured_serial was captured-once at startup (2026-05-24)

- **Symptom:** Pair an iPod via the wizard in a fresh daemon session, plug it in — auto-sync never fires until you restart the daemon. Same for `TriggerSync`, which rejects with `not_configured` even though the config file on disk has the identity.
- **Root cause:** `run_daemon_with_deps` read `configured_serial` once at startup via `deps.configured_serial` and held it as an immutable local. `SaveConfig` and `ForgetIpod` wrote the config file but never touched the in-memory value. So the wizard's first SaveConfig was invisible to the daemon's auto-sync gate for the lifetime of that daemon process.
- **Fix:** `let mut configured_serial = configured_serial;`, thread `&mut configured_serial` into `handle_client_command`, mutate it in the SaveConfig arm (mirror the persisted `ipod_identity.serial`) and the ForgetIpod arm (set to None). Plug-in auto-sync now sees the post-wizard state on the same daemon session.

## Popover lost sync progress when reopened mid-sync (2026-05-24)

- **Symptom:** Sync is running (subprocess has emitted `TrackStart current=472 total=1275`); user opens the tray popover and sees "Syncing now / Preparing…" — the indeterminate state — even though the sync is clearly past the prepare phase. Stays stuck until either the popover is left open (events arrive live) or the sync finishes.
- **Root cause:** `App.OnIpcEvent` did `_popover?.ViewModel.ApplyIpcProgress(e)` — when popover was closed, the event was dropped on the floor. The new popover was constructed fresh from `LatestStatus` (which carries only state-machine info, no `(current, total, track-label)`), so it defaulted to `ProgressTotal == 0` → "Preparing…" via `PopoverViewModel.ProgressLabel`.
- **Fix:** Hoist progress accumulation into `App.xaml.cs` static fields (`_progressCurrent`, `_progressTotal`, `_currentTrackLabel`, `_currentLogLine`). `OnIpcEvent` always updates these regardless of popover existence; `OnPopoverRequested` seeds the new VM from them when daemon state is `"syncing"`. `OnStatusUpdated` clears them on any non-syncing transition so a stale snapshot doesn't leak into the next session.

## Daemon integration tests share the user's real config — fragile (2026-05-24)

- `tests/daemon_runtime_integration.rs` calls `run_daemon_with_deps()` which resolves `config_file::default_path()` → `%APPDATA%\ipod-sync\config.toml`. There is no config-path injection in `DaemonDeps`. So `auto_sync_enabled()` reads the developer's *real* config; if `subsequent_sync_mode = "review"` is set, `auto_sync_fires_when_configured_device_connects` and `runtime_stays_responsive_during_long_sync` start failing with `Elapsed(())` because the auto-sync gate now returns false. Either add `config_path: Option<PathBuf>` to `DaemonDeps` and have tests point at a temp file, or have the tests scoped-set the config to a known state via a guard. Tracked: pre-existing M3-WIP failure mode.

## Phase 6 M2 gate (2026-05-24) — PASS

- **Result:** PASS. All 4 M2 scenarios validated.
- **Scenario M2-1 (fresh install + wizard):** Wizard opened, 3 steps completed cleanly, source folder + iPod identity persisted to `config.toml` under `[ipod_identity]` (serial + model_label).
- **Scenario M2-2 (re-launch with config):** App started hidden in tray (no wizard window). Tray icon appeared in notification area. Right-click → Quit worked.
- **Scenario M2-3 (Quit cleanly exits both processes):** After Quit, neither `IpodSync.UI.exe` nor `ipod-sync.exe` (daemon) remained in Task Manager.
- **Scenario M2-4 (build + tests):** 129 Rust lib tests + 47 C# tests passing.
- **One real bug found + fixed during gate:** First smoke run, app exited after wizard finished instead of settling into tray. Root cause: WinUI 3 lifetime gotcha (see next entry). Fix: commit `4e5c011`.

## WinUI 3 tray-only lifetime: TaskbarIcon must be an XAML resource (2026-05-24)

- **The trap:** WinUI 3 apps auto-exit when the last window closes UNLESS a long-lived lifetime anchor exists. Creating an `H.NotifyIcon.TaskbarIcon` in C# code (e.g. `new TaskbarIcon()` inside a controller's `Initialize()`) does NOT anchor the Application's lifetime. The tray icon appears briefly then the process exits.
- **The fix:** Define the `TaskbarIcon` as an `<Application.Resources>` entry in `App.xaml` and pull it via `Application.Current.Resources["TrayIcon"]` + `ForceCreate()` in `OnLaunched`. Application-resource lifetime keeps the dispatcher alive. The canonical reference is the H.NotifyIcon "Windowless" sample (`HavenDV/H.NotifyIcon` repo, `src/apps/H.NotifyIcon.Apps.WinUI.Windowless/`).
- **Two more gotchas from the same library:** (1) Use `ContextMenuMode="SecondWindow"` not `"PopupMenu"` — per their issue #66, PopupMenu mode leaks the process on Exit. (2) Use `Environment.Exit(0)` not `Application.Current.Exit()` to actually terminate — same issue #66.

## Phase 6 M2 parallel-agent races: small-team gains from named-file staging (2026-05-24)

- **Wave 2 (5 parallel implementers) hit two index races:** (a) T13's commit absorbed T3's already-staged files, mis-attributing them; (b) T2 was BLOCKED because the plan said "don't touch src/config.rs" but adding fields to `PersistedConfig` mechanically breaks `Config::to_persisted`'s field-by-field struct literal.
- **Lessons:** (a) Named-file `git add` prevents agents from stealing *others'* files but doesn't prevent commits from absorbing your *own* staged files when another agent commits between your `add` and `commit`. A worktree-per-agent or commit-mutex would be the durable fix; for solo projects, accept the occasional mis-attribution and document in commit body. (b) Plans that constrain which files an agent can touch must verify the constraint is satisfiable — `PersistedConfig` couldn't grow without src/config.rs growing too. Best practice for plan authors: when adding a struct field, identify every literal that constructs the struct and include those files in the task scope.
- **Wave structure that worked: 2 parallel implementers per wave is safe; 4-5 is risky.** Waves 3, 4, 5 of M2 (each 2 implementers) all completed without races.

## Phase 6 M2 — intentional M2 limitations (carry-forward to M3)

- **TriggerSync command:** returns `sync_rejected { reason: not_configured }` always. Real sync orchestration is M3.
- **SubscribeDeviceEvents / UnsubscribeDeviceEvents commands:** no-op responses. The wizard uses C#-side drive-letter polling instead (via `ScanLocalDrives` in `WizardWindow.xaml.cs`). M3 adds the real `DeviceWatcher` trait + Windows impl (via `windows-rs` SetupDi notifications), then the daemon emits real `device_connected` events that subscribed clients receive.
- **Tray icon states:** only Idle/Offline implemented; Syncing/Error placeholders point at `tray-idle.ico` (same file). M3 wires the state transitions; M4/M5 ship differentiated icons.
- **Tray menu:** only Quit. Sync Now (M3), Settings (M4), Status popover (M4) come later.
- **NotifyOn setting + notifications:** persisted but not consumed; toast wiring is M4 (`AppNotificationManager`).
- **Schedule:** `schedule_minutes` persisted but no timer fires; SyncScheduler lands in M3.

## TUI thread join: bounded-time + force-exit (2026-05-24)

- **`Progress::finish` now has a 5-second join deadline.** If the TUI thread doesn't reach a terminal state within that window after we send `ProgressEvent::Finish`, we `eprintln!` a warning and `std::process::exit(2)` instead of waiting forever. Driver: Phase 3.z Gate Scenario 5 produced a 2-hour zombie process (PID 39196, 60s CPU = idle, responsive but no window). The catastrophic 1275-remove apply loop completed, but the TUI thread never returned. Most likely crossterm's `LeaveAlternateScreen` or `disable_raw_mode` wedged on a Windows console handle after the gauge/panel rendering had visibly degraded earlier in the run. Couldn't reproduce safely (would mean wiping the iPod again) so we fixed defensively: poll `JoinHandle::is_finished()` with a 50ms tick, force-exit on timeout. If the warning fires repeatedly, the root cause matters and should be investigated; until then the bound is good insurance.

## Phase 6 M1 Wave 3 — parallel-agent git index race (2026-05-24)

- **Dispatched 4 agents simultaneously (T3 Rust, T6 ReviewVM, T7 ProgressVM, T8 CoreLocator); two of them collided on the git index.** T7 finished, ran `git add` for its 4 files, then went off to verify. Meanwhile T8 also ran `git add` of its 3 files and committed — sweeping T7's already-staged files into T8's commit. Net result: T7's ProgressViewModel + ProgressPage + tests live in commit `d3b966f` under T8's "CoreLocator" message. Code is correct, tests pass; only commit hygiene is wrong.
- **Lesson: for 3+ parallel agents touching the same workspace, use git worktrees.** The `superpowers:using-git-worktrees` skill exists; future parallel waves should isolate each agent in its own worktree so the shared index can't race. The 2-agent waves we've run successfully (Wave 1, Wave 2) didn't hit this because the windows were narrow; 4-way fan-out widens the race.
- **Wasn't worth rewriting history to fix.** Interactive rebase to split `d3b966f` would mutate 3 downstream commits and risks losing attribution worse than the original race. For a solo-developer project the cost-benefit isn't there. Documented here so a future audit can find Task 7's work.

## Phase 3 gate (2026-05-24) — PASS (5/6 driven; refalac optional)

- **Scenario 6 (Phase 2 manifest back-compat) PASS** — `cargo run --release` against 1275-track manifest written by pre-Phase 3 code produced `Modify=0`, `Unchanged=1275`. The `is_encoder_mismatch` carve-out for `encoder="unknown"` correctly prevented a thundering re-encode after Phase 3 upgrade. **Most important regression test — no users broken by Phase 3.**
- **Scenario 5 (`--force-reencode`) PASS** — `cargo run --release -- --force-reencode` produced `Modify=1275`, every transcodable track flagged regardless of encoder match. Override flag works as designed.
- **Scenarios 1, 2 N/A** — user's library is FLAC-only; no mixed-source preview to exercise and no MP3/AAC to byte-compare against passthrough output.
- **Scenarios 3, 4 deferred** — refalac install is opt-in (user runs ffmpeg by default); skip unless/until user installs qaac and wants Apple's reference ALAC encoder.
- **Scenario 7 (iPod-level acceptance) trivial** — both gate runs ended at Review screen with `q`; no new tracks written, existing tracks from yesterday's recovery sync play normally.
- **Source-change safeguard verified not firing** — manifest's `last_source_root` correctly matches current `config.source`, so the safeguard prompt stayed dormant. The safeguard's positive case was exercised the painful way during the Phase 3.z gate (see entry below).

## Phase 3 Task 5 — refalac vendoring (2026-05-23)

- **Vendor binaries are gitignored.** `vendor/refalac/` is in .gitignore;
  build.rs gracefully skips the copy when the dir is empty. Users who
  want `--encoder refalac` either drop `refalac64.exe` + `libFLAC.dll`
  into `vendor/refalac/` (build.rs picks them up) or put `refalac64.exe`
  on PATH (preflight finds it via `Config::refalac_path` default
  `PathBuf::from("refalac64")`). Default encoder is ffmpeg, so most
  users never touch refalac.
- **refalac version-string parsing is best-effort.** `verify_refalac_available`
  greps for "refalac" anywhere in the --help output; if it can't parse
  a version line, records `"refalac (version unknown)"` in the manifest.
  Acceptable because the version string is forensic-only — diff's
  encoder-mismatch logic only compares the encoder name ("refalac" vs
  "ffmpeg"), not the version.

## Phase 3.z Wave C — review fix limitations (2026-05-23)

- **`do_metadata_only` Skip after partial tag write — v1 limitation.** `update_track_metadata` calls `apply_tags` (which `g_free`s the old tag pointers and overwrites them via `g_strdup`) BEFORE attempting the thumbnail update. If the thumbnail step fails and the user picks Skip in the retry prompt, the new tag values are already in the in-memory `Itdb_Track` and will be persisted by the run-end `db.write()` — but the manifest stays at the old state, so the next run sees "Unchanged"/"MetadataOnly" while iTunesDB tags are mid-state. A proper fix would snapshot the old `*mut gchar` tag pointers before mutation and restore them on Err, but the FFI ownership is fiddly (`set_str` calls `g_free` on the slot before overwriting, so a naive snapshot becomes a dangling pointer that the restore would double-free). For Wave C we surfaced the limitation via `progress.error` in the Skip branch ("partial tag write may persist; recommended: eject the iPod and re-run") and deferred the snapshot+restore work. If/when this gets revisited: don't reuse `set_str` on the restore path — write a separate `restore_str(slot, snapshotted_ptr)` that swaps without `g_free`-ing, and have the mutation path own the freeing of replaced pointers explicitly.

## Phase 3.z gate (2026-05-23) — PARTIAL PASS (driven scenarios PASS, TUI scenarios pending iPod)

Driven via Bash pipe (forces plain mode since stdout is not a TTY). Plain mode exercises the error-wrapper *placement* and the *fall-through-to-Abort* path; visual TUI rendering still needs an interactive run with the iPod plugged in.

- **Scenario 1 (ffmpeg-missing) PASS** — narrowed PATH to hide ffmpeg, `cargo run -- --dry-run --source <empty>` produced one prompt print and exited 1 with `Error: ffmpeg/ffprobe required; aborted`. Initial run revealed an **infinite retry loop** because plain mode's default `Decision::Prompt { choice: 0 }` mapped to `outcomes[0] = Retry`. Fix in commit `a7d58eb` — plain mode now sends `choice: usize::MAX` so `await_prompt` falls into its `unwrap_or(Abort)` branch.
- **Scenario 2 (no iPod) PASS** — iPod was unplugged for this run. `cargo run -- --dry-run` produced one prompt print and exited 1 with `Error: iPod required; aborted`.
- **Scenario 3 (source unreachable) PASS** — passed `--source "\\nonexistent-host\nope"` plus a fake `iPod_Control\iTunes\iTunesDB` file under `%TEMP%\fake-ipod\` to bypass mount detect; got the "Choose:" prompt and clean abort.
- **Scenario 4 (corrupt config) PASS (after fix)** — corrupt `config.toml` triggered the orchestrate-level wrapper. First run silently **deleted** the user's config.toml because plain-mode `choice: 0` mapped to `Custom(0) = Reset`. Same root cause as Scenario 1; same fix (`a7d58eb`). Re-run confirmed config is preserved and the tool aborts with `Error: config parse failed; aborted`.
- **Scenario 5 (per-track failure):** pending — needs iPod connected.
- **Scenario 6 (SMB glitch):** deferred — no deterministic way to inject intermittent failures.
- **Scenario 7 (wizard via Progress::Form):** plain-mode noop path verified (`ensure_source_or_wizard` correctly skips wizard when stdout is not a TTY, lets `config::resolve` produce its standard `no source library specified` error). Actual Form rendering needs an interactive run.
- **Plain-mode default-abort gotcha:** original plan used `choice: 0` as the "default-abort" sentinel for plain mode. This is wrong whenever option index 0 isn't an inert/abort choice — Retry loops infinitely; destructive options (e.g. config Reset) fire silently. Lesson: rejecting an interactive prompt in a non-interactive context should NEVER map to a real option index. Use an out-of-range index so `outcomes.get(...).unwrap_or(Abort)` handles it generically.

## Phase 2 Gate B (2026-05-17)

- **Result:** PASS.
- **Test subset:** `<source-library-path>\Big Wild\Superdream\` (12 FLACs).
- **First-run action plan:** Add=12, Modify=0, Remove=0, Unchanged=0.
- **First-run elapsed:** 23.7s (release build; ~2.0s per track including transcode + cp + DB add). Much faster than the plan's "a few minutes" estimate — release-mode transcode + a 12-track album fits comfortably under 30s on this hardware.
- **Second-run action plan:** Add=0, Modify=0, Remove=0, Unchanged=12.
- **Second-run elapsed:** 0.8s (walk + fingerprint + diff; no transcoding).
- **Manifest persistence:** JSON valid, all 12 entries have non-zero `ipod_dbid`, backslashed `ipod_relpath` like `iPod_Control\Music\F08\libgpod712455.m4a`, `source_known=true`, full UNC `source_path`. Round-trips cleanly across the no-op second run.
- **GLib noise observed:** `WARNING: Error parsing recent playcounts` (open) and `CRITICAL: itdb_splr_validate: assertion 'at != ITDB_SPLAT_UNKNOWN' failed` (write) — both benign and expected; Task 11 will route these through tracing.
- **ffprobe duplicate-key bug surfaced and fixed:** Picard-tagged FLACs frequently emit BOTH `TRACKTOTAL` and `TOTALTRACKS` (and `DISCTOTAL`/`TOTALDISCS`). The original `#[derive(Deserialize)]` with `#[serde(alias = ...)]` rejects this as a duplicate field. Replaced with a manual `Deserialize` for `ProbeTags` that lowercases keys and applies first-write-wins. Added regression test `probe_output_handles_duplicate_synonymous_keys`.

## Phase 2 Task 4 — source walker + BLAKE3 fingerprint (2026-05-18)

- **PID-based temp dir shared across parallel tests causes flaky failures.** The plan's `tempdir_under_target()` generates `walker-<pid>` — identical across all tests in one run. Rust test harness runs tests in parallel by default; tests clobber each other's files. Fix: add an `AtomicU32` counter to produce `walker-<pid>-<n>` (unique per test invocation). One-liner fix; zero API impact.

## Phase 0

- **bindgen + libclang on Windows (Task 5):** VS18 Community ships `clang-format.exe` and `clang-tidy.exe` under `VC\Tools\Llvm\x64\bin` but does NOT include `clang.exe` or `libclang.dll`. bindgen 0.72 needs `libclang.dll` to parse C headers. Install the full LLVM toolchain via `winget install --id LLVM.LLVM` (drops it at `C:\Program Files\LLVM\`). Either add `C:\Program Files\LLVM\bin` to `PATH` or set `LIBCLANG_PATH=C:\Program Files\LLVM\bin` for cargo.
- **bindgen needs GLib include paths (Task 5):** `vendor/libgpod/include/gpod/itdb.h` includes `<glib.h>` and `<glib-object.h>`. Those live under `C:/msys64/mingw64/include/glib-2.0` and `C:/msys64/mingw64/lib/glib-2.0/include` (the second has `glibconfig.h`). `build.rs` adds both via `.clang_arg("-I...")`. Without these bindgen errors out on the very first include.
- **bindgen 0.72 allowlist for the spike (Task 5):** Allowlist `itdb_.*`, `Itdb_.*`, `ITDB_.*`, `g_error_.*`, `GError`, `GList`. `GError` and `g_error_*` are pre-added so Task 6 doesn't have to revisit `build.rs`. `GList` is needed for walking the track list in Task 6.
- **`Itdb_Track` type name (Task 5):** bindgen 0.72 generates `Itdb_Track` (matching the C typedef) directly under the `ffi` module — no mangling. `size_of::<ffi::Itdb_Track>()` on x86_64-pc-windows-msvc with this libgpod build = **640 bytes**.
- **build.rs DLL copy is load-bearing for `cargo run`:** Without copying `vendor/libgpod/bin/*.dll` into `target/<profile>/` at build time, `cargo run` fails immediately with "gpod.dll was not found". The current `build.rs` copies the full closure (16 DLLs: gpod.dll + 15 MinGW/GLib runtime DLLs).
- **build.rs target dir must come from `OUT_DIR` ancestors, not `CARGO_MANIFEST_DIR/target/$PROFILE`:** `CARGO_TARGET_DIR` (or `[build] target-dir` in `.cargo/config.toml`) relocates the real target tree. Computing it from the manifest dir copies DLLs into the wrong place. `OUT_DIR = <real_target>/<profile>/build/<pkg>-<hash>/out`, so `out_dir.ancestors().nth(3)` yields `<real_target>/<profile>` reliably.
- **bindgen allowlist `allowlist_type("Itdb_.*")` covers most types but misses the smart-playlist enums (`ItdbSPLMatch`, `ItdbLimitType`, `ItdbLimitSort`, `ItdbSPLField`) because they lack the underscore after `Itdb`. If/when Phase 1+ touches smart playlists, broaden to `allowlist_type("Itdb.*")` or add explicit entries.

## libgpod acquisition research (2026-05-17)

### Searches conducted

- **MSYS2**: Not found — `packages.msys2.org/search?q=libgpod` returned zero results as of 2026-05-16. No `mingw-w64-x86_64-libgpod`, `mingw-w64-ucrt-x86_64-libgpod`, or any variant exists in the MSYS2 package database. Confirmed by checking the MSYS2 GitHub repo `msys2/MINGW-packages` via `gh api` search (no results).
- **gtkpod SourceForge**: Last libgpod source release is v0.8.3 in the `libgpod-0.8` folder (folder last modified 2013-09-04). No Windows binaries, DLLs, or installers found in any subfolder (`libgpod-0.8`, `libgpod-unstable`, `libgpod-0.7.9x`, `libgpod-0.7.2`, `libgpod-0.7.0`, `libgpod-0.6.0`). The `libgpod` root was last touched 2011-01-03.
- **GitHub (fadingred/libgpod and forks)**: `fadingred/libgpod` — no Releases, no Windows artifacts, Unix autotools only. `gtkpod/libgpod` — no Releases published. `strawberrymusicplayer/strawberry-libgpod` — has a CMakeLists.txt (added 2021-08-19) but no Releases, no Windows binaries. The CMakeLists.txt uses GCC-only flags (`-std=c99`, `-Wall`, `-Wmissing-declarations`, etc.) that are incompatible with MSVC cl.exe. `jburton/libgpod`, `hyperair/libgpod`, `gerion0/libgpod` — no Windows artifacts in any.
- **vcpkg port**: Does **not exist** — confirmed via `gh api repos/microsoft/vcpkg/contents/ports` search and `vcpkg.io/en/packages.html?query=libgpod` (no results). There is no `libgpod` port in the vcpkg curated registry as of May 2026 (2807 total ports).
- **Strawberry MSVC build chain**: `strawberrymusicplayer/strawberry-msvc-build-tools` explicitly sets `-DENABLE_GPOD=OFF` in both debug and release CMake configurations. The `strawberry-msvc-dependencies` releases (most recent: tag 3520, 2026-05-16) contain no libgpod. This is the most active Windows MSVC music-player dependency chain and it deliberately excludes libgpod.
- **Forum / contributor builds**: Strawberry forum thread about libgpod+iPod on Windows discussed only macOS/Linux. No Hydrogenaudio or other community contributor with a known-working Windows MSVC libgpod recipe found via web search for 2022–2026.

### Candidates considered

- `strawberrymusicplayer/strawberry-libgpod` (CMake fork, last commit 2021-08-19): Has a CMakeLists.txt that could theoretically be built on Windows, but uses GCC-only compiler flags, requires GLib/GModule/GObject/libplist/SQLite/zlib all pre-built for MSVC, and has never been released as a Windows binary. Would require patching the CMakeLists.txt and sourcing all transitive MSVC deps. Not viable as a prebuilt.
- Any MSYS2 MinGW build (hypothetical): Even if one were built, it would link against the MinGW runtime, not UCRT/MSVC CRT, making it incompatible for use from an MSVC-compiled Rust binary without a very careful ABI boundary analysis.
- Building from source with autotools + MSYS2/MinGW cross-toolchain: Possible but produces MinGW-linked DLLs, which introduce runtime mismatch risk with `cargo build --target x86_64-pc-windows-msvc`.

### Decision: Branch B — Build from source

- **Reason:** No prebuilt libgpod for Windows x64 exists anywhere (MSYS2, SourceForge, GitHub Releases, vcpkg) as of May 2026; even the most active Windows MSVC music-player project (Strawberry) explicitly disables libgpod support on Windows.
- **Next action:** Proceed to Task 4 — build from source. The recommended path is to use the `strawberrymusicplayer/strawberry-libgpod` CMake fork as the source base (it has already eliminated the autotools dependency), patch the CMakeLists.txt to replace GCC-only flags with MSVC-compatible equivalents, and hand-build its transitive dependencies (GLib, libplist, SQLite, zlib) either via vcpkg (all four are available vcpkg ports) or the strawberry-msvc-dependencies tarball. There is no vcpkg port for libgpod itself, so a custom CMake build step in the repo (vendored under `vendor/libgpod/`) is the cleanest path.

## Task 6 spike — open iTunesDB and list tracks (2026-05-17)

- **`itdb_parse_file` is the right symbol for a known DB file path.** bindgen 0.72 exposes both `itdb_parse(mp, error)` (takes mount path, e.g. `G:\`) and `itdb_parse_file(filename, error)` (takes the full file path to `iTunesDB`). The spike uses `itdb_parse_file` per the plan. Either would have worked on a properly-mounted iPod, but `itdb_parse_file` is the lower-friction choice when you already know the DB path.
- **FirewireGUID was NOT needed for read.** Plain `itdb_parse_file` on the iPod Classic 7G (`EXAMPLE1234`) DB returned a valid `Itdb_iTunesDB *` with `tracks` populated. The SPEC §8 row 2 risk (hashed DB signature blocking parse) did not materialize for reads. Whether it bites on *write* (Phase 1) is still unknown — verifying the hashed signature is a write-side concern in libgpod, not a read-side one. Plan for needing `itdb_device_set_sysinfo` or env-var FirewireGUID setup before the first `itdb_write` call.
- **`Itdb_Track` field names verified live.** `title`, `artist`, `album` (all `*mut gchar`) — accessed via `(*track).title` etc. in `main.rs`. Names match the C header exactly; bindgen did not mangle.
- **`Itdb_iTunesDB::tracks` is a `*mut GList`.** Walked with `node = (*node).next` and `track = (*node).data as *mut Itdb_Track`. `_GList { data, next, prev }` layout confirmed in the bindings (`prev` unused for forward iteration).
- **`g_error_free` requires a separate import lib.** It lives in `libglib-2.0-0.dll`, not `gpod.dll`. The first link attempt failed with `LNK2019: unresolved external symbol g_error_free`. Fix: generated `vendor/libgpod/lib/glib.lib` via `dumpbin /exports libglib-2.0-0.dll` + `lib /def /machine:x64` (same pattern used for `gpod.lib` in Task 3 Step 10) and added `cargo:rustc-link-lib=dylib=glib` to `build.rs`. The `.def` has 1912 exports. Other glib symbols Phase 1 may need (e.g. `g_list_*`, `g_free`) are already covered by this single import lib.
- **libgpod emits non-fatal GLib WARNING on stderr during parse.** Saw `** (process:NNNN): WARNING **: hh:mm:ss.xxx: Error parsing recent playcounts` — likely because the freshly-restored iPod has no `Play Counts` companion file yet. Parse succeeded anyway. For end-user output in Phase 2, consider installing a `g_log_set_handler` to suppress or reformat these.
- **Read-only invariant holds.** After `cargo run`, `Get-ChildItem G:\iPod_Control -Recurse -File | Where-Object LastWriteTime -gt (Get-Date).AddMinutes(-30)` returned empty. `itdb_parse_file` + walk + `itdb_free` does not touch the iPod filesystem.
- **Live spike output (1 track on device):**
  ```
  Opening iTunesDB at: G:\iPod_Control\iTunes\iTunesDB
    [1] Beck — Colors — Colors
  Total tracks: 1
  ```

## Phase 1 design notes (carried from Task 6 spike review)

- **Wrap `Itdb_iTunesDB *` in a RAII type before Phase 1 grows error paths.** The Task 6 spike used a bare pointer with manual `itdb_free` at the end. Currently safe because no `?` operators between open and free — but every error return Phase 1 adds becomes a potential leak. Pattern:
  ```rust
  struct OwnedDb(*mut ffi::Itdb_iTunesDB);
  impl Drop for OwnedDb { fn drop(&mut self) { unsafe { ffi::itdb_free(self.0) }; } }
  ```
  Apply the same pattern to `Itdb_Track *` if Phase 1 holds tracks outside libgpod's internal lists.
- **Use `itdb_tracks_number(db)` for track counts** rather than walking the GList manually. Faster, single source of truth, and avoids the spike's `count`-vs-`printed` ambiguity (the spike's null-data defensive check creates a latent inconsistency that's invisible at 1 track and would be confusing at 1,400).
- **`CString::new(path.to_str().unwrap())` is unsound for arbitrary paths.** The spike gets away with it because `IPOD_MOUNT` is a const. Phase 1 takes paths from CLI args and directory walks where non-UTF-8 is possible. Use `.to_str().ok_or_else(|| anyhow!("path contains non-UTF-8: {}", path.display()))?` instead. Same for `CString::new` itself — a path containing interior NUL bytes (impossible on Windows but reachable via crafted input) would panic on `?`.
- **Rename `cstr_or_empty` → `cstr_to_string_or_default` when promoting it to a helper module.** Current name implies empty string but it returns `"<none>"`. Phase 1 likely wants the caller to choose the default (`fn cstr_to_owned(p: *mut c_char) -> Option<String>` returning `None` on null is cleaner).

## Phase 0 gate (2026-05-17) — PASS

- **Result:** PASS — all four acceptance criteria met.
- **libgpod build provenance:** Branch B — built from source via MSYS2/MinGW from `fadingred/libgpod` commit `4a8a33ef4bc58eee1baca6793618365f75a5c3fa` with two patches (`vendor/libgpod/patches/`). libplist + SQLite/iTunesCDB path stripped per SPEC §7 (nano 5G+ out of scope). See `vendor/libgpod/BUILD-NOTES.md` for full reproduction.
- **Acceptance checks:**
  - `cargo clean && cargo build` from clean checkout: PASS (13.87s).
  - `cargo run` on iPod at `G:\`: prints `Total tracks: 1` and `[1] Beck — Colors — Colors`. Matches what was synced via iTunes 12.6.5.3.
  - Read-only invariant verified: zero files modified under `G:\iPod_Control\` after the spike ran.
  - iPod post-eject state: boots normally, plays the listed track.
- **Hashed iTunesDB signature (SPEC §8 row 2) on read:** **NOT triggered.** libgpod parses the Classic 7G's hashed DB without needing FirewireGUID setup on the read path. Risk remains open for the write path — Phase 1 must verify before the first `itdb_write` (consider `itdb_device_set_sysinfo` or env-var FirewireGUID; libgcrypt is vendored and ready to sign).
- **Phase 1 starting state:** see `## Phase 1 design notes` section above for the four carry-forward design items from the spike code review (RAII drop guard for `Itdb_iTunesDB`, `itdb_tracks_number`, CString/path safety, helper naming). The vendored libgpod + glib import libs cover the FFI surface for Phase 1 with no further `lib /def` work expected for tag/write APIs (`itdb_track_*`, `itdb_playlist_*`, `itdb_cp_track_to_ipod`, `itdb_write`).
- **ffmpeg FLAC art embedding (Task 3 smoke test):** `ffmpeg -i audio.flac -i art.png -map 0:a -map 1:v -c:a copy -c:v png -disposition:v:0 attached_pic out.flac` correctly embeds art as FLAC PICTURE block. `-attach` and `-f lavfi` one-shot approaches don't work; the two-input `-map` approach is the correct recipe. The `?` in `-map 0:v?` (in `ffmpeg_args`) correctly makes art optional — audio-only FLACs transcode cleanly without it.
- **Installed ffmpeg is Gyan.dev full build (ffmpeg 8.0.1)** — includes ALAC encoder (`alac` native), confirmed working for FLAC→ALAC transcoding into `-f ipod` container with PNG art passthrough.

## Phase 1 Task 5 — ipod::db OwnedDb / write path (2026-05-18)

- **`Itdb_Track` field names match plan exactly:** `title`, `artist`, `album`, `albumartist`, `genre`, `composer`, `year`, `track_nr`, `tracks`, `cd_nr`, `cds` — all present, all named as the C header has them. No bindgen mangling. `apply_tags` in `src/ipod/db.rs` writes them directly.
- **`g_strdup` / `g_free` ARE present in bindings as of allowlist update in Task 4 build.rs** even though they are `#define` macros in modern glib headers (`gstrfuncs.h`/`gmem.h`). bindgen 0.72 picks up the underlying function declarations declared by `GLIB_AVAILABLE_IN_ALL void (g_free)(gpointer)`. If a future glib bump drops the function form entirely, fall back to declaring `extern "C" { pub fn g_free(...); pub fn g_strdup(...); }` in `src/ffi.rs` (the DLL exports them either way — verified in `vendor/libgpod/lib/glib.def` at lines 453 and 1344).
- **Build-output staleness gotcha:** there can be multiple `target/debug/build/ipod-sync-<hash>/out/libgpod_bindings.rs` directories from prior dependency-version churn. `Get-ChildItem ... | Select-Object -First 1` picked an old one missing recent allowlist entries. `cargo build` regenerates only into the *current* hash directory, so trust `cargo build`'s redefinition errors (E0428) over manual `Select-String` checks against the wrong file.
- **gboolean check convention:** `itdb_write` and `itdb_cp_track_to_ipod` return GLib `gboolean` (bindgen `i32`). Failure is `== 0`, not Rust-style `!success`. Always pair with `gerror_to_anyhow(api, err)` to extract the GError message and free it.
- **`itdb_cp_track_to_ipod` ownership transfer is "on success only":** the call adds the track to `db.tracks` only when it returns TRUE. On failure (return == 0) we still own the freshly-`itdb_track_new`'d pointer and must `itdb_track_free` it before propagating the error — otherwise it leaks. On success, the track is owned by the DB and will be freed transitively by `itdb_free` in `OwnedDb::drop` — manual `itdb_track_free` would be a double-free.

## Phase 1 album art Plan B — pixbuf gap (2026-05-17)

- **Vendored libgpod has NO gdk-pixbuf support.** `gpod.dll` (built per `BUILD-NOTES.md`) only imports `libglib-2.0-0.dll`, `libgmodule-2.0-0.dll`, `libgobject-2.0-0.dll`, `libintl-8.dll`, `KERNEL32.dll`, `msvcrt.dll`, `libxml2-16.dll`, `zlib1.dll` — verified via `llvm-objdump -p`. No `libgdk_pixbuf-2.0-0.dll`. Confirmed because MSYS2 `mingw-w64-x86_64-gdk-pixbuf2` was not in the build dependencies and `./configure` was not given `--with-gdk-pixbuf` (or its auto-detect path).
- **Consequence:** `itdb_track_set_thumbnails_from_data`, `itdb_track_set_thumbnails_from_file` (a.k.a. `itdb_track_set_thumbnails`), and `itdb_track_set_thumbnails_from_pixbuf` are all exported as symbols but return `FALSE` (0) at runtime without setting a `GError`. The libgpod 0.8.x source conditionally compiles the body on `HAVE_GDKPIXBUF`; without it, the function is a stub.
- **Reproduction:** `cargo run -- "...City of Sound.flac"` with `art_bytes = Some(124919 bytes)` errored with `itdb_track_set_thumbnails_from_data failed`. iPod state unchanged (run aborted before `itdb_write`).
- **Two fix options for the next session:**
  1. **Rebuild libgpod with `--with-gdk-pixbuf`** and ship `libgdk_pixbuf-2.0-0.dll` plus its transitive deps (`libpng`, `libjpeg`, `libtiff`, `libwebp`, `libheif`, `libffi-7`/`libffi-8`, possibly more) in `vendor/libgpod/bin/`. Adds ~10–15 DLLs. Pixbuf needs its loaders module path set at runtime via `GDK_PIXBUF_MODULEDIR` env var — another wrinkle to handle in `build.rs` or main.
  2. **Bypass pixbuf entirely:** decode JPG in Rust (e.g. `image` crate), resize to the iPod Classic 7G's thumb sizes (200x200 + 720x720 from `ipod_artwork_capabilities` in libgpod source, or whatever `itdb_device_get_artwork_formats` reports for this device), convert to the F1024 format (RGB565 little-endian for Classic 7G's primary thumb), then construct `Itdb_Thumb_Ipod_Item` / call `itdb_artwork_set_thumbnail_from_data` after artwork allocation. This works because the no-pixbuf path can still write raw pre-decoded bytes — but only via the `itdb_artwork_*` API set, not the high-level `itdb_track_set_thumbnails_*` API. Bigger code surface, no DLL re-bundling.
- **Plumbing wired up regardless:** `src/transcode.rs::extract_cover_art` + `temp_art_path`, `src/ipod/db.rs::add_track_with_file` signature now accepts `Option<&[u8]>`, `src/main.rs` extracts art from the FLAC via ffmpeg and passes it through. The `itdb_track_set_thumbnails_from_data` call site is correct — just blocked on the lib gap. Either fix above can re-use the orchestration unchanged.

## Phase 1 Task 4 — ipod::device (2026-05-17)

- **Target iPod uses flat-text `SysInfo`, NOT `SysInfoExtended` XML.** The iPod Classic 7G (MB029, drive-modded 160 GB) has `iPod_Control\Device\SysInfo` (no extension) with line-oriented `Key: value` content, not an XML plist. `SysInfoExtended` does not exist on this device. The parser is a trivial `split_once(':')` loop — no XML, no plist. Any code path via `itdb_device_read_sysinfo_xml` would be wrong for this device.
- **`itdb_device_set_sysinfo` is the correct FFI symbol for pushing FirewireGuid.** Confirmed present in bindgen output at line 777. Signature: `fn itdb_device_set_sysinfo(device: *mut Itdb_Device, field: *const gchar, value: *const gchar)`. Called with `"FirewireGuid"` as the field key — matching case exactly as it appears in SysInfo.
- **`iPod_Control` is a hidden directory on Windows** — `Get-ChildItem` needs `-Force` to list it, but `Test-Path` and `Copy-Item` work without it.
- **SysInfo fixture committed at `tests/fixtures/sample-sysinfo.txt`.** Real hardware value: `FirewireGuid: 0x000A27002138B0A8`, `ModelNumStr: MB029`. Not a secret (hardware-bound, like a MAC address).

## Phase 1 gate (2026-05-17) — PASS

- **Result:** PASS — all five acceptance criteria met (boot, both pre-existing tracks present, new track plays, metadata correct, album art on Now Playing).
- **Test track:** Big Wild — Superdream — "City of Sound" (\MUSICHOST\data\media\music\Big Wild\Superdream\01 - City of Sound.flac, 232 sec FLAC, 28 MB, embedded 1000×1000 JPG art, rich MusicBrainz tags).
- **iPod state before Phase 1:** 1 track (Beck "Colors" from Phase 0).
- **iPod state after Phase 1:** 3 tracks (Beck "Colors", Big Wild "City of Sound" without art from first attempt, Big Wild "City of Sound" with art from Plan B retest). The duplicate is a known artifact — libgpod doesn't dedup, Phase 2 manifest will.
- **iTunesDB write (signed):** PASS — itdb_write succeeded twice; DB length grew 21046 → 22718 → 24130 bytes; LastWriteTime updated each run.
- **FirewireGuid wiring:** required and worked — read from `G:\iPod_Control\Device\SysInfo` (flat-text format, not SysInfoExtended XML) and pushed via `itdb_device_set_sysinfo`. Hashed-DB-signing risk SPEC §8 row 2 → **retired** for both read and write paths.
- **Album art Plan A (ffmpeg in-band MP4 atom):** **rejected by iPod Classic UI** — the in-band cover atom is present in the .m4a file but Classic firmware doesn't read it; ArtworkDB + ithmb blobs are the only path. SPEC §8 row 3 risk materialized as expected.
- **Album art Plan B (libgpod itdb_track_set_thumbnails_from_data):** initially failed because the Phase 0 libgpod build lacked gdk-pixbuf (functions exported but no-op). Rebuilt libgpod with gdk-pixbuf + image-format deps (libpng/libjpeg-turbo/libtiff) + vendored pixbuf loader plugins with a GDK_PIXBUF_MODULE_FILE env var wired through build.rs. Verified: 4 new .ithmb blob files (F1055/F1060/F1061/F1068 — multiple iPod display sizes) plus ArtworkDB grew by ~1KB per write. Art shows correctly on Now Playing.
- **iPod post-eject boot:** boots normally, plays all three tracks, art displays on the Plan B Big Wild track.

### Issues to address in Phase 2

- **No deduplication.** libgpod allows the same source to be added repeatedly; right now the iPod has two Big Wild "City of Sound" tracks. SPEC §4.3's manifest-diff logic will handle this — modified tracks are delete-and-add, not duplicate.
- **TRACKTOTAL/DISCTOTAL aliases not handled.** ffprobe extracts `track: "1"` (lone number, not "1/12") + separate `TRACKTOTAL: "12"`. Current `split_pair` loses the total. Add aliases for TRACKTOTAL/TOTALTRACKS/DISCTOTAL/TOTALDISCS in `ProbeTags` and fold them into `Tags.tracks` / `Tags.discs` in `tags_from_probe`.
- **`loaders.cache` contains dev-tree absolute paths.** Works on this machine; breaks for distribution and on a fresh checkout. Fix in build.rs: regenerate the cache at build time by invoking `gdk-pixbuf-query-loaders.exe` against the staged `target/<profile>/pixbuf-loaders/` directory.
- **Two benign GLib warnings during write** that are noisy but not failures:
  - `WARNING: Error parsing recent playcounts` — iPod's `PlayCounts.plist` isn't always present on freshly-restored devices.
  - `CRITICAL: itdb_splr_validate: assertion 'at != ITDB_SPLAT_UNKNOWN' failed` — libgpod's smart-playlist validator walking pre-existing empty/unrecognized rules.
  Install a `g_log_set_handler` in Phase 2 to suppress (or reformat) these so they don't clutter user output.
- **Cleanup orphan tracks if write fails mid-way.** Currently if `itdb_cp_track_to_ipod` succeeds but `itdb_write` fails, the .m4a is orphaned on the iPod. `--rebuild-manifest` recovers from this; document the failure mode in the user-facing error message.

## Phase 2 Gate A (2026-05-18)

- **Result:** PASS.
- **Source:** `<source-library-path>\`
- **FLACs found:** 1407
- **Walk elapsed (release build, end-to-end `cargo run --release -- --dry-run`):** 80.3s
- **Action plan:** Add=1407, Modify=0, Remove=0, Unchanged=0 (expected — no manifest yet).
- **Notes:** Count lines up with SPEC §11's "≈1,400" target. SMB walk + first-MiB BLAKE3 read across 1,407 files completed in 80s — comfortably inside the 30-180s window. No hangs, no errors, no warnings. `cargo build --release` from a clean release tree took 27.5s (dep graph compile); subsequent re-link inside the `cargo run` invocation was 0.16s. Bare-walk elapsed (excluding cargo's already-built check) is dominated by SMB I/O, not Rust work.

## Phase 2 §6 #2 stat-only diff fast path (2026-05-18)

- **Result:** PASS — 1,407-file no-op second run drops from 93.8s to ~0.55s (~170× speedup, ~9× under the 5s SPEC §6 #2 budget).
- **Design:** `SourceEntry` is now stat-only (path/mtime/size, no fingerprint). `manifest::diff` takes `impl FnMut(&Path) -> Result<String>` and only invokes it on the slow path — when stored (mtime, size) doesn't match. New `diff_unchanged_after_touch_but_same_content` test plus `never_called()` callback helper assert the fast path doesn't read file content.
- **Bench-diff example (`examples/bench-diff.rs`):** lets us measure walk + diff time against the real manifest without the iPod plugged in. Source = `\\HOST\data\media\music`. Reproducible target for any future I/O regression.
- **Live numbers (release, SMB):** load manifest 0.001s; walk 1407 files 0.548s; diff 0.002s with 0 fingerprint reads. Pure SMB stat alone is the floor; we're already on it.
- **Fingerprint computation moved to `add_one`:** `add_one(&db, &src) -> Result<(TrackHandle, String)>` — the orchestrator computes the fingerprint once per Add/Modify and threads it into `entry_from(&src, &handle, &fp)`. Walker never reads file content anymore.
- **mtime-touched-but-content-identical case** correctly classified as Unchanged for Phase 2 (slow path runs once, callback returns matching fp). Acceptable mild inefficiency: next run still re-fingerprints because the manifest's stored mtime is stale. Refreshing stored mtime to suppress that is Phase 3+.

## Phase 2 Task 1 — scaffold + carry-forwards (2026-05-18)

- **`itdb_get_mountpoint` IS in bindgen output** (line 722 of `libgpod_bindings.rs`): `pub fn itdb_get_mountpoint(itdb: *mut Itdb_iTunesDB) -> *const gchar`. So the Play Counts.bak fix used the FFI-based approach (read mount from the DB pointer at write time) rather than the stored-mount-path fallback. No `OwnedDb` field addition was needed.
- **`build.rs` loaders.cache regen at build time confirmed working.** `target/debug/pixbuf-loaders/loaders.cache` now references `F:/repos/ipod-sync/target/debug/pixbuf-loaders/libpixbufloader-*.dll` (staged paths) instead of vendor absolute paths. Generated via `C:\msys64\mingw64\bin\gdk-pixbuf-query-loaders.exe` passed the staged DLL list as args; tool writes a header `Created by gdk-pixbuf-query-loaders from gdk-pixbuf-2.44.6`. Fallback to vendor cache copy still in place for envs without MSYS2.

## wipe-tracks dev utility (2026-05-17)

- **`itdb_playlist_remove_track(NULL, track)` with a null playlist removes the track from every playlist** — confirmed working for the wipe case. Do not call `itdb_track_unlink` separately; `itdb_track_remove` covers the DB tracks list removal and struct free in one call.
- **`itdb_filename_on_ipod` returns a `g_strdup`'d path — must `g_free` it.** Returns `NULL` if the track has no on-disk path (can happen for tracks added without `itdb_cp_track_to_ipod`). Always null-check before use.
- **`itdb_write` on Windows fails with "Error renaming 'Play Counts' to 'Play Counts.bak' (File exists)"** when both files are present. Windows rename does not atomically replace an existing file (unlike POSIX `rename(2)`). Fix: delete `Play Counts.bak` before calling `itdb_write`. The DB track data is written BEFORE the play counts rotation, so even if the rename error is raised, the iTunesDB on disk will reflect the in-memory state. Verified: after first run (which errored on play counts rotate), second run saw 0 tracks in the DB.
- **lib.rs + bin target coexist cleanly.** Adding `src/lib.rs` with `pub mod ffi; pub mod ipod; pub mod transcode;` alongside the existing `[[bin]]` target required no Cargo.toml change (Cargo auto-detects `src/lib.rs`). Replace `mod ffi;` etc. in `main.rs` with `use ipod_sync::ffi;` etc. Tests in main.rs continue to work via `use super::*`. The library crate name matches the package name with hyphens → underscores.


## Phase 2 Gate C — full library acceptance (2026-05-17) — PASS

- **Result:** PASS. All exercised SPEC §6 acceptance criteria met.
- **Source library:** `<source-host>\data\media\music` (1,407 FLACs).
- **iPod:** Classic 7G at G:, empty going in.
- **Full sync wall-clock:** ~90 minutes (TUI-driven, transcode-bound).
- **iPod state after sync:** 1,407 m4a files in `iPod_Control\Music\F*`, iTunesDB grew from 18 KB → 2,094,506 bytes, 5 files in `iPod_Control\Artwork\` (ArtworkDB + 4 .ithmb thumbnail blobs).
- **Manifest:** 1,407 entries, valid JSON.

### SPEC §6 acceptance scorecard

- **#1** (empty iPod → full sync, playable, metadata + art): **PASS** — physical verification: iPod boots normally, Music → Songs lists ~1,407 tracks, sampled tracks play with correct metadata + art on Now Playing.
- **#2** (no changes → < 5s): **PASS** after Phase 2.1 mtime+size fast-path optimization. Actual second-run: 945 ms (PowerShell-measured command time). The original implementation was 93.8s (re-fingerprinting all files unconditionally); the fix drops `SourceEntry.fingerprint` from the walker entirely and only computes it inside the diff when mtime+size disagree with the manifest. For an all-unchanged library, zero file reads beyond stat() — ~100× speedup.
- **#3** (add 5 → only 5 processed): **NOT EXERCISED** in Gate C — same code path as the 1,400 Adds in the main run.
- **#4** (delete 5 → only 5 removed): **NOT EXERCISED** in Gate C — same code path as the manifest's Remove handling.
- **#5** (--rebuild-manifest works): **NOT EXERCISED** in Gate C — deferred to future verification.
- **#6** (--dry-run writes nothing): **PASS** — manifest LastWriteTime unchanged after dry-run invocation.

### Phase 1 carry-forwards verified at scale

- **Pixbuf-backed artwork** (Plan B from Phase 1 Task 6b): worked for all 1,407 tracks. ArtworkDB + thumbnail blobs created correctly.
- **Play Counts.bak rename fix**: never re-surfaced during the run.
- **TRACKTOTAL/DISCTOTAL alias handling**: all Picard- and Plex-tagged albums processed without serde duplicate-key errors.
- **GLib log handler**: kept stderr quiet; benign WARNING/CRITICAL noise routed through tracing.

### Observations from the full-scale run

- **Plex-written album art has bad metadata on some files.** Surfaced during physical verification — some tracks showed wrong art on the iPod's Now Playing. Root cause is Plex's media-scanner writing inconsistent cover-art bytes into FLAC tags on the server. Source-data fix, not a tool bug. The user is going to clean up Plex's tagging on the server side.
- **Walker time** is the dominant cost when nothing has changed: ~0.55s for stat()-ing 1,407 SMB files. With the fingerprint short-circuit, that's the whole runtime. Acceptable.

### Phase 3 carry-forwards

- **mtime-touched-but-content-identical files** correctly classify as Unchanged but re-fingerprint every subsequent run because the stored mtime stays stale. Phase 3+ refinement: refresh stored mtime on the slow-path-Unchanged case so the next run hits the fast path again. Tiny code change, real-world impact on libraries with `touch`-style operations.
- **Plex-bad-art investigation**: worth a small forensic pass to confirm which tracks have which issue, so the user can fix at the source.

## Phase 3.y gate (2026-05-17) — PASS

- **Result:** PASS — UX layer ships.
- **Wizard:** launches when no source set; saves to `%APPDATA%\ipod-sync\config.toml`; orchestrator continues after Enter.
- **Review state:** action plan renders correctly; `t` toggles `--no-delete` and flips the Remove count display; `d` exits cleanly with "Dry run; nothing was written"; `q` quits without changes; `a` proceeds to apply.
- **--apply flag:** skips review, applies immediately. Validated in no-change run (~1s).
- **--dry-run flag:** skips review, exits after summary.
- **--save-config:** persists effective config (tested implicitly via wizard write — explicit `--save-config` flag still standing for future ad-hoc persist cases).
- **Non-TTY rejection:** confirmed errors clearly when `--no-tui` is set without explicit `--apply` or `--dry-run`.

### Phase 3.z carry-forward

User flagged: "we might want to make the UX a bit more interactive so that all interactions are done in the TUI (even errors, etc.)" — captured as discrete roadmap item "Phase 3.z — TUI-first error UX" in `docs/superpowers/specs/2026-05-18-post-v1-roadmap.md`.

## Phase 6 M3 — paused mid-smoke (2026-05-25)

- **Status:** Rust implementation complete (149 lib + 3 integration tests pass; PollingDeviceWatcher, SyncScheduler, SyncOrchestrator with >50% bail, runtime select! loop with auto-sync, manual TriggerSync, mid-sync detach Aborted, broadcast device events). C# side has the tray-state machine, Sync Now menu, wizard-via-daemon-events. NOT tagged `phase-6-m3-complete` — smoke surfaced UX bugs that are easier to debug once M4 lands the visible status surface (popover + toasts).
- **Bugs caught + fixed during smoke (in order):**
  1. `d451294` — `spawn_sync_session` emitted Syncing StatusUpdate but no Idle on completion; tooltip stuck on "Syncing...". Fixed by broadcasting Idle StatusUpdate at the end of the session.
  2. `e88c6b9` — App.xaml.cs only started the tray event loop in the configured-no-wizard branch. After first-run wizard close the tray loop never started, so all subsequent daemon broadcasts went unread. Fixed by hooking `StartTrayEventLoop` into `WizardWindow.Closed`.
  3. `e9a54a8` — DaemonClient.TryDeserialize tried `IpcEvent` first via JsonPolymorphic; System.Text.Json throws `NotSupportedException` (NOT `JsonException`) on unknown discriminators. The catch only caught JsonException, so the first daemon-only event killed the reader loop entirely. Fixed by peek-the-`type`-discriminator-then-dispatch.
  4. `0aaee57` — `spawn_sync_session` was `.await`ed inline inside the runtime's `tokio::select!` loop, so the whole loop blocked for the duration of a sync (potentially hours for a real library). All client commands queued, all device events buffered, no broadcasts during sync. Fixed by fire-and-forget: orchestrator runs as a `tokio::spawn`'d task; completion ships back via a new `InternalEvent::SyncCompleted` mpsc channel; a 4th select arm handles state mutation + history append + post-sync Idle broadcast.
- **Open M3 questions to verify when M4 lands:**
  - Auto-sync on already-plugged-in iPod at daemon launch (initial broadcasts go out before UI subscribes; the M4 popover's GetStatus-on-open will paper over this for the user, and the new non-blocking runtime means GetStatus actually replies promptly).
  - Mid-sync flap from polling watcher's transient SysInfo read failures (libgpod holds writes during sync; scan_drive_for_ipod returns None briefly). Without a visible status surface, hard to tell if flap is happening. M4's activity feed will make this immediately visible.
  - tray-syncing.ico and tray-error.ico are M3 placeholders (copies of tray-idle.ico). Real artwork is M5 polish.
  - `format_iso8601` in runtime.rs is a placeholder (`@{unix_secs}`). M4 popover formats timestamps for display, so swap in proper RFC3339 emission then.
- **What works definitively:**
  - Daemon detects iPod, spawns sync subprocess, sync writes real tracks to iPod (verified via my own probe-launch when the daemon synced a 1275-track library against the user's actual hardware mid-debug).
  - Wizard subscribes to daemon DeviceConnected events and identifies the iPod when plugged in.
  - 50% per-track failure bail-out logic is unit-tested.
  - Runtime stays responsive during long-running syncs (new integration test).

## Phase 6 — diagnostic helper: scripts/probe-daemon.ps1 (2026-05-25)

- **Purpose:** Connect to `\.\pipe\ipod-sync`, dump every event, and probe with `get_status`. Bypasses the C# UI entirely so we can see exactly what the daemon emits on the wire.
- **When to use:** Any time the UI seems to misrepresent daemon state — proves whether the bug is daemon-side or client-side.
- **Usage:** `pwsh F:\repos\ipod-sync\scripts\probe-daemon.ps1`

- **macOS IOKit device identity:** the Apple vendor id (0x05AC) appears on multiple IORegistry nodes up the parent chain (USB interfaces, then the device). Only the `IOUSBHostDevice` node carries the `USB Serial Number` (= FirewireGuid). Match on `idVendor==0x05AC` AND serial-present, or you stop at an interface node and get a `None` identity. Capacity comes from the IOMedia `Size` property (fixes the old ioreg path's hardcoded-None capacity). See `ipod/macos_iokit.rs`.

- **macOS filename encoding vs DB tags (NFD vs NFC):** the APFS/HFS+ filesystem stores names in Unicode NFD (decomposed diacritics), while iTunesDB tags are NFC. A byte-comparison of a `/Volumes/data/media/music/<Artist>` folder name against the DB's artist string will falsely differ for diacritic names (e.g. "RÜFÜS DU SOL"). Normalize before comparing, or cross-check by album title. Matters for any source↔iPod dedup and for SP2 UI.
- **Daemon `configured` gate needs a persisted iPod, not just a source:** `trigger_sync` returns `sync_rejected{not_configured}` until `save_config` includes `ipod: {serial, model_label}` — `configured = configured_serial.is_some()` (runtime.rs). A source alone isn't enough.
- **classick source-change safeguard blocks non-TTY mass-deletes:** pointing `--source` at an empty/different root when the manifest has entries triggers an interactive "would REMOVE N tracks" prompt that aborts in plain mode even with `--apply`. For targeted removal use the `remove-synced` example (matches manifest DBIDs) instead.

- **afconvert upconverts to 32-bit ALAC → iPod Classic can't play it (skips after a few seconds, wrong time remaining).** `afconvert -d alac` preserves the source bit depth, and FLAC decodes to 32-bit in Core Audio, so a direct encode yields 32-bit ALAC (`afinfo`: "from 32-bit source"). The iPod Classic only decodes 16-bit ALAC. Fix (transcode.rs macOS path): two-step — `afconvert -f caff -d LEI16@44100` (→16-bit LPCM temp CAF) then `afconvert -f m4af -d alac` (→16-bit ALAC, "from 16-bit source", flag 0x1). This mirrors what iTunes does when syncing hi-res to an iPod. Symptom is track-specific: only hi-res (24/32-bit) sources skip; native-16-bit sources happen to survive the direct path.

- **Reconcile orphan-deletion used Windows backslash paths on macOS.** `TrackHandle.ipod_relpath` is stored backslash-encoded (`iPod_Control\Music\F09\x.m4a`, to match libgpod's colon-path). `reconcile_with_disk` joined that with the mount and `remove_file`'d it — a literal-backslash filename that never exists on macOS, so orphan cleanup silently failed and files piled up (harmless but wasteful). Fix: `relpath.replace('\\', std::path::MAIN_SEPARATOR_STR)` before the join (db.rs).

## SP3 released-build bug sweep (2026-07-13)

- **"Stuck on Set Up" / Settings shows defaults = config_update was never delivered.** The daemon only *pushes* `ConfigUpdate` on an iPod-name **change** or after a `save_config`; on attach it seeds the name from config, so `IpodNameResolved` dedups (`c.name == name`) and re-broadcasts nothing. The macOS app only sent `subscribe_device_events` + `get_status` on connect — never `get_config` — so `configuredSerial` stayed nil, `device.serial == nil` → `notConfigured`, and `SettingsView.syncFromConfig` bailed on nil config and showed hardcoded defaults. Fix (both sides): app sends `get_config` on every handshake (`DaemonClient`), and the daemon emits a `ConfigUpdate` on every device attach (`runtime.rs`). Config lives on the **Mac** (`~/Library/Application Support/classick/config.toml`), never the iPod — so "slow to figure out config" was this illusion, not real I/O.
- **macOS unplug was never detected: `IOServiceAddMatchingNotification` typo.** The removal registration passed `"IOServiceTerminated"` (trailing 'd'); the real IOKit key is `"IOServiceTerminate"` (no 'd') — `kIOMatchedNotification` *does* end in 'd', the asymmetry is the trap. Wrong string → `kIOReturnUnsupported` (0xe00002c7) → the terminate notification silently never fires, and macOS has no polling fallback, so unplugs went unnoticed. Now named constants + a regression test in `macos_iokit.rs`.
- **`daemon.enabled` (the auto-sync toggle) was never read — the daemon gated on `subsequent_sync_mode` instead.** macOS writes the toggle to `enabled` and hardcodes `subsequent_sync_mode=auto_apply`, so it auto-synced even when the user turned it off. Fixed the gate to read `daemon.enabled` (`auto_sync_enabled` → pure `config_auto_sync`, fail-safe off). **Windows still encodes on/off in `subsequent_sync_mode` + always sends `enabled: true`** — it needs its UI remapped to `enabled` or Manual mode will auto-sync (TODO comments in `WizardViewModel`/`SettingsViewModel`).
- **Audit before trusting a setting is wired.** Of the daemon settings with UI: `enabled` was never read (fixed), `first_sync_mode` is never read (review flow unimplemented — the sync subprocess is always spawned `--apply`; v1.1), `notify_on` was ignored on macOS (fixed — `Notifier.shouldPostSyncFinished`), `schedule_minutes` was only read at daemon startup (fixed — `SaveConfig` now calls `scheduler.rearm` on change). Grep each field's read sites when a "setting does nothing" bug appears.
- **Menu-bar icon "missing" was menu-bar crowding, not a bug.** A notarized, hardened-runtime, `/Applications`-launched build with `LSUIElement=true` ran fine (app + daemon alive) but the `MenuBarExtra` icon was hidden behind the notch / overflow. Quitting other menu-bar items revealed it. Don't chase Release-vs-Debug/signing ghosts before ruling out crowding.

## Resumable/parallel sync architecture (2026-07-13)

- **The apply loop is an ordered bounded-window parallel map, not a thread pool.** N afconvert transcode workers (`pipeline::OrderedTranscoder`, `crate::transcode_workers()`) run ahead of a single libgpod committer thread — libgpod is not thread-safe, so exactly one thread ever touches `OwnedDb`. Workers transcode filesystem-only jobs into a bounded window (`PIPELINE_WINDOW = 8`); the committer calls `take(seq)` in strict plan order so on-device track order matches the source plan regardless of which worker finishes first. Pause = graceful drain (stop accepting new actions, drain the in-flight window, commit what's already reached in plan order) + checkpoint (`db.write()` + manifest save) → `Paused` outcome, not a hard stop. Resume is **not** a new command — it's an ordinary diff-based `TriggerSync`/`--apply` run; the manifest already reflects exactly what got committed before the pause, so the next plan naturally picks up from there.
- **Any per-process-static temp path is a landmine once transcode goes parallel.** `transcode::project_temp_path` derived every temp filename from `PROJECT_DIR` + `std::process::id()` only — fine when transcoding was serial, but with up to 4 `OrderedTranscoder` workers calling `transcode_one` concurrently, every worker wrote to the SAME `classick-<pid>.m4a`/`.wav`/`.jpg`, corrupting audio/art and racing `remove_file` against a job still reading the file. Fixed with a process-global `AtomicU64` counter mixed into the filename (`classick-<infix>-<pid>-<seq>.<ext>`); every `project_temp_path` call is now guaranteed unique. Grep for `process::id()` without an accompanying per-call counter/uuid any time a serial pipeline gets parallelized — it's the same shape of bug every time.
- **A panic inside a `pipeline::OrderedTranscoder` worker used to wedge the whole sync forever, not just fail one track.** The worker loop called `transcode(&job)` unguarded; a panic unwound the worker thread with no entry inserted into `results.ready` for that `seq`, so the committer's `take(seq)` blocked on the condvar with nothing left to ever wake it — the daemon sat in `Syncing` indefinitely. Fixed by wrapping the call in `std::panic::catch_unwind(AssertUnwindSafe(...))` and mapping a caught panic to an ordinary `Err`, so `take` returns a deterministic failure and the track is skipped instead of hanging the sync. Any bounded-worker-pool-with-a-single-consumer pattern needs this guard, not just this one.
- **`IpcEvent::Summary.metadata_only` was wired as a permanent `0` and it stayed that way in three different places.** `ProgressEvent::Summary` (progress.rs) never carried the field, `ipc.rs`'s conversion hardcoded `metadata_only: 0` with a "post-M1 follow-up" comment that was never followed up, and the daemon's `summary_from_value` (sync_orchestrator.rs) didn't parse it either — so `library_count_cache = add + modify + unchanged` (runtime.rs) silently missed every metadata-only track, and "X of Y synced" could show X > Y after a tag-only sync. All three links had to move together: widen the internal event, stop hardcoding the wire conversion, and parse+sum it on the daemon side — patching only the daemon-side parsing (the smaller, more obvious fix) would have been a no-op because the wire always sent 0.
- iPod cover art needs a per-model `SysInfoExtended` at `iPod_Control/Device/`
  before libgpod opens the DB. Without it libgpod guesses the artwork ithmb
  format set and omits ones the firmware reads (e.g. `F1069` on Classic
  Late-2009) → valid thumbnails written but never displayed. classick provisions
  it (`ipod::sysinfo_provision`) from embedded CC0 templates.
- Rockbox reads track tags + cover art from the FILE, not the iTunesDB, and
  ignores embedded art in FLAC/Vorbis. classick's afconvert path produced bare
  .m4a (no tags/art). The `rockbox_compat` toggle embeds MP4 tags + a
  normalized (≤600px baseline JPEG) covr atom into transcoded output
  (`artwork.rs`), and the "Update existing library" backfill embeds them into
  already-synced files in place. One normalized image feeds both the covr atom
  (Rockbox) and libgpod's ithmb thumbnails (Apple). Keep hand-copied Rockbox
  files OUTSIDE iPod_Control/ — reconcile deletes non-DB files under
  iPod_Control/Music as orphans.
- **`itdb_write` DELETES cover-art thumbnails (F1069) when rewriting a parsed
  DB.** Verified on-device: `OwnedDb::open` + `db.write()` alone — no provision,
  no changes, not even re-setting thumbnails via
  `itdb_track_set_thumbnails_from_data` — deletes `F1069_1.ithmb` and blanks
  Apple-firmware cover art. libgpod's *rewrite/rearrange* path drops thumbnails
  loaded from disk as references; only its *fresh-build* path (no stale ithmb
  present) writes them correctly. To regenerate Apple artwork in-place (no track
  re-copy): re-thumbnail every track from source art, **delete the stale
  ithmb + ArtworkDB**, THEN `db.write()` — this forces the fresh-build path.
  See `apply_loop::rebuild_apple_artwork`. Corollary: any op that opens + writes
  the DB without re-thumbnailing every track drops existing art — suspect the
  normal incremental-sync / `do_metadata_only` path has the same latent bug.
- **macOS release (`scripts/release-macos.sh <version>`) appcast-URL gotcha.**
  The script signs + notarizes + `gh release create`s the dmg, then runs
  Sparkle's `generate_appcast`, which writes enclosure URLs pointing at
  `gh-pages` (`itsmichaelwest.github.io/...`). But dmgs are hosted on GitHub
  Releases, so before publishing, REWRITE the `dist/appcast.xml` enclosure URL
  to the Releases download path
  (`github.com/itsmichaelwest/classick/releases/download/vX/Classick-X.dmg`) and
  publish a single-entry `appcast.xml` to the `gh-pages` branch (that push is
  what rolls the Sparkle auto-update to everyone — GitHub's pre-release flag
  does NOT gate Sparkle). Version lives in BOTH `ui/macos/project.yml`
  (MARKETING_VERSION + CURRENT_PROJECT_VERSION) and `ui/macos/Info.plist`.
- **macOS side must never depend on ffmpeg — in runtime OR tests.** The library
  tag scan (`library_index::read_track_tags`) uses lofty (pure Rust), not
  ffmpeg. Test fixtures follow the same rule: use the committed
  `crates/classick/tests/fixtures/tagged.flac` (via
  `concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tagged.flac")`) instead
  of ffmpeg-synthesizing a FLAC, so the suite runs on a bare macOS box.
- **Unknown daemon `status_update.state` MUST decode to `.idle` on the UI
  side.** The Swift `DaemonEvent` decoder reads `state` as a raw String and maps
  unknown values to `.idle` (`StatusInfo.State(rawValue:) ?? .idle`), NOT via a
  hard `decode(State.self)`. A hard decode would throw on a newer daemon's new
  state, dropping the whole `status_update` and freezing the menu on stale
  state. This is the standing forward-compat rule for all future state values.
- **Adding `DaemonCommand`/`DaemonEvent` variants breaks exhaustive matches —
  land the wire types with interim arms so each commit compiles.** New Rust
  command variants break `daemon::runtime::handle_client_command`'s match; new
  Swift event cases break `AppModel.apply` and the `status_update.state` switch.
  Add a temporary catch-all/no-op arm in the wire-types commit, then replace it
  with real handling in the wiring commit. Same for the new `.scanning`/
  `Phase.scanning` cases (menuBarSystemImage, MenuContent.phaseContent).
- **Adding a Swift file needs `xcodegen generate` — `bundle.sh` does NOT run
  it.** `swift build`/`swift test` (SPM) auto-discover files under
  `Sources/`, so tests pass, but `ui/macos/bundle.sh` builds the committed
  `Classick.xcodeproj` via xcodebuild, which lists sources from the last
  `xcodegen generate`. Add a file → run `xcodegen generate` (from `ui/macos`) →
  commit the regenerated `project.pbxproj`, or the .app build fails with
  "cannot find <NewType> in scope" even though the SPM build is green.
