using System.Text.Json;
using Classick_UI.Ipc;

namespace Classick_UI.Tests;

/// <summary>
/// JSON wire-format tests for the IPC event/command records. These are pure
/// (no subprocess) and verify the schema documented in
/// <c>docs/ipc-protocol.md</c>: snake_case discriminators, snake_case field
/// names, the nested envelope on <c>review_decision</c>, and the
/// fail-loud-on-unknown-type contract that protects the UI from silently
/// misinterpreting a future event.
/// </summary>
public class IpcWireFormatTests
{
    // ---- Events ----------------------------------------------------------

    [Fact]
    public void Hello_event_deserializes_with_versions()
    {
        var json = """{"type":"hello","protocol_version":"1.0.0","core_version":"0.0.1"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var hello = Assert.IsType<HelloEvent>(evt);
        Assert.Equal("1.0.0", hello.ProtocolVersion);
        Assert.Equal("0.0.1", hello.CoreVersion);
    }

    [Fact]
    public void Header_event_deserializes_paths()
    {
        var json = """{"type":"header","source":"\\\\nas\\music","ipod":"G:\\","manifest":"C:\\state\\m.json"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var header = Assert.IsType<HeaderEvent>(evt);
        Assert.Equal(@"\\nas\music", header.Source);
        Assert.Equal(@"G:\", header.Ipod);
        Assert.Equal(@"C:\state\m.json", header.Manifest);
    }

    [Fact]
    public void Summary_event_deserializes_counts()
    {
        var json = """{"type":"summary","add":12,"modify":3,"metadata_only":0,"remove":0,"unchanged":1260,"total_planned":15}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var summary = Assert.IsType<SummaryEvent>(evt);
        Assert.Equal(12, summary.Add);
        Assert.Equal(3, summary.Modify);
        Assert.Equal(0, summary.MetadataOnly);
        Assert.Equal(15, summary.TotalPlanned);
    }

    [Fact]
    public void Review_event_deserializes_nested_summary()
    {
        var json = """{"type":"review","summary":{"add":12,"modify":3,"metadata_only":2,"remove":1,"unchanged":1260},"no_delete":false}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var review = Assert.IsType<ReviewEvent>(evt);
        Assert.False(review.NoDelete);
        Assert.Equal(12, review.Summary.Add);
        Assert.Equal(2, review.Summary.MetadataOnly);
        Assert.Equal(1, review.Summary.Remove);
        Assert.Equal(1260, review.Summary.Unchanged);
    }

    [Fact]
    public void Prompt_event_deserializes_options()
    {
        var json = """{"type":"prompt","id":7,"message":"Pick","options":["Retry","Skip","Abort"]}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var prompt = Assert.IsType<PromptEvent>(evt);
        Assert.Equal(7UL, prompt.Id);
        Assert.Equal("Pick", prompt.Message);
        Assert.Equal(new[] { "Retry", "Skip", "Abort" }, prompt.Options);
    }

    [Fact]
    public void Form_event_deserializes_label_and_initial()
    {
        var json = """{"type":"form","id":1,"label":"Path?","initial":"","hint":"UNC ok"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var form = Assert.IsType<FormEvent>(evt);
        Assert.Equal(1UL, form.Id);
        Assert.Equal("Path?", form.Label);
        Assert.Equal(string.Empty, form.Initial);
        Assert.Equal("UNC ok", form.Hint);
    }

    [Fact]
    public void TrackStart_event_deserializes_progress()
    {
        var json = """{"type":"track_start","current":2,"total":15,"label":"Aphex Twin - SAW II"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var ts = Assert.IsType<TrackStartEvent>(evt);
        Assert.Equal(2, ts.Current);
        Assert.Equal(15, ts.Total);
        Assert.Equal("Aphex Twin - SAW II", ts.Label);
    }

    [Fact]
    public void TrackDone_event_deserializes_with_type_only()
    {
        var json = """{"type":"track_done"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        Assert.IsType<TrackDoneEvent>(evt);
    }

    [Fact]
    public void TrackDone_event_serializes_with_type_only()
    {
        var json = JsonSerializer.Serialize<IpcEvent>(new TrackDoneEvent());
        Assert.Contains("\"type\":\"track_done\"", json);
    }

    [Fact]
    public void Log_event_deserializes_message()
    {
        var json = """{"type":"log","message":"transcoded in 6.3s"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var log = Assert.IsType<LogEvent>(evt);
        Assert.Equal("transcoded in 6.3s", log.Message);
    }

    [Fact]
    public void Error_event_deserializes_with_recovery_hints()
    {
        var json = """{"type":"error","message":"ffmpeg failed","recovery_hints":["Skip","Retry"]}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var err = Assert.IsType<ErrorEvent>(evt);
        Assert.Equal("ffmpeg failed", err.Message);
        Assert.NotNull(err.RecoveryHints);
        Assert.Equal(2, err.RecoveryHints!.Count);
    }

    [Fact]
    public void Error_event_deserializes_without_recovery_hints()
    {
        // §4.10: recovery_hints is optional / omitted-when-empty.
        var json = """{"type":"error","message":"manifest write failed"}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var err = Assert.IsType<ErrorEvent>(evt);
        Assert.Equal("manifest write failed", err.Message);
        Assert.Null(err.RecoveryHints);
    }

    [Fact]
    public void Finish_event_deserializes_success_flag()
    {
        var json = """{"type":"finish","success":true}""";
        var evt = JsonSerializer.Deserialize<IpcEvent>(json);
        var finish = Assert.IsType<FinishEvent>(evt);
        Assert.True(finish.Success);
    }

    [Fact]
    public void Unknown_event_type_throws_meaningfully()
    {
        // §2 says the UI's stdout reader logs and skips unknown types — the
        // skip happens in CoreProcess, but the JSON layer itself MUST throw
        // so the reader can distinguish a parseable-but-unknown line from a
        // genuinely malformed one. (Both end up logged-and-dropped, but the
        // distinction matters for diagnostics.)
        var json = """{"type":"future_event_we_dont_know","foo":42}""";
        Assert.Throws<JsonException>(() => JsonSerializer.Deserialize<IpcEvent>(json));
    }

    // ---- Commands --------------------------------------------------------

    [Fact]
    public void Review_decision_apply_nested_envelope_serializes_correctly()
    {
        // §5.1: the wire format nests another typed envelope inside `decision`.
        var cmd = new ReviewDecisionCommand(new ApplyDecision(NoDelete: true));
        var json = JsonSerializer.Serialize<IpcCommand>(cmd);

        Assert.Contains("\"type\":\"review_decision\"", json);
        Assert.Contains("\"decision\":", json);
        Assert.Contains("\"type\":\"apply\"", json);
        Assert.Contains("\"no_delete\":true", json);
    }

    [Fact]
    public void Review_decision_dry_run_serializes_with_type_only()
    {
        var cmd = new ReviewDecisionCommand(new DryRunDecision());
        var json = JsonSerializer.Serialize<IpcCommand>(cmd);
        Assert.Contains("\"type\":\"review_decision\"", json);
        Assert.Contains("\"type\":\"dry_run\"", json);
        // DryRun carries no other fields.
        Assert.DoesNotContain("no_delete", json);
    }

    [Fact]
    public void Review_decision_quit_serializes_with_type_only()
    {
        var cmd = new ReviewDecisionCommand(new QuitDecision());
        var json = JsonSerializer.Serialize<IpcCommand>(cmd);
        Assert.Contains("\"type\":\"quit\"", json);
    }

    [Fact]
    public void Prompt_decision_round_trips()
    {
        var cmd = new PromptDecisionCommand(Id: 7, Choice: 2);
        var json = JsonSerializer.Serialize<IpcCommand>(cmd);
        Assert.Contains("\"type\":\"prompt_decision\"", json);
        Assert.Contains("\"id\":7", json);
        Assert.Contains("\"choice\":2", json);

        var back = JsonSerializer.Deserialize<IpcCommand>(json);
        var promptDec = Assert.IsType<PromptDecisionCommand>(back);
        Assert.Equal(7UL, promptDec.Id);
        Assert.Equal(2, promptDec.Choice);
    }

    [Fact]
    public void Form_decision_with_value_serializes()
    {
        var cmd = new FormDecisionCommand(Id: 1, Value: @"\\nas\music\flac");
        var json = JsonSerializer.Serialize<IpcCommand>(cmd);
        Assert.Contains("\"type\":\"form_decision\"", json);
        Assert.Contains("\"id\":1", json);
        // Backslashes get JSON-escaped; verify the raw input survives a round-trip.
        var back = JsonSerializer.Deserialize<IpcCommand>(json);
        var formDec = Assert.IsType<FormDecisionCommand>(back);
        Assert.Equal(@"\\nas\music\flac", formDec.Value);
    }

    [Fact]
    public void Form_decision_with_null_value_serializes()
    {
        // §5.4: null value = user abort. MUST appear literally on the wire.
        var cmd = new FormDecisionCommand(Id: 3, Value: null);
        var json = JsonSerializer.Serialize<IpcCommand>(cmd);
        Assert.Contains("\"value\":null", json);
    }

    [Fact]
    public void Cancel_command_serializes_with_type_only()
    {
        var json = JsonSerializer.Serialize<IpcCommand>(new CancelCommand());
        Assert.Contains("\"type\":\"cancel\"", json);
    }

    [Fact]
    public void Start_command_serializes_with_type_only()
    {
        var json = JsonSerializer.Serialize<IpcCommand>(new StartCommand());
        Assert.Contains("\"type\":\"start\"", json);
    }

    [Fact]
    public void Decide_prompt_daemon_command_round_trips()
    {
        // Distinct from PromptDecisionCommand (above) which is the
        // M1 subprocess-stdio command. DecidePromptCommand is the
        // daemon-IPC command sent by the UI to the daemon, which in
        // turn forwards a PromptDecisionCommand to the subprocess
        // stdin. They share field names + values but live on
        // different transports.
        var cmd = new DecidePromptCommand(Id: 17, Choice: 1);
        var json = JsonSerializer.Serialize<DaemonCommand>(cmd);
        Assert.Contains("\"type\":\"decide_prompt\"", json);
        Assert.Contains("\"id\":17", json);
        Assert.Contains("\"choice\":1", json);

        var back = JsonSerializer.Deserialize<DaemonCommand>(json);
        var decide = Assert.IsType<DecidePromptCommand>(back);
        Assert.Equal(17UL, decide.Id);
        Assert.Equal(1, decide.Choice);
    }
}
