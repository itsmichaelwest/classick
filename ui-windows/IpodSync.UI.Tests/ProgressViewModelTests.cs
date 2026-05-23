using IpodSync_UI.Ipc;
using IpodSync_UI.ViewModels;

namespace IpodSync_UI.Tests;

/// <summary>
/// Pure-VM tests for <see cref="ProgressViewModel"/>. No UI thread, no
/// CoreProcess subprocess — tests just exercise the <c>Apply*</c> methods
/// the host would call after marshaling an IPC event onto the UI thread.
/// </summary>
public class ProgressViewModelTests
{
    [Fact]
    public void Summary_sets_total_and_resets_done()
    {
        var vm = new ProgressViewModel();
        vm.ApplySummary(new SummaryEvent(Add: 5, Modify: 3, MetadataOnly: 0, Remove: 2, Unchanged: 100, TotalPlanned: 10));
        Assert.Equal(10, vm.TotalPlanned);
        Assert.Equal(0, vm.Done);
        Assert.Equal(0, vm.PercentComplete);
    }

    [Fact]
    public void TrackDone_increments_progress()
    {
        var vm = new ProgressViewModel();
        vm.ApplySummary(new SummaryEvent(0, 0, 0, 0, 0, 10));
        vm.ApplyTrackDone();
        vm.ApplyTrackDone();
        Assert.Equal(2, vm.Done);
        Assert.Equal(20.0, vm.PercentComplete);
    }

    [Fact]
    public void TrackStart_sets_current_label()
    {
        var vm = new ProgressViewModel();
        vm.ApplyTrackStart(new TrackStartEvent(Current: 1, Total: 10, Label: "Album\\Track.flac"));
        Assert.Equal("Album\\Track.flac", vm.CurrentTrackLabel);
        Assert.Equal(1, vm.CurrentTrackIndex);
        Assert.Equal(10, vm.CurrentTrackTotal);
    }

    [Fact]
    public void Log_appended_to_tail()
    {
        var vm = new ProgressViewModel();
        vm.ApplyLog(new LogEvent("Wrote 12 tracks"));
        Assert.Single(vm.LogTail);
        Assert.Equal(LogLevel.Info, vm.LogTail[0].Level);
        Assert.Equal("Wrote 12 tracks", vm.LogTail[0].Message);
    }

    [Fact]
    public void Error_appended_as_error_level()
    {
        var vm = new ProgressViewModel();
        vm.ApplyError(new ErrorEvent("ffmpeg failed"));
        Assert.Single(vm.LogTail);
        Assert.Equal(LogLevel.Error, vm.LogTail[0].Level);
        Assert.Equal("ffmpeg failed", vm.LogTail[0].Message);
    }

    [Fact]
    public void Log_tail_capped_at_200()
    {
        var vm = new ProgressViewModel();
        for (int i = 0; i < 250; i++)
        {
            vm.ApplyLog(new LogEvent($"line {i}"));
        }
        Assert.Equal(200, vm.LogTail.Count);
        // Oldest 50 dropped: first surviving entry is "line 50", last is "line 249".
        Assert.Equal("line 50", vm.LogTail[0].Message);
        Assert.Equal("line 249", vm.LogTail[199].Message);
    }

    [Fact]
    public void Finish_success_snaps_to_complete()
    {
        var vm = new ProgressViewModel();
        vm.ApplySummary(new SummaryEvent(0, 0, 0, 0, 0, 10));
        vm.ApplyTrackDone();
        vm.ApplyFinish(new FinishEvent(Success: true));
        Assert.True(vm.IsFinished);
        Assert.True(vm.FinishedSuccessfully);
        Assert.Equal(10, vm.Done);
        Assert.Contains("Eject", vm.FinishMessage);
    }

    [Fact]
    public void Finish_failure_keeps_partial_progress_and_warns()
    {
        var vm = new ProgressViewModel();
        vm.ApplySummary(new SummaryEvent(0, 0, 0, 0, 0, 10));
        vm.ApplyTrackDone();
        vm.ApplyTrackDone();
        vm.ApplyFinish(new FinishEvent(Success: false));
        Assert.True(vm.IsFinished);
        Assert.False(vm.FinishedSuccessfully);
        Assert.Equal(2, vm.Done);
        Assert.Contains("rebuild-manifest", vm.FinishMessage);
    }

    [Fact]
    public void IsBusy_true_only_while_in_progress()
    {
        var vm = new ProgressViewModel();
        Assert.False(vm.IsBusy);
        vm.ApplySummary(new SummaryEvent(0, 0, 0, 0, 0, 10));
        Assert.True(vm.IsBusy);
        vm.ApplyFinish(new FinishEvent(true));
        Assert.False(vm.IsBusy);
    }
}
