using System.Text.Json.Serialization;

namespace Classick_UI.Ipc;

// TODO(windows): pause/resume + X-of-Y not yet wired on Windows.
// Subprocess protocol bumped to 1.1.0 with a new `pause` command
// ({"type":"pause"}, no payload) and a new terminal `paused` event
// ({"type":"paused"}) — see docs/ipc-protocol.md §5.6 / §4.12. This
// IpcCommand hierarchy has no PauseCommand yet, and IpcEvent (see
// IpcEvent.cs) has no Paused case. Mirror the Rust core
// (crates/classick/src/ipc.rs) and the macOS client
// (ui/macos/Sources/Classick/Ipc/WireModels.swift, `DaemonCommand.pause` /
// `SyncEvent.paused`) plus a Pause/Resume affordance in the tray UI. Can't
// build/verify Windows in this environment — see docs/ipc-protocol.md.

/// <summary>
/// Base type for commands sent by the UI on the core's stdin in --ipc-mode.
/// Wire format: newline-delimited JSON, snake_case "type" discriminator.
/// See docs/ipc-protocol.md §5 for the authoritative schema.
/// </summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(StartCommand), "start")]
[JsonDerivedType(typeof(ReviewDecisionCommand), "review_decision")]
[JsonDerivedType(typeof(PromptDecisionCommand), "prompt_decision")]
[JsonDerivedType(typeof(FormDecisionCommand), "form_decision")]
[JsonDerivedType(typeof(CancelCommand), "cancel")]
public abstract record IpcCommand;

/// <summary>
/// Reserved for future milestones; M1 cores begin orchestration implicitly on
/// spawn and silently ignore this command. See §5 header note.
/// </summary>
public sealed record StartCommand : IpcCommand;

/// <summary>Reply to a <see cref="ReviewEvent"/>. See §5.2.</summary>
public sealed record ReviewDecisionCommand(
    [property: JsonPropertyName("decision")] ReviewDecisionPayload Decision
) : IpcCommand;

/// <summary>
/// Nested typed-envelope inside <see cref="ReviewDecisionCommand.Decision"/>.
/// Mirrors the Rust <c>ReviewDecision</c> enum. See §5.1.
/// </summary>
[JsonPolymorphic(TypeDiscriminatorPropertyName = "type")]
[JsonDerivedType(typeof(ApplyDecision), "apply")]
[JsonDerivedType(typeof(DryRunDecision), "dry_run")]
[JsonDerivedType(typeof(QuitDecision), "quit")]
public abstract record ReviewDecisionPayload;

/// <summary>Apply the plan; <paramref name="NoDelete"/> mirrors the --no-delete toggle.</summary>
public sealed record ApplyDecision(
    [property: JsonPropertyName("no_delete")] bool NoDelete
) : ReviewDecisionPayload;

/// <summary>Run as dry-run (no destructive changes).</summary>
public sealed record DryRunDecision : ReviewDecisionPayload;

/// <summary>User aborted the review; orchestrator unwinds without writing the manifest.</summary>
public sealed record QuitDecision : ReviewDecisionPayload;

/// <summary>Reply to a <see cref="PromptEvent"/>. See §5.3.</summary>
public sealed record PromptDecisionCommand(
    [property: JsonPropertyName("id")] ulong Id,
    [property: JsonPropertyName("choice")] int Choice
) : IpcCommand;

/// <summary>Reply to a <see cref="FormEvent"/>. See §5.4. <paramref name="Value"/> null = user abort.</summary>
public sealed record FormDecisionCommand(
    [property: JsonPropertyName("id")] ulong Id,
    [property: JsonPropertyName("value")] string? Value
) : IpcCommand;

/// <summary>Graceful shutdown request. See §5.5.</summary>
public sealed record CancelCommand : IpcCommand;
