#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use encoding_rs::{UTF_16LE, WINDOWS_1252};
use fs2::FileExt;
use rusty_leveldb::{CompressorId, DB, LdbIterator, Options, compressor::SnappyCompressor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wooting_rgb_sys as rgb;

const GET_CURRENT_KEYBOARD_PROFILE_INDEX: u8 = 11;
const RELOAD_PROFILE: u8 = 7;
const ACTIVATE_PROFILE: u8 = 23;
const REFRESH_RGB_COLORS: u8 = 29;
const WOOT_DEV_RESET_ALL: u8 = 32;
const GET_PROFILE_METADATA: u8 = 55;
const PROFILE_COUNT: u8 = 4;
const IPC_ADDRESS: &str = "127.0.0.1:50053";
const WOOTING_COMMAND_SIZE: i32 = 8;

unsafe extern "C" {
    fn wooting_usb_send_feature_buff(
        command_id: u8,
        parameter0: u8,
        parameter1: u8,
        parameter2: u8,
        parameter3: u8,
    ) -> i32;
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
        #[arg(long = "interval-ms", visible_alias = "interval", default_value_t = 100, value_parser = clap::value_parser!(u64).range(10..))]
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

fn config_path(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path.to_path_buf());
    }

    let base =
        dirs::config_dir().context("could not determine the user configuration directory")?;
    Ok(base.join("WootingHostProfile").join("config.toml"))
}

fn load_config(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config {
            enabled: default_true(),
            show_status_icon: default_true(),
            profiles: Profiles::default(),
            profile_names: BTreeMap::new(),
            refresh_lighting: default_true(),
            command_delay_ms: default_command_delay_ms(),
            enforce: false,
            enforce_interval_ms: default_enforce_interval_ms(),
        });
    }

    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read configuration from {}", path.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("failed to parse configuration at {}", path.display()))
}

fn save_config(path: &Path, config: &Config) -> Result<()> {
    let parent = path.parent().context("configuration path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;

    let text = toml::to_string_pretty(config).context("failed to serialize configuration")?;
    fs::write(path, text).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
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

fn load_profiles_from_leveldb(source: &Path) -> Result<Vec<(u8, String)>> {
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
    let onboard = profiles
        .get("devices")
        .and_then(|value| value.get(active_device))
        .and_then(|value| value.get("onboard"))
        .and_then(Value::as_array)
        .context("Wootility had no onboard profiles for the active keyboard")?;

    onboard
        .iter()
        .enumerate()
        .map(|(index, profile)| {
            let human_index = u8::try_from(index + 1).context("too many onboard profiles")?;
            let name = profile
                .get("details")
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .context("an onboard profile had no name")?;
            Ok((human_index, name.to_owned()))
        })
        .collect()
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

fn discover_profiles(config: &mut Config) -> Vec<ProfileInfo> {
    let keyboard = Keyboard::connect_first();
    let current = keyboard
        .as_ref()
        .and_then(|connected| connected.current_profile().ok())
        .map(|index| index + 1);
    let assigned = config.configured_profile().ok().map(|index| index + 1);

    if let Some(connected) = keyboard.as_ref() {
        let profiles: Vec<_> = (0..PROFILE_COUNT)
            .filter_map(|index| {
                connected
                    .profile_name(index)
                    .ok()
                    .flatten()
                    .map(|name| (index + 1, name))
            })
            .collect();
        if !profiles.is_empty() {
            cache_profile_names(config, &profiles);
            return profile_info(profiles, current, assigned);
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

struct Keyboard;

impl Keyboard {
    fn connect_first() -> Option<Self> {
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
        Some(Self)
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn current_profile(&self) -> Result<u8> {
        unsafe {
            let response_size = rgb::wooting_usb_get_response_size() as usize;
            if response_size == 0 {
                bail!("keyboard reported a zero-length response buffer")
            }

            let uses_multi_report = rgb::wooting_usb_use_multi_report();
            let report_offset = usize::from(uses_multi_report);
            let extra_data_offset = usize::from(uses_multi_report);
            let mut buffer = vec![0_u8; response_size];

            let response = rgb::wooting_usb_send_feature_with_response(
                buffer.as_mut_ptr(),
                response_size,
                GET_CURRENT_KEYBOARD_PROFILE_INDEX,
                0,
                0,
                0,
                0,
            );

            let expected_response = i32::try_from(response_size)
                .context("keyboard response size does not fit in an i32")?;
            if response != expected_response {
                bail!("profile read failed: received {response} bytes, expected {response_size}")
            }

            let is_v2 = rgb::wooting_usb_use_v2_interface();
            let data_offset = report_offset + if is_v2 { 5 } else { 4 } + extra_data_offset;
            let profile = *buffer
                .get(data_offset)
                .ok_or_else(|| anyhow!("profile response did not contain byte {data_offset}"))?;

            if profile >= PROFILE_COUNT {
                bail!("keyboard returned unsupported onboard profile index {profile}")
            }

            Ok(profile)
        }
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn profile_name(&self, profile: u8) -> Result<Option<String>> {
        unsafe {
            let response_size = rgb::wooting_usb_get_response_size() as usize;
            if response_size == 0 {
                bail!("keyboard reported a zero-length response buffer")
            }

            let uses_multi_report = rgb::wooting_usb_use_multi_report();
            let report_offset = usize::from(uses_multi_report);
            let extra_data_offset = usize::from(uses_multi_report);
            let mut buffer = vec![0_u8; response_size];
            let response = rgb::wooting_usb_send_feature_with_response(
                buffer.as_mut_ptr(),
                response_size,
                GET_PROFILE_METADATA,
                0,
                0,
                0,
                profile,
            );
            let expected_response = i32::try_from(response_size)?;
            if response != expected_response {
                bail!("profile metadata read failed")
            }

            let is_v2 = rgb::wooting_usb_use_v2_interface();
            let data_offset = report_offset + if is_v2 { 5 } else { 4 } + extra_data_offset;
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
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn activate_profile(&self, profile: u8, config: &Config) -> Result<()> {
        if profile >= PROFILE_COUNT {
            bail!("profile index {profile} is outside the onboard profile range")
        }

        self.send_profile_commands_fast(profile)?;
        self.finish_profile_switch(profile, config)
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn send_profile_commands_fast(&self, profile: u8) -> Result<()> {
        unsafe {
            let activate = wooting_usb_send_feature_buff(ACTIVATE_PROFILE, 0, 0, 0, profile);
            let reload = wooting_usb_send_feature_buff(RELOAD_PROFILE, 0, 0, 0, profile);
            if activate != WOOTING_COMMAND_SIZE || reload != WOOTING_COMMAND_SIZE {
                bail!("keyboard rejected the immediate profile commands")
            }
        }
        Ok(())
    }

    #[allow(clippy::unused_self)] // The value is a guard for the selected global SDK device.
    fn drain_feature_response(&self) {
        unsafe {
            let response_size = rgb::wooting_usb_get_response_size() as usize;
            let mut buffer = vec![0_u8; response_size];
            let _ =
                rgb::wooting_usb_read_response_timeout(buffer.as_mut_ptr(), response_size, 1_000);
        }
    }

    fn finish_profile_switch(&self, profile: u8, config: &Config) -> Result<()> {
        // Clear acknowledgements for activate and reload before sending a query.
        self.drain_feature_response();
        self.drain_feature_response();

        // Modern firmware accepts activate + reload back-to-back. Polling avoids
        // the fixed 250 ms sleeps used by older third-party switchers.
        if !self.wait_for_profile(profile, Duration::from_millis(75)) {
            // Legacy fallback: older firmware may require time before another reload.
            thread::sleep(Duration::from_millis(config.command_delay_ms.max(250)));
            unsafe {
                if !rgb::wooting_usb_send_feature(RELOAD_PROFILE, 0, 0, 0, profile) {
                    bail!("keyboard rejected the legacy reload-profile command")
                }
            }
            let legacy_timeout = Duration::from_millis(250);
            if !self.wait_for_profile(profile, legacy_timeout) {
                bail!("keyboard did not report P{} after activation", profile + 1)
            }
        }

        // Profile activation and lighting activation are separate firmware states.
        // A successful profile-index read therefore does not prove that the RGB
        // engine loaded the selected profile. Always finish the lighting sequence
        // when requested, including after the fast profile-switch path.
        if config.refresh_lighting {
            let delay = Duration::from_millis(config.command_delay_ms);
            thread::sleep(delay);
            unsafe {
                if !rgb::wooting_usb_send_feature(WOOT_DEV_RESET_ALL, 0, 0, 0, 0) {
                    bail!("keyboard rejected the RGB reset command")
                }
            }
            thread::sleep(delay);
            unsafe {
                if !rgb::wooting_usb_send_feature(REFRESH_RGB_COLORS, 0, 0, 0, profile) {
                    bail!("keyboard rejected the RGB refresh command")
                }
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
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{message}");
    }
}

fn try_ipc_set(human_profile: u8) -> Result<bool> {
    validate_human_profile(human_profile)?;
    let address = IPC_ADDRESS.parse()?;
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
    writeln!(stream, "SET {human_profile}")?;
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

fn handle_ipc_stream(mut stream: TcpStream, keyboard: &mut Option<Keyboard>, config_path: &Path) {
    let prepared = (|| -> Result<(u8, Config)> {
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
        let mut request = String::new();
        stream.read_to_string(&mut request)?;
        let profile = request
            .trim()
            .strip_prefix("SET ")
            .context("unknown IPC command")?
            .parse::<u8>()?;
        validate_human_profile(profile)?;
        if keyboard.is_none() {
            *keyboard = Keyboard::connect_first();
        }
        keyboard
            .as_ref()
            .context("no Wooting keyboard is connected")?;
        let mut config = load_config(config_path)?;
        config.learn_for_current_os(profile)?;
        Ok((profile, config))
    })();

    match prepared {
        Err(error) => {
            let _ = writeln!(stream, "ERR {error:#}");
        }
        Ok((profile, config)) => {
            let Some(connected) = keyboard.as_ref() else {
                return;
            };
            match connected.activate_profile(profile - 1, &config) {
                Ok(()) => {
                    let _ = stream.write_all(b"OK\n");
                    let _ = stream.flush();
                }
                Err(error) => {
                    append_log(
                        config_path,
                        &format!("IPC profile application failed: {error:#}"),
                    );
                    let _ = writeln!(stream, "ERR {error:#}");
                    let _ = stream.flush();
                }
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
    let listener = TcpListener::bind(IPC_ADDRESS)
        .with_context(|| format!("failed to bind local control socket {IPC_ADDRESS}"))?;
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
    let mut next_device_check = Instant::now();
    let mut next_enforcement_check = Instant::now();

    while running.load(Ordering::SeqCst) {
        loop {
            match listener.accept() {
                Ok((stream, _)) => handle_ipc_stream(stream, &mut keyboard, config_path),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    append_log(config_path, &format!("IPC accept failed: {error}"));
                    break;
                }
            }
        }

        if Instant::now() >= next_device_check {
            if keyboard.is_none() {
                keyboard = Keyboard::connect_first();
                if let Some(connected) = keyboard.as_ref() {
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
                if connected.current_profile().is_ok() {
                    let config = load_config(config_path)?;
                    let enforcement_due = config.enabled
                        && (enforce_override || config.enforce)
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
                } else {
                    append_log(config_path, "Keyboard disconnected");
                    keyboard = None;
                }
            }
            next_device_check = Instant::now() + interval;
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
            save_config(&self.config_path, &self.config)?;
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

    let profiles = discover_profiles(&mut config);
    save_config(config_path, &config)?;
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

    config.learn_for_current_os(selected_profile)?;
    save_config(config_path, &config)?;
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
    config.enabled = enabled;
    save_config(path, config)?;
    if enabled {
        if let Ok(profile) = config.configured_profile()
            && !try_ipc_set(profile + 1)?
            && let Some(keyboard) = Keyboard::connect_first()
        {
            apply_and_verify(&keyboard, config, false)?;
        }
        start_hidden_watcher(path)?;
    }
    Ok(())
}

fn set_enforcement(path: &Path, config: &mut Config, enabled: bool) -> Result<()> {
    config.enforce = enabled;
    save_config(path, config)
}

fn set_status_icon(path: &Path, config: &mut Config, visible: bool) -> Result<()> {
    config.show_status_icon = visible;
    save_config(path, config)
}

fn list_profiles(path: &Path, config: &mut Config, json: bool) -> Result<()> {
    let profiles = discover_profiles(config);
    save_config(path, config)?;
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let path = config_path(cli.config.as_deref())?;
    let mut config = load_config(&path)?;

    match cli.command {
        Some(Command::Profiles { json }) => {
            list_profiles(&path, &mut config, json)?;
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
            config.learn_for_current_os(human_profile)?;
            save_config(&path, &config)?;
            println!(
                "Learned P{human_profile} for {} and saved {}",
                std::env::consts::OS,
                path.display()
            );
        }
        Some(Command::Apply) => {
            let profile = config.configured_profile()? + 1;
            if !try_ipc_set(profile)? {
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
            config.learn_for_current_os(profile)?;
            save_config(&path, &config)?;
            match startup {
                StartupChoice::Keep => {}
                StartupChoice::Enable => set_startup_enabled(true, &path)?,
                StartupChoice::Disable => set_startup_enabled(false, &path)?,
            }
            if config.enabled
                && !try_ipc_set(profile)?
                && let Some(keyboard) = Keyboard::connect_first()
            {
                apply_and_verify(&keyboard, &config, false)?;
            }
            start_hidden_watcher(&path)?;
        }
        Some(Command::Set { profile }) => {
            validate_human_profile(profile)?;
            if !try_ipc_set(profile)? {
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
}
