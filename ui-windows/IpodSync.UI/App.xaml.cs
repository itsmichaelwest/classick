using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
using IpodSync_UI.Ipc;
using IpodSync_UI.Views;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace IpodSync_UI;

/// <summary>
/// M2 application shell. Starts hidden in the system tray, probes/launches
/// the ipod-sync daemon, connects via <see cref="DaemonClient"/>, then opens
/// <see cref="WizardWindow"/> if the user hasn't picked an iPod identity yet.
/// Otherwise stays hidden until tray menu / device events kick off a sync
/// (M3 territory).
/// </summary>
public partial class App : Application
{
    /// <summary>
    /// Currently displayed top-level window, if any. Null while the app sits
    /// in the tray with no UI surface. Set to a <see cref="WizardWindow"/>
    /// during first-run setup; M3 may swap in a progress / status window.
    /// </summary>
    public static Window? Window { get; private set; }

    /// <summary>
    /// Native handle (HWND) of <see cref="Window"/>, used by interop callers
    /// (file pickers, <c>InitializeWithWindow</c>). Zero while no window is open.
    /// </summary>
    public static IntPtr WindowHandle { get; private set; }

    /// <summary>
    /// UI thread dispatcher. Fully qualified type avoids CS0104 ambiguity with
    /// <c>Windows.System.DispatcherQueue</c>.
    /// </summary>
    public static DispatcherQueue DispatcherQueue { get; private set; } = default!;

    /// <summary>Persistent daemon connection, available after OnLaunched.</summary>
    public static DaemonClient? Daemon { get; private set; }

    /// <summary>Tray icon owner. Always initialized; Quit menu wires through here.</summary>
    public static TrayIconController? Tray { get; private set; }

    public App()
    {
        this.InitializeComponent();
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        DispatcherQueue = DispatcherQueue.GetForCurrentThread();

        // 1. Set up tray icon early so something visible exists even if the
        //    daemon connection takes a moment.
        Tray = new TrayIconController();
        Tray.Initialize();
        Tray.QuitRequested += OnQuitRequested;

        // 2. Ensure daemon is running.
        if (!await IsDaemonRunningAsync())
        {
            SpawnDaemon();
            // Give it a moment to create the pipe.
            await Task.Delay(500);
        }

        // 3. Connect to daemon.
        try
        {
            Daemon = await DaemonClient.ConnectAsync();
        }
        catch (Exception e)
        {
            Debug.WriteLine($"app: failed to connect to daemon: {e}");
            // Surface as a tray notification rather than a window pop.
            // For now, just quit cleanly.
            Tray?.Dispose();
            Application.Current.Exit();
            return;
        }

        // 4. Ask daemon for config status. If unconfigured, open the wizard.
        await Daemon.SendAsync(new GetConfigCommand());
        var first = await Daemon.Events.ReadAsync();
        if (first is ConfigUpdateEvent cfg && cfg.Ipod is null)
        {
            ShowWizard();
        }
        else
        {
            // Configured: stay hidden in tray. M3 starts the auto-sync flow.
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
            Tray?.Dispose();
            Application.Current.Exit();
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
        catch
        {
            return false;
        }
    }

    private static void SpawnDaemon()
    {
        // Locate ipod-sync.exe (bundled alongside the UI exe).
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
