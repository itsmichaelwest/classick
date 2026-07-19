using System.Text.Json;
using System.Threading.Channels;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;

public sealed class SourceRecoveryWireTests
{
    [Fact]
    public void Retry_source_mount_command_serializes_exact_v2_shape()
    {
        var command = new RetrySourceMountCommand(AllowUi: true, RequestId: "request-source");

        var json = JsonSerializer.Serialize<DaemonCommand>(command);

        Assert.Equal(
            """{"type":"retry_source_mount","allow_ui":true,"request_id":"request-source"}""",
            json);
    }

    [Theory]
    [InlineData("""{"type":"retry_source_mount","request_id":"request-source"}""")]
    [InlineData("""{"type":"retry_source_mount","allow_ui":true}""")]
    public void Retry_source_mount_command_rejects_missing_required_fields(string json)
    {
        Assert.Throws<JsonException>(() => JsonSerializer.Deserialize<DaemonCommand>(json));
    }

    [Theory]
    [InlineData("""{"type":"source_availability","state":"available","source_root":"X:\\Music","acknowledged_request_id":"request-source"}""", SourceAvailabilityState.Available, "X:\\Music", "request-source")]
    [InlineData("""{"type":"source_availability","state":"remounting"}""", SourceAvailabilityState.Remounting, null, null)]
    [InlineData("""{"type":"source_availability","state":"auth_required"}""", SourceAvailabilityState.AuthRequired, null, null)]
    [InlineData("""{"type":"source_availability","state":"unavailable","acknowledged_request_id":"other-client"}""", SourceAvailabilityState.Unavailable, null, "other-client")]
    public void Source_availability_event_decodes_all_v2_states(
        string json,
        SourceAvailabilityState expectedState,
        string? expectedRoot,
        string? expectedRequestId)
    {
        var decoded = JsonSerializer.Deserialize<DaemonEvent>(json);

        var availability = Assert.IsType<SourceAvailabilityEvent>(decoded);
        Assert.Equal(expectedState, availability.State);
        Assert.Equal(expectedRoot, availability.SourceRoot);
        Assert.Equal(expectedRequestId, availability.AcknowledgedRequestId);
    }

    [Theory]
    [InlineData("""{"type":"source_availability"}""")]
    [InlineData("""{"type":"source_availability","state":"unknown"}""")]
    [InlineData("""{"type":"source_availability","state":"available"}""")]
    [InlineData("""{"type":"source_availability","state":"available","source_root":null}""")]
    [InlineData("""{"type":"source_availability","state":"remounting","source_root":"X:\\Music"}""")]
    [InlineData("""{"type":"source_availability","state":"auth_required","source_root":null}""")]
    [InlineData("""{"type":"source_availability","state":"unavailable","source_root":"X:\\Music"}""")]
    public void Source_availability_event_rejects_invalid_v2_shapes(string json)
    {
        Assert.Throws<JsonException>(() => JsonSerializer.Deserialize<DaemonEvent>(json));
    }

    [Fact]
    public void Source_availability_event_serializes_state_dependent_root_shape()
    {
        var available = JsonSerializer.Serialize<DaemonEvent>(
            new SourceAvailabilityEvent(SourceAvailabilityState.Available, "X:\\Music"));
        var remounting = JsonSerializer.Serialize<DaemonEvent>(
            new SourceAvailabilityEvent(SourceAvailabilityState.Remounting));

        Assert.Equal(
            """{"type":"source_availability","state":"available","source_root":"X:\\Music"}""",
            available);
        Assert.Equal(
            """{"type":"source_availability","state":"remounting"}""",
            remounting);
    }
}

public sealed class SourceRecoveryRouterTests
{
    [Fact]
    public async Task Router_delivers_source_availability_without_dropping_it()
    {
        var channel = Channel.CreateUnbounded<object>();
        var received = new TaskCompletionSource<SourceAvailabilityEvent>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        var router = new DaemonEventRouter(channel.Reader);
        router.SourceAvailabilityUpdated += received.SetResult;
        router.Start();

        await channel.Writer.WriteAsync(
            new SourceAvailabilityEvent(SourceAvailabilityState.AuthRequired));

        var availability = await received.Task.WaitAsync(TimeSpan.FromSeconds(1));
        Assert.Equal(SourceAvailabilityState.AuthRequired, availability.State);
        await router.StopAsync();
    }
}

public sealed class SourceRecoveryViewModelTests
{
    [Fact]
    public void Lifecycle_broadcasts_drive_attention_and_remounting_state()
    {
        var viewModel = new PopoverViewModel();

        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.Unavailable));
        Assert.True(viewModel.SourceAttentionVisible);
        Assert.False(viewModel.SourceRemounting);
        Assert.True(viewModel.ShowSourceRecovery);
        Assert.True(viewModel.SourceRetryAvailable);

        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.Remounting));
        Assert.False(viewModel.SourceAttentionVisible);
        Assert.True(viewModel.SourceRemounting);
        Assert.True(viewModel.ShowSourceRecovery);
        Assert.False(viewModel.SourceRetryAvailable);

        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.Available, "X:\\Music"));
        Assert.False(viewModel.SourceAttentionVisible);
        Assert.False(viewModel.SourceRemounting);
        Assert.False(viewModel.ShowSourceRecovery);
        Assert.Equal("X:\\Music", viewModel.AvailableSourceRoot);
    }

    [Fact]
    public void Duplicate_connect_clicks_coalesce_while_retry_is_pending()
    {
        var viewModel = new PopoverViewModel();
        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.AuthRequired));

        var first = viewModel.CreateSourceRetryCommand("request-current");
        var duplicate = viewModel.CreateSourceRetryCommand("request-duplicate");

        Assert.NotNull(first);
        Assert.True(first!.AllowUi);
        Assert.Equal("request-current", first.RequestId);
        Assert.Null(duplicate);
        Assert.True(viewModel.SourceRetryPending);
        Assert.False(viewModel.SourceRetryAvailable);
    }

    [Fact]
    public void Matching_terminal_reply_clears_pending_but_stale_reply_is_ignored()
    {
        var viewModel = new PopoverViewModel();
        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.AuthRequired));
        _ = viewModel.CreateSourceRetryCommand("request-current");

        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(
                SourceAvailabilityState.Available,
                "X:\\Stale",
                "request-stale"));
        Assert.True(viewModel.SourceAttentionVisible);
        Assert.True(viewModel.SourceRetryPending);
        Assert.Null(viewModel.AvailableSourceRoot);

        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(
                SourceAvailabilityState.Available,
                "X:\\Music",
                "request-current"));
        Assert.False(viewModel.SourceAttentionVisible);
        Assert.False(viewModel.SourceRetryPending);
        Assert.Equal("X:\\Music", viewModel.AvailableSourceRoot);
    }

    [Fact]
    public void Other_client_terminal_is_authoritative_when_no_local_retry_is_pending()
    {
        var viewModel = new PopoverViewModel();
        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.Remounting));

        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(
                SourceAvailabilityState.Available,
                "X:\\Music",
                "another-client"));

        Assert.False(viewModel.SourceRemounting);
        Assert.Equal("X:\\Music", viewModel.AvailableSourceRoot);
    }

    [Fact]
    public void Source_failure_preserves_cached_device_display_state()
    {
        var viewModel = new PopoverViewModel();
        viewModel.Update(new StatusUpdateEvent(
            State: "idle",
            Configured: true,
            IpodConnected: true,
            LastSync: null,
            NextScheduledUnixSecs: null,
            Storage: new StorageInfo(1_000, 400),
            SyncedCount: 12,
            LibraryCount: 20,
            AcknowledgedRequestId: null));

        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.AuthRequired));

        Assert.True(viewModel.IpodConnected);
        Assert.True(viewModel.HasStorage);
        Assert.Equal("Up to date · iPod connected", viewModel.StatusText);
        Assert.True(viewModel.SourceAttentionVisible);
        Assert.False(viewModel.ShowConnectedContent);
    }

    [Fact]
    public void Sync_prompt_temporarily_takes_precedence_over_source_recovery()
    {
        var viewModel = new PopoverViewModel();
        viewModel.ApplySourceAvailability(
            new SourceAvailabilityEvent(SourceAvailabilityState.AuthRequired));

        viewModel.ApplyIpcProgress(new PromptEvent(7, "Choose", ["Retry"]));
        Assert.False(viewModel.ShowSourceRecovery);

        viewModel.ClearPrompt();
        Assert.True(viewModel.ShowSourceRecovery);
    }
}
