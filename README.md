# Wooting Switch

Wooting Switch automatically chooses the right onboard Wooting profile for the
computer you are using.

Use a gaming profile on Windows, move the keyboard to a Mac, and let the same
keyboard switch to your macOS profile automatically. Your profiles stay stored
on the keyboard and are still created and edited in Wootility.

## Download for Windows

1. Open the [latest release](https://github.com/notKleja/wooting-host-profile/releases/latest).
2. Download `Wooting-Switch-Setup-Windows-x64.exe`.
3. Run the installer.
4. Open **Wooting Switch**, choose a profile, and select **Remember & activate**.

The Windows build is currently unsigned, so Windows may show an Unknown
Publisher warning. The complete source code is available in this repository.

Prefer not to install it? Download the portable ZIP and run
`WootingHostProfile.WinUI.exe` directly.

## What it does

- Shows the onboard profiles already saved on your Wooting keyboard.
- Remembers one profile for Windows and one for macOS.
- Applies the saved profile after sign-in, wake, reconnect, or a USB/KVM switch.
- Skips the switch when the correct profile is already active.
- Lives quietly in the Windows tray or macOS menu bar.
- Works offline and does not send telemetry.
- Pauses safely while Wootility is editing the keyboard.
- Warns when Wootility App Linking could override your chosen profile.

## Important Wootility setting

Avoid using **Wootility App Linking** together with **Keep this profile active**.
Both features try to control the active profile. Wooting Switch detects linked
profiles and shows a warning when it finds a conflict.

Use Wootility to create, rename, reorder, or edit profiles. Wooting Switch only
selects profiles that already exist on the keyboard.

## macOS

The native SwiftUI app supports macOS 13 or newer. Install Xcode command-line
tools and Rust, then build, install, and open it with:

```sh
./scripts/install-macos.sh
```

The app is installed under `~/Applications`. To produce release artifacts, run:

```sh
./scripts/package-macos.sh
```

The command writes a portable ZIP, an installer PKG targeting `/Applications`,
and their SHA-256 checksums to `dist/macos`. The app is ad-hoc signed, while the
PKG is unsigned; a Developer ID signature and Apple notarization are not
included. Builds shared with other Macs may therefore require Gatekeeper
approval. See the
[macOS installation guide](https://github.com/notKleja/wooting-host-profile/wiki/macOS-Installation)
for more detail.

## Help and documentation

- [Getting started](https://github.com/notKleja/wooting-host-profile/wiki/Getting-Started)
- [Windows installation](https://github.com/notKleja/wooting-host-profile/wiki/Windows-Installation)
- [Troubleshooting](https://github.com/notKleja/wooting-host-profile/wiki/Troubleshooting)
- [How it works](https://github.com/notKleja/wooting-host-profile/wiki/How-It-Works)
- [Privacy and security](https://github.com/notKleja/wooting-host-profile/wiki/Privacy-and-Security)
- [Full wiki](https://github.com/notKleja/wooting-host-profile/wiki)

## Compatibility

The Windows release has been tested with Windows 11 and a Wooting 60HE+. Other
modern Wooting keyboards using the same onboard-profile protocol should work,
but have not all been tested individually.

This is an independent community project and is not an official Wooting product.

## License

[MIT](LICENSE). Third-party notices are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
