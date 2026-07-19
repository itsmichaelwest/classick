using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
using Classick_UI.Ipc;
using Classick_UI.Notifications;
using Classick_UI.ViewModels;
using Classick_UI.Views;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace Classick_UI;

public partial class App : Application
{
    public static Window? Window { get; private set; }
    public static IntPtr WindowHandle { get; private set; }
    public static DispatcherQueue DispatcherQueue { get; private set; } = default!;
    public static DaemonClient? Daemon { get; private set; }
    public static DaemonEventRouter? Router { get; private set; }
    public static TrayIconController? Tray { get; private set; }
    public static NotificationService? Notifications { get; private set; }

    /// <summary>Last ConfigUpdate seen from the daemon. Popover + settings read from this.</summary>
    public static ConfigUpdateEvent? LatestConfig { get; private set; }
    public static string? ConfiguredSerial => LatestConfig?.Ipod?.Serial;
    /// <summary>Latest StatusUpdate. Used to drive popover initial state.</summary>
    public static StatusUpdateEvent? LatestStatus { get; private set; }
    /// <summary>Latest HistoryUpdate. Used to seed popover activity feed.</summary>
    public static HistoryUpdateEvent? LatestHistory { get; private set; }

    private static readonly PopoverViewModel _popoverState = new();
    private static DeviceInventorySnapshotEvent? _latestInventory;
    private static PopoverWindow? _popover;
    private static SettingsWindow? _settings;

    /// <summary>Monotonic timestamp (UTC ticks) of the most recent
    /// popover close, used to debounce the tray-icon toggle path. The
    /// OS fires PopoverRequested on every tray-icon click; if the
    /// click also caused the open popover to lose focus, the focus-
    /// loss handler closes the window BEFORE PopoverRequested fires
    /// (or vice versa, depending on input timing). Without this
    /// debounce, the tray click would close-then-immediately-reopen
    /// and look like the click did nothing. Window matches typical
    /// double-click latency; tuned for "intentional re-click reopens
    /// quickly, accidental re-click after dismiss is swallowed".</summary>
    private static long _popoverClosedAtTicks;
    private static readonly long PopoverToggleDebounceTicks =
        TimeSpan.FromMilliseconds(300).Ticks;

    /// <summary>HWND of the currently-open settings window, or zero if
    /// closed. Used by the General page to anchor the folder picker
    /// (InitializeWithWindow needs the owning HWND on WinUI 3).</summary>
    public static IntPtr SettingsWindowHandle { get; private set; }

    public App() { InitializeComponent(); }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        DispatcherQueue = DispatcherQueue.GetForCurrentThread();

        Tray = new TrayIconController();
        Tray.Initialize();
        Tray.QuitRequested += OnQuitRequested;
        Tray.SyncNowRequested += OnSyncNowRequested;
        Tray.SettingsRequested += OnSettingsRequested;
        Tray.PopoverRequested += OnPopoverRequested;

        if (!await IsDaemonRunningAsync())
        {
            SpawnDaemon();
            // No pre-sleep — DaemonClient.ConnectAsync already has its own
            // backoff loop (1s, 2s, 4s) that absorbs daemon startup latency.
        }

        try { Daemon = await DaemonClient.ConnectAsync(); }
        catch (Exception e)
        {
            Debug.WriteLine($"app: failed to connect to daemon: {e}");
            Tray?.Dispose();
            Environment.Exit(0);
            return;
        }

        // Start the router. All consumers (tray, popover, notifications,
        // wizard) subscribe through it instead of reading the channel
        // directly.
        Router = new DaemonEventRouter(Daemon.Events);
        Router.StatusUpdated += OnStatusUpdated;
        Router.ConfigUpdated += OnConfigUpdated;
        Router.HistoryUpdated += OnHistoryUpdated;
        Router.DeviceConnected += OnDeviceConnected;
        Router.DeviceDisconnected += OnDeviceDisconnected;
        Router.DeviceInventorySnapshotReceived += OnDeviceInventorySnapshot;
        Router.SourceAvailabilityUpdated += OnSourceAvailabilityUpdated;
        Router.SyncEventReceived += OnSyncEvent;
        Router.Start();

        // Notification service subscribes to router internally.
        Notifications = new NotificationService(Router,
            getNotifyOn: () => LatestConfig?.Daemon?.NotifyOn ?? "all");
        Notifications.Initialize();

        // Subscribe a one-shot TCS for the daemon's ConfigUpdate event BEFORE
        // sending GetConfig — guarantees we observe the reply even if it
        // arrives before SendAsync returns. The 2s cap is a defensive ceiling,
        // not the primary signal.
        var configReceived = new TaskCompletionSource<ConfigUpdateEvent>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        void OneShotConfig(ConfigUpdateEvent c)
        {
            Router!.ConfigUpdated -= OneShotConfig;
            configReceived.TrySetResult(c);
        }
        Router.ConfigUpdated += OneShotConfig;

        // Ask for the initial config + status + history.
        await Daemon.SendAsync(new GetConfigCommand(Guid.NewGuid().ToString("N")));
        await Daemon.SendAsync(new GetStatusCommand(Guid.NewGuid().ToString("N")));
        await Daemon.SendAsync(new GetHistoryCommand(Limit: 10, RequestId: Guid.NewGuid().ToString("N")));

        // Wait for the actual ConfigUpdate, capped at 2s so a dead daemon
        // doesn't wedge startup forever. Either outcome: we make the wizard
        // decision below using whatever LatestConfig holds.
        try { await configReceived.Task.WaitAsync(TimeSpan.FromSeconds(2)); }
        catch (TimeoutException) { Router.ConfigUpdated -= OneShotConfig; }

        // Open wizard if config has no iPod identity. The wizard also
        // subscribes to the router (T14) so the channel-exclusivity
        // hack from M3 goes away.
        if (LatestConfig?.Ipod is null)
        {
            ShowWizard();
        }
        else
        {
            // Paired iPod present → reveal the tray (XAML starts it
            // hidden so the user doesn't see it flash before this
            // decision lands).
            UpdateTrayVisibility();
        }
    }

    private void ShowWizard() => ShowWizardStatic();

    /// <summary>True while the pair wizard owns the user's attention.
    /// The tray icon is hidden and popover requests are no-ops in
    /// this state — the wizard is the only legitimate surface until
    /// an iPod identity has been committed.</summary>
    private static bool _wizardActive;

    /// <summary>Tray is visible iff an iPod is paired AND the wizard
    /// isn't currently in front. Idempotent — safe to call from
    /// every code path that flips either signal.</summary>
    private static void UpdateTrayVisibility()
    {
        Tray?.SetVisible(!_wizardActive && LatestConfig?.Ipod is not null);
    }

    private void OnStatusUpdated(StatusUpdateEvent s)
    {
        LatestStatus = s;
        DispatcherQueue.TryEnqueue(() =>
        {
            UpdateTrayFromStatus(s);
            if (_latestInventory is null && _popoverState.ActiveSyncContext is null)
            {
                _popoverState.Update(s);
            }
        });
    }

    private void OnConfigUpdated(ConfigUpdateEvent c)
    {
        LatestConfig = c;
        // Config updates carry the latest friendly iPod name (the
        // daemon writes it after reading iTunesDB on plug-in). Push
        // it into an open popover so the label flips from model →
        // friendly name without needing the user to reopen the flyout.
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_popoverState.ActiveSyncContext is null)
            {
                _popoverState.SetDeviceLabel(c.Ipod?.Name, c.Ipod?.ModelLabel);
            }
            // Pair / forget transitions flip the tray's visibility
            // automatically — no separate notification needed.
            UpdateTrayVisibility();
        });
    }
    private void OnHistoryUpdated(HistoryUpdateEvent h)
    {
        LatestHistory = h;
        DispatcherQueue.TryEnqueue(() => _popoverState.ApplyHistory(h));
    }

    private void OnSourceAvailabilityUpdated(SourceAvailabilityEvent availability)
    {
        DispatcherQueue.TryEnqueue(
            () => _popoverState.ApplySourceAvailability(availability));
    }
    private void OnDeviceConnected(DeviceConnectedEvent dc)
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            Tray?.SetState(TrayState.Idle, $"iPod connected ({dc.Name ?? dc.ModelLabel})");
            if (_latestInventory is null &&
                _popoverState.ActiveSyncContext is null &&
                string.Equals(dc.Serial, ConfiguredSerial, StringComparison.OrdinalIgnoreCase))
            {
                _popoverState.SetDeviceLabel(dc.Name, dc.ModelLabel);
            }
        });
    }
    private void OnDeviceDisconnected(DeviceDisconnectedEvent disconnected)
    {
        if (!string.Equals(disconnected.Serial, ConfiguredSerial, StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        DispatcherQueue.TryEnqueue(() => Tray?.SetState(TrayState.Offline, "iPod not connected"));
    }

    private void UpdateTrayFromStatus(StatusUpdateEvent s)
    {
        if (Tray is null) return;
        var (state, tooltip) = (s.State, s.IpodConnected) switch
        {
            ("syncing", _) => (TrayState.Syncing, "Syncing iPod…"),
            (_, true) => (TrayState.Idle, "iPod connected · idle"),
            _ => (TrayState.Offline, "iPod not connected"),
        };
        Tray.SetState(state, tooltip);
    }

    private void OnDeviceInventorySnapshot(DeviceInventorySnapshotEvent snapshot)
    {
        _latestInventory = snapshot;
        DispatcherQueue.TryEnqueue(() =>
        {
            var activeDevices = snapshot.Devices
                .Where(device => device.SessionId is not null)
                .ToArray();
            var focused = activeDevices.FirstOrDefault(device =>
                _popoverState.ActiveSyncContext is { } current &&
                string.Equals(
                    device.Identity.Serial,
                    current.Serial,
                    StringComparison.OrdinalIgnoreCase) &&
                device.SessionId == current.SessionId);
            if (focused is null && activeDevices.Length == 1)
            {
                focused = activeDevices[0];
            }

            if (focused?.SessionId is { } sessionId)
            {
                _popoverState.SetActiveSyncSession(
                    new SyncEventContext(sessionId, focused.Identity.Serial));
                _popoverState.Update(focused);
                return;
            }

            if (activeDevices.Length == 0)
            {
                var previousSerial = _popoverState.ActiveSyncContext?.Serial;
                _popoverState.ClearActiveSyncSession();
                var pausedDevices = snapshot.Devices
                    .Where(device => device.Connected && device.Phase == "paused")
                    .ToArray();
                var destination = snapshot.Devices.FirstOrDefault(device =>
                    string.Equals(
                        device.Identity.Serial,
                        previousSerial,
                        StringComparison.OrdinalIgnoreCase));
                destination ??= pausedDevices.Length == 1
                    ? pausedDevices[0]
                    : snapshot.Devices.FirstOrDefault(device =>
                    string.Equals(
                        device.Identity.Serial,
                        ConfiguredSerial,
                        StringComparison.OrdinalIgnoreCase));
                if (destination is not null)
                {
                    _popoverState.Update(destination);
                }
            }
        });
    }

    private void OnSyncEvent(RoutedSyncEvent routed)
    {
        if (!routed.Context.IsDeviceSession)
        {
            return;
        }

        DispatcherQueue.TryEnqueue(() => _popoverState.ApplySyncProgress(routed));
    }

    private void OnPopoverRequested()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            // Suppress the popover while the wizard owns the
            // foreground. We also hide the tray during the wizard so
            // in normal use this branch is unreachable, but the
            // guard is cheap and survives the user re-showing the
            // tray via another path (e.g. notification action).
            if (_wizardActive || LatestConfig?.Ipod is null) return;

            // Tray-icon toggle: a tray click while the popover is open
            // means "close it". Replaces the prior "re-Activate the
            // existing one" behaviour, which made the tray icon look
            // dead on second click.
            if (_popover is not null)
            {
                _popover.Close();
                return;
            }

            // Debounce: if the popover was open within the last
            // ~300ms, the tray click that brought us here is the same
            // click that just dismissed the previous instance via
            // focus loss. Don't immediately reopen — the user clicked
            // the tray intending to close.
            var sinceClose = DateTime.UtcNow.Ticks - _popoverClosedAtTicks;
            if (sinceClose < PopoverToggleDebounceTicks) return;
            _popover = new PopoverWindow(_popoverState, Daemon!);
            _popover.Closed += (_, _) =>
            {
                _popover = null;
                _popoverClosedAtTicks = DateTime.UtcNow.Ticks;
            };
            _popover.Activate();
        });
    }

    private void OnSettingsRequested() => RequestOpenSettings();

    public static void RequestOpenSettings()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            // Dismiss the popover so the user doesn't end up with two
            // overlapping flyouts anchored to the same tray corner.
            _popover?.Close();
            if (_settings is not null) { _settings.Activate(); return; }
            if (Daemon is null || Router is null || LatestConfig is null) return;
            var vm = new SettingsViewModel(Daemon, Router, LatestConfig);
            _settings = new SettingsWindow(vm);
            SettingsWindowHandle = WinRT.Interop.WindowNative.GetWindowHandle(_settings);
            _settings.Closed += (_, _) =>
            {
                _settings = null;
                SettingsWindowHandle = IntPtr.Zero;
            };
            _settings.Activate();
        });
    }

    /// <summary>Open the pair wizard to add a new iPod. Closes any open
    /// settings window so the wizard can take focus without the user
    /// juggling overlapping windows.</summary>
    public static void RequestPairNewIpod()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            _settings?.Close();
            _popover?.Close();
            ShowWizardStatic();
        });
    }

    private static void ShowWizardStatic()
    {
        _wizardActive = true;
        UpdateTrayVisibility();
        var wizard = new WizardWindow();
        Window = wizard;
        WindowHandle = WinRT.Interop.WindowNative.GetWindowHandle(wizard);

        // Track whether the wizard was completed normally (Finish click on
        // step 5 fires WizardFinished BEFORE the window closes). If the
        // window closes any other way the user has bailed out of setup —
        // exit the app cleanly rather than leaving a half-configured
        // process holding a tray icon.
        bool completedNormally = false;
        wizard.ViewModel.WizardFinished += () => completedNormally = true;

        wizard.Closed += (_, _) =>
        {
            Window = null;
            WindowHandle = IntPtr.Zero;
            _wizardActive = false;
            if (completedNormally)
            {
                UpdateTrayVisibility();
                return;
            }
            // Same shutdown sequence as the tray Quit menu — try a graceful
            // daemon stop then bail. Fire-and-forget on the UI dispatcher
            // because Closed runs synchronously and we can't await here.
            DispatcherQueue.TryEnqueue(async () => await ShutdownAppAsync());
        };
        wizard.Activate();
    }

    private static async Task ShutdownAppAsync()
    {
        if (Daemon is not null)
        {
            try { await Daemon.SendAsync(new ShutdownCommand()); }
            catch { /* daemon may already be dead */ }
            await Daemon.DisposeAsync();
        }
        Router?.Stop();
        Tray?.Dispose();
        Environment.Exit(0);
    }

    private void OnSyncNowRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is null || ConfiguredSerial is not { } serial) return;
            try { await Daemon.SendAsync(new TriggerSyncCommand("manual", serial, Guid.NewGuid().ToString("N"))); }
            catch (Exception e) { Debug.WriteLine($"app: trigger_sync failed: {e}"); }
        });
    }

    private void OnQuitRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is not null)
            {
                try { await Daemon.SendAsync(new ShutdownCommand()); }
                catch { /* daemon may already be dead */ }
                await Daemon.DisposeAsync();
            }
            Router?.Stop();
            Tray?.Dispose();
            Environment.Exit(0);
        });
    }

    private static async Task<bool> IsDaemonRunningAsync()
    {
        try
        {
            using var pipe = new NamedPipeClientStream(
                ".", DaemonClient.PipeName, PipeDirection.InOut, PipeOptions.Asynchronous);
            await pipe.ConnectAsync(500);
            return true;
        }
        catch { return false; }
    }

    private static void SpawnDaemon()
    {
        var uiDir = AppContext.BaseDirectory;
        var coreCandidates = new[]
        {
            Path.Combine(uiDir, "classick.exe"),
            Path.Combine(Directory.GetParent(uiDir)?.FullName ?? "", "classick.exe"),
        };
        string? corePath = null;
        foreach (var c in coreCandidates)
        {
            if (File.Exists(c)) { corePath = c; break; }
        }
        if (corePath is null)
        {
            Debug.WriteLine("app: cannot find classick.exe to spawn daemon");
            return;
        }
        var psi = new ProcessStartInfo
        {
            FileName = corePath,
            ArgumentList = { "--daemon" },
            UseShellExecute = false,
            CreateNoWindow = true,
        };
        Process.Start(psi);
    }
}
