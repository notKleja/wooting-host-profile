using System.Collections.ObjectModel;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Foundation;

namespace WootingHostProfile_WinUI;

public sealed partial class MainPage : Page
{
    private bool _loading;

    public ObservableCollection<ProfileInfo> Profiles { get; } = [];
    public event EventHandler? LayoutReady;

    public MainPage()
    {
        InitializeComponent();
        Loaded += MainPage_Loaded;
    }

    private ProfileInfo? SelectedProfile => ProfilePicker.SelectedItem as ProfileInfo;

    internal Size MeasureContent(double availableWidth)
    {
        ContentGrid.Measure(new Size(availableWidth, double.PositiveInfinity));
        return ContentGrid.DesiredSize;
    }

    private async void MainPage_Loaded(object sender, RoutedEventArgs e)
    {
        _loading = true;
        SetBusy(true);
        HideStatus();
        try
        {
            await LoadSettingsAsync();
            await RefreshProfilesAsync();
        }
        catch (Exception error)
        {
            ShowStatus(error.Message);
        }
        finally
        {
            _loading = false;
            SetBusy(false);
        }
    }

    private async Task LoadSettingsAsync()
    {
        StartupCheckBox.IsChecked = AgentService.IsStartupEnabled();
        AppEnabledCheckBox.IsChecked = await AgentService.IsAppEnabledAsync();
        EnforceCheckBox.IsChecked = await AgentService.IsEnforcementEnabledAsync();
        HideTrayCheckBox.IsChecked = !await AgentService.IsStatusIconVisibleAsync();
        ((App)Application.Current).SetStatusIconVisible(HideTrayCheckBox.IsChecked != true);
    }

    private void SetBusy(bool busy)
    {
        LoadingRing.IsActive = busy;
        ProfilePicker.IsEnabled = !busy;
        RememberButton.IsEnabled = false;
        AppEnabledCheckBox.IsEnabled = !busy;
        EnforceCheckBox.IsEnabled = !busy;
        StartupCheckBox.IsEnabled = !busy;
        HideTrayCheckBox.IsEnabled = !busy;
        if (!busy)
        {
            UpdateRememberButton();
            DispatcherQueue.TryEnqueue(() => LayoutReady?.Invoke(this, EventArgs.Empty));
        }
    }

    private void UpdateRememberButton()
    {
        if (SelectedProfile is not ProfileInfo profile)
        {
            RememberButton.Content = "Remember & activate";
            RememberButton.IsEnabled = false;
            return;
        }

        RememberButton.Content = (profile.Active, profile.Assigned) switch
        {
            (true, true) => "Up to date",
            (false, true) => "Activate",
            _ => "Remember & activate",
        };
        RememberButton.IsEnabled = !_loading && !(profile.Active && profile.Assigned);
    }

    private void ShowStatus(string message)
    {
        StatusText.Text = message;
        StatusOverlay.Visibility = Visibility.Visible;
    }

    private void HideStatus()
    {
        StatusText.Text = string.Empty;
        StatusOverlay.Visibility = Visibility.Collapsed;
    }

    private async Task RefreshProfilesAsync()
    {
        IReadOnlyList<ProfileInfo> profiles = await AgentService.GetProfilesAsync();
        int? linkedProfiles = await AgentService.GetLinkedProfileCountAsync();
        Profiles.Clear();
        foreach (ProfileInfo profile in profiles)
        {
            Profiles.Add(profile);
        }
        ProfilePicker.SelectedItem = Profiles.FirstOrDefault(profile => profile.Assigned)
            ?? Profiles.FirstOrDefault(profile => profile.Active)
            ?? Profiles.FirstOrDefault();
        ProfileSummaryText.Text = $"{Profiles.Count} / 4 onboard profiles";
        AppLinkingText.Text = linkedProfiles switch
        {
            > 0 => $"Conflict detected: {linkedProfiles} Wootility linked profile(s) can override this app.",
            0 => "Wootility App Linking: no linked profiles detected.",
            _ => "Wootility App Linking status is unavailable.",
        };
        if (Profiles.Count == 0)
        {
            ShowStatus("No configured onboard profiles found.");
        }
        else if (!Profiles.Any(profile => profile.Active))
        {
            ShowStatus("Keyboard status unavailable. Showing saved profile information.");
        }
        else
        {
            HideStatus();
        }
        UpdateRememberButton();
    }

    private void ProfilePicker_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        UpdateRememberButton();
        HideStatus();
    }

    private async void Remember_Click(object sender, RoutedEventArgs e)
    {
        if (SelectedProfile is not ProfileInfo profile)
        {
            return;
        }

        HideStatus();
        SetBusy(true);
        bool wasAssigned = profile.Assigned;
        try
        {
            await AgentService.ConfigureAsync(profile.Index, StartupCheckBox.IsChecked == true);
            _loading = true;
            await RefreshProfilesAsync();
            ShowStatus(wasAssigned
                ? $"P{profile.Index} activated and verified."
                : $"P{profile.Index} saved for Windows and verified.");
        }
        catch (Exception error)
        {
            _loading = true;
            await RefreshProfilesAsync();
            ShowStatus(error.Message);
        }
        finally
        {
            _loading = false;
            SetBusy(false);
        }
    }

    private async void Setting_Click(object sender, RoutedEventArgs e)
    {
        if (_loading)
        {
            return;
        }

        HideStatus();
        SetBusy(true);
        try
        {
            if (ReferenceEquals(sender, AppEnabledCheckBox))
            {
                await AgentService.SetAppEnabledAsync(AppEnabledCheckBox.IsChecked == true);
            }
            else if (ReferenceEquals(sender, EnforceCheckBox))
            {
                await AgentService.SetEnforcementEnabledAsync(EnforceCheckBox.IsChecked == true);
            }
            else if (ReferenceEquals(sender, StartupCheckBox))
            {
                AgentService.SetStartupEnabled(StartupCheckBox.IsChecked == true);
            }
            else if (ReferenceEquals(sender, HideTrayCheckBox))
            {
                bool visible = HideTrayCheckBox.IsChecked != true;
                await AgentService.SetStatusIconVisibleAsync(visible);
                ((App)Application.Current).SetStatusIconVisible(visible);
            }
        }
        catch (Exception error)
        {
            _loading = true;
            await LoadSettingsAsync();
            ShowStatus(error.Message);
        }
        finally
        {
            _loading = false;
            SetBusy(false);
        }
    }
}
