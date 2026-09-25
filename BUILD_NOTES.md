# Build and verification notes

Version 0.4.0 was verified on Windows against a connected Wooting 60HE+ on
2026-09-25.

## Profile enumeration

The live Wootility 5.4.2 profile page reported two configured profiles out of
four possible onboard slots:

- P1: `Typing Profile`;
- P2: `mac profile`;
- P3 and P4: empty;
- linked/app profiles: `0 / 10`.

The Rust agent returned exactly those two configured profiles and marked the
active slot. Empty capacity slots were not shown.

A browserless test with an empty Chromium data directory and fresh config read
`Typing Profile` and `mac profile` directly from keyboard metadata and cached
those names locally for future offline fallback.

## Windows WinUI 3 and tray behavior

- The configuration front end uses WinUI 3/XAML and Fluent controls.
- The published app launched successfully from its installed location.
- The main window opened with the title `Wooting Switch`.
- The WinUI process remained responsive.
- The Rust HID watcher launched hidden from the installed app directory.
- Sending a normal close request removed the window from the taskbar while the
  WinUI notification-area process and Rust watcher both remained alive.
- The window opens at a compact preferred client width and the measured final
  content height. Those become DPI-aware minimum dimensions; the window can be
  enlarged, while the content stretches and scrolls when required.
- Startup is managed by the WinUI app and launches it with `--tray`.
- A named instance mutex prevents duplicate WinUI processes; a second launch
  signals the existing process to show and activate its window.
- The tray icon can be hidden persistently and a later app launch still restores
  the existing hidden window through the single-instance signal.

## Icon verification

- Windows PNG and multi-resolution ICO assets were generated.
- macOS PNG and ICNS assets were generated.
- The tile uses solid near-black `#09090B`.
- The angular profile-switch glyph uses solid selective yellow `#FFB900`.
- The mark is an original chamfered P/switch-arrow monogram inspired by
  Wooting's industrial visual language without copying its protected `w` logo.
- Alpha values are exactly `0` or `255`; transparency exists only outside the
  rounded-square silhouette.
- The `P`, tile, and all interior pixels are fully opaque. There are no
  translucent shadows, gradients, glows, bevels, or highlights.

## Keyboard and agent

- Direct HID status read detected the active onboard profile.
- The original implementation measured 256–1,260 ms because it repeatedly
  discovered the keyboard, copied Wootility's browser database, inserted fixed
  250 ms sleeps, and waited synchronously for every HID acknowledgement.
- Profile validation was removed from the switching hot path; the UI already
  supplies a profile returned by enumeration.
- The watcher now keeps the HID device selected and accepts loopback-only IPC
  commands on `127.0.0.1:50053`.
- Activate and reload commands are dispatched without a fixed delay, then the
  active profile index is read back. When lighting refresh is enabled, the
  watcher waits for and validates the RGB reset and refresh acknowledgements
  before reporting success to the UI.
- Reconnect polling was reduced from 2,000 ms to 100 ms.
- The hidden watcher detected the keyboard and verified the configured profile.
- A second watcher exited while the first held the single-instance lock.
- Background logging succeeded.

## Code quality

- Three Rust unit tests passed.
- `cargo fmt --check` passed.
- `cargo clippy --release -- -D warnings` passed.
- The optimized Rust release build completed successfully.
- The WinUI project built and published successfully.
- The Windows installer parsed and installed successfully.

The upstream `wooting-rgb-sys` static library emits Microsoft linker LNK4217
warnings. They do not prevent compilation or the tested USB/profile operations.

The SwiftUI macOS front end and macOS HID behavior must be compiled and tested
on the target Mac; they cannot be validated from this Windows host.
