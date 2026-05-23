# ipod-sync — Windows UI

Native WinUI 3 / .NET 10 frontend for ipod-sync. Drives the existing Rust core
(`ipod-sync.exe --ipc-mode`) over newline-delimited JSON on stdin/stdout. The
IPC protocol is documented at `..\docs\ipc-protocol.md` (added in Phase 6 M1
Task 1, may be in progress concurrently with this bootstrap).

> **Status:** M1 bootstrap — blank window only. The Start-sync button,
> CoreProcess IPC client, ReviewViewModel, ProgressViewModel, and end-to-end
> sync flow land in subsequent M1 waves (Tasks 5–8).

## Prerequisites

- Windows 10 build 17763 (1809) or later, or Windows 11.
- .NET 10 SDK (10.0.300+). `winget install Microsoft.DotNet.SDK.10`.
- Visual Studio 2022 17.10+ or Visual Studio 2026, with the **C# Managed
  Desktop** workload (provides the Windows App SDK and the
  `dotnet new winui-mvvm` template).
- Windows App SDK 2.1.x runtime (NuGet `Microsoft.WindowsAppSDK` is restored
  automatically; the project uses `Microsoft.Windows.SDK.BuildTools.WinApp` to
  register a debug package identity on `dotnet run`).

## Build

```powershell
dotnet build IpodSync.UI.slnx -c Debug
# or
dotnet build IpodSync.UI.slnx -c Release
```

`.slnx` is the .NET 10 XML solution format. Open it in Visual Studio 2026 or
build from the command line as above.

## Run

```powershell
dotnet run --project IpodSync.UI\IpodSync.UI.csproj
```

Or open `IpodSync.UI.slnx` in Visual Studio and press **F5**.

The project ships with the packaged WinUI configuration that the `winui-mvvm`
template emits: `Microsoft.Windows.SDK.BuildTools.WinApp` registers a debug
package identity automatically so `dotnet run` works without `winappsdk` CLI
gymnastics. MSIX signing and a real installer are deferred to M4 (see
`..\docs\superpowers\specs\2026-05-24-phase-6-winui-app.md`).

## How the UI finds the Rust core

The UI process spawns `ipod-sync.exe --ipc-mode` as a child process. The
`CoreLocator` service (landing in M1 Task 8) will probe, in order:

1. **Sibling to `IpodSync.UI.exe`** — the production install layout: the two
   executables live in the same directory.
2. **`..\..\target\release\ipod-sync.exe`** — for the dev loop when running
   from `IpodSync.UI\bin\<config>\net10.0-windows...\`.
3. **`..\..\target\debug\ipod-sync.exe`** — debug builds.
4. **`PATH`** — last-resort fallback.

For the dev loop, build the Rust core first from the repo root:

```powershell
# From F:\repos\ipod-sync\
cargo build --release
```

The IPC contract is in `..\docs\ipc-protocol.md`. The Rust `--ipc-mode` flag
and `IpcBackend` land in M1 Tasks 2 and 3.

## Project layout

```
ui-windows\
├── IpodSync.UI.slnx                  Visual Studio solution (.slnx XML format)
├── README.md                         (this file)
├── IpodSync.UI\                      Main WinUI 3 app project
│   ├── IpodSync.UI.csproj            .NET 10 win10.0.26100, WinUI 3,
│   │                                 CommunityToolkit.Mvvm. References Core.
│   ├── App.xaml / App.xaml.cs        WinUI app entry; exposes App.Window,
│   │                                 App.DispatcherQueue, App.WindowHandle
│   ├── MainWindow.xaml / .cs         Frame host + Mica backdrop + TitleBar
│   ├── MainPage.xaml / .cs           Placeholder content for M1 bootstrap
│   ├── ViewModels\
│   │   └── MainPageViewModel.cs      Template-supplied counter VM (unused;
│   │                                 will be replaced by ReviewViewModel /
│   │                                 ProgressViewModel in M1 Tasks 6 + 7)
│   ├── Properties\                   Publish profiles (template defaults)
│   ├── Assets\                       App icons (template defaults)
│   ├── Package.appxmanifest          MSIX manifest (for packaged builds /
│   │                                 dotnet run debug identity registration)
│   └── app.manifest                  Win32 manifest (per-monitor DPI, etc.)
├── IpodSync.UI.Core\                 Pure System.* class library (no WinUI)
│   ├── IpodSync.UI.Core.csproj       net10.0; RootNamespace=IpodSync_UI
│   └── Ipc\                          IPC wire types + CoreProcess child-process
│       ├── IpcEvent.cs               Polymorphic record hierarchy for events
│       ├── IpcCommand.cs             Polymorphic record hierarchy for commands
│       └── CoreProcess.cs            Spawn ipod-sync.exe --ipc-mode, stream
│                                     events via System.Threading.Channels.
└── IpodSync.UI.Tests\                xUnit test project
    ├── IpodSync.UI.Tests.csproj      net10.0; references Core (NOT the
    │                                 WinUI app — the WindowsAppRuntime
    │                                 module init blows up in vstest hosts).
    └── IpcWireFormatTests.cs         JSON round-trip tests for the IPC
                                      protocol per docs/ipc-protocol.md.
```

### Why a separate `IpodSync.UI.Core` project?

The WinUI 3 app project drags in `Microsoft.WindowsAppSDK`, which injects a
`[ModuleInitializer]` that calls `DeploymentManager.Initialize()` the first
time the assembly is loaded. That works for the packaged app but throws
`REGDB_E_CLASSNOTREG` inside a vstest worker (no package identity).

The IPC plumbing is intentionally pure `System.*` code, so isolating it in a
plain-net10.0 class library lets both the WinUI app and the test project
reference it cleanly, with no module-init pain.

## Test

```powershell
dotnet test IpodSync.UI.Tests\IpodSync.UI.Tests.csproj
```

22 tests cover the IPC wire format: discriminator round-trips for every event
type, the nested `decision` envelope on `review_decision`, null/typed-only
payloads, and a guard that unknown event types raise a `JsonException`
(forward-compat per `docs/ipc-protocol.md` §2).

## Notes for subsequent M1 tasks

- **Namespace is `IpodSync_UI` (underscore), not `IpodSync.UI`.** C# project
  names with a dot end up with an underscored root namespace by default; the
  template baked this in via `<RootNamespace>IpodSync_UI</RootNamespace>`. New
  code should use `namespace IpodSync_UI;` (and `IpodSync_UI.Services`,
  `IpodSync_UI.Models`, etc.).
- **Packaged-by-default deviation from the spec.** The spec called for
  unpackaged (`<WindowsPackageType>None</WindowsPackageType>`), but the .NET 10
  `winui-mvvm` template emits a packaged configuration with the
  `Microsoft.Windows.SDK.BuildTools.WinApp` helper to keep `dotnet run`
  ergonomic. The trade-off favours the template defaults: less migration work
  for the M4 MSIX milestone, and `dotnet run` already works.
- **Frame-based navigation.** `MainWindow.xaml` hosts a `<Frame>` that
  navigates to `MainPage` on startup. Subsequent pages
  (Review / Progress / Wizard / Config / Library) should be `Page` subclasses
  reached via `RootFrame.Navigate(typeof(...))` from a router in MainWindow or
  a navigation service.
- **Mica + extended title bar already wired.** `MainWindow.xaml.cs` sets
  `ExtendsContentIntoTitleBar = true` and binds `AppTitleBar` via
  `SetTitleBar(...)`. Don't recreate this in pages.
