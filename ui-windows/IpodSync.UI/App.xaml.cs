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

    /// <summary>Running snapshot of in-flight sync progress accumulated
    /// from the daemon's SyncEvent stream. Survives popover open/close so
    /// reopening mid-sync shows "Track N of M" instead of an
    /// indefinitely-stuck "Preparing…". Cleared on FinishEvent and on
    /// daemon-reported Idle transitions so a stale prior-sync snapshot
    /// doesn't leak into the next session.</summary>
    private static int _progressCurrent;
    private static int _progressTotal;
    private static string _currentTrackLabel = "";
    private static string _currentLogLine = "";

    /// <summary>Latest unanswered prompt from the sync subprocess, or
    /// null if no prompt is in flight. Survives popover open/close so
    /// reopening the tray when a prompt has been pending sees the
    /// overlay immediately instead of just "Preparing…". Cleared on
    /// FinishEvent and on the popover's own ClearPrompt path after
    /// the user picks an option (the daemon's stdin write is
    /// fire-and-forward).</summary>
    private static PromptEvent? _pendingPrompt;

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
        // Idle transitions mean the previous sync has fully wound down;
        // drop the cached progress so a re-opened popover doesn't show
        // stale "Track 472 of 1275" from the previous run.
        if (s.State != "syncing")
        {
            _progressCurrent = 0;
            _progressTotal = 0;
            _currentTrackLabel = "";
            _currentLogLine = "";
        }
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
        {
            _popover?.ViewModel.SetDeviceLabel(c.Ipod?.Name, c.Ipod?.ModelLabel);
            // Pair / forget transitions flip the tray's visibility
            // automatically — no separate notification needed.
            UpdateTrayVisibility();
        });
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
        // First, accumulate into App-level state so a popover opened
        // mid-sync can be seeded with the latest progress instead of
        // rendering "Preparing…" indefinitely.
        switch (e)
        {
            case SummaryEvent s:
                _progressTotal = s.TotalPlanned;
                _progressCurrent = 0;
                _currentTrackLabel = "";
                _currentLogLine = $"{s.Add} to add, {s.Modify} to update, {s.Remove} to remove";
                break;
            case TrackStartEvent t:
                _progressCurrent = t.Current;
                _progressTotal = t.Total;
                _currentTrackLabel = t.Label;
                _currentLogLine = "";
                // A TrackStart implies the subprocess moved past any
                // pending prompt — clear the App-level snapshot so a
                // popover opened after the answer doesn't re-render
                // the stale prompt.
                _pendingPrompt = null;
                break;
            case HeaderEvent h:
                _currentLogLine = $"Scanning {h.Source}";
                break;
            case LogEvent l:
                _currentLogLine = l.Message;
                break;
            case PromptEvent p:
                // Hold the prompt so a popover opened during the wait
                // can render the overlay immediately on activation.
                _pendingPrompt = p;
                break;
            case FinishEvent:
                _progressCurrent = 0;
                _progressTotal = 0;
                _currentTrackLabel = "";
                _currentLogLine = "";
                _pendingPrompt = null;
                break;
        }
        // Then forward to an open popover so it animates in real time.
        DispatcherQueue.TryEnqueue(() => _popover?.ViewModel.ApplyIpcProgress(e));
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
            var vm = new PopoverViewModel();
            vm.SetDeviceLabel(LatestConfig?.Ipod?.Name, LatestConfig?.Ipod?.ModelLabel);
            if (LatestStatus is not null) vm.Update(LatestStatus);
            if (LatestHistory is not null) vm.ApplyHistory(LatestHistory);
            // Replay accumulated sync progress so the user doesn't see
            // "Preparing…" when the subprocess is already mid-apply.
            // Order matters: ProgressTotal sets NoProgressYet, which
            // controls whether "Preparing…" still renders.
            if (LatestStatus?.State == "syncing" && _progressTotal > 0)
            {
                vm.ProgressTotal = _progressTotal;
                vm.ProgressCurrent = _progressCurrent;
                vm.CurrentTrackLabel = _currentTrackLabel;
                vm.CurrentLogLine = _currentLogLine;
            }
            else if (LatestStatus?.State == "syncing" && !string.IsNullOrEmpty(_currentLogLine))
            {
                // Pre-summary phase: at least give the user a log line.
                vm.CurrentLogLine = _currentLogLine;
            }
            // Re-render any unanswered prompt so a popover opened
            // mid-wait shows the overlay immediately instead of just
            // "Preparing…". Re-using ApplyIpcProgress's PromptEvent
            // arm keeps the seed path and live path in lock-step.
            if (_pendingPrompt is not null)
            {
                vm.ApplyIpcProgress(_pendingPrompt);
            }
            _popover = new PopoverWindow(vm, Daemon!);
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
