# Wooting Host Profile

A small platform-native configuration app and completely hidden background
watcher for choosing one Wooting onboard profile on Windows and another on
macOS.

The background agent communicates directly with the keyboard over USB/HID. It
does not use Wootility Web, Wooting Background Service, application matching,
telemetry, or the network at runtime.

## Native configuration window

Windows uses WinUI 3/XAML and Fluent controls. macOS uses SwiftUI. Both front
ends call the same Rust USB/background core.

Open **Wooting Host Profile** normally. The window:

- lists only the onboard profiles actually configured in Wootility;
- shows their Wootility names, such as `Typing Profile` and `mac profile`;
- marks the profile currently active on the connected keyboard;
- lets you select the profile assigned to the current operating system;
- can apply and verify the selection immediately;
- can enable a fully hidden watcher at sign-in;
- explains that Wootility App Linking and other app-specific profile overrides
  should remain disabled because they can override the system profile.

The Windows window has a fixed compact 540×520 layout. Minimizing or closing it
hides it from the taskbar while keeping it in the Windows notification area.
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

Profile names are read offline from Wootility Web's Chromium local-storage
cache. The app copies that database to a temporary directory before reading it,
so Chrome, Edge, or Brave may remain open. Open `wootility.io` and connect the
keyboard at least once before configuring this app. The hidden watcher does not
need the browser cache after a profile has been selected.

## Background behavior

The watcher starts with the user session and waits for the keyboard. When the
keyboard appears after startup, wake, reconnect, or a USB/KVM host switch, it:

1. reads the active onboard profile;
2. loads the profile selected for Windows or macOS;
3. changes and reloads the profile only when necessary;
4. reads the result back from the keyboard for verification;
5. remains idle until the keyboard disconnects or the profile must be enforced.

The watcher keeps the HID connection warm and exposes a loopback-only local
control channel on `127.0.0.1:50053`. The Fluent UI sends profile selections to
that persistent process instead of starting a fresh USB discovery operation.
Profile commands are acknowledged locally as soon as they are dispatched to
the HID transport, then verified in the watcher. On the tested 60HE+, packaged
P1/P2 requests complete through the UI command path in roughly 24–37 ms. USB
reconnect detection runs every 100 ms instead of every two seconds.

Only one watcher instance can run. Configuration is reloaded while it runs, so
changing the selection in the GUI does not require restarting the watcher.
Diagnostic events are written to `watch.log` beside the configuration file.

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
creates `~/Applications/Wooting Host Profile.app`, and opens it. When enabled
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
refresh_lighting = true
command_delay_ms = 250
enforce = false

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
