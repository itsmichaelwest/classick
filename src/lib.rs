/// Project identifier used as the per-user data directory name
/// (`%APPDATA%\ipod-sync\`, `%LOCALAPPDATA%\ipod-sync\logs\`), the named-pipe
/// label (`\\.\pipe\ipod-sync`), the temp-dir subdirectory, and the user-facing
/// "name" in wizard prompts and toast titles. The .NET side mirrors this in
/// `IpodSync.UI.Core.AppIdentity`; the two MUST stay in sync (named-pipe
/// label is the IPC contract). See findings F-02 for the rationale.
pub const PROJECT_DIR: &str = "ipod-sync";

pub mod apply_loop;
pub mod cli;
pub mod config;
pub mod config_file;
pub mod daemon;
pub mod ffi;
pub mod ipc;
pub mod ipc_daemon;
pub mod ipod;
pub mod logging;
pub mod manifest;
pub mod orchestrator;
pub mod preflight;
pub mod progress;
pub mod source;
pub mod tags;
pub mod transcode;
pub mod try_with_prompt;
pub mod wizard;
