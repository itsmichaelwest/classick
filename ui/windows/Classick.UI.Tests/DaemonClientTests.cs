using System.Text.Json;
using Classick_UI.Ipc;
using Xunit;

public class DaemonClientWireFormatTests
{
    [Fact]
    public void StatusUpdate_event_deserializes_via_DaemonEvent()
    {
        var json = """{"type":"status_update","state":"idle","configured":true,"ipod_connected":false,"last_sync":null,"next_scheduled_unix_secs":null,"synced_count":12,"library_count":20,"acknowledged_request_id":"status"}""";
        var evt = JsonSerializer.Deserialize<DaemonEvent>(json);
        var status = Assert.IsType<StatusUpdateEvent>(evt);
        Assert.Equal("idle", status.State);
        Assert.True(status.Configured);
        Assert.False(status.IpodConnected);
        Assert.Equal(12, status.SyncedCount);
        Assert.Equal(20, status.LibraryCount);
        Assert.Equal("status", status.AcknowledgedRequestId);
    }

    [Fact]
    public void SaveConfig_command_serializes_with_required_correlation()
    {
        var cmd = new SaveConfigCommand(
            Source: null,
            Daemon: null,
            Ipod: new IpodIdentity("EXAMPLE1234", "iPod 7G", null, CustomSelection: false),
            RequestId: "request-save");
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Equal("""{"type":"save_config","ipod":{"serial":"EXAMPLE1234","model_label":"iPod 7G","custom_selection":false},"request_id":"request-save"}""", json);
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

    [Fact]
    public void V2_config_and_history_decode_complete_nested_wire_shapes()
    {
        const string configJson = """
            {"type":"config_update","source":"/music","daemon":{"enabled":true,"autostart_with_windows":false,"first_sync_mode":"review","subsequent_sync_mode":"auto_apply","schedule_minutes":30,"notify_on":"all","rockbox_compat":true},"ipod":{"serial":"A","model_label":"iPod 7G","custom_selection":true},"config_revision":8}
            """;
        const string historyJson = """
            {"type":"history_update","entries":[{"serial":"A","session_id":7,"timestamp":"2026-07-18T12:00:00Z","duration_secs":5,"trigger":"manual","outcome":"ok","summary":{"add":1,"modify":2,"remove":3,"unchanged":4,"skipped":5,"metadata_only":6,"skipped_for_space_tracks":7,"skipped_for_space_bytes":8,"artwork_failed_sources":9},"db_restored":true}],"acknowledged_request_id":"history"}
            """;

        var config = Assert.IsType<ConfigUpdateEvent>(JsonSerializer.Deserialize<DaemonEvent>(configJson));
        var history = Assert.IsType<HistoryUpdateEvent>(JsonSerializer.Deserialize<DaemonEvent>(historyJson));

        Assert.True(config.Daemon!.RockboxCompat);
        Assert.True(config.Ipod!.CustomSelection);
        Assert.True(history.Entries[0].DbRestored);
        Assert.Equal(6, history.Entries[0].Summary!.MetadataOnly);
        Assert.Equal(8UL, history.Entries[0].Summary!.SkippedForSpaceBytes);
    }

    [Fact]
    public void V2_remaining_command_payloads_have_exact_shapes()
    {
        var selection = new SelectionState(SelectionMode.Include, [new ArtistSelectionRule("Bowie")]);
        var playlist = new ManualPlaylistPayload(null, "Favourites", ["Bowie/Heroes.flac"]);

        Assert.Equal("""{"type":"get_library","request_id":"library"}""", JsonSerializer.Serialize<DaemonCommand>(new GetLibraryCommand("library")));
        Assert.Equal("""{"type":"get_history","request_id":"history"}""", JsonSerializer.Serialize<DaemonCommand>(new GetHistoryCommand(RequestId: "history")));
        Assert.Equal("""{"type":"scan_library","request_id":"scan"}""", JsonSerializer.Serialize<DaemonCommand>(new ScanLibraryCommand("scan")));
        Assert.Equal("""{"type":"backfill_rockbox","serial":"A","request_id":"backfill"}""", JsonSerializer.Serialize<DaemonCommand>(new BackfillRockboxCommand("A", "backfill")));
        Assert.Equal("""{"type":"replace_library","serial":"A","request_id":"replace"}""", JsonSerializer.Serialize<DaemonCommand>(new ReplaceLibraryCommand("A", "replace")));
        Assert.Equal("""{"type":"preview_selection","mode":"include","rules":[{"kind":"artist","name":"Bowie"}],"serial":"A","request_id":"selection"}""", JsonSerializer.Serialize<DaemonCommand>(new PreviewSelectionCommand(SelectionMode.Include, [new ArtistSelectionRule("Bowie")], "A", "selection")));
        Assert.Equal("""{"type":"list_playlists","request_id":"list"}""", JsonSerializer.Serialize<DaemonCommand>(new ListPlaylistsCommand("list")));
        Assert.Equal("""{"type":"get_playlist","slug":"favourites","request_id":"get-playlist"}""", JsonSerializer.Serialize<DaemonCommand>(new GetPlaylistCommand("favourites", "get-playlist")));
        Assert.Equal("""{"type":"save_playlist","playlist":{"kind":"manual","name":"Favourites","tracks":["Bowie/Heroes.flac"]},"request_id":"save-playlist"}""", JsonSerializer.Serialize<DaemonCommand>(new SavePlaylistCommand(playlist, "save-playlist")));
        Assert.Equal("""{"type":"delete_playlist","slug":"favourites","request_id":"delete-playlist"}""", JsonSerializer.Serialize<DaemonCommand>(new DeletePlaylistCommand("favourites", "delete-playlist")));
        Assert.Equal("""{"type":"get_device_config","serial":"A","request_id":"get-device"}""", JsonSerializer.Serialize<DaemonCommand>(new GetDeviceConfigCommand("A", "get-device")));
        Assert.Equal("""{"type":"save_device_config","serial":"A","selection":{"mode":"include","rules":[{"kind":"artist","name":"Bowie"}]},"request_id":"save-device"}""", JsonSerializer.Serialize<DaemonCommand>(new SaveDeviceConfigCommand("A", selection, null, null, "save-device")));
        Assert.Equal("""{"type":"preview_device","serial":"A","request_id":"preview-device"}""", JsonSerializer.Serialize<DaemonCommand>(new PreviewDeviceCommand("A", "preview-device")));
        Assert.Equal("""{"type":"resolve_tracks","rules":[{"kind":"artist","name":"Bowie"}],"request_id":"resolve"}""", JsonSerializer.Serialize<DaemonCommand>(new ResolveTracksCommand([new ArtistSelectionRule("Bowie")], "resolve")));
    }

    [Fact]
    public void V2_remaining_daemon_events_decode_through_daemon_hierarchy()
    {
        var events = new[]
        {
            """{"type":"library_update","source_root":null,"scanned_at_unix_secs":null,"artists":[],"genres":[],"total_tracks":0,"total_bytes":0,"acknowledged_request_id":"library"}""",
            """{"type":"selection_update","mode":"all","rules":[],"serial":"A","acknowledged_request_id":"selection"}""",
            """{"type":"selection_preview","selected_tracks":1,"selected_bytes":2,"adds":3,"removes":4,"serial":"A","acknowledged_request_id":"preview"}""",
            """{"type":"playlists_update","playlists":[{"slug":"favourites","name":"Favourites","kind":"manual","tracks":1,"bytes":2}],"acknowledged_request_id":"playlists"}""",
            """{"type":"playlist_detail","slug":"favourites","name":"Favourites","kind":"manual","tracks":["Bowie/Heroes.flac"],"acknowledged_request_id":"detail"}""",
            """{"type":"device_config_update","serial":"A","selection":{"mode":"all","rules":[]},"subscriptions":{"playlists":[]},"settings":{"auto_sync":true,"rockbox_compat":false},"acknowledged_request_id":"config"}""",
            """{"type":"device_preview","serial":"A","selected_tracks":1,"selected_bytes":2,"playlist_extra_tracks":3,"playlist_extra_bytes":4,"projected_free_bytes":null,"acknowledged_request_id":"device-preview"}""",
            """{"type":"resolved_tracks","tracks":["Bowie/Heroes.flac"],"acknowledged_request_id":"resolve"}""",
        };

        var decoded = events.Select(json => JsonSerializer.Deserialize<DaemonEvent>(json)).ToArray();

        Assert.Collection(decoded,
            item => Assert.IsType<LibraryUpdateEvent>(item),
            item => Assert.IsType<SelectionUpdateEvent>(item),
            item => Assert.IsType<SelectionPreviewEvent>(item),
            item => Assert.IsType<PlaylistsUpdateEvent>(item),
            item => Assert.IsType<PlaylistDetailEvent>(item),
            item => Assert.IsType<DeviceConfigUpdateEvent>(item),
            item => Assert.IsType<DevicePreviewEvent>(item),
            item => Assert.IsType<ResolvedTracksEvent>(item));
    }
}
