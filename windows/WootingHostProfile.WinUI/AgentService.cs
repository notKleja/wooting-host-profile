using System.Diagnostics;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.Win32;

namespace WootingHostProfile_WinUI;

public sealed class ProfileInfo
{
    [JsonPropertyName("index")]
    public int Index { get; set; }

    [JsonPropertyName("name")]
    public string Name { get; set; } = string.Empty;

    [JsonPropertyName("active")]
    public bool Active { get; set; }

    [JsonPropertyName("assigned")]
    public bool Assigned { get; set; }

    public string DisplayName => $"P{Index} — {Name}";

    public string SlotLabel => $"P{Index}";

    public string ActiveLabel => Active ? "ACTIVE" : string.Empty;

    public string StateText => (Active, Assigned) switch
    {
        (true, true) => "ACTIVE · WINDOWS DEFAULT",
        (true, false) => "ACTIVE",
        (false, true) => "WINDOWS DEFAULT",
        _ => string.Empty,
    };

}

public static class AgentService
{
    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string RunValue = "Wooting Switch";
    private const string LegacyRunValue = "Wooting Host Profile";

    public static string AgentPath => Path.Combine(AppContext.BaseDirectory, "wooting-host-profile-agent.exe");

    private static ProcessStartInfo CreateStartInfo(IEnumerable<string> arguments, bool capture)
    {
        if (!File.Exists(AgentPath))
        {
            throw new FileNotFoundException("The Wooting background agent is missing.", AgentPath);
        }
        var info = new ProcessStartInfo(AgentPath)
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            WindowStyle = ProcessWindowStyle.Hidden,
            RedirectStandardOutput = capture,
            RedirectStandardError = capture,
        };
        foreach (string argument in arguments)
        {
            info.ArgumentList.Add(argument);
        }
        return info;
    }

    public static async Task<string> RunAsync(params string[] arguments)
    {
        using var process = Process.Start(CreateStartInfo(arguments, capture: true))
            ?? throw new InvalidOperationException("Could not start the Wooting background agent.");
        string output = await process.StandardOutput.ReadToEndAsync();
        string error = await process.StandardError.ReadToEndAsync();
        await process.WaitForExitAsync();
        if (process.ExitCode != 0)
        {
            throw new InvalidOperationException(string.IsNullOrWhiteSpace(error)
                ? "The Wooting background agent failed."
                : error.Trim());
        }
        return output.Trim();
    }

    public static async Task<IReadOnlyList<ProfileInfo>> GetProfilesAsync()
    {
        string json = await RunAsync("profiles", "--json");
        return JsonSerializer.Deserialize<List<ProfileInfo>>(json)
            ?? throw new InvalidOperationException("The agent returned no profile list.");
    }

    public static async Task<bool> IsAppEnabledAsync()
    {
        string enabled = await RunAsync("enabled-status");
        return bool.TryParse(enabled, out bool result) && result;
    }

    public static async Task SetAppEnabledAsync(bool enabled)
    {
        _ = await RunAsync("set-enabled", enabled ? "enable" : "disable");
    }

    public static async Task<bool> IsEnforcementEnabledAsync()
    {
        string enabled = await RunAsync("enforce-status");
        return bool.TryParse(enabled, out bool result) && result;
    }

    public static async Task SetEnforcementEnabledAsync(bool enabled)
    {
        _ = await RunAsync("set-enforce", enabled ? "enable" : "disable");
    }

    public static async Task<bool> IsStatusIconVisibleAsync()
    {
        string visible = await RunAsync("status-icon-status");
        return bool.TryParse(visible, out bool result) && result;
    }

    public static async Task SetStatusIconVisibleAsync(bool visible)
    {
        _ = await RunAsync("set-status-icon", visible ? "show" : "hide");
    }

    public static async Task ConfigureAsync(int profile, bool startAtLogin)
    {
        SetStartupEnabled(startAtLogin);
        await RunAsync(
            "configure", "--profile", profile.ToString(),
            "--startup", "keep");
        EnsureWatcher();
    }

    public static void EnsureWatcher()
    {
        Process.Start(CreateStartInfo(["watch"], capture: false));
    }

    public static bool IsStartupEnabled()
    {
        using RegistryKey? key = Registry.CurrentUser.OpenSubKey(RunKey, writable: true);
        if (key?.GetValue(RunValue) is string)
        {
            return true;
        }
        if (key?.GetValue(LegacyRunValue) is string legacyCommand)
        {
            key.SetValue(RunValue, legacyCommand, RegistryValueKind.String);
            key.DeleteValue(LegacyRunValue, throwOnMissingValue: false);
            return true;
        }
        return false;
    }

    public static void SetStartupEnabled(bool enabled)
    {
        using RegistryKey key = Registry.CurrentUser.CreateSubKey(RunKey, writable: true);
        if (enabled)
        {
            string executable = Environment.ProcessPath
                ?? throw new InvalidOperationException("Could not resolve the WinUI application path.");
            key.SetValue(RunValue, $"\"{executable}\" --tray", RegistryValueKind.String);
            key.DeleteValue(LegacyRunValue, throwOnMissingValue: false);
        }
        else
        {
            key.DeleteValue(RunValue, throwOnMissingValue: false);
            key.DeleteValue(LegacyRunValue, throwOnMissingValue: false);
        }
    }
}
