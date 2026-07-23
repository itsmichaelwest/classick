using Classick_UI.Core;
using Classick_UI.Ipc;

namespace Classick_UI.Notifications;

public enum ToastKind { Started, Complete, Error }

public sealed record ToastDecision(
    ToastKind Kind,
    DeviceId DeviceId,
    ulong SessionId,
    string Title,
    string Body);

public sealed class NotificationDecisionTracker
{
    private readonly HashSet<(DeviceId DeviceId, ulong SessionId, ToastKind Kind)> _shown = [];

    public ToastDecision? Reduce(
        WireEvent wireEvent,
        Func<DeviceId, string> resolveDeviceName,
        string notifyOn)
    {
        if (wireEvent is SyncErrorEvent)
        {
            return null;
        }

        ToastDecision? decision = wireEvent switch
        {
            SyncAcceptedEvent accepted when notifyOn is not ("none" or "errors_only") =>
                Decision(
                    ToastKind.Started,
                    accepted.DeviceId,
                    accepted.SessionId,
                    AppIdentity.Name,
                    $"Syncing {SafeName(resolveDeviceName, accepted.DeviceId)}…"),
            SyncFinishedEvent finished when !finished.Success && notifyOn != "none" =>
                Decision(
                    ToastKind.Error,
                    finished.DeviceId,
                    finished.SessionId,
                    $"{AppIdentity.Name} — sync failed",
                    FailureBody(finished, resolveDeviceName)),
            SyncFinishedEvent finished when notifyOn is not ("none" or "errors_only") =>
                Decision(
                    ToastKind.Complete,
                    finished.DeviceId,
                    finished.SessionId,
                    AppIdentity.Name,
                    CompletionBody(finished, resolveDeviceName)),
            _ => null,
        };

        return decision;
    }

    private ToastDecision? Decision(
        ToastKind kind,
        DeviceId deviceId,
        ulong sessionId,
        string title,
        string body) =>
        _shown.Add((deviceId, sessionId, kind))
            ? new ToastDecision(kind, deviceId, sessionId, title, body)
            : null;

    private string FailureBody(SyncFinishedEvent finished, Func<DeviceId, string> resolveName)
    {
        var name = SafeName(resolveName, finished.DeviceId);
        return $"{name} could not be synced. Open Classick for details.";
    }

    private static string CompletionBody(
        SyncFinishedEvent finished,
        Func<DeviceId, string> resolveName)
    {
        var name = SafeName(resolveName, finished.DeviceId);
        return finished.SkippedForSpace is { Tracks: > 0 } skipped
            ? $"{name} sync complete. {skipped.Tracks} tracks skipped for space."
            : $"{name} sync complete.";
    }

    private static string SafeName(Func<DeviceId, string> resolveName, DeviceId deviceId)
    {
        var name = resolveName(deviceId);
        return string.IsNullOrWhiteSpace(name) || name == deviceId.Value ? "iPod" : name;
    }
}
