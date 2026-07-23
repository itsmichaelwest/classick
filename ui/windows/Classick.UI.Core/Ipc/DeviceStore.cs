using Classick_UI.Devices;

namespace Classick_UI.Ipc;

public sealed record DeviceActionTarget(DeviceId DeviceId);
public sealed record DeviceSessionTarget(DeviceId DeviceId, ulong SessionId);

public sealed class DeviceClientState
{
    internal DeviceClientState(IdentifiedDeviceSnapshot inventory)
    {
        Inventory = inventory;
        ActiveSessionId = inventory.SessionId;
        if (inventory.SessionId is { } sessionId)
        {
            SyncPresentation = new DeviceSyncPresentation(
                new DeviceSessionTarget(inventory.DeviceId, sessionId));
        }
    }

    public IdentifiedDeviceSnapshot Inventory { get; internal set; }
    public DeviceConfigEvent? Config { get; internal set; }
    public IReadOnlyList<WireHistoryEntry> History { get; internal set; } = [];
    public WireEvent? LastProgress { get; internal set; }
    public ulong? ActiveSessionId { get; internal set; }
    public DeviceSyncPresentation? SyncPresentation { get; internal set; }
}

public sealed class DeviceStore
{
    private readonly Dictionary<DeviceId, DeviceClientState> _devices = [];
    private readonly Dictionary<ulong, UnidentifiedDeviceSnapshot> _unidentified = [];
    private readonly Dictionary<DeviceId, DeviceConfigEvent> _pendingConfigs = [];
    private readonly Dictionary<DeviceId, IReadOnlyList<WireHistoryEntry>> _pendingHistory = [];
    private ulong? _inventoryRevision;
    private DeviceId? _explicitSelection;

    public GlobalConfigEvent? GlobalConfig { get; private set; }
    public DeviceSettingsDraftStore SettingsDrafts { get; } = new();
    public DeviceComponentDraftStore ComponentDrafts { get; } = new();
    public WireSourceAvailabilityEvent? SourceAvailability { get; private set; }
    public IReadOnlyDictionary<DeviceId, DeviceClientState> Devices => _devices;
    public IReadOnlyDictionary<ulong, UnidentifiedDeviceSnapshot> Unidentified => _unidentified;
    public DeviceId? FocusedDeviceId { get; private set; }
    public ulong? InventoryRevision => _inventoryRevision;

    public bool Reduce(WireEvent wireEvent)
    {
        ArgumentNullException.ThrowIfNull(wireEvent);
        switch (wireEvent)
        {
            case GlobalConfigEvent global:
                GlobalConfig = global;
                return true;
            case WireSourceAvailabilityEvent source:
                SourceAvailability = source;
                return true;
            case DeviceInventoryEvent inventory:
                return ReduceInventory(inventory);
            case DeviceConfigEvent config:
                var mergedConfig = MergeConfig(
                    _devices.TryGetValue(config.DeviceId, out var device)
                        ? device.Config
                        : _pendingConfigs.GetValueOrDefault(config.DeviceId),
                    config);
                if (device is not null) device.Config = mergedConfig;
                else _pendingConfigs[config.DeviceId] = mergedConfig;
                SettingsDrafts.ApplyCanonical(mergedConfig);
                ComponentDrafts.ApplyCanonical(mergedConfig);
                return true;
            case ConfigMutationFailedEvent failure:
                SettingsDrafts.ApplyFailure(failure);
                ComponentDrafts.ApplyFailure(failure);
                return true;
            case HistoryEvent history:
                _pendingHistory.Clear();
                foreach (var knownDevice in _devices.Values)
                {
                    knownDevice.History = [];
                }
                foreach (var group in history.Entries.GroupBy(entry => entry.DeviceId))
                {
                    if (_devices.TryGetValue(group.Key, out var historyDevice))
                    {
                        historyDevice.History = group.ToArray();
                    }
                    else
                    {
                        _pendingHistory[group.Key] = group.ToArray();
                    }
                }
                return true;
            case DeviceForgottenEvent forgotten:
                var removed = _devices.Remove(forgotten.DeviceId);
                _pendingConfigs.Remove(forgotten.DeviceId);
                _pendingHistory.Remove(forgotten.DeviceId);
                SettingsDrafts.Remove(forgotten.DeviceId);
                ComponentDrafts.Remove(forgotten.DeviceId);
                if (_explicitSelection == forgotten.DeviceId) _explicitSelection = null;
                if (FocusedDeviceId == forgotten.DeviceId) FocusedDeviceId = null;
                RefreshFocus();
                return removed;
            case SyncAcceptedEvent accepted when _devices.TryGetValue(accepted.DeviceId, out var acceptedDevice):
                acceptedDevice.ActiveSessionId = accepted.SessionId;
                acceptedDevice.LastProgress = accepted;
                acceptedDevice.SyncPresentation = new DeviceSyncPresentation(
                    new DeviceSessionTarget(accepted.DeviceId, accepted.SessionId));
                acceptedDevice.SyncPresentation.Apply(accepted);
                RefreshFocus();
                return true;
            case ISessionRoutedMessage routed when wireEvent is WireEvent progress:
                return ReduceProgress(progress, routed);
            default:
                return true;
        }
    }

    public bool SelectDevice(DeviceId? deviceId)
    {
        if (deviceId is not null && !_devices.ContainsKey(deviceId)) return false;
        _explicitSelection = deviceId;
        if (deviceId is not null)
        {
            FocusedDeviceId = deviceId;
            return true;
        }
        RefreshFocus();
        return true;
    }

    public DeviceActionTarget? CaptureFocusedDeviceAction() =>
        FocusedDeviceId is { } deviceId && _devices.ContainsKey(deviceId)
            ? new DeviceActionTarget(deviceId)
            : null;

    public DeviceSessionTarget? CaptureFocusedSessionAction()
    {
        if (FocusedDeviceId is not { } deviceId ||
            !_devices.TryGetValue(deviceId, out var device) ||
            device.ActiveSessionId is not { } sessionId)
        {
            return null;
        }
        return new DeviceSessionTarget(deviceId, sessionId);
    }

    public DeviceMountTarget? CaptureMountAction(DeviceId deviceId)
    {
        if (!_devices.TryGetValue(deviceId, out var device) ||
            !device.Inventory.Connected ||
            string.IsNullOrWhiteSpace(device.Inventory.MountPath))
        {
            return null;
        }

        return new DeviceMountTarget(deviceId, _inventoryRevision, device.Inventory.MountPath);
    }

    public bool IsCurrentMountAction(DeviceMountTarget target) =>
        _inventoryRevision == target.InventoryRevision &&
        _devices.TryGetValue(target.DeviceId, out var device) &&
        device.Inventory.Connected &&
        string.Equals(device.Inventory.MountPath, target.MountPath, StringComparison.OrdinalIgnoreCase);

    public DeviceActionTarget? CaptureDeviceMutation(DeviceId deviceId) =>
        _devices.TryGetValue(deviceId, out var device) &&
        device.Inventory is
        {
            Connected: true,
            Readiness: DeviceReadiness.Ready,
            ProfileStatus: ProfileStatus.Adopted,
        } &&
        device.ActiveSessionId is null
            ? new DeviceActionTarget(deviceId)
            : null;

    private bool ReduceInventory(DeviceInventoryEvent inventory)
    {
        if (_inventoryRevision is not null && inventory.Revision < _inventoryRevision) return false;
        _inventoryRevision = inventory.Revision;

        var present = inventory.Devices.Select(device => device.DeviceId).ToHashSet();
        foreach (var removed in _devices.Keys.Where(deviceId => !present.Contains(deviceId)).ToArray())
        {
            _devices.Remove(removed);
        }
        foreach (var snapshot in inventory.Devices)
        {
            if (_devices.TryGetValue(snapshot.DeviceId, out var existing))
            {
                existing.Inventory = snapshot;
                if (snapshot.SessionId is { } snapshotSession)
                {
                    existing.ActiveSessionId = snapshotSession;
                    if (existing.SyncPresentation?.Target.SessionId != snapshotSession)
                    {
                        existing.SyncPresentation = new DeviceSyncPresentation(
                            new DeviceSessionTarget(snapshot.DeviceId, snapshotSession));
                    }
                }
                else if (existing.LastProgress is not (SyncPausedEvent or SyncCancelledEvent))
                {
                    existing.ActiveSessionId = null;
                    existing.LastProgress = null;
                }
            }
            else
            {
                var added = new DeviceClientState(snapshot);
                if (_pendingConfigs.Remove(snapshot.DeviceId, out var config)) added.Config = config;
                if (_pendingHistory.Remove(snapshot.DeviceId, out var history)) added.History = history;
                _devices.Add(snapshot.DeviceId, added);
            }
        }

        _unidentified.Clear();
        foreach (var observation in inventory.Unidentified)
        {
            _unidentified.Add(observation.ObservationId, observation);
        }

        if (_explicitSelection is not null && !_devices.ContainsKey(_explicitSelection))
            _explicitSelection = null;
        if (FocusedDeviceId is not null && !_devices.ContainsKey(FocusedDeviceId))
            FocusedDeviceId = null;
        RefreshFocus();
        return true;
    }

    private bool ReduceProgress(WireEvent progress, ISessionRoutedMessage routed)
    {
        if (!_devices.TryGetValue(routed.DeviceId, out var device) ||
            device.ActiveSessionId != routed.SessionId)
        {
            return false;
        }

        device.LastProgress = progress;
        device.SyncPresentation ??= new DeviceSyncPresentation(
            new DeviceSessionTarget(routed.DeviceId, routed.SessionId));
        device.SyncPresentation.Apply(progress);
        // Paused/cancelled are intermediate terminal-state notices. The core
        // still publishes the authoritative sync_finished rollup for the same
        // session, so retain the route until that final event arrives.
        if (progress is SyncFinishedEvent)
        {
            device.ActiveSessionId = null;
            RefreshFocus();
        }
        return true;
    }

    private static DeviceConfigEvent MergeConfig(DeviceConfigEvent? current, DeviceConfigEvent incoming)
    {
        if (current is null) return incoming;
        return incoming with
        {
            Selection = incoming.Selection.Revision >= current.Selection.Revision
                ? incoming.Selection
                : current.Selection,
            Settings = incoming.Settings.Revision >= current.Settings.Revision
                ? incoming.Settings
                : current.Settings,
            Subscriptions = incoming.Subscriptions.Revision >= current.Subscriptions.Revision
                ? incoming.Subscriptions
                : current.Subscriptions,
        };
    }

    private void RefreshFocus()
    {
        var active = _devices
            .Where(pair => pair.Value.ActiveSessionId is not null)
            .Select(pair => pair.Key)
            .ToArray();
        if (FocusedDeviceId is { } focused && active.Contains(focused)) return;
        if (active.Length == 1)
        {
            FocusedDeviceId = active[0];
            return;
        }
        if (_explicitSelection is { } selected && _devices.ContainsKey(selected))
        {
            FocusedDeviceId = selected;
            return;
        }

        var soleConnectedConfigured = _devices
            .Where(pair => pair.Value.Inventory is
            { Connected: true, Readiness: DeviceReadiness.Ready, ProfileStatus: ProfileStatus.Adopted })
            .Select(pair => pair.Key)
            .ToArray();
        FocusedDeviceId = soleConnectedConfigured.Length == 1 ? soleConnectedConfigured[0] : null;
    }
}
