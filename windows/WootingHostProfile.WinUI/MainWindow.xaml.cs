using System.Windows.Input;
using H.NotifyIcon;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Windows.Graphics;

namespace WootingHostProfile_WinUI;

public sealed partial class MainWindow : Window
{
    private bool _allowExit;
    private bool _visible = true;

    public ICommand ToggleWindowCommand { get; }

    public MainWindow(bool startInTray)
    {
        ToggleWindowCommand = new RelayCommand(ToggleWindow);
        InitializeComponent();
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.SetIcon("Assets/AppIcon.ico");
        AppWindow.Resize(new SizeInt32(540, 520));
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.IsResizable = false;
            presenter.IsMaximizable = false;
        }
        AppWindow.Changed += OnAppWindowChanged;
        Closed += OnWindowClosed;

        RootFrame.Navigate(typeof(MainPage));
        AgentService.EnsureWatcher();

        if (startInTray)
        {
            DispatcherQueue.TryEnqueue(HideToTray);
        }
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
