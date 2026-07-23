using Classick_UI.Ipc;

namespace Classick_UI.Devices;

public enum DeviceSettingsSaveState
{
    Saved,
    WaitingForDevice,
    Editing,
    Failed,
}

public sealed class DeviceSettingsDraft
{
    internal DeviceSettingsDraft(DeliveredComponent<SettingsValue> canonical) => ApplyCanonical(canonical);

    public SettingsValue Value { get; internal set; } = new(1, false, false);
    public SettingsValue CanonicalValue { get; internal set; } = new(1, false, false);
    public string? PendingRequestId { get; internal set; }
    public string? PendingMutationId { get; internal set; }
    public string? AcceptedMutationId { get; internal set; }
    public string? Error { get; internal set; }
    public DeviceSettingsSaveState SaveState { get; internal set; }
    public ulong Revision { get; internal set; }

    internal void ApplyCanonical(DeliveredComponent<SettingsValue> canonical)
    {
        if (canonical.Revision < Revision) return;
        Revision = canonical.Revision;
        CanonicalValue = canonical.Value;
        AcceptedMutationId = canonical.MutationId;
        if ((PendingMutationId is null && SaveState != DeviceSettingsSaveState.Failed) ||
            PendingMutationId == canonical.MutationId)
        {
            Value = canonical.Value;
            PendingRequestId = null;
            PendingMutationId = null;
            Error = canonical.Delivery is PendingDeviceDelivery { LastFailure: { } failure } ? failure : null;
            SaveState = canonical.Delivery is PendingDeviceDelivery
                ? DeviceSettingsSaveState.WaitingForDevice
                : DeviceSettingsSaveState.Saved;
        }
    }
}

public sealed class DeviceSettingsDraftStore
{
    private readonly Dictionary<DeviceId, DeviceSettingsDraft> _drafts = [];

    public IReadOnlyDictionary<DeviceId, DeviceSettingsDraft> Drafts => _drafts;

    public DeviceSettingsDraft ApplyCanonical(DeviceConfigEvent config)
    {
        if (!_drafts.TryGetValue(config.DeviceId, out var draft))
        {
            draft = new DeviceSettingsDraft(config.Settings);
            _drafts.Add(config.DeviceId, draft);
        }
        else
        {
            draft.ApplyCanonical(config.Settings);
        }
        return draft;
    }

    public SetSettingsCommand Edit(
        DeviceId deviceId,
        SettingsValue value,
        string requestId,
        string mutationId)
    {
        if (!_drafts.TryGetValue(deviceId, out var draft))
        {
            throw new InvalidOperationException("Device settings are not loaded");
        }
        draft.Value = value;
        draft.PendingRequestId = requestId;
        draft.PendingMutationId = mutationId;
        draft.Error = null;
        draft.SaveState = DeviceSettingsSaveState.Editing;
        return new SetSettingsCommand(deviceId, requestId, mutationId, value);
    }

    public bool ApplyFailure(ConfigMutationFailedEvent failure)
    {
        if (failure.Component != ConfigComponent.Settings ||
            !_drafts.TryGetValue(failure.DeviceId, out var draft) ||
            !((draft.PendingRequestId == failure.RequestId && draft.PendingMutationId == failure.MutationId) ||
              (failure.Stage == ConfigFailureStage.DeviceDelivery && draft.AcceptedMutationId == failure.MutationId)))
        {
            return false;
        }
        draft.Error = failure.Message;
        if (failure.Stage == ConfigFailureStage.HostAcceptance)
        {
            draft.PendingRequestId = null;
            draft.PendingMutationId = null;
            draft.SaveState = DeviceSettingsSaveState.Failed;
        }
        else
        {
            draft.SaveState = DeviceSettingsSaveState.WaitingForDevice;
        }
        return true;
    }

    public void MarkTransportFailure(DeviceId deviceId, string message)
    {
        if (!_drafts.TryGetValue(deviceId, out var draft)) return;
        draft.PendingRequestId = null;
        draft.PendingMutationId = null;
        draft.Error = message;
        draft.SaveState = DeviceSettingsSaveState.Failed;
    }

    public void Remove(DeviceId deviceId) => _drafts.Remove(deviceId);
}
