#if DEBUG
using IpodSync_UI.Ipc;

namespace IpodSync_UI.Views;

/// <summary>
/// Canned <see cref="PromptEvent"/> fixtures for iterating on the
/// popover's prompt-overlay XAML in debug builds. Each scenario
/// stresses a different layout dimension (message length, option
/// count, option label width) so a single hot-reload session can
/// cycle through realistic shapes without driving a real sync into
/// a real prompt.
///
/// Triggered from <c>PopoverWindow.xaml.cs</c> via Ctrl+Shift+1/2/3
/// (and Ctrl+Shift+0 to clear). Compiled out of release builds.
/// </summary>
internal static class DebugPromptScenarios
{
    /// <summary>Shortest plausible shape — one sentence + 2 options.
    /// Use this to verify the overlay doesn't pad / overflow when
    /// the message is tiny.</summary>
    public static readonly PromptEvent Short = new(
        Id: 0,
        Message: "Track add failed. Try again?",
        Options: new[] { "Retry", "Skip" });

    /// <summary>Realistic source-change-safeguard shape (the actual
    /// prompt that surfaced today when we first hit this bug). Multi-
    /// paragraph message with three options of varying width.</summary>
    public static readonly PromptEvent SourceChange = new(
        Id: 0,
        Message:
            "Source root has changed since the last sync.\n\n" +
            "Previous: C:\\Users\\Michael\\TestMusic\n" +
            "Current : \\\\HOST\\data\\media\\music\n\n" +
            "The current diff would REMOVE 1 track(s) (everything in the manifest " +
            "that's not in the new source).\n\n" +
            "If this was intentional, choose Continue. If you typo'd --source or " +
            "are pointing at a different library, choose Abort. If you want to add " +
            "new tracks from the new source without touching the iPod's existing " +
            "tracks, choose --no-delete mode.",
        Options: new[]
        {
            "Continue (apply Remove + Add normally)",
            "Use --no-delete for this run",
            "Abort",
        });

    /// <summary>Long-form retry shape with an embedded error. Pushes
    /// the scroll-viewer past one viewport so the overlay's scroll
    /// behavior gets exercised.</summary>
    public static readonly PromptEvent RetryOnFailure = new(
        Id: 0,
        Message:
            "Failed to add 03 - Mahler - Symphony No. 5 - Adagietto (Sehr langsam).flac:\n" +
            "  refalac transcode for \\\\HOST\\data\\media\\music\\Mahler\\Symphony No. 5\\03.flac\n" +
            "  Caused by: ffmpeg decode stage failed: pipe closed unexpectedly after 0 bytes\n" +
            "  Caused by: 'C:\\Program Files\\ffmpeg\\bin\\ffmpeg.exe' exited with status 1\n\n" +
            "This usually means the source file is corrupt or unreadable. You can " +
            "retry now (sometimes a transient network share hiccup resolves itself), " +
            "skip the track and continue the rest of the sync, or abort entirely so " +
            "the iPod isn't left in a partial state.\n\nChoose:",
        Options: new[] { "Retry", "Skip this track", "Abort" });
}
#endif
