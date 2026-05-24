namespace IpodSync_UI.Core;

/// <summary>
/// Project identifier shared by every UI component that names a directory,
/// pipe, or toast title. Mirrors the Rust crate's <c>PROJECT_DIR</c> constant
/// (<c>src/lib.rs</c>). Both MUST stay in sync — the named-pipe label is the
/// IPC contract between the daemon and the UI. See findings F-02 for the
/// rationale.
/// </summary>
public static class AppIdentity
{
    /// <summary>kebab-case identifier used for AppData, LocalAppData, the
    /// named pipe, and toast titles.</summary>
    public const string Name = "ipod-sync";
}
