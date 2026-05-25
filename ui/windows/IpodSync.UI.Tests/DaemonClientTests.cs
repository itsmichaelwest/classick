using System.Text.Json;
using IpodSync_UI.Ipc;
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
    public void SaveConfig_command_serializes_with_ipod_only()
    {
        var cmd = new SaveConfigCommand(Ipod: new IpodIdentity("EXAMPLE1234", "iPod 7G"));
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Contains("\"type\":\"save_config\"", json);
        Assert.Contains("\"serial\":\"EXAMPLE1234\"", json);
        Assert.Contains("\"model_label\":\"iPod 7G\"", json);
    }

    [Fact]
    public void TriggerSync_command_round_trips()
    {
        var cmd = new TriggerSyncCommand("manual");
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        var back = JsonSerializer.Deserialize<DaemonCommand>(json);
        var trig = Assert.IsType<TriggerSyncCommand>(back);
        Assert.Equal("manual", trig.Source);
    }

    [Fact]
    public void Shutdown_command_serializes_with_type_only()
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
}
