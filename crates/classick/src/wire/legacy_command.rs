use super::{validate_worker_command, WireCommand, WorkerCommandAdmission};
use crate::ipc::{IpcCommand, ReviewDecisionPayload};
use anyhow::{bail, Result};

pub fn translate_legacy_worker_command(
    command: &WireCommand,
    admission: &WorkerCommandAdmission,
) -> Result<IpcCommand> {
    validate_worker_command(command, admission)?;
    Ok(match command {
        WireCommand::ApplyReview { no_delete, .. } => IpcCommand::ReviewDecision {
            decision: ReviewDecisionPayload::Apply {
                no_delete: *no_delete,
            },
        },
        WireCommand::DryRunReview { .. } => IpcCommand::ReviewDecision {
            decision: ReviewDecisionPayload::DryRun,
        },
        WireCommand::QuitReview { .. } => IpcCommand::ReviewDecision {
            decision: ReviewDecisionPayload::Quit,
        },
        WireCommand::PromptDecision {
            prompt_id, choice, ..
        } => IpcCommand::PromptDecision {
            id: prompt_id.get(),
            choice: usize::try_from(*choice)?,
        },
        WireCommand::FormDecision {
            prompt_id, value, ..
        } => IpcCommand::FormDecision {
            id: prompt_id.get(),
            value: value.clone(),
        },
        WireCommand::CancelSync { .. } => IpcCommand::Cancel,
        WireCommand::PauseSync { .. } => IpcCommand::Pause,
        _ => bail!("non-session command cannot be translated for a legacy worker"),
    })
}
