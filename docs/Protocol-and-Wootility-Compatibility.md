# Protocol and Wootility Compatibility

Wooting Switch selects profiles already stored on a Wooting keyboard. It does
not edit mappings, actuation settings, RGB layers, firmware, or App Linking.
Use Wootility for those operations.

## Modern switch pipeline

On ARM and multi-report devices such as the tested Wooting 60HE+, a switch is
performed synchronously:

1. `33` (`0x21`) — `WootDevInit`; wait for acknowledgement.
2. `23` (`0x17`) — `ActivateProfile`; wait for acknowledgement.
3. Wait 100 ms for the firmware to settle.
4. `38` (`0x26`) — `ReloadProfile`; wait for acknowledgement.
5. `73` (`0x49`) — heartbeat; verify the runtime-active profile.
6. `32` (`0x20`) — `WootDevResetAll`; release third-party RGB ownership.

If modern reload is rejected, command `7` (`ReloadProfile0`) is attempted as a
legacy fallback. Legacy devices may also require command `29`
(`RefreshRgbColors`) after the ownership reset.

## State queries

Modern multi-report firmware supports command `73`, the same heartbeat used by
current Wootility. Its protobuf response begins with varint fields:

- field 1: runtime-active onboard profile index;
- field 2: whether WootDev/third-party RGB ownership is active.

The tested 60HE+ returned these bodies:

```text
WootDev inactive: 08 00 10 00 18 00 20 00
WootDev active:   08 00 10 01 18 00 20 00
```

Devices without heartbeat support fall back to command `11`
(`GetCurrentKeyboardProfileIndex`). On ARM firmware its runtime profile is at
payload offset 2; payload offset 0 is the flash-stored default.

Legacy state does not reveal WootDev ownership. Wooting Switch therefore treats
ownership as unknown and does not perform periodic enforcement on that state;
startup, reconnect, and explicit profile selections still work. A multi-report
heartbeat parse or command-correlation failure is not silently converted into a
legacy state, because doing so could incorrectly claim that Wootility is idle.

## Profile names

Profile names and populated slots are read with command `55`
(`GetProfileMetadata`). The response contains a protobuf field-1 UTF-8 name.

Fallback order:

1. keyboard metadata;
2. Wootility Chromium cache;
3. Wooting Switch's last-known names in `config.toml`;
4. generic `Profile 1` through `Profile 4` labels.

A browserless test with an empty Chromium directory and a fresh config returned
`Typing Profile` and `mac profile` directly from the connected keyboard.

## Wootility coexistence

Wootility also uses command `23` for activation and command `73` for modern
state polling. Captured console messages such as `Expected: 73, got 11` were
caused by the older Wooting Switch watcher polling command `11` every 100 ms
while Wootility was waiting on the same HID response channel.

The watcher now:

- polls modern state with command `73` once per second;
- keeps command `11` only as a legacy fallback;
- detects WootDev ownership from heartbeat field 2;
- treats missing or malformed ownership as unknown and suppresses enforcement;
- suspends enforcement and backs off to the configured safe cooldown while
  Wootility owns WootDev;
- releases its own WootDev state with command `32` after switching.

Wootility App Linking and Wooting Switch enforcement should not both be enabled.
Both features intentionally select profiles, so they would compete after
Wootility releases its active editing session.

## Command reference

| Decimal | Hex | Meaning | Usage |
|---:|---:|---|---|
| 7 | `0x07` | `ReloadProfile0` | Legacy reload fallback |
| 11 | `0x0B` | `GetCurrentKeyboardProfileIndex` | Legacy state fallback |
| 23 | `0x17` | `ActivateProfile` | Activate requested slot |
| 29 | `0x1D` | `RefreshRgbColors` | Legacy lighting fallback |
| 32 | `0x20` | `WootDevResetAll` | Release RGB ownership |
| 33 | `0x21` | `WootDevInit` | Initialize switch session |
| 38 | `0x26` | `ReloadProfile` | Modern full profile reload |
| 55 | `0x37` | `GetProfileMetadata` | Read slot name and presence |
| 72 | `0x48` | Profile commit | Wootility profile writes only |
| 73 | `0x49` | Idle/state heartbeat | Profile and WootDev state |
| 74 | `0x4A` | Save-progress poll | Wootility profile writes only |

## Verification

The modern sequence was tested on a Wooting 60HE+:

```text
P1 -> P2: command path succeeded; heartbeat readback P2
P2 -> P1: command path succeeded; heartbeat readback P1
```

The saved Windows profile and all configuration values were restored after the
test.
