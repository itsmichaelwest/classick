//! IPC wire types for `--ipc-mode`. See `docs/ipc-protocol.md`.
//!
//! These are serde-serializable mirrors of internal `ProgressEvent` and
//! `Decision` enums. Conversion happens at the channel boundary in the
//! IpcBackend (`run_ipc` in `progress.rs`), NOT in the orchestrator — keeping
//! the wire format independent of internal refactors so we can version it
//! separately.

use serde::{Deserialize, Serialize};

/// Current wire-protocol semver. Bumped per the rules in
/// `docs/ipc-protocol.md` §1.
pub const PROTOCOL_VERSION: &str = "1.4.0";

/// Events emitted from the core to the UI on stdout.
///
/// Serialized as newline-delimited JSON with a `type` discriminator using
/// `snake_case`. Field names are also `snake_case`. See `docs/ipc-protocol.md` §4.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcEvent {
    /// Handshake. MUST be the first event after spawn. See §4.1.
    Hello {
        protocol_version: String, // PROTOCOL_VERSION
        core_version: String,     // env!("CARGO_PKG_VERSION")
    },
    /// Resolved paths. Mirrors `ProgressEvent::Header`. See §4.2.
    Header {
        source: String,
        ipod: String,
        manifest: String,
    },
    /// Action-plan counts. Mirrors `ProgressEvent::Summary`. See §4.3.
    Summary {
        add: usize,
        modify: usize,
        metadata_only: usize,
        remove: usize,
        unchanged: usize,
        total_planned: usize,
    },
    /// Action-plan review request. Reply with `review_decision`. See §4.4.
    Review {
        summary: IpcActionPlanSummary,
        no_delete: bool,
    },
    /// Modal multi-choice prompt. Reply with `prompt_decision`. See §4.5.
    Prompt {
        id: u64,
        message: String,
        options: Vec<String>,
    },
    /// Modal text-input prompt. Reply with `form_decision`. See §4.6.
    Form {
        id: u64,
        label: String,
        initial: String,
        hint: String,
    },
    /// Per-track start. See §4.7.
    TrackStart {
        current: usize,
        total: usize,
        label: String,
        /// Estimated seconds remaining (whole-run average). Omitted before the
        /// first track completes. Added in protocol 1.2.0.
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_secs: Option<u64>,
    },
    /// Per-track done. No fields. See §4.8.
    TrackDone {
        result: crate::progress::TrackResult,
    },
    Finalizing {
        reason: crate::progress::StopReason,
        staged_albums: usize,
        staged_tracks: usize,
    },
    Cancelled,
    /// Informational log line. See §4.9.
    Log {
        message: String,
    },
    /// Non-fatal or fatal error. `recovery_hints` is currently always empty
    /// in M1; it's reserved for future use and omitted on the wire when
    /// empty. See §4.10.
    Error {
        message: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        recovery_hints: Vec<String>,
    },
    /// Terminal event. The core closes stdout shortly after. See §4.11.
    Finish {
        success: bool,
        /// Fit-pass (Task 8) deferral rollup: whole albums that didn't fit
        /// the device's remaining space even after the end-of-run retry.
        /// Absent when nothing was deferred (all-fit run, or a core older
        /// than 1.3.0).
        #[serde(skip_serializing_if = "Option::is_none")]
        skipped_for_space: Option<SkippedForSpace>,
        /// Artwork embed/refresh rollup for this run's Add/Modify/MetadataOnly
        /// actions (Task 13). Absent when the run never reached the apply
        /// loop (dry-run, "nothing to do" with no pending artwork repair, or
        /// an early abort), or when talking to a core older than this field.
        #[serde(skip_serializing_if = "Option::is_none")]
        artwork: Option<ArtworkSummary>,
        /// True when Task 4's auto-restore-from-backup path fired this run
        /// (the iTunesDB failed to parse and was replaced from the session
        /// backup before the sync proceeded). Omitted (not `false`) on the
        /// wire when it didn't fire, for old-client compat.
        #[serde(skip_serializing_if = "std::ops::Not::not", default)]
        db_restored: bool,
    },
    /// Terminal event: the sync was gracefully paused (Task 6). Completed
    /// tracks were committed to the iTunesDB + manifest before this was
    /// emitted; the core exits shortly after, same as `finish`. No fields.
    Paused,
}

/// Action-plan summary carried inside `IpcEvent::Review`. See §4.4.
#[derive(Debug, Serialize)]
pub struct IpcActionPlanSummary {
    pub add: usize,
    pub modify: usize,
    pub metadata_only: usize,
    pub remove: usize,
    pub unchanged: usize,
}

/// Fit-pass (Task 8) deferral rollup attached to `finish`. Whole-album
/// granularity mirrors `fit::DeferredAlbum` — `albums`/`tracks`/`bytes` are
/// sums across every album that still didn't fit after the end-of-run retry.
/// See `docs/ipc-protocol.md` §4.11.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedForSpace {
    pub albums: usize,
    pub tracks: usize,
    pub bytes: u64,
}

impl SkippedForSpace {
    /// Human-readable line for TUI/plain-mode output (`progress::run_plain`,
    /// `apply_event`, and the apply loop's `--dry-run` plan summary). IPC-mode
    /// UIs render their own copy from the structured fields instead.
    pub fn describe(&self) -> String {
        format!(
            "{} album{} ({}) didn't fit — narrow your selection or free space",
            self.albums,
            if self.albums == 1 { "" } else { "s" },
            format_bytes_human(self.bytes),
        )
    }
}

/// `1.2 GB` / `340 MB` — coarse, human-scale formatting; not used for
/// anything precision-sensitive. `pub(crate)` so `apply_loop`'s Task-8
/// bytes-written log line can reuse it.
pub(crate) fn format_bytes_human(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

/// Artwork embed/refresh rollup attached to `finish` (Task 13). Counted
/// across a run's Add/Modify/MetadataOnly actions — see
/// `apply_loop::ArtworkCounts`. See `docs/ipc-protocol.md` §4.11.
#[derive(Debug, Clone, Serialize)]
pub struct ArtworkSummary {
    pub embedded: usize,
    pub eligible: usize,
    pub failed_sources: usize,
}

/// Commands the UI sends to the core over stdin.
///
/// Serialized as newline-delimited JSON with a `type` discriminator using
/// `snake_case`. See `docs/ipc-protocol.md` §5.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcCommand {
    /// Reserved for M2+ (handshake / explicit start). M1 ignores it.
    Start,
    /// Reply to a `review` event. Carries the choice as a nested typed
    /// envelope; see §5.1.
    ReviewDecision { decision: ReviewDecisionPayload },
    /// Reply to a `prompt` event. `id` MUST echo the originating prompt's id.
    PromptDecision { id: u64, choice: usize },
    /// Reply to a `form` event. `value: None` means user aborted.
    FormDecision { id: u64, value: Option<String> },
    /// Best-effort graceful shutdown. M1 maps it to `Quit` at the next
    /// review-decision recv() point; the UI must back this with a 5s
    /// force-kill timer (see §5.5 and §7).
    Cancel,
    /// Graceful pause: finish in-flight/completed tracks, checkpoint, then
    /// stop. Unlike `cancel`, this is not a shutdown request — the core
    /// exits after emitting `paused`, and a later run resumes from the
    /// manifest. Mapped to `Decision::Pause` at the next action-loop poll.
    Pause,
}

/// Nested `decision` object inside `review_decision`. See §5.1.
///
/// The wire shape uses the same typed-envelope pattern as the top-level
/// `IpcCommand`, but scoped to this field. The Rust `ReviewDecision` enum
/// in `progress.rs` is the source of truth for variants.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewDecisionPayload {
    Apply { no_delete: bool },
    DryRun,
    Quit,
}

impl IpcEvent {
    /// Convert an internal `ProgressEvent` to a wire `IpcEvent`. Returns
    /// `None` for events that don't translate (none in M1, but an extension
    /// point for the future).
    pub fn from_progress(event: &crate::progress::ProgressEvent) -> Option<Self> {
        use crate::progress::ProgressEvent as PE;
        Some(match event {
            PE::Header {
                source,
                ipod,
                manifest,
            } => IpcEvent::Header {
                source: source.clone(),
                ipod: ipod.clone(),
                manifest: manifest.clone(),
            },
            PE::Summary {
                add,
                modify,
                metadata_only,
                remove,
                unchanged,
                total_planned,
            } => IpcEvent::Summary {
                add: *add,
                modify: *modify,
                metadata_only: *metadata_only,
                remove: *remove,
                unchanged: *unchanged,
                total_planned: *total_planned,
            },
            PE::Review { summary, no_delete } => IpcEvent::Review {
                summary: IpcActionPlanSummary {
                    add: summary.add,
                    modify: summary.modify,
                    metadata_only: summary.metadata_only,
                    remove: summary.remove,
                    unchanged: summary.unchanged,
                },
                no_delete: *no_delete,
            },
            PE::Prompt(req) => IpcEvent::Prompt {
                id: req.id,
                message: req.message.clone(),
                options: req.options.clone(),
            },
            PE::Form(req) => IpcEvent::Form {
                id: req.id,
                label: req.label.clone(),
                initial: req.initial.clone(),
                hint: req.hint.clone(),
            },
            PE::TrackStart {
                current,
                total,
                label,
            } => IpcEvent::TrackStart {
                current: *current,
                total: *total,
                label: label.clone(),
                eta_secs: None,
            },
            PE::TrackDone(result) => IpcEvent::TrackDone { result: *result },
            PE::Finalizing {
                reason,
                staged_albums,
                staged_tracks,
            } => IpcEvent::Finalizing {
                reason: *reason,
                staged_albums: *staged_albums,
                staged_tracks: *staged_tracks,
            },
            PE::Cancelled => IpcEvent::Cancelled,
            PE::Log(m) => IpcEvent::Log { message: m.clone() },
            PE::Error(m) => IpcEvent::Error {
                message: m.clone(),
                recovery_hints: Vec::new(),
            },
            // ProgressEvent::Finish now carries the orchestrator's outcome
            // (Ok → true, Err → false). main.rs is responsible for passing
            // the right value to Progress::finish(); the IPC backend just
            // mirrors it. A fatal failure should emit an `error` event first
            // (so the UI gets the message) and then this `finish` with
            // success: false; the process exits non-zero shortly after.
            //
            // skipped_for_space/artwork/db_restored are populated on
            // `Progress` via `note_*` calls during the run (Task 8's fit
            // pass, Task 4's auto-restore) and read back here unchanged.
            PE::Finish {
                success,
                skipped_for_space,
                artwork,
                db_restored,
            } => IpcEvent::Finish {
                success: *success,
                skipped_for_space: skipped_for_space.clone(),
                artwork: artwork.clone(),
                db_restored: *db_restored,
            },
            PE::Paused => IpcEvent::Paused,
        })
    }
}

impl IpcCommand {
    /// Convert a wire `IpcCommand` to an internal `Decision`. Returns `None`
    /// for commands that don't yield a `Decision` (currently just `Start`,
    /// which is reserved for M2+).
    pub fn to_decision(&self) -> Option<crate::progress::Decision> {
        use crate::progress::{Decision, ReviewDecision};
        Some(match self {
            IpcCommand::Start => return None,
            IpcCommand::ReviewDecision { decision } => match decision {
                ReviewDecisionPayload::Apply { no_delete } => {
                    Decision::Review(ReviewDecision::Apply {
                        no_delete: *no_delete,
                    })
                }
                ReviewDecisionPayload::DryRun => Decision::Review(ReviewDecision::DryRun),
                ReviewDecisionPayload::Quit => Decision::Review(ReviewDecision::Quit),
            },
            IpcCommand::PromptDecision { id, choice } => Decision::Prompt {
                id: *id,
                choice: *choice,
            },
            IpcCommand::FormDecision { id, value } => Decision::Form {
                id: *id,
                value: value.clone(),
            },
            // Mid-sync cancel is best-effort in M1: the orchestrator only
            // observes it at the next decision point. Mapping to Quit means
            // the Review-await path tears down cleanly; if no Review is in
            // flight, the orchestrator's own EOF/closed-channel handling
            // takes over. The UI is expected to back this with a 5s
            // force-kill timer (§5.5).
            IpcCommand::Cancel => Decision::Review(ReviewDecision::Quit),
            IpcCommand::Pause => Decision::Pause,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_event_serializes_correctly() {
        let event = IpcEvent::Hello {
            protocol_version: "1.0.0".to_string(),
            core_version: "0.0.1".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"hello""#), "got: {json}");
        assert!(
            json.contains(r#""protocol_version":"1.0.0""#),
            "got: {json}"
        );
        assert!(json.contains(r#""core_version":"0.0.1""#), "got: {json}");
    }

    #[test]
    fn summary_event_carries_metadata_only_through() {
        // Regression test: IpcEvent::Summary used to hardcode metadata_only
        // to 0 regardless of the internal ProgressEvent's value (a documented
        // "post-M1 follow-up" that was never done), so the daemon's
        // library_count cache (which reads this event's metadata_only) always
        // undercounted metadata-only tracks. from_progress must now carry the
        // real value through unchanged.
        use crate::progress::ProgressEvent;
        let event = ProgressEvent::Summary {
            add: 12,
            modify: 3,
            metadata_only: 7,
            remove: 0,
            unchanged: 1260,
            total_planned: 15,
        };
        let wire = IpcEvent::from_progress(&event).unwrap();
        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains(r#""metadata_only":7"#), "got: {json}");
    }

    #[test]
    fn review_decision_nested_envelope_round_trips() {
        // This is the critical nuance from docs/ipc-protocol.md §5.1: the
        // `decision` field is a nested typed envelope, NOT a flat shape.
        let cmd_json = r#"{"type":"review_decision","decision":{"type":"apply","no_delete":true}}"#;
        let cmd: IpcCommand = serde_json::from_str(cmd_json).unwrap();
        let decision = cmd.to_decision().unwrap();
        use crate::progress::{Decision, ReviewDecision};
        match decision {
            Decision::Review(ReviewDecision::Apply { no_delete }) => assert!(no_delete),
            other => panic!("expected Apply with no_delete=true, got {other:?}"),
        }
    }

    #[test]
    fn pause_command_maps_to_pause_decision() {
        let cmd: IpcCommand = serde_json::from_str(r#"{"type":"pause"}"#).unwrap();
        assert!(matches!(
            cmd.to_decision(),
            Some(crate::progress::Decision::Pause)
        ));
    }

    #[test]
    fn cancel_maps_to_quit() {
        let cmd: IpcCommand = serde_json::from_str(r#"{"type":"cancel"}"#).unwrap();
        let decision = cmd.to_decision().unwrap();
        use crate::progress::{Decision, ReviewDecision};
        assert!(matches!(decision, Decision::Review(ReviewDecision::Quit)));
    }

    #[test]
    fn track_done_event_serializes_with_result() {
        let event = IpcEvent::TrackDone {
            result: crate::progress::TrackResult::Applied,
        };
        let json = serde_json::to_string(&event).unwrap();
        // Internally tagged enum with no fields serializes as just {"type": "..."}.
        assert!(json.contains(r#""type":"track_done""#), "got: {json}");
        assert!(json.contains(r#""result":"applied""#), "got: {json}");
    }

    #[test]
    fn track_start_event_omits_eta_secs_when_none() {
        // Compatibility guarantee (protocol 1.2.0 is additive): a `None` ETA
        // must serialize identically to pre-1.2.0 — the `eta_secs` field is
        // omitted entirely, not emitted as `null`. `skip_serializing_if`
        // enforces this.
        let event = IpcEvent::TrackStart {
            current: 1,
            total: 15,
            label: "Aphex Twin - #ATC1".to_string(),
            eta_secs: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("eta_secs"), "got: {json}");
        assert!(json.contains(r#""current":1"#), "got: {json}");
        assert!(json.contains(r#""total":15"#), "got: {json}");
        assert!(
            json.contains(r#""label":"Aphex Twin - #ATC1""#),
            "got: {json}"
        );
    }

    #[test]
    fn track_start_event_includes_eta_secs_when_some() {
        let event = IpcEvent::TrackStart {
            current: 6,
            total: 15,
            label: "Aphex Twin - #ATC2".to_string(),
            eta_secs: Some(42),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""eta_secs":42"#), "got: {json}");
    }

    #[test]
    fn protocol_version_is_1_4_0() {
        assert_eq!(PROTOCOL_VERSION, "1.4.0");
    }

    #[test]
    fn finalizing_cancelled_and_finish_serialize_in_order() {
        use crate::progress::{ProgressEvent, StopReason};

        let events = [
            ProgressEvent::Finalizing {
                reason: StopReason::Cancelled,
                staged_albums: 2,
                staged_tracks: 17,
            },
            ProgressEvent::Cancelled,
            ProgressEvent::Finish {
                success: true,
                skipped_for_space: None,
                artwork: None,
                db_restored: false,
            },
        ];
        let lines = events
            .iter()
            .map(|event| serde_json::to_string(&IpcEvent::from_progress(event).unwrap()).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                r#"{"type":"finalizing","reason":"cancelled","staged_albums":2,"staged_tracks":17}"#,
                r#"{"type":"cancelled"}"#,
                r#"{"type":"finish","success":true}"#,
            ]
        );
    }

    // -- Task 8: Finish gains skipped_for_space / artwork / db_restored ----

    #[test]
    fn finish_event_omits_new_fields_when_absent() {
        // Old-client compat: a Finish with nothing new to report must
        // serialize byte-identical to pre-1.3.0 (no null placeholders).
        let event = IpcEvent::Finish {
            success: true,
            skipped_for_space: None,
            artwork: None,
            db_restored: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""success":true"#), "got: {json}");
        assert!(!json.contains("skipped_for_space"), "got: {json}");
        assert!(!json.contains("artwork"), "got: {json}");
        assert!(!json.contains("db_restored"), "got: {json}");
    }

    #[test]
    fn finish_event_includes_new_fields_when_present() {
        let event = IpcEvent::Finish {
            success: true,
            skipped_for_space: Some(SkippedForSpace {
                albums: 3,
                tracks: 40,
                bytes: 1_234_567,
            }),
            artwork: Some(ArtworkSummary {
                embedded: 10,
                eligible: 12,
                failed_sources: 1,
            }),
            db_restored: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""albums":3"#), "got: {json}");
        assert!(json.contains(r#""tracks":40"#), "got: {json}");
        assert!(json.contains(r#""bytes":1234567"#), "got: {json}");
        assert!(json.contains(r#""embedded":10"#), "got: {json}");
        assert!(json.contains(r#""eligible":12"#), "got: {json}");
        assert!(json.contains(r#""failed_sources":1"#), "got: {json}");
        assert!(json.contains(r#""db_restored":true"#), "got: {json}");
    }

    #[test]
    fn finish_event_db_restored_alone_omits_the_option_fields() {
        // A restore with nothing deferred must not spuriously emit
        // skipped_for_space/artwork just because db_restored is true.
        let event = IpcEvent::Finish {
            success: true,
            skipped_for_space: None,
            artwork: None,
            db_restored: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""db_restored":true"#), "got: {json}");
        assert!(!json.contains("skipped_for_space"), "got: {json}");
        assert!(!json.contains("artwork"), "got: {json}");
    }

    #[test]
    fn from_progress_carries_finish_details_through() {
        use crate::progress::ProgressEvent;
        let event = ProgressEvent::Finish {
            success: false,
            skipped_for_space: Some(SkippedForSpace {
                albums: 1,
                tracks: 2,
                bytes: 3,
            }),
            artwork: None,
            db_restored: true,
        };
        let wire = IpcEvent::from_progress(&event).unwrap();
        match wire {
            IpcEvent::Finish {
                success,
                skipped_for_space,
                artwork,
                db_restored,
            } => {
                assert!(!success);
                assert_eq!(skipped_for_space.unwrap().albums, 1);
                assert!(artwork.is_none());
                assert!(db_restored);
            }
            other => panic!("expected Finish, got {other:?}"),
        }
    }

    #[test]
    fn skipped_for_space_describe_reads_naturally() {
        let s = SkippedForSpace {
            albums: 1,
            tracks: 8,
            bytes: 1_200_000_000,
        };
        let text = s.describe();
        assert!(text.starts_with("1 album ("), "got: {text}");
        assert!(text.contains("didn't fit"), "got: {text}");

        let plural = SkippedForSpace {
            albums: 3,
            tracks: 8,
            bytes: 1_200_000_000,
        };
        assert!(
            plural.describe().starts_with("3 albums ("),
            "got: {}",
            plural.describe()
        );
    }
}
