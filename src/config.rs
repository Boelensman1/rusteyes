use serde::{Deserialize, Serialize, de};
use std::collections::{BTreeMap, btree_map::Entry};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const ENV_CONFIG: &str = "RUSTEYES_CONFIG";
const XDG_CONFIG_HOME: &str = "XDG_CONFIG_HOME";
const HOME: &str = "HOME";
const CONFIG_DIR: &str = "rusteyes";
const CONFIG_FILE: &str = "config.yaml";

pub(crate) const DEFAULT_BREAK_AFTER_ACTIVE: Duration = Duration::from_mins(20);
pub(crate) const DEFAULT_SHORT_BREAK_INTERVAL: usize = 1;
pub(crate) const DEFAULT_SHORT_BREAK_DURATION: Duration = Duration::from_secs(20);
pub(crate) const DEFAULT_LONG_BREAK_INTERVAL: usize = 2;
pub(crate) const DEFAULT_LONG_BREAK_DURATION: Duration = Duration::from_mins(5);
pub(crate) const DEFAULT_BREAK_RESET_AFTER_IDLE: Duration = Duration::from_mins(5);
pub(crate) const DEFAULT_DISABLE_PRESETS: [Duration; 4] = [
    Duration::from_mins(30),
    Duration::from_hours(1),
    Duration::from_hours(2),
    Duration::from_hours(3),
];
pub(crate) const MIN_SYNC_SHARED_SECRET_LENGTH: usize = 32;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) breaks: Breaks,
    pub(crate) disable_presets: Vec<Duration>,
    pub(crate) lock: LockConfig,
    pub(crate) sync: SyncConfig,
}

impl Config {
    /// Loads config defaults and overlays a YAML config file when one is present.
    ///
    /// The config file is resolved from `RUSTEYES_CONFIG`, then the XDG config
    /// path, then the default `$HOME/.config/rusteyes/config.yaml` path.
    ///
    /// # Errors
    ///
    /// Returns an error when an explicit config cannot be read, an implicit
    /// config exists but cannot be read, YAML parsing fails, or validation fails.
    pub(crate) fn load() -> Result<Self, ConfigLoadError> {
        Self::load_from_env(
            std::env::var_os(ENV_CONFIG),
            std::env::var_os(XDG_CONFIG_HOME),
            std::env::var_os(HOME),
        )
    }

    #[cfg(test)]
    fn from_yaml_str(input: &str) -> Result<Self, ConfigLoadError> {
        Self::from_yaml_str_with_path(input, None)
    }

    /// Validates config values after defaults and file overrides are applied.
    ///
    /// # Errors
    ///
    /// Returns the first invalid value found.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        self.breaks.validate()?;
        validate_disable_presets(&self.disable_presets)?;
        self.lock.validate()?;
        self.sync.validate()
    }

    fn load_from_env(
        rusteyes_config: Option<OsString>,
        xdg_config_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, ConfigLoadError> {
        if let Some(path) = non_empty_os(rusteyes_config).map(PathBuf::from) {
            return Self::load_from_path(path, ConfigPathMode::Required);
        }

        if let Some(base) = non_empty_os(xdg_config_home).map(PathBuf::from) {
            let path = config_path_from_base(&base);
            return Self::load_from_path(path, ConfigPathMode::Optional);
        }

        if let Some(home) = non_empty_os(home).map(PathBuf::from) {
            let path = config_path_from_base(&home.join(".config"));
            return Self::load_from_path(path, ConfigPathMode::Optional);
        }

        Ok(Self::default())
    }

    fn load_from_path(path: PathBuf, mode: ConfigPathMode) -> Result<Self, ConfigLoadError> {
        match fs::read_to_string(&path) {
            Ok(input) => Self::from_yaml_str_with_path(&input, Some(path)),
            Err(error)
                if mode == ConfigPathMode::Optional && error.kind() == io::ErrorKind::NotFound =>
            {
                write_default_config(&path)?;
                Ok(Self::default())
            }
            Err(error) => Err(ConfigLoadError::Read {
                path,
                message: error.to_string(),
            }),
        }
    }

    fn from_yaml_str_with_path(
        input: &str,
        path: Option<PathBuf>,
    ) -> Result<Self, ConfigLoadError> {
        let partial = serde_saphyr::from_str::<Option<PartialConfig>>(input).map_err(|error| {
            ConfigLoadError::Parse {
                path: path.clone(),
                message: error.to_string(),
            }
        })?;
        let mut config = Self::default();
        let partial = partial.unwrap_or_default();

        partial.apply_to(&mut config);

        config
            .validate()
            .map_err(|error| ConfigLoadError::Invalid { path, error })?;

        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            breaks: Breaks::default(),
            disable_presets: DEFAULT_DISABLE_PRESETS.to_vec(),
            lock: LockConfig::default(),
            sync: SyncConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigLoadError {
    Read {
        path: PathBuf,
        message: String,
    },
    Write {
        path: PathBuf,
        message: String,
    },
    Parse {
        path: Option<PathBuf>,
        message: String,
    },
    Invalid {
        path: Option<PathBuf>,
        error: ConfigError,
    },
}

impl fmt::Display for ConfigLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, message } => {
                write!(
                    formatter,
                    "failed to read config {}: {message}",
                    path.display()
                )
            }
            Self::Write { path, message } => {
                write!(
                    formatter,
                    "failed to write default config {}: {message}",
                    path.display()
                )
            }
            Self::Parse { path, message } => {
                write!(
                    formatter,
                    "failed to parse {}: {message}",
                    config_location(path.as_deref())
                )
            }
            Self::Invalid { path, error } => {
                write!(
                    formatter,
                    "invalid config in {}: {error}",
                    config_location(path.as_deref())
                )
            }
        }
    }
}

impl std::error::Error for ConfigLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid { error, .. } => Some(error),
            Self::Read { .. } | Self::Write { .. } | Self::Parse { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Breaks {
    pub(crate) after_active: Duration,
    pub(crate) reset_after_idle: Option<Duration>,
    pub(crate) types: BTreeMap<String, BreakTypeConfig>,
}

impl Breaks {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.after_active.is_zero() {
            return Err(ConfigError::ZeroBreakAfterActiveDuration);
        }

        if self
            .reset_after_idle
            .is_some_and(|duration| duration.is_zero())
        {
            return Err(ConfigError::ZeroBreakResetAfterIdleDuration);
        }

        if self.types.is_empty() {
            return Err(ConfigError::EmptyBreakTypes);
        }

        let mut intervals = BTreeMap::new();
        for (name, break_type) in &self.types {
            validate_break_type_name(name)?;
            break_type.validate(name)?;

            match intervals.entry(break_type.interval) {
                Entry::Vacant(entry) => {
                    entry.insert(name.as_str());
                }
                Entry::Occupied(entry) => {
                    return Err(ConfigError::DuplicateBreakInterval {
                        interval: break_type.interval,
                        first_name: (*entry.get()).to_owned(),
                        duplicate_name: name.to_owned(),
                    });
                }
            }
        }

        Ok(())
    }
}

impl Default for Breaks {
    fn default() -> Self {
        let mut types = BTreeMap::new();
        types.insert(
            String::from("short"),
            BreakTypeConfig {
                interval: DEFAULT_SHORT_BREAK_INTERVAL,
                duration: DEFAULT_SHORT_BREAK_DURATION,
                messages: vec![String::from("Rest your eyes")],
                autolock: false,
            },
        );
        types.insert(
            String::from("long"),
            BreakTypeConfig {
                interval: DEFAULT_LONG_BREAK_INTERVAL,
                duration: DEFAULT_LONG_BREAK_DURATION,
                messages: vec![String::from("Take a longer break")],
                autolock: true,
            },
        );

        Self {
            after_active: DEFAULT_BREAK_AFTER_ACTIVE,
            reset_after_idle: Some(DEFAULT_BREAK_RESET_AFTER_IDLE),
            types,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakTypeConfig {
    pub(crate) interval: usize,
    pub(crate) duration: Duration,
    pub(crate) messages: Vec<String>,
    pub(crate) autolock: bool,
}

impl BreakTypeConfig {
    fn validate(&self, name: &str) -> Result<(), ConfigError> {
        if self.interval == 0 {
            return Err(ConfigError::ZeroBreakInterval {
                name: name.to_owned(),
            });
        }

        if self.duration.is_zero() {
            return Err(ConfigError::ZeroBreakDuration {
                name: name.to_owned(),
            });
        }

        if self.messages.is_empty() {
            return Err(ConfigError::EmptyBreakMessages {
                name: name.to_owned(),
            });
        }

        for (index, message) in self.messages.iter().enumerate() {
            if message.trim().is_empty() {
                return Err(ConfigError::EmptyBreakMessage {
                    name: name.to_owned(),
                    index,
                });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LockConfig {
    pub(crate) command: Option<Vec<String>>,
}

impl LockConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let Some(command) = &self.command else {
            return Ok(());
        };

        let Some(program) = command.first() else {
            return Err(ConfigError::EmptyLockCommand);
        };

        if program.trim().is_empty() {
            return Err(ConfigError::BlankLockProgram);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SyncConfig {
    pub(crate) enabled: bool,
    pub(crate) shared_secret: Option<SharedSecret>,
}

impl SyncConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        match &self.shared_secret {
            Some(shared_secret) => validate_sync_shared_secret(shared_secret),
            None if self.enabled => Err(ConfigError::MissingSyncSharedSecret),
            None => Ok(()),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SharedSecret(String);

impl SharedSecret {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SharedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedSecret(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPathMode {
    Required,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigError {
    ZeroBreakAfterActiveDuration,
    ZeroBreakResetAfterIdleDuration,
    EmptyBreakTypes,
    InvalidBreakTypeName {
        name: String,
    },
    ZeroBreakInterval {
        name: String,
    },
    DuplicateBreakInterval {
        interval: usize,
        first_name: String,
        duplicate_name: String,
    },
    ZeroBreakDuration {
        name: String,
    },
    EmptyBreakMessages {
        name: String,
    },
    EmptyBreakMessage {
        name: String,
        index: usize,
    },
    EmptyDisablePresets,
    ZeroDisablePreset {
        index: usize,
    },
    DuplicateDisablePreset {
        duration: Duration,
    },
    EmptyLockCommand,
    BlankLockProgram,
    MissingSyncSharedSecret,
    BlankSyncSharedSecret,
    WhitespacePaddedSyncSharedSecret,
    ShortSyncSharedSecret {
        min_length: usize,
        actual_length: usize,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroBreakAfterActiveDuration => {
                formatter.write_str("break active duration must be greater than zero")
            }
            Self::ZeroBreakResetAfterIdleDuration => {
                formatter.write_str("break idle reset duration must be greater than zero")
            }
            Self::EmptyBreakTypes => formatter.write_str("at least one break type must be defined"),
            Self::InvalidBreakTypeName { name } => {
                write!(
                    formatter,
                    "break type name {name:?} must not be empty or contain surrounding whitespace"
                )
            }
            Self::ZeroBreakInterval { name } => {
                write!(
                    formatter,
                    "break type {name} interval must be greater than zero"
                )
            }
            Self::DuplicateBreakInterval {
                interval,
                first_name,
                duplicate_name,
            } => {
                write!(
                    formatter,
                    "break interval {interval} is duplicated by {first_name} and {duplicate_name}"
                )
            }
            Self::ZeroBreakDuration { name } => {
                write!(
                    formatter,
                    "break type {name} duration must be greater than zero"
                )
            }
            Self::EmptyBreakMessages { name } => {
                write!(formatter, "break type {name} messages must not be empty")
            }
            Self::EmptyBreakMessage { name, index } => {
                write!(
                    formatter,
                    "break type {name} message {index} must not be empty"
                )
            }
            Self::EmptyDisablePresets => formatter.write_str("disable presets must not be empty"),
            Self::ZeroDisablePreset { index } => {
                write!(
                    formatter,
                    "disable preset {index} must be greater than zero"
                )
            }
            Self::DuplicateDisablePreset { duration } => {
                write!(formatter, "disable preset {duration:?} is duplicated")
            }
            Self::EmptyLockCommand => formatter.write_str("lock command must not be empty"),
            Self::BlankLockProgram => formatter.write_str("lock command program must not be blank"),
            Self::MissingSyncSharedSecret => {
                formatter.write_str("sync shared_secret is required when sync is enabled")
            }
            Self::BlankSyncSharedSecret => {
                formatter.write_str("sync shared_secret must not be empty")
            }
            Self::WhitespacePaddedSyncSharedSecret => {
                formatter.write_str("sync shared_secret must not contain surrounding whitespace")
            }
            Self::ShortSyncSharedSecret {
                min_length,
                actual_length,
            } => {
                write!(
                    formatter,
                    "sync shared_secret must be at least {min_length} characters long, got {actual_length}"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialConfig {
    breaks: Option<PartialBreaks>,
    disable_presets: Option<Vec<ConfigDuration>>,
    lock: Option<PartialLockConfig>,
    sync: Option<PartialSyncConfig>,
}

impl PartialConfig {
    fn apply_to(self, config: &mut Config) {
        if let Some(breaks) = self.breaks {
            breaks.apply_to(&mut config.breaks);
        }

        if let Some(disable_presets) = self.disable_presets {
            config.disable_presets = disable_presets
                .into_iter()
                .map(ConfigDuration::into_duration)
                .collect();
        }

        if let Some(lock) = self.lock {
            lock.apply_to(&mut config.lock);
        }

        if let Some(sync) = self.sync {
            sync.apply_to(&mut config.sync);
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialBreaks {
    after_active: Option<ConfigDuration>,
    #[serde(default)]
    reset_after_idle: NullableConfigDuration,
    types: Option<BTreeMap<String, YamlBreakTypeConfig>>,
}

impl PartialBreaks {
    fn apply_to(self, breaks: &mut Breaks) {
        if let Some(after_active) = self.after_active {
            breaks.after_active = after_active.into_duration();
        }

        match self.reset_after_idle {
            NullableConfigDuration::Unset => {}
            NullableConfigDuration::Null => breaks.reset_after_idle = None,
            NullableConfigDuration::Value(reset_after_idle) => {
                breaks.reset_after_idle = Some(reset_after_idle.into_duration());
            }
        }

        if let Some(types) = self.types {
            breaks.types = types
                .into_iter()
                .map(|(name, break_type)| (name, break_type.into_config()))
                .collect();
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct YamlBreakTypeConfig {
    interval: usize,
    duration: ConfigDuration,
    messages: Vec<String>,
    #[serde(default)]
    autolock: bool,
}

impl YamlBreakTypeConfig {
    fn into_config(self) -> BreakTypeConfig {
        BreakTypeConfig {
            interval: self.interval,
            duration: self.duration.into_duration(),
            messages: self.messages,
            autolock: self.autolock,
        }
    }
}

#[derive(Debug, Default)]
enum NullableConfigDuration {
    #[default]
    Unset,
    Null,
    Value(ConfigDuration),
}

impl<'de> Deserialize<'de> for NullableConfigDuration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct NullableConfigDurationVisitor;

        impl<'de> de::Visitor<'de> for NullableConfigDurationVisitor {
            type Value = NullableConfigDuration;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a duration string or null")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NullableConfigDuration::Null)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NullableConfigDuration::Null)
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                ConfigDuration::deserialize(deserializer).map(NullableConfigDuration::Value)
            }
        }

        deserializer.deserialize_option(NullableConfigDurationVisitor)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialLockConfig {
    command: Option<Vec<String>>,
}

impl PartialLockConfig {
    fn apply_to(self, lock: &mut LockConfig) {
        lock.command = self.command;
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialSyncConfig {
    enabled: Option<bool>,
    shared_secret: Option<String>,
}

impl PartialSyncConfig {
    fn apply_to(self, sync: &mut SyncConfig) {
        if let Some(enabled) = self.enabled {
            sync.enabled = enabled;
        }

        if let Some(shared_secret) = self.shared_secret {
            sync.shared_secret = Some(SharedSecret::new(shared_secret));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigDuration(Duration);

impl ConfigDuration {
    const fn into_duration(self) -> Duration {
        self.0
    }
}

impl<'de> Deserialize<'de> for ConfigDuration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;

        humantime::parse_duration(&value)
            .map(ConfigDuration)
            .map_err(de::Error::custom)
    }
}

fn validate_break_type_name(name: &str) -> Result<(), ConfigError> {
    if name.is_empty() || name.trim() != name {
        return Err(ConfigError::InvalidBreakTypeName {
            name: name.to_owned(),
        });
    }

    Ok(())
}

fn validate_disable_presets(disable_presets: &[Duration]) -> Result<(), ConfigError> {
    if disable_presets.is_empty() {
        return Err(ConfigError::EmptyDisablePresets);
    }

    for (index, preset) in disable_presets.iter().enumerate() {
        if preset.is_zero() {
            return Err(ConfigError::ZeroDisablePreset { index });
        }

        if disable_presets[..index].contains(preset) {
            return Err(ConfigError::DuplicateDisablePreset { duration: *preset });
        }
    }

    Ok(())
}

fn validate_sync_shared_secret(shared_secret: &SharedSecret) -> Result<(), ConfigError> {
    let value = shared_secret.as_str();

    if value.trim().is_empty() {
        return Err(ConfigError::BlankSyncSharedSecret);
    }

    if value.trim() != value {
        return Err(ConfigError::WhitespacePaddedSyncSharedSecret);
    }

    let actual_length = value.chars().count();
    if actual_length < MIN_SYNC_SHARED_SECRET_LENGTH {
        return Err(ConfigError::ShortSyncSharedSecret {
            min_length: MIN_SYNC_SHARED_SECRET_LENGTH,
            actual_length,
        });
    }

    Ok(())
}

fn non_empty_os(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

fn config_path_from_base(base: &Path) -> PathBuf {
    base.join(CONFIG_DIR).join(CONFIG_FILE)
}

fn write_default_config(path: &Path) -> Result<(), ConfigLoadError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ConfigLoadError::Write {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    }

    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => {
            return Err(ConfigLoadError::Write {
                path: path.to_path_buf(),
                message: error.to_string(),
            });
        }
    };

    let contents = default_config_yaml().map_err(|error| ConfigLoadError::Write {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;

    file.write_all(contents.as_bytes())
        .map_err(|error| ConfigLoadError::Write {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

fn default_config_yaml() -> Result<String, serde_saphyr::ser_error::Error> {
    let config = Config::default();
    let mut output = String::from("# RustEyes config\n");
    output.push_str(&serde_saphyr::to_string(&SerializableConfig::from_config(
        &config,
    ))?);
    Ok(output)
}

fn config_location(path: Option<&Path>) -> String {
    path.map_or_else(
        || String::from("config YAML"),
        |path| path.display().to_string(),
    )
}

#[derive(Serialize)]
struct SerializableConfig<'a> {
    breaks: SerializableBreaks<'a>,
    disable_presets: Vec<SerializableDuration>,
    lock: SerializableLockConfig<'a>,
    sync: SerializableSyncConfig<'a>,
}

impl<'a> SerializableConfig<'a> {
    fn from_config(config: &'a Config) -> Self {
        Self {
            breaks: SerializableBreaks::from_config(&config.breaks),
            disable_presets: config
                .disable_presets
                .iter()
                .copied()
                .map(SerializableDuration)
                .collect(),
            lock: SerializableLockConfig::from_config(&config.lock),
            sync: SerializableSyncConfig::from_config(&config.sync),
        }
    }
}

#[derive(Serialize)]
struct SerializableBreaks<'a> {
    after_active: SerializableDuration,
    reset_after_idle: Option<SerializableDuration>,
    types: BTreeMap<&'a str, SerializableBreakType<'a>>,
}

impl<'a> SerializableBreaks<'a> {
    fn from_config(breaks: &'a Breaks) -> Self {
        Self {
            after_active: SerializableDuration(breaks.after_active),
            reset_after_idle: breaks.reset_after_idle.map(SerializableDuration),
            types: breaks
                .types
                .iter()
                .map(|(name, break_type)| {
                    (
                        name.as_str(),
                        SerializableBreakType::from_config(break_type),
                    )
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct SerializableBreakType<'a> {
    interval: usize,
    duration: SerializableDuration,
    messages: &'a [String],
    autolock: bool,
}

impl<'a> SerializableBreakType<'a> {
    fn from_config(break_type: &'a BreakTypeConfig) -> Self {
        Self {
            interval: break_type.interval,
            duration: SerializableDuration(break_type.duration),
            messages: &break_type.messages,
            autolock: break_type.autolock,
        }
    }
}

#[derive(Serialize)]
struct SerializableLockConfig<'a> {
    command: Option<&'a [String]>,
}

impl<'a> SerializableLockConfig<'a> {
    fn from_config(lock: &'a LockConfig) -> Self {
        Self {
            command: lock.command.as_deref(),
        }
    }
}

#[derive(Serialize)]
struct SerializableSyncConfig<'a> {
    enabled: bool,
    shared_secret: Option<&'a str>,
}

impl<'a> SerializableSyncConfig<'a> {
    fn from_config(sync: &'a SyncConfig) -> Self {
        Self {
            enabled: sync.enabled,
            shared_secret: sync.shared_secret.as_ref().map(SharedSecret::as_str),
        }
    }
}

#[derive(Clone, Copy)]
struct SerializableDuration(Duration);

impl Serialize for SerializableDuration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&humantime::format_duration(self.0).to_string())
    }
}

#[cfg(test)]
mod tests;
