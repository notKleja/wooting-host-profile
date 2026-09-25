using Microsoft.UI.Xaml;

namespace WootingHostProfile_WinUI;

public partial class App : Application
{
    private MainWindow? _window;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        bool startInTray = Environment.GetCommandLineArgs()
            .Any(argument => string.Equals(argument, "--tray", StringComparison.OrdinalIgnoreCase));
        _window = new MainWindow(startInTray);
        _window.Activate();
    }
}
