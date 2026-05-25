# ipod-sync — Windows UI

Native WinUI 3 / .NET 10 tray app for ipod-sync. Runs in the system tray, talks
to the long-lived `ipod-sync.exe --daemon` over a named pipe, and surfaces
device state, sync progress, settings, and a first-run wizard. The daemon in
turn spawns `ipod-sync.exe --ipc-mode --apply` subprocesses to do the actual
sync work — see `..\..\docs\ipc-protocol.md` for the JSON wire format.

For repo-wide context (Rust core, layout, conventions), see `..\..\AGENTS.md`.

## Prerequisites

- Windows 10 build 17763 (1809) or later, or Windows 11.
- .NET 10 SDK (10.0.300+). `winget install Microsoft.DotNet.SDK.10`.
- Windows App SDK 2.1.x runtime (restored automatically via the
  `Microsoft.WindowsAppSDK` NuGet package).
- Visual Studio 2022 17.10+ or Visual Studio 2026 with the **C# Managed
  Desktop** workload is optional but ergonomic — the command-line flow below
  works without it.

## Build

```powershell
dotnet build IpodSync.UI.slnx -c Debug
# or
dotnet build IpodSync.UI.slnx -c Release
```

`.slnx` is the .NET 10 XML solution format. The solution targets `x64` and
`ARM64` only — `AnyCPU` is intentionally absent because WinUI 3 needs an
explicit RID for the WinAppSDK runtime.

The UI csproj bundles `..\..\..\target\release\ipod-sync.exe` plus the libgpod
runtime DLLs as content, so they're copied next to `IpodSync.UI.exe` at build
time and shipped inside the MSIX. Build the Rust core first or expect a
"core not found" dialog at startup:

```powershell
# From F:\repos\ipod-sync\
cargo build --release
```

A `WarnIfCoreMissing` MSBuild target emits a friendly warning rather than
failing the build when the core binary is absent.

## Run

```powershell
dotnet run --project IpodSync.UI\IpodSync.UI.csproj
```

The `Microsoft.Windows.SDK.BuildTools.WinApp` package handles debug-identity
registration automatically — no manual `winappsdk` CLI steps. F5 in Visual
Studio does the same thing.

## Test

```powershell
dotnet test IpodSync.UI.Tests\IpodSync.UI.Tests.csproj
```

xUnit on plain `net10.0`. Covers IPC wire-format round trips, daemon client +
event router behaviour, notification-service decisions, and the four user-
visible view models (Popover, Review, Wizard, NotificationDecision). Tests are
plain CLR — see the **WinAppSDK module-init hazard** note below.

## How the UI finds the daemon

On startup, `App.xaml.cs` calls `IsDaemonRunningAsync()`; if no daemon is
listening on the named pipe, `SpawnDaemon()` looks for `ipod-sync.exe` next to
`IpodSync.UI.exe` or one directory up, then launches it with `--daemon`. The
csproj copies the Rust core (and its libgpod runtime DLLs) into the UI's
output directory at build time, so the dev loop and the packaged install both
satisfy the "sibling to UI exe" probe.

The named-pipe label is `\\.\pipe\ipod-sync`, set on the Rust side by
`PIPE_NAME` in `crates/ipod-sync/src/daemon/ipc_server.rs` and mirrored on
the UI side by `IpodSync.UI.Core.AppIdentity`. **These two MUST stay in
sync** — the pipe label is the IPC contract.

## Project layout

```
ui\windows\
├── IpodSync.UI.slnx                  .NET 10 XML solution; x64 + ARM64
├── README.md                         (this file)
│
├── IpodSync.UI\                      Main WinUI 3 tray app
│   ├── App.xaml / App.xaml.cs        Entry point; owns tray + popover + daemon
│   │                                 lifecycle + IPC routing
│   ├── TrayIconController.cs         H.NotifyIcon-based tray (icon, tooltip,
│   │                                 left-click popover toggle, theme variants)
│   ├── Notifications\                Toast notification service + decision
│   │                                 model (sync-failed alerts, etc.)
│   ├── Converters\                   XAML value converters
│   ├── ViewModels\
│   │   ├── PopoverViewModel.cs       Tray popover state (storage, sync caption,
│   │   │                             ETA, prompt overlay)
│   │   ├── ReviewViewModel.cs        Action-plan review dialog
│   │   ├── WizardViewModel.cs        First-run wizard (device → folder →
│   │   │                             sync settings → done)
│   │   ├── SettingsViewModel.cs      Settings window VMs (General / History /
│   │   │                             Notifications)
│   │   └── HistoryEntryViewModel.cs  Shared sync-history row VM
│   ├── Views\
│   │   ├── PopoverWindow.xaml        Always-available tray popover
│   │   ├── ReviewPage.xaml           Action-plan confirmation
│   │   ├── WizardWindow.xaml + Wizard*Page.xaml
│   │   ├── SettingsWindow.xaml + Settings*Page.xaml
│   │   ├── WindowAnchor.cs           Helpers for popover-near-tray positioning
│   │   └── DebugPromptScenarios.cs   Manual-test harness for prompt overlays
│   ├── Assets\                       App icons, tray icons (state × theme ×
│   │                                 size), in-app SVGs
│   ├── Package.appxmanifest          MSIX manifest (debug identity + future
│   │                                 packaged build)
│   └── app.manifest                  Win32 manifest (per-monitor DPI, etc.)
│
├── IpodSync.UI.Core\                 Pure System.* class library (no WinUI)
│   ├── AppIdentity.cs                Pipe name + project-dir constants
│   │                                 mirrored from Rust src/lib.rs
│   ├── Diag.cs                       Lightweight diagnostic logging
│   └── Ipc\
│       ├── DaemonClient.cs           Named-pipe connect + line reader/writer
│       ├── DaemonCommand.cs          Outbound command record hierarchy
│       ├── DaemonEvent.cs            Inbound event record hierarchy
│       ├── DaemonEventRouter.cs      Multiplex events to subscribers
│       ├── IpcCommand.cs             Sync-subprocess command records
│       │                             (per docs/ipc-protocol.md)
│       └── IpcEvent.cs               Sync-subprocess event records
│                                     (per docs/ipc-protocol.md)
│
└── IpodSync.UI.Tests\                xUnit on plain net10.0
    ├── IpcWireFormatTests.cs         JSON round-trips for IPC types
    ├── DaemonClientTests.cs
    ├── DaemonEventRouterTests.cs
    ├── NotificationServiceTests.cs
    ├── PopoverViewModelTests.cs
    ├── ReviewViewModelTests.cs
    └── WizardViewModelTests.cs
```

### Why a separate `IpodSync.UI.Core` project — the WinAppSDK module-init hazard

The WinUI 3 app project drags in `Microsoft.WindowsAppSDK`, which injects a
`[ModuleInitializer]` that calls `DeploymentManager.Initialize()` the first
time the assembly is loaded. That works for the packaged app but throws
`REGDB_E_CLASSNOTREG` inside a vstest worker (no package identity).

`IpodSync.UI.Core` is intentionally pure `System.*` code so both the WinUI app
and the test project can reference it cleanly. The test project goes one
further: it **link-compiles** select VM source files (see `<Compile Include=`
in the csproj) rather than project-referencing the WinUI app — the source is
still single-truth in `IpodSync.UI\ViewModels\`, but the test process never
touches the WinAppSDK module initializer.

## Conventions and gotchas

- **Namespace is `IpodSync_UI` (underscore), not `IpodSync.UI`.** The csproj
  pins `<RootNamespace>IpodSync_UI</RootNamespace>`. New files should use
  `namespace IpodSync_UI;` (and `IpodSync_UI.Services`, `IpodSync_UI.Views`,
  etc.).
- **UI thread contract.** IPC events arrive on a background channel-reader
  thread. Every `Apply*` on a view model touches observable state and must be
  marshalled through `App.DispatcherQueue.TryEnqueue(...)` first. Calling on a
  worker thread throws `COMException` RPC_E_WRONG_THREAD or silently corrupts
  collection-changed notifications.
- **`MVVMTK0045` warnings are noise.** CommunityToolkit.Mvvm's
  `[ObservableProperty]` source generator emits this for `field` syntax — not
  AOT-clean for WinRT marshalling but harmless for our JIT scenarios. Migration
  to partial properties is a chore for another day.
- **`PRI249` resource warnings are noise.** WinAppSDK PRI tooling complains
  about empty qualifier strings on some assets — cosmetic only.
- **No `AnyCPU`.** Build x64 or ARM64 explicitly. WinUI 3 needs the RID.
- **Packaged-by-default.** Deviates from the original spec (`<WindowsPackageType>None</WindowsPackageType>`),
  but the `winui-mvvm` template's packaged + debug-identity setup is what
  keeps `dotnet run` ergonomic; staying with it makes the future MSIX
  milestone smaller. Trade documented in
  `..\..\docs\superpowers\specs\2026-05-24-phase-6-winui-app.md`.
- **Live UI changes need a real device.** Most VM logic is exercised by unit
  tests, but window positioning, tray theming, popover anchoring, and toast
  delivery only show their real behaviour against a running daemon with an
  iPod connected. The `DebugPromptScenarios` harness covers prompt overlays
  without needing the apply loop.
