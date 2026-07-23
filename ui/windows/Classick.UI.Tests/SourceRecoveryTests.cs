using System.Threading.Channels;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;

namespace Classick_UI.Tests;

public sealed class SourceRecoveryTests
{
    private const string Request = "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8840";

    [Fact]
    public void RetryClearsOnlyOnMatchingCorrelatedTerminalState()
    {
        var viewModel = AttentionViewModel();
        _ = viewModel.CreateWireSourceRetryCommand(Request);

        viewModel.ApplySourceAvailability(new WireSourceAvailabilityEvent(
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8841",
            SourceAvailabilityState.Unavailable,
            null));
        Assert.True(viewModel.SourceRetryPending);

        viewModel.ApplySourceAvailability(new WireSourceAvailabilityEvent(
            Request,
            SourceAvailabilityState.Available,
            "X:\\Music"));
        Assert.False(viewModel.SourceRetryPending);
        Assert.Equal("X:\\Music", viewModel.AvailableSourceRoot);
    }

    [Fact]
    public void DuplicateRetryClicksCoalesceWhileRequestIsPending()
    {
        var viewModel = AttentionViewModel();

        var first = viewModel.CreateWireSourceRetryCommand(Request);
        var duplicate = viewModel.CreateWireSourceRetryCommand(
            "018f9d7e-2f2b-7b52-9f1d-f78bdb2f8842");

        Assert.True(first!.AllowUi);
        Assert.Null(duplicate);
        Assert.False(viewModel.SourceRetryAvailable);
    }

    [Fact]
    public void SourceFailurePreservesTypedDevicePresentation()
    {
        var viewModel = new PopoverViewModel();
        viewModel.Update(Device());

        viewModel.ApplySourceAvailability(new WireSourceAvailabilityEvent(
            null,
            SourceAvailabilityState.AuthRequired,
            null));

        Assert.True(viewModel.IpodConnected);
        Assert.True(viewModel.HasStorage);
        Assert.True(viewModel.SourceAttentionVisible);
        Assert.False(viewModel.ShowConnectedContent);
    }

    [Fact]
    public async Task RouterDeliversTypedAvailabilityInOrder()
    {
        var channel = Channel.CreateUnbounded<WireEvent>();
        var received = new TaskCompletionSource<WireSourceAvailabilityEvent>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        using var router = new DaemonEventRouter(channel.Reader);
        router.EventReceived += wireEvent =>
        {
            if (wireEvent is WireSourceAvailabilityEvent availability)
                received.SetResult(availability);
        };
        router.Start();

        await channel.Writer.WriteAsync(new WireSourceAvailabilityEvent(
            null,
            SourceAvailabilityState.AuthRequired,
            null));

        Assert.Equal(
            SourceAvailabilityState.AuthRequired,
            (await received.Task.WaitAsync(TimeSpan.FromSeconds(1))).State);
    }

    private static PopoverViewModel AttentionViewModel()
    {
        var viewModel = new PopoverViewModel();
        viewModel.ApplySourceAvailability(new WireSourceAvailabilityEvent(
            null,
            SourceAvailabilityState.AuthRequired,
            null));
        return viewModel;
    }

    private static IdentifiedDeviceSnapshot Device() => new(
        DeviceId.Parse("000A27002138B0A8"),
        "Michael's iPod",
        DeviceReadiness.Ready,
        new HardwareFacts(),
        ProfileStatus.Adopted,
        true,
        "D:\\",
        DevicePhase.Idle,
        null,
        new StorageSnapshot(1_000, 400, StorageFreshness.Live),
        12,
        null,
        null);
}
