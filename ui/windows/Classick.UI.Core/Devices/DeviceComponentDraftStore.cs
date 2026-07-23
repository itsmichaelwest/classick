using Classick_UI.Ipc;

namespace Classick_UI.Devices;

public sealed class DeviceComponentDraft<T>
{
    internal DeviceComponentDraft(DeliveredComponent<T> component) => Apply(component);
    public T Value { get; internal set; } = default!;
    public ulong Revision { get; internal set; }
    public string? PendingRequestId { get; internal set; }
    public string? PendingMutationId { get; internal set; }
    public string? AcceptedMutationId { get; internal set; }
    public string? Error { get; internal set; }
    public DeviceSettingsSaveState SaveState { get; internal set; }

    internal void Apply(DeliveredComponent<T> component)
    {
        if (component.Revision < Revision) return;
        Revision = component.Revision;
        AcceptedMutationId = component.MutationId;
        if ((PendingMutationId is null && SaveState != DeviceSettingsSaveState.Failed) ||
            PendingMutationId == component.MutationId)
        {
            Value = component.Value;
            PendingRequestId = null;
            PendingMutationId = null;
            Error = component.Delivery is PendingDeviceDelivery { LastFailure: { } failure } ? failure : null;
            SaveState = component.Delivery is PendingDeviceDelivery
                ? DeviceSettingsSaveState.WaitingForDevice
                : DeviceSettingsSaveState.Saved;
        }
    }
}

public sealed class DeviceComponentDraftStore
{
    private readonly Dictionary<DeviceId, DeviceComponentDraft<SelectionValue>> _selections = [];
    private readonly Dictionary<DeviceId, DeviceComponentDraft<SubscriptionsValue>> _subscriptions = [];
    public IReadOnlyDictionary<DeviceId, DeviceComponentDraft<SelectionValue>> Selections => _selections;
    public IReadOnlyDictionary<DeviceId, DeviceComponentDraft<SubscriptionsValue>> Subscriptions => _subscriptions;

    public void ApplyCanonical(DeviceConfigEvent config)
    {
        Apply(_selections, config.DeviceId, config.Selection);
        Apply(_subscriptions, config.DeviceId, config.Subscriptions);
    }

    public SetSelectionCommand EditSelection(DeviceId deviceId, SelectionValue value, string requestId, string mutationId)
    {
        var draft = Require(_selections, deviceId, "selection");
        BeginEdit(draft, value, requestId, mutationId);
        return new SetSelectionCommand(deviceId, requestId, mutationId, value);
    }

    public SetSubscriptionsCommand EditSubscriptions(
        DeviceId deviceId,
        SubscriptionsValue value,
        string requestId,
        string mutationId)
    {
        var draft = Require(_subscriptions, deviceId, "subscriptions");
        BeginEdit(draft, value, requestId, mutationId);
        return new SetSubscriptionsCommand(deviceId, requestId, mutationId, value);
    }

    public bool ApplyFailure(ConfigMutationFailedEvent failure) => failure.Component switch
    {
        ConfigComponent.Selection => Fail(_selections, failure),
        ConfigComponent.Subscriptions => Fail(_subscriptions, failure),
        _ => false,
    };

    public void Remove(DeviceId deviceId)
    {
        _selections.Remove(deviceId);
        _subscriptions.Remove(deviceId);
    }

    private static void Apply<T>(
        Dictionary<DeviceId, DeviceComponentDraft<T>> drafts,
        DeviceId deviceId,
        DeliveredComponent<T> component)
    {
        if (drafts.TryGetValue(deviceId, out var draft)) draft.Apply(component);
        else drafts.Add(deviceId, new DeviceComponentDraft<T>(component));
    }

    private static DeviceComponentDraft<T> Require<T>(
        Dictionary<DeviceId, DeviceComponentDraft<T>> drafts,
        DeviceId deviceId,
        string component) => drafts.TryGetValue(deviceId, out var draft)
            ? draft
            : throw new InvalidOperationException($"Device {component} is not loaded");

    private static void BeginEdit<T>(DeviceComponentDraft<T> draft, T value, string requestId, string mutationId)
    {
        draft.Value = value;
        draft.PendingRequestId = requestId;
        draft.PendingMutationId = mutationId;
        draft.Error = null;
        draft.SaveState = DeviceSettingsSaveState.Editing;
    }

    private static bool Fail<T>(
        Dictionary<DeviceId, DeviceComponentDraft<T>> drafts,
        ConfigMutationFailedEvent failure)
    {
        if (!drafts.TryGetValue(failure.DeviceId, out var draft) ||
            !((draft.PendingRequestId == failure.RequestId && draft.PendingMutationId == failure.MutationId) ||
              (failure.Stage == ConfigFailureStage.DeviceDelivery && draft.AcceptedMutationId == failure.MutationId))) return false;
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
}
