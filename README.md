# Wooting Switch

A small platform-native configuration app and completely hidden background
watcher for choosing one Wooting onboard profile on Windows and another on
macOS.

The background agent communicates directly with the keyboard over USB/HID. It
does not use Wootility Web, Wooting Background Service, application matching,
telemetry, or the network at runtime.

## Native configuration window

Windows uses WinUI 3/XAML and Fluent controls. macOS uses SwiftUI. Both front
ends call the same Rust USB/background core.

Open **Wooting Switch** normally. The window:

- lists only the onboard profiles actually configured in Wootility;
- shows their Wootility names, such as `Typing Profile` and `mac profile`;
- selects the operating-system profile from one compact dropdown;
- saves and applies that selection with the adjacent **Remember** button;
- can disable automatic switching without stopping the app;
- can periodically restore the selected profile after a safe cooldown;
- can enable the fully hidden watcher at sign-in;
- can hide its Windows tray or macOS Dock/menu icon;
- explains that Wootility App Linking and other app-specific profile overrides
  should remain disabled because they can override the system profile.

The Windows window opens at a compact preferred width and the final rendered
content height. Those values become its minimum size; the window may be made
larger, while content stretches with the available width and scrolls only when
the display work area cannot fit it. Minimizing or closing it hides it from the
taskbar while keeping it in the Windows notification area.
Left-click the custom `P` tray icon to reopen it, or right-click it for **Open**
and **Exit**. At sign-in the WinUI app can start directly in tray mode.

The Rust watcher itself has no window, console, or tray icon. It is managed by
the WinUI/SwiftUI front end.

The application icon is an original angular profile-switch monogram inspired
by Wooting's energetic industrial design language without reproducing its
trademarked `w` symbol. It uses a fully opaque selective-yellow `#FFB900` glyph
on a fully opaque near-black `#09090B` tile. Transparency is used only outside
the rounded-square silhouette—there are no translucent shadows, gradients,
glows, or highlights.

Profile names and populated onboard slots are read directly from keyboard
metadata. If a keyboard or firmware does not expose that metadata, the app
falls back to Wootility Web's Chromium cache, then its own persisted last-known
names, and finally usable generic P1-P4 labels. The UI and watcher therefore do
not require a browser, Wootility, or network access for supported keyboards.

## Background behavior

When automatic switching is enabled, the watcher starts with the user session
and waits for the keyboard. When the keyboard appears after startup, wake,
reconnect, or a USB/KVM host switch, it:

1. reads the active onboard profile;
2. loads the profile selected for Windows or macOS;
3. changes and reloads the profile only when necessary;
4. reads the result back from the keyboard for verification;
5. remains idle until the keyboard disconnects or the profile must be enforced.

The watcher keeps the HID connection warm and exposes a loopback-only local
control channel on `127.0.0.1:50053`. The Fluent UI sends profile selections to
that persistent process instead of starting a fresh USB discovery operation.
The watcher acknowledges a selection only after the keyboard has accepted the
profile and lighting commands and the active profile index has been read back.
USB reconnect detection runs every 100 ms instead of every two seconds.

Only one watcher instance can run. Configuration is reloaded while it runs, so
changing the selection in the GUI does not require restarting the watcher.
Diagnostic events are written to `watch.log` beside the configuration file.
The native UI is also single-instance. Launching it again foregrounds the
existing window, including when its tray, Dock, or menu icon is hidden.

## Windows installation

The packaged Windows distribution includes the native app, Rust agent, bundled
WinUI runtime, and installation helper. Extract it, then run:

```powershell
.\scripts\install-windows.ps1
```

The helper copies the prebuilt native app to
`%LOCALAPPDATA%\WootingHostProfile`, adds a Start-menu shortcut, and opens the
configuration window. The app itself enables or disables automatic startup
when you save. The distribution bundles its WinUI runtime components, avoiding
a separate Windows App Runtime prompt.

Generated binaries are intentionally excluded from the source repository. To
create the Windows distribution yourself, follow `BUILD_NOTES.md` and publish
the WinUI project into `dist/windows` before running the helper.

## macOS installation

Install the stable Rust toolchain and Xcode command-line tools, then run:

```bash
chmod +x ./scripts/install-macos.sh
./scripts/install-macos.sh
```

The helper compiles the shared Rust agent and the native SwiftUI front end,
creates `~/Applications/Wooting Switch.app`, and opens it. When enabled
in the GUI, the agent creates a per-user LaunchAgent for the invisible watcher.

The macOS build is intentionally local and unsigned. The source is identical
to the tested Windows build, but USB/HID behavior still needs to be verified on
the target Mac.

## Optional command-line interface

The GUI and watcher are the primary interface. The underlying commands remain
available for diagnostics and scripting:

```text
wooting-host-profile status
wooting-host-profile profiles
wooting-host-profile profiles --json
wooting-host-profile startup-status
wooting-host-profile enabled-status
wooting-host-profile set-enabled enable
wooting-host-profile set-enabled disable
wooting-host-profile enforce-status
wooting-host-profile set-enforce enable
wooting-host-profile set-enforce disable
wooting-host-profile status-icon-status
wooting-host-profile set-status-icon show
wooting-host-profile set-status-icon hide
wooting-host-profile learn
wooting-host-profile learn --profile 2
wooting-host-profile apply
wooting-host-profile watch
wooting-host-profile watch --enforce
wooting-host-profile config
wooting-host-profile configure --profile 2 --startup enable
wooting-host-profile set --profile 2
```

## Configuration

The configuration uses human-facing profile numbers. Only configured profile
slots are offered by the native UI:

```toml
enabled = true
show_status_icon = true
refresh_lighting = true
command_delay_ms = 250
enforce = false
enforce_interval_ms = 5000

[profiles]
windows = 2
macos = 1
```

The settings are stored under the normal per-user configuration directory:

- Windows: `%APPDATA%\WootingHostProfile\config.toml`
- macOS: `~/Library/Application Support/WootingHostProfile/config.toml`

## Building manually

Build the shared Rust agent:

```text
cargo build --release
```

The Windows SDK build also needs Visual Studio Build Tools and `libclang` for
the upstream `wooting-rgb-sys` binding generator. The WinUI 3 front end is in
`windows/WootingHostProfile.WinUI` and requires the .NET 9 SDK:

```text
dotnet publish -c Release -p:Platform=x64 -r win-x64 --self-contained false
```

## USB protocol

This project uses the Wooting RGB SDK's low-level USB transport and the profile
commands independently documented by the MIT-licensed Wooting Profile Switcher:

- `11`: read current onboard profile index
- `23`: activate profile
- `7`: reload profile
- `32` and `29`: reset/refresh profile lighting

## Attribution

The profile-command sequence and multi-report response handling were adapted
from ShayBox/Wooting-Profile-Switcher, licensed under the MIT License:
https://github.com/ShayBox/Wooting-Profile-Switcher
