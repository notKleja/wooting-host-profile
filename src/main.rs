#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs::{self, OpenOptions},
    hash::{DefaultHasher, Hash, Hasher},
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use encoding_rs::{UTF_16LE, WINDOWS_1252};
use fs2::FileExt;
#[cfg(target_os = "macos")]
use hidapi::{HidApi, HidDevice};
use rusty_leveldb::{CompressorId, DB, LdbIterator, Options, compressor::SnappyCompressor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(not(target_os = "macos"))]
use wooting_rgb_sys as rgb;

const GET_CURRENT_KEYBOARD_PROFILE_INDEX: u8 = 11;
const RELOAD_PROFILE_LEGACY: u8 = 7;
const ACTIVATE_PROFILE: u8 = 23;
const REFRESH_RGB_COLORS: u8 = 29;
const WOOT_DEV_RESET_ALL: u8 = 32;
const WOOT_DEV_INIT: u8 = 33;
const RELOAD_PROFILE: u8 = 38;
const GET_PROFILE_METADATA: u8 = 55;
const HEARTBEAT: u8 = 73;
const PROFILE_COUNT: u8 = 4;
const IPC_PORT_BASE: u16 = 49_152;
const IPC_PORT_COUNT: u16 = 16_384;
const IPC_REQUEST_LIMIT: u64 = 128;
const MAX_PROFILE_NAME_BYTES: usize = 256;
const MIN_COMMAND_DELAY_MS: u64 = 10;
const MAX_COMMAND_DELAY_MS: u64 = 5_000;
const MIN_ENFORCE_INTERVAL_MS: u64 = 1_000;
const MAX_ENFORCE_INTERVAL_MS: u64 = 3_600_000;
const MAX_LOG_BYTES: u64 = 1_048_576;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const WOOTING_VENDOR_IDS: [u16; 2] = [0x03EB, 0x31E3];
const WOOTING_CONFIG_USAGE_PAGE: u16 = 0x1337;
const WOOTING_V3_CONFIG_USAGE_PAGE: u16 = 0xFF55;
const MAX_HID_RESPONSE_SIZE: usize = 2_047;

fn validated_response_length(response: i32, buffer_size: usize) -> Result<usize> {
    let length = usize::try_from(response).context("keyboard response read failed")?;
    if length == 0 {
        bail!("keyboard response timed out")
    }
    if length > buffer_size {
        bail!("keyboard response exceeded its allocated buffer")
    }
    Ok(length)
}

fn command_report(multi_report: bool, command: u8, profile: u8) -> [u8; 8] {
    [
        u8::from(multi_report),
        if multi_report { 0xD1 } else { 0xD0 },
        0xDA,
        command,
        profile,
        0,
        0,
        0,
    ]
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    /// Use a specific configuration file.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List configured onboard profiles discovered from Wootility's local cache.
    Profiles {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Report how many Wootility App Linking profiles are configured.
    LinkedStatus,

    /// Report whether the hidden watcher is registered to start at login.
    StartupStatus,

    /// Report whether automatic profile switching is enabled.
    EnabledStatus,

    /// Enable or disable automatic profile switching.
    SetEnabled {
        #[arg(value_enum)]
        state: EnabledChoice,
    },

    /// Report whether the saved profile is periodically enforced.
    EnforceStatus,

    /// Enable or disable periodic profile enforcement.
    SetEnforce {
        #[arg(value_enum)]
        state: EnabledChoice,
    },

    /// Report whether the app's tray or menu-bar icon should be shown.
    StatusIconStatus,

    /// Show or hide the app's tray or menu-bar icon.
    SetStatusIcon {
        #[arg(value_enum)]
        state: StatusIconChoice,
    },

    /// Show the connected keyboard's current onboard profile.
    Status,

    /// Remember the currently selected profile for this operating system.
    Learn {
        /// Learn this profile number instead of reading the keyboard (P1-P4).
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=PROFILE_COUNT as i64))]
        profile: Option<u8>,
    },

    /// Apply the profile configured for this operating system once.
    Apply,

    /// Apply on startup and whenever the keyboard reconnects.
    Watch {
        /// Milliseconds between lightweight device checks.
        #[arg(long = "interval-ms", visible_alias = "interval", default_value_t = 1_000, value_parser = clap::value_parser!(u64).range(100..))]
        interval_ms: u64,

        /// Reapply after a manual profile change, not only after reconnect.
        #[arg(long)]
        enforce: bool,
    },

    /// Print the configuration path and current configuration.
    Config,

    /// Save a profile for this OS, configure startup, apply it, and start watching.
    Configure {
        /// Human-facing profile number, starting at 1.
        #[arg(long)]
        profile: u8,
        /// Keep, enable, or disable automatic startup.
        #[arg(long, value_enum, default_value_t = StartupChoice::Keep)]
        startup: StartupChoice,
    },

    /// Apply a profile immediately without saving it.
    Set {
        /// Human-facing profile number, starting at 1.
        #[arg(long)]
        profile: u8,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum StartupChoice {
    #[default]
    Keep,
    Enable,
    Disable,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum EnabledChoice {
    Enable,
    Disable,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum StatusIconChoice {
    Show,
    Hide,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProfileInfo {
    /// Human-facing profile number.
    index: u8,
    name: String,
    active: bool,
    assigned: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct Config {
    /// Apply the saved OS profile automatically.
    #[serde(default = "default_true")]
    enabled: bool,

    /// Show the app icon in the Windows tray or macOS menu/Dock area.
    #[serde(default = "default_true")]
    show_status_icon: bool,

    #[serde(default)]
    profiles: Profiles,

    /// Last profile names read from the keyboard or Wootility cache.
    #[serde(default)]
    profile_names: BTreeMap<u8, String>,

    /// Refresh the profile's RGB state after switching.
    #[serde(default = "default_true")]
    refresh_lighting: bool,

    /// Delay between firmware commands.
    #[serde(default = "default_command_delay_ms")]
    command_delay_ms: u64,

    /// Reapply if the profile changes while the keyboard stays connected.
    #[serde(default)]
    enforce: bool,

    /// Minimum delay between enforcement attempts.
    #[serde(default = "default_enforce_interval_ms")]
    enforce_interval_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Profiles {
    /// Human-facing profile number, P1-P4.
    windows: Option<u8>,
    /// Human-facing profile number, P1-P4.
    macos: Option<u8>,
}

const fn default_true() -> bool {
    true
}

const fn default_command_delay_ms() -> u64 {
    250
}

const fn default_enforce_interval_ms() -> u64 {
    5_000
}

impl Config {
    fn validate(&self) -> Result<()> {
        if let Some(profile) = self.profiles.windows {
            validate_human_profile(profile).context("invalid Windows profile in configuration")?;
        }
        if let Some(profile) = self.profiles.macos {
            validate_human_profile(profile).context("invalid macOS profile in configuration")?;
        }
        if !(MIN_COMMAND_DELAY_MS..=MAX_COMMAND_DELAY_MS).contains(&self.command_delay_ms) {
            bail!(
                "command_delay_ms must be between {MIN_COMMAND_DELAY_MS} and {MAX_COMMAND_DELAY_MS}"
            )
        }
        if !(MIN_ENFORCE_INTERVAL_MS..=MAX_ENFORCE_INTERVAL_MS).contains(&self.enforce_interval_ms)
        {
            bail!(
                "enforce_interval_ms must be between {MIN_ENFORCE_INTERVAL_MS} and {MAX_ENFORCE_INTERVAL_MS}"
            )
        }
        for (&profile, name) in &self.profile_names {
            validate_human_profile(profile).context("invalid cached profile index")?;
            validate_profile_name(name).context("invalid cached profile name")?;
        }
        Ok(())
    }

    fn configured_profile(&self) -> Result<u8> {
        let profile = match std::env::consts::OS {
            "windows" => self.profiles.windows,
            "macos" => self.profiles.macos,
            os => bail!("unsupported operating system: {os}"),
        };

        let profile = profile.ok_or_else(|| {
            anyhow!(
                "no profile learned for {}; select it on the keyboard, then run `wooting-host-profile learn`",
                std::env::consts::OS
            )
        })?;

        validate_human_profile(profile)?;
        Ok(profile - 1)
    }

    fn learn_for_current_os(&mut self, human_profile: u8) -> Result<()> {
        validate_human_profile(human_profile)?;
        match std::env::consts::OS {
            "windows" => self.profiles.windows = Some(human_profile),
            "macos" => self.profiles.macos = Some(human_profile),
            os => bail!("unsupported operating system: {os}"),
        }
        Ok(())
    }
}

fn validate_human_profile(profile: u8) -> Result<()> {
    if (1..=PROFILE_COUNT).contains(&profile) {
        Ok(())
    } else {
        bail!("profile must be between P1 and P{PROFILE_COUNT}, got P{profile}")
    }
}

fn validate_profile_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("profile name must not be empty")
    }
    if name.len() > MAX_PROFILE_NAME_BYTES {
        bail!("profile name exceeds {MAX_PROFILE_NAME_BYTES} UTF-8 bytes")
    }
    if name.chars().any(char::is_control) {
        bail!("profile name contains control characters")
    }
    Ok(())
}

fn config_path(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return std::path::absolute(path).context("could not resolve the configuration path");
    }

    let base =
        dirs::config_dir().context("could not determine the user configuration directory")?;
    Ok(base.join("WootingHostProfile").join("config.toml"))
}

fn default_config() -> Config {
    Config {
        enabled: default_true(),
        show_status_icon: default_true(),
        profiles: Profiles::default(),
        profile_names: BTreeMap::new(),
        refresh_lighting: default_true(),
        command_delay_ms: default_command_delay_ms(),
        enforce: false,
        enforce_interval_ms: default_enforce_interval_ms(),
    }
}

fn load_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(default_config());
    }

    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read configuration from {}", path.display()))?;
    let config: Config = toml::from_str(&text)
        .with_context(|| format!("failed to parse configuration at {}", path.display()))?;
    config
        .validate()
        .with_context(|| format!("invalid configuration at {}", path.display()))?;
    Ok(config)
}

fn config_lock(path: &Path) -> Result<fs::File> {
    let parent = path.parent().context("configuration path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(parent.join("config.lock"))?;
    FileExt::lock_exclusive(&lock).context("failed to lock the configuration")?;
    Ok(lock)
}

fn write_config_atomic(path: &Path, config: &Config) -> Result<()> {
    config.validate()?;
    let parent = path.parent().context("configuration path has no parent")?;
    let text = toml::to_string_pretty(config).context("failed to serialize configuration")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create a temporary file in {}", parent.display()))?;
    temporary.write_all(text.as_bytes())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to atomically replace {}", path.display()))?;
    Ok(())
}

fn update_config(path: &Path, mutate: impl FnOnce(&mut Config) -> Result<()>) -> Result<Config> {
    let _lock = config_lock(path)?;
    let mut config = load_config(path)?;
    mutate(&mut config)?;
    write_config_atomic(path, &config)?;
    Ok(config)
}

fn decode_chromium_string(bytes: &[u8]) -> Result<Cow<'_, str>> {
    let prefix = bytes.first().context("Chromium value was empty")?;
    match prefix {
        0 => Ok(UTF_16LE.decode(&bytes[1..]).0),
        1 => Ok(WINDOWS_1252.decode(&bytes[1..]).0),
        value => bail!("unsupported Chromium string prefix {value}"),
    }
}

fn chromium_user_data_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let Some(local) = dirs::data_local_dir() else {
            return Vec::new();
        };
        return vec![
            local.join("Google/Chrome/User Data"),
            local.join("Microsoft/Edge/User Data"),
            local.join("BraveSoftware/Brave-Browser/User Data"),
        ];
    }

    #[cfg(target_os = "macos")]
    {
        let Some(config) = dirs::config_dir() else {
            return Vec::new();
        };
        return vec![
            config.join("Google/Chrome"),
            config.join("Microsoft Edge"),
            config.join("BraveSoftware/Brave-Browser"),
        ];
    }

    #[allow(unreachable_code)]
    Vec::new()
}

fn wootility_leveldb_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in chromium_user_data_roots() {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name != "Default" && !name.starts_with("Profile ") {
                continue;
            }
            let path = entry.path().join("Local Storage/leveldb");
            if path.exists() {
                paths.push(path);
            }
        }
    }
    paths
}

fn copy_leveldb(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || entry.file_name() == "LOCK" {
            continue;
        }
        fs::copy(entry.path(), destination.join(entry.file_name()))?;
    }
    Ok(())
}

struct WootilityProfileCache {
    onboard: Vec<(u8, String)>,
    linked_count: Option<usize>,
}

fn load_profile_cache_from_leveldb(source: &Path) -> Result<WootilityProfileCache> {
    const WEB_KEY: &[u8] = b"_https://wootility.io\x00\x01persist:root";
    const WEB_KEY_SUFFIX: &[u8] = b"persist:root";
    let temporary = tempfile::tempdir()?;
    copy_leveldb(source, temporary.path())?;

    let options = Options {
        compressor: SnappyCompressor::ID,
        create_if_missing: false,
        paranoid_checks: false,
        ..Default::default()
    };
    let mut database = DB::open(temporary.path(), options)?;
    let root = if let Some(encoded) = database.get(WEB_KEY) {
        let decoded = decode_chromium_string(&encoded)?;
        serde_json::from_str(&decoded)?
    } else {
        let mut iterator = database.new_iter()?;
        let mut discovered = None;
        while let Some((key, encoded)) = iterator.next() {
            if !key.ends_with(WEB_KEY_SUFFIX) {
                continue;
            }
            let Ok(decoded) = decode_chromium_string(&encoded) else {
                continue;
            };
            let Ok(candidate) = serde_json::from_str::<Value>(&decoded) else {
                continue;
            };
            if candidate.get("devices").is_some() && candidate.get("profiles").is_some() {
                discovered = Some(candidate);
                break;
            }
        }
        discovered.context("Wootility Web state was not present in this browser profile")?
    };

    let devices_text = root
        .get("devices")
        .and_then(Value::as_str)
        .context("Wootility device state was missing")?;
    let devices: Value = serde_json::from_str(devices_text)?;
    let active_device = devices
        .get("activeDeviceId")
        .and_then(Value::as_str)
        .context("Wootility did not identify an active keyboard")?;

    let profiles_text = root
        .get("profiles")
        .and_then(Value::as_str)
        .context("Wootility profile state was missing")?;
    let profiles: Value = serde_json::from_str(profiles_text)?;
    let device_profiles = profiles
        .get("devices")
        .and_then(|value| value.get(active_device))
        .context("Wootility had no profiles for the active keyboard")?;
    let onboard = device_profiles
        .get("onboard")
        .and_then(Value::as_array)
        .context("Wootility had no onboard profiles for the active keyboard")?;

    let onboard = onboard
        .iter()
        .take(usize::from(PROFILE_COUNT))
        .enumerate()
        .filter_map(|(index, profile)| {
            let human_index = u8::try_from(index + 1).ok()?;
            let name = profile
                .get("details")
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)?;
            Some(validate_profile_name(name).map(|()| (human_index, name.to_owned())))
        })
        .collect::<Result<Vec<_>>>()?;
    let linked_count = device_profiles
        .get("linked")
        .and_then(Value::as_array)
        .map(Vec::len);
    Ok(WootilityProfileCache {
        onboard,
        linked_count,
    })
}

fn load_profiles_from_leveldb(source: &Path) -> Result<Vec<(u8, String)>> {
    Ok(load_profile_cache_from_leveldb(source)?.onboard)
}

fn discover_linked_profile_count() -> Option<usize> {
    wootility_leveldb_paths().into_iter().find_map(|path| {
        load_profile_cache_from_leveldb(&path)
            .ok()
            .and_then(|cache| cache.linked_count)
    })
}

fn profile_info(
    profiles: Vec<(u8, String)>,
    current: Option<u8>,
    assigned: Option<u8>,
) -> Vec<ProfileInfo> {
    profiles
        .into_iter()
        .map(|(index, name)| ProfileInfo {
            index,
            name,
            active: current == Some(index),
            assigned: assigned == Some(index),
        })
        .collect()
}

fn cache_profile_names(config: &mut Config, profiles: &[(u8, String)]) {
    config.profile_names = profiles.iter().cloned().collect();
}

fn discover_profiles_with_keyboard(
    keyboard: Option<&Keyboard>,
    config: &mut Config,
) -> Vec<ProfileInfo> {
    let current = keyboard
        .and_then(|connected| connected.current_profile().ok())
        .map(|index| index + 1);
    let assigned = config.configured_profile().ok().map(|index| index + 1);

    if let Some(connected) = keyboard {
        for attempt in 0..2 {
            let profiles: Result<Vec<_>> = (0..PROFILE_COUNT)
                .map(|index| {
                    connected.profile_name(index).and_then(|name| {
                        name.map(|name| {
                            validate_profile_name(&name)?;
                            Ok((index + 1, name))
                        })
                        .transpose()
                    })
                })
                .filter_map(Result::transpose)
                .collect();
            if let Ok(profiles) = profiles {
                cache_profile_names(config, &profiles);
                return profile_info(profiles, current, assigned);
            }
            if attempt == 0 {
                thread::sleep(Duration::from_millis(25));
            }
        }
    }

    for path in wootility_leveldb_paths() {
        if let Ok(profiles) = load_profiles_from_leveldb(&path)
            && !profiles.is_empty()
        {
            cache_profile_names(config, &profiles);
            return profile_info(profiles, current, assigned);
        }
    }

    if !config.profile_names.is_empty() {
        let profiles = config
            .profile_names
            .iter()
            .map(|(index, name)| (*index, name.clone()))
            .collect();
        return profile_info(profiles, current, assigned);
    }

    profile_info(
        (1..=PROFILE_COUNT)
            .map(|index| (index, format!("Profile {index}")))
            .collect(),
        current,
        assigned,
    )
}

fn discover_profiles(config: &mut Config) -> Vec<ProfileInfo> {
    let keyboard = Keyboard::connect_first();
    discover_profiles_with_keyboard(keyboard.as_ref(), config)
}

#[derive(Clone, Copy, Debug)]
struct KeyboardState {
    profile: u8,
    wootdev: WootDevOwnership,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WootDevOwnership {
    Active,
    Inactive,
    Unknown,
}

fn validate_response_command(buffer: &[u8], expected: u8, multi_report: bool) -> Result<()> {
    let command_offset = 2 + usize::from(multi_report);
    match buffer.get(command_offset) {
        Some(&actual) if actual == expected => Ok(()),
        Some(&actual) => bail!("command {expected} response echoed command {actual}"),
        None => bail!("command {expected} response omitted its command envelope"),
    }
}

fn parse_legacy_profile(buffer: &[u8], data_offset: usize) -> Result<u8> {
    let profile = *buffer
        .get(data_offset)
        .ok_or_else(|| anyhow!("profile response did not contain byte {data_offset}"))?;
    if profile >= PROFILE_COUNT {
        bail!("keyboard returned unsupported onboard profile index {profile}")
    }
    Ok(profile)
}

const STATE_QUERY_FAILURES_BEFORE_DISCONNECT: u8 = 3;

fn should_enforce(state: KeyboardState, enabled: bool, enforce: bool) -> bool {
    state.wootdev == WootDevOwnership::Inactive && enabled && enforce
}

fn read_current_state_with<F, G>(
    supports_heartbeat: bool,
    heartbeat: F,
    legacy: G,
) -> Result<KeyboardState>
where
    F: FnOnce() -> Result<KeyboardState>,
    G: FnOnce() -> Result<KeyboardState>,
{
    if supports_heartbeat {
        heartbeat().or_else(|heartbeat_error| {
            legacy().with_context(|| format!("heartbeat state failed: {heartbeat_error:#}"))
        })
    } else {
        legacy()
    }
}

struct Keyboard {
    _device_lock: fs::File,
    #[cfg(target_os = "macos")]
    device: HidDevice,
    #[cfg(target_os = "macos")]
    multi_report: bool,
    #[cfg(target_os = "macos")]
    v2_interface: bool,
}

impl Keyboard {
    fn lock_device() -> Option<fs::File> {
        let lock_directory = dirs::cache_dir()?.join("WootingHostProfile");
        fs::create_dir_all(&lock_directory).ok()?;
        let device_lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_directory.join("device.lock"))
            .ok()?;
        FileExt::try_lock_exclusive(&device_lock).ok()?;
        Some(device_lock)
    }

    #[cfg(target_os = "macos")]
    fn connect_first() -> Option<Self> {
        let device_lock = Self::lock_device()?;
        let api = HidApi::new().ok()?;
        let info = api.device_list().find(|device| {
            WOOTING_VENDOR_IDS.contains(&device.vendor_id())
                && matches!(
                    device.usage_page(),
                    WOOTING_CONFIG_USAGE_PAGE | WOOTING_V3_CONFIG_USAGE_PAGE
                )
        })?;
        let multi_report = info.usage_page() == WOOTING_V3_CONFIG_USAGE_PAGE;
        let v2_interface = !matches!(info.product_id(), 0xFF01 | 0xFF02);
        let device = info.open_device(&api).ok()?;
        let keyboard = Self {
            _device_lock: device_lock,
            device,
            multi_report,
            v2_interface,
        };
        let _ = keyboard.send_feature(WOOT_DEV_RESET_ALL, 0, "RGB ownership reset");
        Some(keyboard)
    }

    #[cfg(not(target_os = "macos"))]
    fn connect_first() -> Option<Self> {
        let device_lock = Self::lock_device()?;
        unsafe {
            rgb::wooting_usb_disconnect(false);
            if !rgb::wooting_usb_find_keyboard() {
                return None;
            }
            if !rgb::wooting_usb_select_device(0) {
                rgb::wooting_usb_disconnect(false);
                return None;
            }
            // The RGB SDK automatically sends WootDevInit while opening the
            // keyboard. Relinquish that state immediately: this application
            // uses the configuration protocol, not live third-party RGB.
            let _ = rgb::wooting_usb_send_feature(WOOT_DEV_RESET_ALL, 0, 0, 0, 0);
        }
        Some(Self {
            _device_lock: device_lock,
        })
    }

    #[cfg(target_os = "macos")]
    fn command_response(&self, command: u8, profile: u8) -> Result<Vec<u8>> {
        let report = command_report(self.multi_report, command, profile);
        self.device
            .send_feature_report(&report)
            .with_context(|| format!("keyboard rejected command {command}"))?;
        let mut buffer = vec![0_u8; MAX_HID_RESPONSE_SIZE];
        let response = self
            .device
            .read_timeout(&mut buffer, 1_000)
            .with_context(|| format!("keyboard could not read command {command} response"))?;
        let response_length = validated_response_length(i32::try_from(response)?, buffer.len())?;
        buffer.truncate(response_length);
        Ok(buffer)
    }

    #[cfg(not(target_os = "macos"))]
    #[allow(clippy::unused_self)]
    fn command_response(&self, command: u8, profile: u8) -> Result<Vec<u8>> {
        unsafe {
            let response_size = rgb::wooting_usb_get_response_size() as usize;
            if response_size == 0 {
                bail!("keyboard reported a zero-length response buffer")
            }
            let mut buffer = vec![0_u8; response_size];
            let response = rgb::wooting_usb_send_feature_with_response(
                buffer.as_mut_ptr(),
                response_size,
                command,
                0,
                0,
                0,
                profile,
            );
            if response != i32::try_from(response_size)? {
                bail!("command {command} did not return a complete response")
            }
            Ok(buffer)
        }
    }

    fn uses_multi_report(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.multi_report
        }
        #[cfg(not(target_os = "macos"))]
        unsafe {
            rgb::wooting_usb_use_multi_report()
        }
    }

    fn uses_v2_interface(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.v2_interface
        }
        #[cfg(not(target_os = "macos"))]
        unsafe {
            rgb::wooting_usb_use_v2_interface()
        }
    }

    fn legacy_state(&self) -> Result<KeyboardState> {
        let buffer = self.command_response(GET_CURRENT_KEYBOARD_PROFILE_INDEX, 0)?;
        let uses_multi_report = self.uses_multi_report();
        validate_response_command(
            &buffer,
            GET_CURRENT_KEYBOARD_PROFILE_INDEX,
            uses_multi_report,
        )?;
        let report_offset = usize::from(uses_multi_report);
        let extra_data_offset = usize::from(uses_multi_report);
        let data_offset =
            report_offset + if self.uses_v2_interface() { 5 } else { 4 } + extra_data_offset;
        let profile = parse_legacy_profile(&buffer, data_offset)?;
        Ok(KeyboardState {
            profile,
            wootdev: WootDevOwnership::Unknown,
        })
    }

    fn heartbeat_state(&self) -> Result<KeyboardState> {
        let buffer = self.command_response(HEARTBEAT, 0)?;
        let multi_report = self.uses_multi_report();
        validate_response_command(&buffer, HEARTBEAT, multi_report)?;
        let command_offset = 2 + usize::from(multi_report);
        let mut cursor = command_offset + 2;
        let mut profile = None;
        let mut wootdev = None;
        while let Some(&tag) = buffer.get(cursor) {
            if tag == 0 {
                break;
            }
            cursor += 1;
            if tag & 0x07 != 0 {
                bail!("heartbeat contained an unsupported protobuf field")
            }
            let mut value = 0_u64;
            let mut shift = 0_u32;
            loop {
                let byte = *buffer
                    .get(cursor)
                    .context("heartbeat protobuf was truncated")?;
                cursor += 1;
                value |= u64::from(byte & 0x7f) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift >= 64 {
                    bail!("heartbeat protobuf varint was invalid")
                }
            }
            match tag >> 3 {
                1 => profile = Some(u8::try_from(value)?),
                2 => {
                    wootdev = Some(if value != 0 {
                        WootDevOwnership::Active
                    } else {
                        WootDevOwnership::Inactive
                    });
                }
                _ => {}
            }
        }
        let profile = profile.context("heartbeat omitted the active profile")?;
        if profile >= PROFILE_COUNT {
            bail!("heartbeat returned unsupported onboard profile index {profile}")
        }
        Ok(KeyboardState {
            profile,
            wootdev: wootdev.unwrap_or(WootDevOwnership::Unknown),
        })
    }

    fn current_state(&self) -> Result<KeyboardState> {
        let supports_heartbeat = self.uses_multi_report();
        read_current_state_with(
            supports_heartbeat,
            || self.heartbeat_state(),
            || self.legacy_state(),
        )
    }

    fn current_profile(&self) -> Result<u8> {
        self.current_state().map(|state| state.profile)
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn profile_name(&self, profile: u8) -> Result<Option<String>> {
        let buffer = self.command_response(GET_PROFILE_METADATA, profile)?;
        let uses_multi_report = self.uses_multi_report();
        let report_offset = usize::from(uses_multi_report);
        let extra_data_offset = usize::from(uses_multi_report);
        validate_response_command(&buffer, GET_PROFILE_METADATA, uses_multi_report)?;

        let data_offset =
            report_offset + if self.uses_v2_interface() { 5 } else { 4 } + extra_data_offset;
        let data = buffer
            .get(data_offset..)
            .context("profile metadata response had no payload")?;
        let payload_length = usize::from(*data.first().context("metadata length was missing")?);
        if payload_length == 0 {
            return Ok(None);
        }
        let protobuf = data
            .get(2..2 + payload_length)
            .context("profile metadata payload was truncated")?;
        if protobuf.first() != Some(&0x0A) {
            bail!("profile metadata did not contain a name")
        }
        let name_length = usize::from(
            *protobuf
                .get(1)
                .context("profile metadata name length was missing")?,
        );
        let name = protobuf
            .get(2..2 + name_length)
            .context("profile metadata name was truncated")?;
        Ok(Some(String::from_utf8(name.to_vec())?))
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn activate_profile(&self, profile: u8, config: &Config) -> Result<()> {
        if profile >= PROFILE_COUNT {
            bail!("profile index {profile} is outside the onboard profile range")
        }

        self.send_feature(WOOT_DEV_INIT, 0, "initialization")?;
        let mut session = WootDevSession::new(self);
        let operation = (|| {
            self.send_feature(ACTIVATE_PROFILE, profile, "profile activation")?;
            thread::sleep(Duration::from_millis(100));

            let modern = self.uses_v2_interface() || self.uses_multi_report();
            let reload = if modern {
                RELOAD_PROFILE
            } else {
                RELOAD_PROFILE_LEGACY
            };
            if let Err(error) = self.send_feature(reload, profile, "profile reload") {
                if !modern {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(config.command_delay_ms.max(250)));
                self.send_feature(RELOAD_PROFILE_LEGACY, profile, "legacy profile reload")?;
            }

            if !self.wait_for_profile(profile, Duration::from_millis(500)) {
                bail!("keyboard did not report P{} after activation", profile + 1)
            }
            Ok(())
        })();

        match operation {
            Ok(()) => session.reset()?,
            Err(error) => {
                if let Err(cleanup_error) = session.reset() {
                    return Err(
                        error.context(format!("WootDev cleanup also failed: {cleanup_error:#}"))
                    );
                }
                return Err(error);
            }
        }

        let modern = self.uses_v2_interface() || self.uses_multi_report();
        if config.refresh_lighting && !modern {
            thread::sleep(Duration::from_millis(config.command_delay_ms));
            self.send_feature(REFRESH_RGB_COLORS, profile, "legacy RGB refresh")?;
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn send_feature(&self, command: u8, profile: u8, operation: &str) -> Result<()> {
        self.command_response(command, profile)
            .with_context(|| format!("keyboard rejected {operation} command {command}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn send_feature(&self, command: u8, profile: u8, operation: &str) -> Result<()> {
        unsafe {
            if !rgb::wooting_usb_send_feature(command, 0, 0, 0, profile) {
                bail!("keyboard rejected {operation} command {command}")
            }
        }
        Ok(())
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn wait_for_profile(&self, profile: u8, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self
                .current_profile()
                .is_ok_and(|current| current == profile)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

struct WootDevSession<'a> {
    keyboard: &'a Keyboard,
    reset: bool,
}

impl<'a> WootDevSession<'a> {
    fn new(keyboard: &'a Keyboard) -> Self {
        Self {
            keyboard,
            reset: false,
        }
    }

    fn reset(&mut self) -> Result<()> {
        self.keyboard
            .send_feature(WOOT_DEV_RESET_ALL, 0, "RGB ownership reset")?;
        self.reset = true;
        Ok(())
    }
}

impl Drop for WootDevSession<'_> {
    fn drop(&mut self) {
        if !self.reset {
            let _ = self.keyboard.send_feature(
                WOOT_DEV_RESET_ALL,
                0,
                "best-effort RGB ownership reset",
            );
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl Drop for Keyboard {
    fn drop(&mut self) {
        unsafe {
            rgb::wooting_usb_disconnect(false);
        }
    }
}

fn connect_required() -> Result<Keyboard> {
    Keyboard::connect_first().ok_or_else(|| anyhow!("no Wooting keyboard was detected"))
}

fn apply_and_verify(keyboard: &Keyboard, config: &Config, announce_unchanged: bool) -> Result<()> {
    let expected = config.configured_profile()?;
    let current = keyboard.current_profile()?;

    if current == expected {
        if announce_unchanged {
            println!(
                "Already correct: P{} for {}",
                expected + 1,
                std::env::consts::OS
            );
        }
        return Ok(());
    }

    println!(
        "Switching {} from P{} to P{}...",
        std::env::consts::OS,
        current + 1,
        expected + 1
    );
    keyboard.activate_profile(expected, config)?;
    println!(
        "Applied and verified P{} for {}.",
        expected + 1,
        std::env::consts::OS
    );
    Ok(())
}

fn append_log(config_path: &Path, message: &str) {
    let Some(parent) = config_path.parent() else {
        return;
    };
    let _ = fs::create_dir_all(parent);
    let log_path = parent.join("watch.log");
    if fs::metadata(&log_path).is_ok_and(|metadata| metadata.len() >= MAX_LOG_BYTES) {
        let previous = parent.join("watch.log.1");
        let _ = fs::remove_file(&previous);
        let _ = fs::rename(&log_path, previous);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let _ = writeln!(file, "{timestamp} {message}");
    }
}

fn ipc_address(config_path: &Path) -> SocketAddrV4 {
    let mut hasher = DefaultHasher::new();
    let normalized = if cfg!(windows) {
        config_path.to_string_lossy().to_lowercase()
    } else {
        config_path.to_string_lossy().into_owned()
    };
    normalized.hash(&mut hasher);
    let offset = u16::try_from(hasher.finish() % u64::from(IPC_PORT_COUNT))
        .expect("IPC port offset is bounded");
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, IPC_PORT_BASE + offset)
}

fn ipc_token_path(config_path: &Path) -> Result<PathBuf> {
    let parent = config_path
        .parent()
        .context("configuration path has no parent directory")?;
    Ok(parent.join(format!("ipc-{}.token", ipc_address(config_path).port())))
}

fn read_ipc_token(config_path: &Path) -> Result<String> {
    let path = ipc_token_path(config_path)?;
    let token = fs::read_to_string(&path)
        .with_context(|| format!("failed to read IPC token from {}", path.display()))?;
    let token = token.trim();
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("IPC token file was invalid")
    }
    Ok(token.to_owned())
}

fn create_ipc_token(config_path: &Path) -> Result<String> {
    let path = ipc_token_path(config_path)?;
    let parent = path.parent().context("IPC token path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow!("could not generate IPC token: {error}"))?;
    let mut token = String::with_capacity(64);
    for byte in bytes {
        token.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        token.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(token.as_bytes())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace IPC token at {}", path.display()))?;
    Ok(token)
}

fn try_ipc_set(config_path: &Path, human_profile: u8) -> Result<bool> {
    validate_human_profile(human_profile)?;
    let Ok(token) = read_ipc_token(config_path) else {
        return Ok(false);
    };
    let address = SocketAddr::V4(ipc_address(config_path));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(25)) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::NotFound
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };
    // A verified switch may include the configured delays around the RGB reset
    // and refresh commands. Leave enough time for that complete transaction.
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "SET {human_profile} {token}")?;
    stream.shutdown(Shutdown::Write)?;
    let mut buffer = [0_u8; 256];
    let count = stream.read(&mut buffer)?;
    let response = String::from_utf8_lossy(&buffer[..count]);
    if response.trim() == "OK" {
        Ok(true)
    } else {
        bail!(
            "background watcher rejected the profile: {}",
            response.trim()
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IpcRequest {
    Set(u8),
    Profiles,
}

fn try_ipc_profiles(config_path: &Path) -> Result<Option<Vec<ProfileInfo>>> {
    let Ok(token) = read_ipc_token(config_path) else {
        return Ok(None);
    };
    let address = SocketAddr::V4(ipc_address(config_path));
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(25)) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::NotFound
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    writeln!(stream, "PROFILES {token}")?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = Vec::new();
    (&mut stream).take(8_193).read_to_end(&mut response)?;
    if response.len() > 8_192 {
        bail!("background watcher returned an oversized profile list")
    }
    let response = std::str::from_utf8(&response)?.trim();
    if let Some(error) = response.strip_prefix("ERR ") {
        bail!("background watcher rejected profile discovery: {error}")
    }
    Ok(Some(serde_json::from_str(response)?))
}

fn parse_ipc_request(request: &[u8], expected_token: &str) -> Result<IpcRequest> {
    if request.len() > usize::try_from(IPC_REQUEST_LIMIT)? {
        bail!("IPC request was too large")
    }
    if !request.ends_with(b"\n") {
        bail!("IPC request was incomplete")
    }
    let request = std::str::from_utf8(request)?;
    let mut fields = request.trim_end_matches(['\r', '\n']).split(' ');
    match fields.next() {
        Some("SET") => {
            let profile = fields
                .next()
                .context("IPC profile was missing")?
                .parse::<u8>()?;
            let token = fields.next().context("IPC token was missing")?;
            if fields.next().is_some() || token != expected_token {
                bail!("IPC authentication failed")
            }
            validate_human_profile(profile)?;
            Ok(IpcRequest::Set(profile))
        }
        Some("PROFILES") => {
            let token = fields.next().context("IPC token was missing")?;
            if fields.next().is_some() || token != expected_token {
                bail!("IPC authentication failed")
            }
            Ok(IpcRequest::Profiles)
        }
        _ => bail!("unknown IPC command"),
    }
}

fn handle_ipc_stream(
    mut stream: TcpStream,
    keyboard: &mut Option<Keyboard>,
    config_path: &Path,
    expected_token: &str,
) {
    let response = (|| -> Result<String> {
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
        let mut request = Vec::new();
        (&mut stream)
            .take(IPC_REQUEST_LIMIT + 1)
            .read_to_end(&mut request)?;
        let request = parse_ipc_request(&request, expected_token)?;
        if keyboard.is_none() {
            *keyboard = Keyboard::connect_first();
        }
        let connected = keyboard
            .as_ref()
            .context("no Wooting keyboard is connected")?;
        match request {
            IpcRequest::Set(profile) => {
                let mut config = load_config(config_path)?;
                config.learn_for_current_os(profile)?;
                apply_and_verify(connected, &config, false)?;
                Ok("OK".to_owned())
            }
            IpcRequest::Profiles => {
                let mut config = load_config(config_path)?;
                let mut profiles = discover_profiles_with_keyboard(Some(connected), &mut config);
                let discovered_names = config.profile_names.clone();
                config = update_config(config_path, move |latest| {
                    latest.profile_names = discovered_names;
                    Ok(())
                })?;
                let assigned = config.configured_profile().ok().map(|index| index + 1);
                for profile in &mut profiles {
                    profile.assigned = assigned == Some(profile.index);
                }
                Ok(serde_json::to_string(&profiles)?)
            }
        }
    })();

    match response {
        Ok(response) => {
            let _ = writeln!(stream, "{response}");
            let _ = stream.flush();
        }
        Err(error) => {
            append_log(config_path, &format!("IPC request failed: {error:#}"));
            let _ = writeln!(stream, "ERR {error:#}");
            let _ = stream.flush();
        }
    }
}

fn service_ipc_clients(
    listener: &TcpListener,
    keyboard: &mut Option<Keyboard>,
    config_path: &Path,
    ipc_token: &str,
) {
    for _ in 0..8 {
        match listener.accept() {
            Ok((stream, _)) => handle_ipc_stream(stream, keyboard, config_path, ipc_token),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => {
                append_log(config_path, &format!("IPC accept failed: {error}"));
                break;
            }
        }
    }
}

fn watch(config_path: &Path, interval: Duration, enforce_override: bool) -> Result<()> {
    let parent = config_path
        .parent()
        .context("configuration path has no parent directory")?;
    fs::create_dir_all(parent)?;
    let lock_path = parent.join("watch.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    if FileExt::try_lock_exclusive(&lock).is_err() {
        return Ok(());
    }
    let ipc_token = create_ipc_token(config_path)?;
    let ipc_address = ipc_address(config_path);
    let listener = TcpListener::bind(ipc_address)
        .with_context(|| format!("failed to bind local control socket {ipc_address}"))?;
    listener.set_nonblocking(true)?;

    let running = Arc::new(AtomicBool::new(true));
    let signal = Arc::clone(&running);
    ctrlc::set_handler(move || signal.store(false, Ordering::SeqCst))
        .context("failed to install the shutdown handler")?;

    append_log(
        config_path,
        &format!("Watcher started on {}", std::env::consts::OS),
    );

    let mut keyboard: Option<Keyboard> = None;
    let mut consecutive_state_errors = 0_u8;
    let mut next_device_check = Instant::now();
    let mut next_enforcement_check = Instant::now();

    while running.load(Ordering::SeqCst) {
        service_ipc_clients(&listener, &mut keyboard, config_path, &ipc_token);

        if Instant::now() >= next_device_check {
            let mut next_check_delay = interval;
            if keyboard.is_none() {
                keyboard = Keyboard::connect_first();
                if let Some(connected) = keyboard.as_ref() {
                    consecutive_state_errors = 0;
                    let config = load_config(config_path)?;
                    if !config.enabled {
                        append_log(
                            config_path,
                            "Keyboard connected; automatic switching is disabled",
                        );
                    } else if let Err(error) = apply_and_verify(connected, &config, false) {
                        append_log(
                            config_path,
                            &format!("Profile application failed: {error:#}"),
                        );
                    } else {
                        append_log(config_path, "Keyboard connected; OS profile verified");
                    }
                    next_enforcement_check =
                        Instant::now() + Duration::from_millis(config.enforce_interval_ms);
                }
            } else if let Some(connected) = keyboard.as_ref() {
                if let Ok(state) = connected.current_state() {
                    consecutive_state_errors = 0;
                    let config = load_config(config_path)?;
                    let enforcement_due =
                        should_enforce(state, config.enabled, enforce_override || config.enforce)
                            && Instant::now() >= next_enforcement_check;
                    if enforcement_due {
                        if let Err(error) = apply_and_verify(connected, &config, false) {
                            append_log(
                                config_path,
                                &format!("Profile enforcement failed: {error:#}"),
                            );
                        }
                        next_enforcement_check =
                            Instant::now() + Duration::from_millis(config.enforce_interval_ms);
                    }
                    if state.wootdev == WootDevOwnership::Active {
                        next_enforcement_check =
                            Instant::now() + Duration::from_millis(config.enforce_interval_ms);
                        next_check_delay =
                            interval.max(Duration::from_millis(config.enforce_interval_ms));
                    }
                } else {
                    consecutive_state_errors = consecutive_state_errors.saturating_add(1);
                    if consecutive_state_errors == 1 {
                        append_log(config_path, "Keyboard state query failed; retrying");
                    }
                    if consecutive_state_errors >= STATE_QUERY_FAILURES_BEFORE_DISCONNECT {
                        append_log(
                            config_path,
                            "Keyboard disconnected after repeated state query failures",
                        );
                        keyboard = None;
                        consecutive_state_errors = 0;
                    }
                }
            }
            next_device_check = Instant::now() + next_check_delay;
        }

        thread::sleep(Duration::from_millis(5));
    }

    append_log(config_path, "Watcher stopped");
    Ok(())
}

#[cfg(target_os = "windows")]
fn startup_enabled() -> bool {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
    else {
        return false;
    };
    key.get_value::<String, _>("Wooting Switch").is_ok()
        || key.get_value::<String, _>("Wooting Host Profile").is_ok()
}

#[cfg(target_os = "windows")]
fn set_startup_enabled(enabled: bool, config_path: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    if enabled {
        let executable = std::env::current_exe()?;
        let command = format!(
            "\"{}\" --config \"{}\" watch",
            executable.display(),
            config_path.display()
        );
        key.set_value("Wooting Switch", &command)?;
        let _ = key.delete_value("Wooting Host Profile");
    } else {
        let _ = key.delete_value("Wooting Switch");
        let _ = key.delete_value("Wooting Host Profile");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("could not determine the home directory")?
        .join("Library/LaunchAgents/io.local.wooting-host-profile.plist"))
}

#[cfg(target_os = "macos")]
fn startup_enabled() -> bool {
    launch_agent_path().is_ok_and(|path| path.exists())
}

#[cfg(target_os = "macos")]
fn set_startup_enabled(enabled: bool, config_path: &Path) -> Result<()> {
    let plist = launch_agent_path()?;
    let uid = String::from_utf8(
        ProcessCommand::new("id")
            .arg("-u")
            .output()
            .context("failed to read the macOS user id")?
            .stdout,
    )?
    .trim()
    .to_owned();
    let domain = format!("gui/{uid}");
    let label = "io.local.wooting-host-profile";
    let _ = ProcessCommand::new("launchctl")
        .args(["bootout", &format!("{domain}/{label}")])
        .status();

    if enabled {
        let executable = xml_escape(&std::env::current_exe()?.display().to_string());
        let config = xml_escape(&config_path.display().to_string());
        let content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array>
<string>{executable}</string><string>--config</string><string>{config}</string><string>watch</string>
</array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
</dict></plist>
"#
        );
        let parent = plist.parent().context("LaunchAgent path has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(&plist, content)?;
        let status = ProcessCommand::new("launchctl")
            .args(["bootstrap", &domain])
            .arg(&plist)
            .status()?;
        if !status.success() {
            bail!("launchctl could not enable the background watcher")
        }
    } else if plist.exists() {
        fs::remove_file(plist)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn startup_enabled() -> bool {
    false
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn set_startup_enabled(_enabled: bool, _config_path: &Path) -> Result<()> {
    bail!("automatic startup is supported only on Windows and macOS")
}

fn start_hidden_watcher(config_path: &Path) -> Result<()> {
    let mut command = ProcessCommand::new(std::env::current_exe()?);
    command
        .arg("--config")
        .arg(config_path)
        .arg("watch")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .context("failed to start the background watcher")?;
    Ok(())
}

#[cfg(any())]
mod legacy_custom_drawn_gui {
    use super::*;

    struct ConfigApp {
        config_path: PathBuf,
        config: Config,
        selected_profile: u8,
        current_profile: Option<u8>,
        start_at_login: bool,
        message: String,
    }

    impl ConfigApp {
        fn new(config_path: PathBuf, config: Config) -> Self {
            let current_profile = Keyboard::connect_first()
                .and_then(|keyboard| keyboard.current_profile().ok())
                .map(|profile| profile + 1);
            let selected_profile = config
                .configured_profile()
                .ok()
                .map_or_else(|| current_profile.unwrap_or(1), |profile| profile + 1);
            Self {
                config_path,
                config,
                selected_profile,
                current_profile,
                start_at_login: startup_enabled(),
                message: String::new(),
            }
        }

        fn refresh(&mut self) {
            self.current_profile = Keyboard::connect_first()
                .and_then(|keyboard| keyboard.current_profile().ok())
                .map(|profile| profile + 1);
            self.message = self.current_profile.map_or_else(
                || "No Wooting keyboard detected.".to_owned(),
                |profile| format!("Keyboard currently uses P{profile}."),
            );
        }

        fn apply_selected(&mut self) -> Result<()> {
            let mut candidate = self.config.clone();
            candidate.learn_for_current_os(self.selected_profile)?;
            let keyboard = connect_required()?;
            apply_and_verify(&keyboard, &candidate, false)?;
            self.current_profile = Some(self.selected_profile);
            Ok(())
        }

        fn save_and_run(&mut self) -> Result<()> {
            self.config.learn_for_current_os(self.selected_profile)?;
            let desired = self.config.clone();
            self.config = update_config(&self.config_path, move |latest| {
                *latest = desired;
                Ok(())
            })?;
            set_startup_enabled(self.start_at_login, &self.config_path)?;
            if Keyboard::connect_first().is_some() {
                self.apply_selected()?;
            }
            start_hidden_watcher(&self.config_path)?;
            Ok(())
        }
    }

    impl eframe::App for ConfigApp {
        fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
            egui::CentralPanel::default().show(context, |ui| {
                ui.heading("Wooting Switch");
                ui.label(format!(
                    "Choose the onboard profile this {} system should use.",
                    if cfg!(target_os = "macos") {
                        "macOS"
                    } else {
                        "Windows"
                    }
                ));
                ui.add_space(10.0);

                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for profile in 1..=PROFILE_COUNT {
                        let mut label = format!("P{profile}");
                        if self.current_profile == Some(profile) {
                            label.push_str("  —  active now");
                        }
                        ui.radio_value(&mut self.selected_profile, profile, label);
                    }
                });

                ui.add_space(10.0);
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(54, 44, 24))
                    .corner_radius(6.0)
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("Avoid app-specific profile overrides")
                                .strong()
                                .color(egui::Color32::from_rgb(255, 208, 96)),
                        );
                        ui.label(
                            "Do not configure Wootility App Linking or another profile switcher. "
                                .to_owned()
                                + "Those tools can override the profile selected here.",
                        );
                    });

                ui.add_space(10.0);
                ui.checkbox(&mut self.start_at_login, "Start hidden when I sign in");
                ui.checkbox(
                    &mut self.config.enforce,
                    "Reapply if the profile is changed while connected",
                );
                ui.checkbox(
                    &mut self.config.refresh_lighting,
                    "Refresh profile lighting after switching",
                );

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.refresh();
                    }
                    if ui.button("Apply now").clicked() {
                        self.message = match self.apply_selected() {
                            Ok(()) => format!("Applied and verified P{}.", self.selected_profile),
                            Err(error) => format!("Apply failed: {error:#}"),
                        };
                    }
                    if ui
                        .add(
                            egui::Button::new("Save and run in background")
                                .fill(egui::Color32::from_rgb(67, 139, 202)),
                        )
                        .clicked()
                    {
                        self.message = match self.save_and_run() {
                            Ok(()) => format!(
                                "Saved P{} for this system. Background watcher is running.",
                                self.selected_profile
                            ),
                            Err(error) => format!("Save failed: {error:#}"),
                        };
                    }
                });

                ui.add_space(10.0);
                if self.message.is_empty() {
                    match self.current_profile {
                        Some(profile) => ui.label(format!("Connected keyboard: P{profile}")),
                        None => ui.label("No Wooting keyboard detected."),
                    };
                } else {
                    ui.label(&self.message);
                }
            });
        }
    }

    fn run_gui(config_path: PathBuf, config: Config) -> Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([500.0, 390.0])
                .with_min_inner_size([460.0, 360.0])
                .with_resizable(false),
            ..Default::default()
        };
        eframe::run_native(
            "Wooting Switch",
            options,
            Box::new(move |_creation_context| Ok(Box::new(ConfigApp::new(config_path, config)))),
        )
        .map_err(|error| anyhow!("native GUI failed: {error}"))
    }
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
#[allow(clippy::too_many_lines)]
fn run_native_config(config_path: &Path, mut config: Config) -> Result<()> {
    use windows::{
        Win32::UI::Controls::{
            TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON,
            TDF_ALLOW_DIALOG_CANCELLATION, TDF_POSITION_RELATIVE_TO_WINDOW,
            TDF_VERIFICATION_FLAG_CHECKED, TaskDialogIndirect,
        },
        core::PCWSTR,
    };

    let mut profiles = discover_profiles(&mut config);
    let discovered_names = config.profile_names.clone();
    config = update_config(config_path, move |latest| {
        latest.profile_names = discovered_names;
        Ok(())
    })?;
    let assigned = config.configured_profile().ok().map(|index| index + 1);
    for profile in &mut profiles {
        profile.assigned = assigned == Some(profile.index);
    }
    let title = wide("Wooting Switch");
    let instruction = wide("Choose the profile for this Windows system");
    let content = wide(
        "Only configured onboard profiles are shown. The background watcher will apply the saved profile after sign-in, wake, reconnect, or a USB/KVM host switch.",
    );
    let footer = wide(
        "Do not enable Wootility App Linking or another app-specific profile override. It can replace the system profile selected here.",
    );
    let verification = wide("Start the watcher hidden when I sign in");
    let save_text = wide("Save and run in background");
    let button_labels = [save_text];
    let buttons = [TASKDIALOG_BUTTON {
        nButtonID: 100,
        pszButtonText: PCWSTR(button_labels[0].as_ptr()),
    }];

    let radio_labels: Vec<Vec<u16>> = profiles
        .iter()
        .map(|profile| {
            let active = if profile.active { "  (active now)" } else { "" };
            wide(&format!("P{} — {}{active}", profile.index, profile.name))
        })
        .collect();
    let radio_buttons: Vec<TASKDIALOG_BUTTON> = profiles
        .iter()
        .zip(&radio_labels)
        .map(|(profile, label)| TASKDIALOG_BUTTON {
            nButtonID: i32::from(profile.index),
            pszButtonText: PCWSTR(label.as_ptr()),
        })
        .collect();

    let selected = config
        .configured_profile()
        .ok()
        .map_or_else(
            || {
                profiles
                    .iter()
                    .find(|profile| profile.active)
                    .map(|profile| profile.index)
            },
            |index| Some(index + 1),
        )
        .unwrap_or(profiles[0].index);

    let mut flags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW;
    if startup_enabled() {
        flags |= TDF_VERIFICATION_FLAG_CHECKED;
    }

    let dialog = TASKDIALOGCONFIG {
        cbSize: u32::try_from(std::mem::size_of::<TASKDIALOGCONFIG>())?,
        dwFlags: flags,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: u32::try_from(buttons.len())?,
        pButtons: buttons.as_ptr(),
        nDefaultButton: 100,
        cRadioButtons: u32::try_from(radio_buttons.len())?,
        pRadioButtons: radio_buttons.as_ptr(),
        nDefaultRadioButton: i32::from(selected),
        pszVerificationText: PCWSTR(verification.as_ptr()),
        pszFooter: PCWSTR(footer.as_ptr()),
        ..Default::default()
    };

    let mut pressed_button = 0;
    let mut selected_radio = i32::from(selected);
    let mut verification_checked = false.into();
    unsafe {
        TaskDialogIndirect(
            &raw const dialog,
            Some(&raw mut pressed_button),
            Some(&raw mut selected_radio),
            Some(&raw mut verification_checked),
        )?;
    }

    if pressed_button != 100 {
        return Ok(());
    }
    let selected_profile = u8::try_from(selected_radio)?;
    if !profiles
        .iter()
        .any(|profile| profile.index == selected_profile)
    {
        bail!("the selected profile is not configured on this keyboard")
    }

    config = update_config(config_path, |latest| {
        latest.learn_for_current_os(selected_profile)
    })?;
    set_startup_enabled(verification_checked.as_bool(), config_path)?;
    if let Some(keyboard) = Keyboard::connect_first() {
        apply_and_verify(&keyboard, &config, false)?;
    }
    start_hidden_watcher(config_path)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_native_config(_config_path: &Path, _config: Config) -> Result<()> {
    bail!("open the Wooting Switch.app SwiftUI front end")
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn run_native_config(_config_path: &Path, _config: Config) -> Result<()> {
    bail!("the native configuration UI supports Windows and macOS")
}

fn set_automatic_switching(path: &Path, config: &mut Config, enabled: bool) -> Result<()> {
    *config = update_config(path, |latest| {
        latest.enabled = enabled;
        Ok(())
    })?;
    if enabled {
        if let Ok(profile) = config.configured_profile()
            && !try_ipc_set(path, profile + 1)?
            && let Some(keyboard) = Keyboard::connect_first()
        {
            apply_and_verify(&keyboard, config, false)?;
        }
        start_hidden_watcher(path)?;
    }
    Ok(())
}

fn set_enforcement(path: &Path, config: &mut Config, enabled: bool) -> Result<()> {
    *config = update_config(path, |latest| {
        latest.enforce = enabled;
        Ok(())
    })?;
    Ok(())
}

fn set_status_icon(path: &Path, config: &mut Config, visible: bool) -> Result<()> {
    *config = update_config(path, |latest| {
        latest.show_status_icon = visible;
        Ok(())
    })?;
    Ok(())
}

fn list_profiles(path: &Path, config: &mut Config, json: bool) -> Result<()> {
    let profiles = if let Some(profiles) = try_ipc_profiles(path)? {
        profiles
    } else {
        let mut profiles = discover_profiles(config);
        let discovered_names = config.profile_names.clone();
        *config = update_config(path, move |latest| {
            latest.profile_names = discovered_names;
            Ok(())
        })?;
        let assigned = config.configured_profile().ok().map(|index| index + 1);
        for profile in &mut profiles {
            profile.assigned = assigned == Some(profile.index);
        }
        profiles
    };
    if json {
        println!("{}", serde_json::to_string(&profiles)?);
    } else {
        for profile in profiles {
            let active = if profile.active { " (active)" } else { "" };
            println!("P{}: {}{active}", profile.index, profile.name);
        }
    }
    Ok(())
}

fn print_linked_status() {
    discover_linked_profile_count()
        .map_or_else(|| println!("unknown"), |count| println!("{count}"));
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = config_path(cli.config.as_deref())?;
    let mut config = load_config(&path)?;

    match cli.command {
        Some(Command::Profiles { json }) => {
            list_profiles(&path, &mut config, json)?;
        }
        Some(Command::LinkedStatus) => {
            print_linked_status();
        }
        Some(Command::StartupStatus) => {
            println!("{}", startup_enabled());
        }
        Some(Command::EnabledStatus) => {
            println!("{}", config.enabled);
        }
        Some(Command::SetEnabled { state }) => {
            set_automatic_switching(&path, &mut config, matches!(state, EnabledChoice::Enable))?;
        }
        Some(Command::EnforceStatus) => {
            println!("{}", config.enforce);
        }
        Some(Command::SetEnforce { state }) => {
            set_enforcement(&path, &mut config, matches!(state, EnabledChoice::Enable))?;
        }
        Some(Command::StatusIconStatus) => {
            println!("{}", config.show_status_icon);
        }
        Some(Command::SetStatusIcon { state }) => {
            set_status_icon(&path, &mut config, matches!(state, StatusIconChoice::Show))?;
        }
        Some(Command::Status) => {
            let keyboard = connect_required()?;
            let profile = keyboard.current_profile()?;
            println!(
                "Connected Wooting profile: P{} (index {profile})",
                profile + 1
            );
        }
        Some(Command::Learn { profile }) => {
            let human_profile = if let Some(profile) = profile {
                profile
            } else {
                let keyboard = connect_required()?;
                keyboard.current_profile()? + 1
            };
            let _updated =
                update_config(&path, |latest| latest.learn_for_current_os(human_profile))?;
            println!(
                "Learned P{human_profile} for {} and saved {}",
                std::env::consts::OS,
                path.display()
            );
        }
        Some(Command::Apply) => {
            let profile = config.configured_profile()? + 1;
            if !try_ipc_set(&path, profile)? {
                let keyboard = connect_required()?;
                apply_and_verify(&keyboard, &config, true)?;
            }
        }
        Some(Command::Watch {
            interval_ms,
            enforce,
        }) => {
            config.configured_profile()?;
            watch(&path, Duration::from_millis(interval_ms), enforce)?;
        }
        Some(Command::Config) => {
            println!("{}", path.display());
            println!("{}", toml::to_string_pretty(&config)?);
        }
        Some(Command::Configure { profile, startup }) => {
            validate_human_profile(profile)?;
            config = update_config(&path, |latest| latest.learn_for_current_os(profile))?;
            match startup {
                StartupChoice::Keep => {}
                StartupChoice::Enable => set_startup_enabled(true, &path)?,
                StartupChoice::Disable => set_startup_enabled(false, &path)?,
            }
            if config.enabled
                && !try_ipc_set(&path, profile)?
                && let Some(keyboard) = Keyboard::connect_first()
            {
                apply_and_verify(&keyboard, &config, false)?;
            }
            start_hidden_watcher(&path)?;
        }
        Some(Command::Set { profile }) => {
            validate_human_profile(profile)?;
            if !try_ipc_set(&path, profile)? {
                let mut candidate = config.clone();
                candidate.learn_for_current_os(profile)?;
                let keyboard = connect_required()?;
                apply_and_verify(&keyboard, &candidate, false)?;
            }
        }
        None => run_native_config(&path, config)?,
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_four_onboard_profiles() {
        for profile in 1..=4 {
            assert!(validate_human_profile(profile).is_ok());
        }
    }

    #[test]
    fn rejects_profiles_outside_the_onboard_range() {
        assert!(validate_human_profile(0).is_err());
        assert!(validate_human_profile(5).is_err());
    }

    #[test]
    fn validates_response_command_envelopes() {
        assert!(validate_response_command(&[0, 0, HEARTBEAT], HEARTBEAT, false).is_ok());
        assert!(validate_response_command(&[0, 0, 0, HEARTBEAT], HEARTBEAT, true).is_ok());
        assert!(
            validate_response_command(&[0, 0, GET_PROFILE_METADATA], HEARTBEAT, false).is_err()
        );
        assert!(validate_response_command(&[0, 0], HEARTBEAT, false).is_err());
    }

    #[test]
    fn accepts_nonempty_hid_reports_up_to_the_buffer_size() {
        assert_eq!(validated_response_length(32, 2_046).unwrap(), 32);
        assert_eq!(validated_response_length(2_046, 2_046).unwrap(), 2_046);
        assert!(validated_response_length(0, 2_046).is_err());
        assert!(validated_response_length(-1, 2_046).is_err());
        assert!(validated_response_length(2_047, 2_046).is_err());
    }

    #[test]
    fn builds_wooting_command_reports_for_each_transport() {
        assert_eq!(
            command_report(true, HEARTBEAT, 2),
            [1, 0xD1, 0xDA, HEARTBEAT, 2, 0, 0, 0]
        );
        assert_eq!(
            command_report(false, GET_CURRENT_KEYBOARD_PROFILE_INDEX, 1),
            [
                0,
                0xD0,
                0xDA,
                GET_CURRENT_KEYBOARD_PROFILE_INDEX,
                1,
                0,
                0,
                0,
            ]
        );
    }

    #[test]
    fn parses_only_valid_legacy_profile_indexes() {
        assert_eq!(parse_legacy_profile(&[0, 1], 1).unwrap(), 1);
        assert!(parse_legacy_profile(&[0, 8], 1).is_err());
        assert!(parse_legacy_profile(&[0], 1).is_err());
    }

    #[test]
    fn only_known_inactive_wootdev_state_allows_enforcement() {
        let mut state = KeyboardState {
            profile: 0,
            wootdev: WootDevOwnership::Inactive,
        };
        assert!(should_enforce(state, true, true));
        state.wootdev = WootDevOwnership::Active;
        assert!(!should_enforce(state, true, true));
        state.wootdev = WootDevOwnership::Unknown;
        assert!(!should_enforce(state, true, true));
        assert!(!should_enforce(state, false, true));
        assert!(!should_enforce(state, true, false));
    }

    #[test]
    fn falls_back_to_legacy_state_when_heartbeat_fails() {
        let legacy = KeyboardState {
            profile: 1,
            wootdev: WootDevOwnership::Unknown,
        };
        let state = read_current_state_with(true, || bail!("heartbeat unavailable"), || Ok(legacy))
            .unwrap();
        assert_eq!(state.profile, 1);
        assert_eq!(state.wootdev, WootDevOwnership::Unknown);
    }

    #[test]
    fn config_round_trips() {
        let config = Config {
            enabled: true,
            show_status_icon: true,
            profiles: Profiles {
                windows: Some(2),
                macos: Some(1),
            },
            profile_names: BTreeMap::from([
                (1, "Typing Profile".to_owned()),
                (2, "Game Profile".to_owned()),
            ]),
            refresh_lighting: true,
            command_delay_ms: 250,
            enforce: false,
            enforce_interval_ms: 5_000,
        };
        let text = toml::to_string(&config).expect("serialize config");
        let decoded: Config = toml::from_str(&text).expect("parse config");
        assert_eq!(decoded.profiles.windows, Some(2));
        assert_eq!(decoded.profiles.macos, Some(1));
    }

    #[test]
    fn rejects_unsafe_configuration_values() {
        let mut config = default_config();
        config.command_delay_ms = 0;
        assert!(config.validate().is_err());

        config = default_config();
        config.enforce_interval_ms = 100;
        assert!(config.validate().is_err());

        config = default_config();
        config.profile_names.insert(5, "Out of range".to_owned());
        assert!(config.validate().is_err());

        config = default_config();
        config.profile_names.insert(1, "Bad\nName".to_owned());
        assert!(config.validate().is_err());
    }

    #[test]
    fn parses_only_authenticated_bounded_ipc_requests() {
        assert_eq!(
            parse_ipc_request(b"SET 2 secret\n", "secret").unwrap(),
            IpcRequest::Set(2)
        );
        assert_eq!(
            parse_ipc_request(b"PROFILES secret\n", "secret").unwrap(),
            IpcRequest::Profiles
        );
        assert!(parse_ipc_request(b"SET 2 wrong\n", "secret").is_err());
        assert!(parse_ipc_request(b"SET 2 secret", "secret").is_err());
        assert!(parse_ipc_request(b"SET 5 secret\n", "secret").is_err());
        assert!(parse_ipc_request(&[b'x'; 129], "secret").is_err());
    }

    #[test]
    fn scoped_config_updates_preserve_other_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let first = update_config(&path, |config| {
            config.enabled = false;
            Ok(())
        })
        .unwrap();
        assert!(!first.enabled);

        let second = update_config(&path, |config| {
            config.enforce = true;
            Ok(())
        })
        .unwrap();
        assert!(!second.enabled);
        assert!(second.enforce);
        assert!(!load_config(&path).unwrap().enabled);
    }

    #[test]
    fn ipc_endpoint_is_stable_and_config_scoped() {
        let first = Path::new("C:/one/config.toml");
        let second = Path::new("C:/two/config.toml");
        assert_eq!(ipc_address(first), ipc_address(first));
        assert_ne!(ipc_address(first), ipc_address(second));
    }
}
