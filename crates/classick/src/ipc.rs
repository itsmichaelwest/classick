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
pub const PROTOCOL_VERSION: &str = "1.2.0";

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
    TrackDone,
    /// Informational log line. See §4.9.
    Log { message: String },
    /// Non-fatal or fatal error. `recovery_hints` is currently always empty
    /// in M1; it's reserved for future use and omitted on the wire when
    /// empty. See §4.10.
    Error {
        message: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        recovery_hints: Vec<String>,
    },
    /// Terminal event. The core closes stdout shortly after. See §4.11.
    Finish { success: bool },
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
            PE::TrackDone => IpcEvent::TrackDone,
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
            PE::Finish { success } => IpcEvent::Finish { success: *success },
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
        assert!(matches!(cmd.to_decision(), Some(crate::progress::Decision::Pause)));
    }

    #[test]
    fn cancel_maps_to_quit() {
        let cmd: IpcCommand = serde_json::from_str(r#"{"type":"cancel"}"#).unwrap();
        let decision = cmd.to_decision().unwrap();
        use crate::progress::{Decision, ReviewDecision};
        assert!(matches!(
            decision,
            Decision::Review(ReviewDecision::Quit)
        ));
    }

    #[test]
    fn track_done_event_serializes_with_no_fields() {
        let event = IpcEvent::TrackDone;
        let json = serde_json::to_string(&event).unwrap();
        // Internally tagged enum with no fields serializes as just {"type": "..."}.
        assert!(json.contains(r#""type":"track_done""#), "got: {json}");
    }
}
