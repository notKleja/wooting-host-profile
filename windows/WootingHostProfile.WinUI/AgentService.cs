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

    public string DisplayName => Active
        ? $"P{Index} — {Name}   ·   Active now"
        : $"P{Index} — {Name}";
}

public static class AgentService
{
    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string RunValue = "Wooting Host Profile";

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

    public static async Task ApplyAsync(int profile)
    {
        _ = await RunAsync("set", "--profile", profile.ToString());
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
        using RegistryKey? key = Registry.CurrentUser.OpenSubKey(RunKey);
        return key?.GetValue(RunValue) is string;
    }

    public static void SetStartupEnabled(bool enabled)
    {
        using RegistryKey key = Registry.CurrentUser.CreateSubKey(RunKey, writable: true);
        if (enabled)
        {
            string executable = Environment.ProcessPath
                ?? throw new InvalidOperationException("Could not resolve the WinUI application path.");
            key.SetValue(RunValue, $"\"{executable}\" --tray", RegistryValueKind.String);
        }
        else
        {
            key.DeleteValue(RunValue, throwOnMissingValue: false);
        }
    }
}
