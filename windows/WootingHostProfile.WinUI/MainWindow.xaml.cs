using System.Windows.Input;
using H.NotifyIcon;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Navigation;
using Windows.Foundation;
using Windows.Graphics;

namespace WootingHostProfile_WinUI;

public sealed partial class MainWindow : Window
{
    private const double PreferredClientWidthDip = 460.0;

    private bool _allowExit;
    private bool _correctingWindowSize;
    private bool _visible = true;
    private SizeInt32 _minimumWindowSize;

    public ICommand ToggleWindowCommand { get; }

    public MainWindow(bool startInTray)
    {
        ToggleWindowCommand = new RelayCommand(ToggleWindow);
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.SetIcon("Assets/AppIcon.ico");
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = true;
            presenter.IsMaximizable = false;
        }
        AppWindow.Changed += OnAppWindowChanged;
        Closed += OnWindowClosed;

        RootFrame.Navigated += OnRootFrameNavigated;
        RootFrame.Navigate(typeof(MainPage));
        AgentService.EnsureWatcher();

        if (startInTray)
        {
            DispatcherQueue.TryEnqueue(HideToTray);
        }
    }

    private void OnRootFrameNavigated(object sender, NavigationEventArgs args)
    {
        if (args.Content is FrameworkElement content)
        {
            content.Loaded += OnContentLoaded;
        }
        if (args.Content is MainPage page)
        {
            page.LayoutReady += OnPageLayoutReady;
        }
    }

    private void OnPageLayoutReady(object? sender, EventArgs args) =>
        DispatcherQueue.TryEnqueue(FitWindowToContent);

    private void OnContentLoaded(object sender, RoutedEventArgs args)
    {
        if (sender is FrameworkElement content)
        {
            content.Loaded -= OnContentLoaded;
        }
        DispatcherQueue.TryEnqueue(FitWindowToContent);
    }

    private void FitWindowToContent()
    {
        if (RootFrame.Content is not FrameworkElement content || content.XamlRoot is null)
        {
            return;
        }

        double scale = content.XamlRoot.RasterizationScale;
        int frameWidth = Math.Max(
            0,
            AppWindow.Size.Width - (int)Math.Round(RootFrame.ActualWidth * scale));
        int frameHeight = Math.Max(
            0,
            AppWindow.Size.Height - (int)Math.Round(RootFrame.ActualHeight * scale));

        DisplayArea display = DisplayArea.GetFromWindowId(AppWindow.Id, DisplayAreaFallback.Primary);
        double availableClientWidth = Math.Max(1, display.WorkArea.Width - frameWidth) / scale;
        double clientWidth = Math.Min(PreferredClientWidthDip, availableClientWidth);

        Size desired;
        if (content is MainPage page)
        {
            desired = page.MeasureContent(clientWidth);
        }
        else
        {
            content.Measure(new Size(clientWidth, double.PositiveInfinity));
            desired = content.DesiredSize;
        }
        var target = new SizeInt32(
            (int)Math.Ceiling(clientWidth * scale) + frameWidth,
            Math.Min(
                (int)Math.Ceiling(desired.Height * scale) + frameHeight,
                display.WorkArea.Height));
        _minimumWindowSize = target;

        if (AppWindow.Presenter is OverlappedPresenter presenter
            && presenter.State == OverlappedPresenterState.Maximized)
        {
            presenter.Restore();
        }
        DispatcherQueue.TryEnqueue(() => AppWindow.Resize(target));
    }

    private void OnWindowClosed(object sender, WindowEventArgs args)
    {
        if (_allowExit)
        {
            return;
        }
        args.Handled = true;
        HideToTray();
    }

    private void OnAppWindowChanged(AppWindow sender, AppWindowChangedEventArgs args)
    {
        if (sender.Presenter is OverlappedPresenter presenter
            && presenter.State == OverlappedPresenterState.Minimized)
        {
            HideToTray();
        }
        if (args.DidSizeChange
            && !_correctingWindowSize
            && _minimumWindowSize.Width > 0
            && (sender.Size.Width < _minimumWindowSize.Width
                || sender.Size.Height < _minimumWindowSize.Height))
        {
            _correctingWindowSize = true;
            sender.Resize(new SizeInt32(
                Math.Max(sender.Size.Width, _minimumWindowSize.Width),
                Math.Max(sender.Size.Height, _minimumWindowSize.Height)));
            DispatcherQueue.TryEnqueue(() => _correctingWindowSize = false);
        }
    }

    private void ToggleWindow()
    {
        if (_visible)
        {
            HideToTray();
        }
        else
        {
            ShowFromTray();
        }
    }

    private void HideToTray()
    {
        this.Hide();
        _visible = false;
    }

    private void ShowFromTray()
    {
        this.Show();
        Activate();
        _visible = true;
    }

    internal void ShowFromExternalActivation() => ShowFromTray();

    internal void SetStatusIconVisible(bool visible)
    {
        TrayIcon.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
    }

    private void OpenTray_Click(object sender, RoutedEventArgs e)
    {
        ShowFromTray();
    }

    private void ExitTray_Click(object sender, RoutedEventArgs e)
    {
        _allowExit = true;
        TrayIcon.Dispose();
        Close();
    }

    private sealed class RelayCommand(Action execute) : ICommand
    {
        public event EventHandler? CanExecuteChanged;

        public bool CanExecute(object? parameter) => true;

        public void Execute(object? parameter) => execute();

        public void RaiseCanExecuteChanged() => CanExecuteChanged?.Invoke(this, EventArgs.Empty);
    }
}
