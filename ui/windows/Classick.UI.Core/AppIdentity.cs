namespace Classick_UI.Core;

/// <summary>
/// Project identifier shared by every UI component that names a directory,
/// pipe, or toast title. Mirrors the Rust crate's <c>PROJECT_DIR</c> constant
/// (<c>src/lib.rs</c>). Both MUST stay in sync — the named-pipe label is the
/// IPC contract between the daemon and the UI. See findings F-02 for the
/// rationale.
/// </summary>
public static class AppIdentity
{
    /// <summary>Lowercase identifier used for AppData, LocalAppData, the
    /// named pipe, and on-disk paths. Mirrors Rust's <c>PROJECT_DIR</c>.</summary>
    public const string Name = "classick";

    /// <summary>User-facing brand name. Used in toast titles, window
    /// titles, wizard copy, and anywhere the product is named to a human.
    /// Mirrors Rust's <c>DISPLAY_NAME</c>.</summary>
    public const string DisplayName = "Classick";
}
