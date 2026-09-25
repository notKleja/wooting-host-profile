using Microsoft.UI.Xaml;
using System.Threading;

namespace WootingHostProfile_WinUI;

public partial class App : Application
{
    private const string InstanceMutexName = @"Local\WootingSwitch.WinUI.Instance";
    private const string ShowEventName = @"Local\WootingSwitch.WinUI.Show";

    private MainWindow? _window;
    private Mutex? _instanceMutex;
    private EventWaitHandle? _showEvent;

    public App()
    {
        InitializeComponent();
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        _instanceMutex = new Mutex(initiallyOwned: true, InstanceMutexName, out bool isFirstInstance);
        if (!isFirstInstance)
        {
            try
            {
                using EventWaitHandle showEvent = EventWaitHandle.OpenExisting(ShowEventName);
                showEvent.Set();
            }
            catch (WaitHandleCannotBeOpenedException)
            {
                // The first process is still starting; it remains authoritative.
            }
            Exit();
            return;
        }

        _showEvent = new EventWaitHandle(false, EventResetMode.AutoReset, ShowEventName);
        bool startInTray = Environment.GetCommandLineArgs()
            .Any(argument => string.Equals(argument, "--tray", StringComparison.OrdinalIgnoreCase));
        _window = new MainWindow(startInTray);
        _window.Activate();

        _ = Task.Run(() =>
        {
            while (_showEvent.WaitOne())
            {
                _window?.DispatcherQueue.TryEnqueue(() => _window.ShowFromExternalActivation());
            }
        });
    }

    internal void SetStatusIconVisible(bool visible) => _window?.SetStatusIconVisible(visible);
}
