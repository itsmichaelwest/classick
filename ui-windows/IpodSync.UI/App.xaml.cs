using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.Notifications;
using IpodSync_UI.ViewModels;
using IpodSync_UI.Views;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace IpodSync_UI;

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
    /// <summary>Latest StatusUpdate. Used to drive popover initial state.</summary>
    public static StatusUpdateEvent? LatestStatus { get; private set; }
    /// <summary>Latest HistoryUpdate. Used to seed popover activity feed.</summary>
    public static HistoryUpdateEvent? LatestHistory { get; private set; }

    private static PopoverWindow? _popover;
    private static SettingsWindow? _settings;

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
        Router.IpcEventReceived += OnIpcEvent;
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
        await Daemon.SendAsync(new GetConfigCommand());
        await Daemon.SendAsync(new GetStatusCommand());
        await Daemon.SendAsync(new GetHistoryCommand(Limit: 10));

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
    }

    private void ShowWizard()
    {
        Window = new WizardWindow();
        WindowHandle = WinRT.Interop.WindowNative.GetWindowHandle(Window);
        Window.Closed += (_, _) =>
        {
            Window = null;
            WindowHandle = IntPtr.Zero;
        };
        Window.Activate();
    }

    private void OnStatusUpdated(StatusUpdateEvent s)
    {
        LatestStatus = s;
        DispatcherQueue.TryEnqueue(() =>
        {
            UpdateTrayFromStatus(s);
            _popover?.ViewModel.Update(s);
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
            _popover?.ViewModel.SetDeviceLabel(c.Ipod?.Name, c.Ipod?.ModelLabel));
    }
    private void OnHistoryUpdated(HistoryUpdateEvent h)
    {
        LatestHistory = h;
        DispatcherQueue.TryEnqueue(() => _popover?.ViewModel.ApplyHistory(h));
    }
    private void OnDeviceConnected(DeviceConnectedEvent dc)
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            Tray?.SetState(TrayState.Idle, $"iPod connected ({dc.Name ?? dc.ModelLabel})");
            // The daemon re-broadcasts DeviceConnected with the resolved
            // name after the async iTunesDB parse completes — keep the
            // popover label in sync if it's open.
            _popover?.ViewModel.SetDeviceLabel(dc.Name, dc.ModelLabel);
        });
    }
    private void OnDeviceDisconnected(DeviceDisconnectedEvent _)
    {
        DispatcherQueue.TryEnqueue(() =>
            Tray?.SetState(TrayState.Offline, "iPod not connected"));
    }

    private void UpdateTrayFromStatus(StatusUpdateEvent s)
    {
        if (Tray is null) return;
        var (state, tooltip) = (s.State, s.IpodConnected) switch
        {
            ("syncing", _)   => (TrayState.Syncing, "Syncing iPod…"),
            (_,    true)     => (TrayState.Idle,    "iPod connected · idle"),
            _                => (TrayState.Offline, "iPod not connected"),
        };
        Tray.SetState(state, tooltip);
    }

    private void OnIpcEvent(IpcEvent e)
    {
        // Forward live sync-subprocess progress to the popover so
        // ProgressBar + CurrentTrackLabel update during a sync.
        DispatcherQueue.TryEnqueue(() => _popover?.ViewModel.ApplyIpcProgress(e));
    }

    private void OnPopoverRequested()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_popover is not null) { _popover.Activate(); return; }
            var vm = new PopoverViewModel();
            vm.SetDeviceLabel(LatestConfig?.Ipod?.Name, LatestConfig?.Ipod?.ModelLabel);
            if (LatestStatus is not null) vm.Update(LatestStatus);
            if (LatestHistory is not null) vm.ApplyHistory(LatestHistory);
            _popover = new PopoverWindow(vm, Daemon!, LatestConfig?.Source ?? "");
            _popover.Closed += (_, _) => _popover = null;
            _popover.Activate();
        });
    }

    private void OnSettingsRequested() => RequestOpenSettings();

    public static void RequestOpenSettings()
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_settings is not null) { _settings.Activate(); return; }
            if (Daemon is null || Router is null || LatestConfig is null) return;
            var vm = new SettingsViewModel(Daemon, Router, LatestConfig);
            _settings = new SettingsWindow(vm);
            _settings.Closed += (_, _) => _settings = null;
            _settings.Activate();
        });
    }

    private void OnSyncNowRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is null) return;
            try { await Daemon.SendAsync(new TriggerSyncCommand("manual")); }
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
            Path.Combine(uiDir, "ipod-sync.exe"),
            Path.Combine(Directory.GetParent(uiDir)?.FullName ?? "", "ipod-sync.exe"),
        };
        string? corePath = null;
        foreach (var c in coreCandidates)
        {
            if (File.Exists(c)) { corePath = c; break; }
        }
        if (corePath is null)
        {
            Debug.WriteLine("app: cannot find ipod-sync.exe to spawn daemon");
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
