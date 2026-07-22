use classick::device::DeviceId;
use classick::wire::{
    translate_legacy_worker_command, OwnedSessionRoute, PendingWorkerInteraction, PromptId,
    RequestId, SessionId, WireCommand, WorkerCommandAdmission,
};

#[test]
fn every_v3_worker_command_translates_to_the_legacy_subprocess_shape() {
    let cases = [
        (
            WireCommand::ApplyReview {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
                no_delete: true,
            },
            PendingWorkerInteraction::Review,
            r#"{"type":"review_decision","decision":{"type":"apply","no_delete":true}}"#,
        ),
        (
            WireCommand::DryRunReview {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
            },
            PendingWorkerInteraction::Review,
            r#"{"type":"review_decision","decision":{"type":"dry_run"}}"#,
        ),
        (
            WireCommand::QuitReview {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
            },
            PendingWorkerInteraction::Review,
            r#"{"type":"review_decision","decision":{"type":"quit"}}"#,
        ),
        (
            WireCommand::PromptDecision {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
                prompt_id: PromptId::new(7).unwrap(),
                choice: 2,
            },
            PendingWorkerInteraction::Prompt {
                prompt_id: PromptId::new(7).unwrap(),
                option_count: 3,
            },
            r#"{"type":"prompt_decision","id":7,"choice":2}"#,
        ),
        (
            WireCommand::FormDecision {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
                prompt_id: PromptId::new(8).unwrap(),
                value: None,
            },
            PendingWorkerInteraction::Form {
                prompt_id: PromptId::new(8).unwrap(),
            },
            r#"{"type":"form_decision","id":8,"value":null}"#,
        ),
        (
            WireCommand::FormDecision {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
                prompt_id: PromptId::new(8).unwrap(),
                value: Some("Michael West's iPod".to_string()),
            },
            PendingWorkerInteraction::Form {
                prompt_id: PromptId::new(8).unwrap(),
            },
            r#"{"type":"form_decision","id":8,"value":"Michael West's iPod"}"#,
        ),
        (
            WireCommand::CancelSync {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
            },
            PendingWorkerInteraction::None,
            r#"{"type":"cancel"}"#,
        ),
        (
            WireCommand::PauseSync {
                device_id: device_id(),
                session_id: session_id(),
                request_id: request_id(),
            },
            PendingWorkerInteraction::None,
            r#"{"type":"pause"}"#,
        ),
    ];

    for (command, pending, expected) in cases {
        let admission = WorkerCommandAdmission::new(route(), pending);
        let translated = translate_legacy_worker_command(&command, &admission).unwrap();
        assert_eq!(serde_json::to_string(&translated).unwrap(), expected);
    }
}

#[test]
fn translation_rejects_wrong_routes_and_pending_interactions() {
    let command = WireCommand::PromptDecision {
        device_id: device_id(),
        session_id: session_id(),
        request_id: request_id(),
        prompt_id: PromptId::new(7).unwrap(),
        choice: 2,
    };
    let wrong_route = WorkerCommandAdmission::new(
        OwnedSessionRoute::new(DeviceId::parse("000A27002138B0A9").unwrap(), session_id()),
        PendingWorkerInteraction::Prompt {
            prompt_id: PromptId::new(7).unwrap(),
            option_count: 3,
        },
    );
    assert!(translate_legacy_worker_command(&command, &wrong_route).is_err());

    let wrong_prompt = WorkerCommandAdmission::new(
        route(),
        PendingWorkerInteraction::Prompt {
            prompt_id: PromptId::new(8).unwrap(),
            option_count: 3,
        },
    );
    assert!(translate_legacy_worker_command(&command, &wrong_prompt).is_err());

    let out_of_range = WorkerCommandAdmission::new(
        route(),
        PendingWorkerInteraction::Prompt {
            prompt_id: PromptId::new(7).unwrap(),
            option_count: 2,
        },
    );
    assert!(translate_legacy_worker_command(&command, &out_of_range).is_err());
}

#[test]
fn global_commands_never_reach_a_worker() {
    let command = WireCommand::GetGlobalConfig {
        request_id: request_id(),
    };
    assert!(translate_legacy_worker_command(
        &command,
        &WorkerCommandAdmission::new(route(), PendingWorkerInteraction::None)
    )
    .is_err());
}

fn route() -> OwnedSessionRoute {
    OwnedSessionRoute::new(device_id(), session_id())
}

fn device_id() -> DeviceId {
    DeviceId::parse("000A27002138B0A8").unwrap()
}

fn session_id() -> SessionId {
    SessionId::new(42).unwrap()
}

fn request_id() -> RequestId {
    RequestId::parse("018f9d7e-2f2b-7b52-9f1d-f78bdb2f8801").unwrap()
}
