using System;
using System.ComponentModel;
using System.Threading.Tasks;
using Classick_UI.Devices;
using Classick_UI.Ipc;
using Classick_UI.ViewModels;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using WinRT.Interop;

namespace Classick_UI.Views;

public sealed partial class WizardWindow : Window
{
    private const int FixedWidthDip = 640;
    private const int FixedHeightDip = 500;

    public WizardViewModel ViewModel { get; }

    private int _previousStep;
    private bool _hasNavigatedOnce;
    private bool _centered;

    public WizardWindow()
    {
        ViewModel = new WizardViewModel(sendConfigFunc: SendSaveConfigAsync);
        ViewModel.WizardFinished += () => DispatcherQueue.TryEnqueue(Close);
        ViewModel.PropertyChanged += OnViewModelPropertyChanged;

        InitializeComponent();

        // Footer bindings resolve {StaticResource BoolToVis} via the Grid's
        // DataContext — x:Bind's compile-time converter lookup requires a
        // FrameworkElement root and Window isn't one.
        WizardRoot.DataContext = ViewModel;

        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(hwnd));

        if (appWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
            presenter.IsMinimizable = false;
        }

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(WizardTitleBar);

        if (AppWindowTitleBar.IsCustomizationSupported())
        {
            appWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;
        }

        SystemBackdrop = new MicaBackdrop();

        WindowAnchor.DisableTransitions(hwnd);
        WindowAnchor.SizeClientArea(hwnd, appWindow, FixedWidthDip, FixedHeightDip);
        CenterOnDisplay(hwnd, appWindow);

        Activated += OnFirstActivated;

        NavigateToCurrentStep();
    }

    private void OnFirstActivated(object sender, WindowActivatedEventArgs args)
    {
        if (_centered) return;
        _centered = true;
        Activated -= OnFirstActivated;
        var hwnd = WindowNative.GetWindowHandle(this);
        var appWindow = AppWindow.GetFromWindowId(Win32Interop.GetWindowIdFromWindow(hwnd));
        CenterOnDisplay(hwnd, appWindow);
    }

    private static void CenterOnDisplay(IntPtr hwnd, AppWindow appWindow)
    {
        var display = DisplayArea.GetFromWindowId(
            Win32Interop.GetWindowIdFromWindow(hwnd),
            DisplayAreaFallback.Primary);
        var work = display.WorkArea;
        var x = work.X + (work.Width - appWindow.Size.Width) / 2;
        var y = work.Y + (work.Height - appWindow.Size.Height) / 2;
        appWindow.Move(new Windows.Graphics.PointInt32(x, y));
    }

    private void OnViewModelPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName != nameof(WizardViewModel.CurrentStep)) return;
        DispatcherQueue.TryEnqueue(NavigateToCurrentStep);
    }

    private void NavigateToCurrentStep()
    {
        var pageType = ViewModel.CurrentStep switch
        {
            1 => typeof(WizardWelcomePage),
            2 => typeof(WizardFolderPage),
            3 => typeof(WizardDevicePage),
            4 => typeof(WizardSyncSettingsPage),
            5 => typeof(WizardDonePage),
            _ => typeof(WizardWelcomePage),
        };
        NavigationTransitionInfo info;
        if (!_hasNavigatedOnce)
        {
            info = new SuppressNavigationTransitionInfo();
            _hasNavigatedOnce = true;
        }
        else if (ViewModel.CurrentStep < _previousStep)
        {
            info = new SlideNavigationTransitionInfo { Effect = SlideNavigationTransitionEffect.FromLeft };
        }
        else
        {
            info = new SlideNavigationTransitionInfo { Effect = SlideNavigationTransitionEffect.FromRight };
        }
        _previousStep = ViewModel.CurrentStep;
        StepFrame.Navigate(pageType, ViewModel, info);
    }

    private void OnBackRequested(TitleBar sender, object args) => ViewModel.BackCommand.Execute(null);

    private async Task SendSaveConfigAsync(SaveConfigPayload payload)
    {
        var daemon = App.Daemon ?? throw new InvalidOperationException("daemon not connected");
        var intent = new DeviceSetupIntent(
            payload.Source,
            payload.DeviceId,
            payload.AutoSync);
        var commands = DeviceSetupCommandFactory.Create(intent, NewId);
        var source = commands.OfType<SetSourceLocationCommand>().Single();
        var adopt = commands.OfType<AdoptDeviceCommand>().Single();
        var tracker = new DeviceSetupAcknowledgementTracker(source, adopt);
        var sourceAccepted = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var completion = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        void OnEvent(WireEvent wireEvent)
        {
            tracker.Observe(wireEvent);
            if (tracker.SourceAccepted) sourceAccepted.TrySetResult();
            if (tracker.Failure is { } failure)
            {
                sourceAccepted.TrySetException(new InvalidOperationException(failure));
                completion.TrySetException(new InvalidOperationException(failure));
            }
            else if (tracker.IsComplete)
            {
                completion.TrySetResult();
            }
        }

        var router = App.Router ?? throw new InvalidOperationException("event router not connected");
        router.EventReceived += OnEvent;
        try
        {
            await daemon.SendAsync(source);
            await sourceAccepted.Task.WaitAsync(TimeSpan.FromSeconds(10));
            await daemon.SendAsync(adopt);
            await completion.Task.WaitAsync(TimeSpan.FromSeconds(10));
        }
        catch (TimeoutException)
        {
            throw new InvalidOperationException("Classick did not confirm the new device settings. Try again.");
        }
        finally
        {
            router.EventReceived -= OnEvent;
        }
    }

    private static string NewId() => Guid.NewGuid().ToString("D");
}
