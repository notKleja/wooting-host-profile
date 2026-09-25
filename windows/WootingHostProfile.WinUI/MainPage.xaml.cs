using System.Collections.ObjectModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace WootingHostProfile_WinUI;

public sealed partial class MainPage : Page
{
    public ObservableCollection<ProfileInfo> Profiles { get; } = [];

    public MainPage()
    {
        InitializeComponent();
        Loaded += MainPage_Loaded;
    }

    private async void MainPage_Loaded(object sender, RoutedEventArgs e)
    {
        StartupToggle.IsOn = AgentService.IsStartupEnabled();
        await RefreshProfilesAsync();
    }

    private ProfileInfo? SelectedProfile => ProfilePicker.SelectedItem as ProfileInfo;

    private void SetBusy(bool busy)
    {
        LoadingRing.IsActive = busy;
        ProfilePicker.IsEnabled = !busy;
        StartupToggle.IsEnabled = !busy;
        ApplyButton.IsEnabled = !busy && SelectedProfile is not null;
        SaveButton.IsEnabled = !busy && SelectedProfile is not null;
    }

    private async Task RefreshProfilesAsync()
    {
        SetBusy(true);
        StatusText.Text = "Reading configured profiles…";
        try
        {
            IReadOnlyList<ProfileInfo> profiles = await AgentService.GetProfilesAsync();
            Profiles.Clear();
            foreach (ProfileInfo profile in profiles)
            {
                Profiles.Add(profile);
            }
            ProfilePicker.SelectedItem = Profiles.FirstOrDefault(profile => profile.Active)
                ?? Profiles.FirstOrDefault();
            StatusText.Text = Profiles.Count == 0
                ? "No configured onboard profiles were found. Open Wootility Web and connect the keyboard once."
                : $"Found {Profiles.Count} configured profile{(Profiles.Count == 1 ? "" : "s")}.";
        }
        catch (Exception error)
        {
            Profiles.Clear();
            StatusText.Text = error.Message;
        }
        finally
        {
            SetBusy(false);
        }
    }

    private async void Refresh_Click(object sender, RoutedEventArgs e)
    {
        await RefreshProfilesAsync();
    }

    private async void Apply_Click(object sender, RoutedEventArgs e)
    {
        if (SelectedProfile is not ProfileInfo profile)
        {
            return;
        }
        SetBusy(true);
        StatusText.Text = $"Applying {profile.Name}…";
        try
        {
            await AgentService.ApplyAsync(profile.Index);
            StatusText.Text = $"Applied and verified P{profile.Index} — {profile.Name}.";
            await RefreshProfilesAsync();
        }
        catch (Exception error)
        {
            StatusText.Text = error.Message;
        }
        finally
        {
            SetBusy(false);
        }
    }

    private async void Save_Click(object sender, RoutedEventArgs e)
    {
        if (SelectedProfile is not ProfileInfo profile)
        {
            return;
        }
        SetBusy(true);
        StatusText.Text = $"Saving {profile.Name} for Windows…";
        try
        {
            await AgentService.ConfigureAsync(profile.Index, StartupToggle.IsOn);
            StatusText.Text = StartupToggle.IsOn
                ? $"Saved P{profile.Index} — {profile.Name}. The watcher and tray app will start at sign-in."
                : $"Saved P{profile.Index} — {profile.Name}. The hidden watcher is running for this session.";
        }
        catch (Exception error)
        {
            StatusText.Text = error.Message;
        }
        finally
        {
            SetBusy(false);
        }
    }
}
