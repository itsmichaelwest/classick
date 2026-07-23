using Classick_UI.Ipc;

namespace Classick_UI.Devices;

public sealed class DeviceSetupAcknowledgementTracker
{
    private readonly SetSourceLocationCommand _source;
    private readonly AdoptDeviceCommand _adopt;
    private bool _sourceAccepted;
    private bool _deviceAccepted;

    public DeviceSetupAcknowledgementTracker(
        SetSourceLocationCommand source,
        AdoptDeviceCommand adopt)
    {
        _source = source;
        _adopt = adopt;
    }

    public bool IsComplete => _sourceAccepted && _deviceAccepted;
    public bool SourceAccepted => _sourceAccepted;
    public string? Failure { get; private set; }

    public void Observe(WireEvent wireEvent)
    {
        ArgumentNullException.ThrowIfNull(wireEvent);
        switch (wireEvent)
        {
            case GlobalConfigEvent { RequestId: var requestId } when requestId == _source.RequestId:
                _sourceAccepted = true;
                break;
            case DeviceConfigEvent config when ConfigMatches(config):
                _deviceAccepted = true;
                break;
            case CommandFailedEvent failed when
                failed.RequestId == _source.RequestId || failed.RequestId == _adopt.RequestId:
                Failure = failed.Message;
                break;
            case ConfigMutationFailedEvent { Stage: ConfigFailureStage.HostAcceptance } failed when MutationMatches(failed):
                Failure = failed.Message;
                break;
        }
    }

    private bool ConfigMatches(DeviceConfigEvent config) =>
        config.DeviceId == _adopt.DeviceId &&
        config.Selection.MutationId == _adopt.SelectionMutationId &&
        config.Settings.MutationId == _adopt.SettingsMutationId &&
        config.Subscriptions.MutationId == _adopt.SubscriptionsMutationId;

    private bool MutationMatches(ConfigMutationFailedEvent failed) =>
        failed.DeviceId == _adopt.DeviceId &&
        (failed.RequestId == _adopt.RequestId ||
         failed.MutationId == _adopt.SelectionMutationId ||
         failed.MutationId == _adopt.SettingsMutationId ||
         failed.MutationId == _adopt.SubscriptionsMutationId);
}
