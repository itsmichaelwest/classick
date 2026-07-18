using System.Text.Json;
using Classick_UI.Ipc;
using Xunit;

public class DaemonClientWireFormatTests
{
    [Fact]
    public void StatusUpdate_event_deserializes_via_DaemonEvent()
    {
        var json = """{"type":"status_update","state":"idle","configured":true,"ipod_connected":false,"last_sync":null,"next_scheduled_unix_secs":null}""";
        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);
        var status = Assert.IsType<StatusUpdateEvent>(evt);
        Assert.Equal("idle", status.State);
        Assert.True(status.Configured);
        Assert.False(status.IpodConnected);
    }

    [Fact]
    public void SaveConfig_command_serializes_with_required_correlation()
    {
        var cmd = new SaveConfigCommand(
            Source: null,
            Daemon: null,
            Ipod: new IpodIdentity("EXAMPLE1234", "iPod 7G"),
            RequestId: "request-save");
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Equal("""{"type":"save_config","source":null,"daemon":null,"ipod":{"serial":"EXAMPLE1234","model_label":"iPod 7G","name":null},"request_id":"request-save"}""", json);
    }

    [Fact]
    public void Targeted_command_round_trips_with_required_serial_and_correlation()
    {
        var cmd = new TriggerSyncCommand("manual", "SERIAL-A", "request-sync");
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Equal("""{"type":"trigger_sync","source":"manual","serial":"SERIAL-A","request_id":"request-sync"}""", json);
        var back = JsonSerializer.Deserialize<DaemonCommand>(json);
        var trig = Assert.IsType<TriggerSyncCommand>(back);
        Assert.Equal("manual", trig.Source);
        Assert.Equal("SERIAL-A", trig.Serial);
        Assert.Equal("request-sync", trig.RequestId);
    }

    [Fact]
    public void Fieldless_global_shutdown_command_serializes_with_type_only()
    {
        var cmd = new ShutdownCommand();
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Equal("{\"type\":\"shutdown\"}", json);
    }

    [Fact]
    public void DeviceConnected_event_carries_all_fields()
    {
        var json = """{"type":"device_connected","serial":"X","model_label":"iPod 7G","drive":"G:\\"}""";
        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);
        var dev = Assert.IsType<DeviceConnectedEvent>(evt);
        Assert.Equal("X", dev.Serial);
        Assert.Equal("iPod 7G", dev.ModelLabel);
        Assert.Equal("G:\\", dev.Drive);
    }

    [Theory]
    [InlineData("{\"type\":\"get_status\"}")]
    [InlineData("{\"type\":\"save_config\",\"source\":null,\"daemon\":null,\"ipod\":null}")]
    [InlineData("{\"type\":\"trigger_sync\",\"source\":\"manual\"}")]
    [InlineData("{\"type\":\"cancel_sync\"}")]
    public void V2_rejects_old_command_payloads_without_required_fields(string json)
    {
        Assert.Throws<JsonException>(() => JsonSerializer.Deserialize<DaemonCommand>(json));
    }

    [Fact]
    public void Device_inventory_snapshot_decodes_two_independent_devices()
    {
        var json = """
            {"type":"device_inventory_snapshot","revision":42,"devices":[
              {"identity":{"serial":"A","model_label":"iPod 5G","name":"Alpha"},"configured":true,"connected":true,"mount":"G:\\","phase":"syncing","session_id":7,"storage":{"total_bytes":1000,"free_bytes":400},"synced_count":12,"library_count":20,"latest_successful_sync":null,"latest_attempt":null,"last_terminal_error":null,"selection_revision":3,"settings_revision":4,"subscriptions_revision":5},
              {"identity":{"serial":"B","model_label":"iPod 7G"},"configured":false,"connected":false,"phase":"unconfigured","synced_count":0,"selection_revision":0,"settings_revision":0,"subscriptions_revision":0}
            ]}
            """;

        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);

        var snapshot = Assert.IsType<DeviceInventorySnapshotEvent>(evt);
        Assert.Equal(42UL, snapshot.Revision);
        Assert.Collection(snapshot.Devices,
            first =>
            {
                Assert.Equal("A", first.Identity.Serial);
                Assert.Equal("syncing", first.Phase);
                Assert.Equal(7UL, first.SessionId);
                Assert.Equal(400UL, first.Storage!.FreeBytes);
            },
            second =>
            {
                Assert.Equal("B", second.Identity.Serial);
                Assert.False(second.Configured);
                Assert.False(second.Connected);
                Assert.Equal("unconfigured", second.Phase);
                Assert.Null(second.SessionId);
            });
    }

    [Fact]
    public void Sync_event_requires_session_and_preserves_optional_serial()
    {
        const string json = """{"type":"sync_event","line":"{\"type\":\"track_done\"}","serial":"A","session_id":9}""";

        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);

        var sync = Assert.IsType<SyncEventEnvelope>(evt);
        Assert.Equal("A", sync.Serial);
        Assert.Equal(9UL, sync.SessionId);
    }

    [Fact]
    public void Device_inventory_snapshot_round_trips_without_optional_null_fields()
    {
        const string json = """{"type":"device_inventory_snapshot","revision":1,"devices":[{"identity":{"serial":"B","model_label":"iPod 7G"},"configured":false,"connected":false,"phase":"unconfigured","synced_count":0,"selection_revision":0,"settings_revision":0,"subscriptions_revision":0}]}""";

        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);
        var snapshot = Assert.IsType<DeviceInventorySnapshotEvent>(evt);
        var roundTripped = JsonSerializer.Serialize<DaemonEvent>(snapshot);

        Assert.Equal(json, roundTripped);
    }

    [Fact]
    public void V2_targeted_mutating_commands_emit_serial_and_correlation()
    {
        var forget = JsonSerializer.Serialize<DaemonCommand>(new ForgetIpodCommand("SERIAL-A", "request-forget"));
        var cancel = JsonSerializer.Serialize<DaemonCommand>(new CancelSyncCommand("SERIAL-A", "request-cancel"));
        var prompt = JsonSerializer.Serialize<DaemonCommand>(new DecidePromptCommand(17, 1, "SERIAL-A", "request-prompt"));

        Assert.Equal("""{"type":"forget_ipod","serial":"SERIAL-A","request_id":"request-forget"}""", forget);
        Assert.Equal("""{"type":"cancel_sync","serial":"SERIAL-A","request_id":"request-cancel"}""", cancel);
        Assert.Equal("""{"type":"decide_prompt","id":17,"choice":1,"serial":"SERIAL-A","request_id":"request-prompt"}""", prompt);
    }

    [Fact]
    public void Config_update_requires_revision_and_decodes_optional_correlation()
    {
        const string json = """{"type":"config_update","source":null,"daemon":null,"ipod":null,"config_revision":5,"acknowledged_request_id":"request-config"}""";

        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);
        var config = Assert.IsType<ConfigUpdateEvent>(evt);

        Assert.Equal(5UL, config.ConfigRevision);
        Assert.Equal("request-config", config.AcknowledgedRequestId);
        Assert.Throws<JsonException>(() => JsonSerializer.Deserialize<DaemonEvent>("""{"type":"config_update","source":null,"daemon":null,"ipod":null}"""));
    }
}
