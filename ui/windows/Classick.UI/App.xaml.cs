using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
using Classick_UI.Devices;
using Classick_UI.Ipc;
using Classick_UI.Notifications;
using Classick_UI.Startup;
using Classick_UI.ViewModels;
using Classick_UI.Views;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

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

    public static DeviceStore Store { get; } = new();

    private static readonly PopoverViewModel _popoverState = new();
    private static PopoverWindow? _popover;
    private static SettingsWindow? _settings;
    private static TaskCompletionSource<DeviceInventoryEvent>? _initialInventoryWaiter;

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
        catch (WireCompatibilityException e)
        {
            Debug.WriteLine($"app: incompatible daemon: {e}");
            ShowStartupFailure(StartupFailurePresentationFactory.For(e));
            return;
        }
        catch (Exception e)
        {
            Debug.WriteLine($"app: failed to connect to daemon: {e}");
            ShowStartupFailure(StartupFailurePresentationFactory.For(e));
            return;
        }

        // Start the router. All consumers (tray, popover, notifications,
        // wizard) subscribe through it instead of reading the channel
        // directly.
        Router = new DaemonEventRouter(Daemon.Events);
        Router.EventReceived += OnWireEvent;
        Router.Start();

        // Notification service subscribes to router internally.
        Notifications = new NotificationService(Router,
            getNotifyOn: () => Store.GlobalConfig?.Settings.NotifyOn switch
            {
                NotifyLevel.ErrorsOnly => "errors_only",
                NotifyLevel.None => "none",
                _ => "all",
            },
            resolveDeviceName: deviceId => Store.Devices.TryGetValue(deviceId, out var device)
                ? DevicePresentationFactory.For(device.Inventory).Title
                : "iPod");
        Notifications.Initialize();

        var inventoryReceived = new TaskCompletionSource<DeviceInventoryEvent>(
            TaskCreationOptions.RunContinuationsAsynchronously);
        _initialInventoryWaiter = inventoryReceived;

        await Daemon.SendAsync(new GetGlobalConfigCommand(NewRequestId()));
        await Daemon.SendAsync(new GetInventoryCommand(NewRequestId()));
        await Daemon.SendAsync(new WireGetHistoryCommand(NewRequestId(), 10));
        await Daemon.SendAsync(new SubscribeInventoryCommand(NewRequestId()));

        // Wait for the actual ConfigUpdate, capped at 2s so a dead daemon
        // doesn't wedge startup forever. Either outcome: we make the wizard
        // decision below using whatever LatestConfig holds.
        try { await inventoryReceived.Task.WaitAsync(TimeSpan.FromSeconds(2)); }
        catch (TimeoutException) { }
        finally
        {
            if (ReferenceEquals(_initialInventoryWaiter, inventoryReceived))
                _initialInventoryWaiter = null;
        }

        // Open wizard if config has no iPod identity. The wizard also
        // subscribes to the router (T14) so the channel-exclusivity
        // hack from M3 goes away.
        if (!HasAdoptedDevice())
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

    private static void ShowStartupFailure(StartupFailurePresentation presentation)
    {
        Tray?.Dispose();
        var close = new Button { Content = "Close", HorizontalAlignment = HorizontalAlignment.Left };
        var content = new StackPanel
        {
            Padding = new Thickness(32),
            Spacing = 16,
            Children =
            {
                new TextBlock
                {
                    Text = presentation.Title,
                    Style = Application.Current.Resources["TitleTextBlockStyle"] as Style,
                },
                new TextBlock
                {
                    Text = presentation.Message,
                    TextWrapping = TextWrapping.Wrap,
                    MaxWidth = 480,
                },
                close,
            },
        };
        var window = new Window { Title = presentation.Title, Content = content };
        Window = window;
        close.Click += (_, _) => window.Close();
        window.Closed += (_, _) => Environment.Exit(0);
        window.Activate();
    }

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
        Tray?.SetVisible(!_wizardActive && HasAdoptedDevice());
    }

    private void OnWireEvent(WireEvent wireEvent)
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!Store.Reduce(wireEvent)) return;
            if (wireEvent is CommandFailedEvent failed)
            {
                _popoverState.InteractionCommandFailed(failed.RequestId, failed.Message);
            }
            if (wireEvent is WireSourceAvailabilityEvent availability)
            {
                _popoverState.ApplySourceAvailability(availability);
            }
            ApplyFocusedDevice();
            UpdateTrayFromStore();
            UpdateTrayVisibility();
            if (wireEvent is DeviceInventoryEvent inventory)
            {
                _initialInventoryWaiter?.TrySetResult(inventory);
            }
        });
    }

    private static void ApplyFocusedDevice()
    {
        _popoverState.UpdateDeviceChoices(Store.Devices.Values, Store.FocusedDeviceId);
        if (Store.FocusedDeviceId is not { } focusedId ||
            !Store.Devices.TryGetValue(focusedId, out var focused))
        {
            _popoverState.ClearDisplayedDevice();
            return;
        }

        _popoverState.Update(focused.Inventory);
        _popoverState.ApplyHistory(focused.History);
        if (focused.ActiveSessionId is { } sessionId)
        {
            _popoverState.SetActiveDeviceSession(new DeviceSessionTarget(focusedId, sessionId));
            _popoverState.ApplySyncPresentation(focused.SyncPresentation);
        }
        else
        {
            if (focused.SyncPresentation?.Finished is not null)
            {
                _popoverState.SetActiveDeviceSession(focused.SyncPresentation.Target);
                _popoverState.ApplySyncPresentation(focused.SyncPresentation);
            }
            _popoverState.ClearActiveDeviceSession();
        }
    }

    public static void RequestPopoverDeviceFocus(DeviceId deviceId)
    {
        if (!Store.SelectDevice(deviceId)) return;
        ApplyFocusedDevice();
    }

    private static void UpdateTrayFromStore()
    {
        if (Tray is null) return;
        var presentation = DeviceActivityPresentationFactory.For(Store.Devices.Values);
        var state = presentation.Activity switch
        {
            AggregateDeviceActivity.Syncing => TrayState.Syncing,
            AggregateDeviceActivity.Idle => TrayState.Idle,
            _ => TrayState.Offline,
        };
        Tray.SetState(state, presentation.Tooltip);
    }

    public static async Task RequestEjectAsync(DeviceId deviceId)
    {
        var target = Store.CaptureMountAction(deviceId);
        if (target is null || !Store.IsCurrentMountAction(target)) return;
        try { await WindowsEjectService.EjectAsync(target); }
        catch (Exception e) { Debug.WriteLine($"app: eject failed: {e}"); }
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
            if (_wizardActive || !HasAdoptedDevice()) return;

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
            if (Daemon is null || Router is null) return;
            _settings = new SettingsWindow(new SettingsViewModel(Daemon, Router, Store));
            SettingsWindowHandle = WinRT.Interop.WindowNative.GetWindowHandle(_settings);
            _settings.Closed += (_, _) =>
            {
                _settings?.ViewModel.Dispose();
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
            try { await Daemon.SendAsync(new WireShutdownCommand(NewRequestId())); }
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
            if (Daemon is null ||
                Store.FocusedDeviceId is not { } deviceId ||
                Store.CaptureDeviceMutation(deviceId) is not { } target) return;
            try { await Daemon.SendAsync(new WireTriggerSyncCommand(target.DeviceId, NewRequestId(), SyncTrigger.Manual)); }
            catch (Exception e) { Debug.WriteLine($"app: trigger_sync failed: {e}"); }
        });
    }

    private void OnQuitRequested()
    {
        DispatcherQueue.TryEnqueue(async () =>
        {
            if (Daemon is not null)
            {
                try { await Daemon.SendAsync(new WireShutdownCommand(NewRequestId())); }
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

    private static bool HasAdoptedDevice() => Store.Devices.Values.Any(device =>
        device.Inventory.ProfileStatus == ProfileStatus.Adopted);

    private static string NewRequestId() => Guid.NewGuid().ToString("D");
}
