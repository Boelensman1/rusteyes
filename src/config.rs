use serde::{Deserialize, de};
use std::collections::{BTreeMap, btree_map::Entry};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ENV_CONFIG: &str = "RESTEYES_CONFIG";
const XDG_CONFIG_HOME: &str = "XDG_CONFIG_HOME";
const HOME: &str = "HOME";
const CONFIG_DIR: &str = "resteyes";
const CONFIG_FILE: &str = "config.yaml";

pub const DEFAULT_BREAK_AFTER_ACTIVE: Duration = Duration::from_secs(20 * 60);
pub const DEFAULT_SHORT_BREAK_INTERVAL: usize = 1;
pub const DEFAULT_SHORT_BREAK_DURATION: Duration = Duration::from_secs(20);
pub const DEFAULT_LONG_BREAK_INTERVAL: usize = 2;
pub const DEFAULT_LONG_BREAK_DURATION: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_DISABLE_PRESETS: [Duration; 4] = [
    Duration::from_secs(30 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(2 * 60 * 60),
    Duration::from_secs(3 * 60 * 60),
];

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub breaks: Breaks,
    pub disable_presets: Vec<Duration>,
}

impl Config {
    /// Loads config defaults and overlays a YAML config file when one is present.
    ///
    /// The config file is resolved from `RESTEYES_CONFIG`, then the XDG config
    /// path, then the default `$HOME/.config/resteyes/config.yaml` path.
    ///
    /// # Errors
    ///
    /// Returns an error when an explicit config cannot be read, an implicit
    /// config exists but cannot be read, YAML parsing fails, or validation fails.
    pub fn load() -> Result<Self, ConfigLoadError> {
        Self::load_from_env(
            std::env::var_os(ENV_CONFIG),
            std::env::var_os(XDG_CONFIG_HOME),
            std::env::var_os(HOME),
        )
    }

    /// Parses YAML config and applies it over the built-in defaults.
    ///
    /// Scalar values overlay defaults. When `breaks.types` is present, it
    /// replaces the default break type map.
    ///
    /// # Errors
    ///
    /// Returns an error when YAML parsing or config validation fails.
    pub fn from_yaml_str(input: &str) -> Result<Self, ConfigLoadError> {
        Self::from_yaml_str_with_path(input, None)
    }

    /// Validates config values after defaults and file overrides are applied.
    ///
    /// # Errors
    ///
    /// Returns the first invalid value found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.breaks.validate()?;
        validate_disable_presets(&self.disable_presets)
    }

    fn load_from_env(
        resteyes_config: Option<OsString>,
        xdg_config_home: Option<OsString>,
        home: Option<OsString>,
    ) -> Result<Self, ConfigLoadError> {
        if let Some(path) = non_empty_os(resteyes_config).map(PathBuf::from) {
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLoadError {
    Read {
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
            Self::Read { .. } | Self::Parse { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breaks {
    pub after_active: Duration,
    pub types: BTreeMap<String, BreakTypeConfig>,
}

impl Breaks {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.after_active.is_zero() {
            return Err(ConfigError::ZeroBreakAfterActiveDuration);
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
            types,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakTypeConfig {
    pub interval: usize,
    pub duration: Duration,
    pub messages: Vec<String>,
    pub autolock: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPathMode {
    Required,
    Optional,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    ZeroBreakAfterActiveDuration,
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
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroBreakAfterActiveDuration => {
                formatter.write_str("break active duration must be greater than zero")
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
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialConfig {
    breaks: Option<PartialBreaks>,
    disable_presets: Option<Vec<ConfigDuration>>,
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
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialBreaks {
    after_active: Option<ConfigDuration>,
    types: Option<BTreeMap<String, YamlBreakTypeConfig>>,
}

impl PartialBreaks {
    fn apply_to(self, breaks: &mut Breaks) {
        if let Some(after_active) = self.after_active {
            breaks.after_active = after_active.into_duration();
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

fn non_empty_os(value: Option<OsString>) -> Option<OsString> {
    value.filter(|value| !value.is_empty())
}

fn config_path_from_base(base: &Path) -> PathBuf {
    base.join(CONFIG_DIR).join(CONFIG_FILE)
}

fn config_location(path: Option<&Path>) -> String {
    path.map_or_else(
        || String::from("config YAML"),
        |path| path.display().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        BreakTypeConfig, Config, ConfigError, ConfigLoadError, DEFAULT_BREAK_AFTER_ACTIVE,
        DEFAULT_DISABLE_PRESETS, DEFAULT_LONG_BREAK_DURATION, DEFAULT_LONG_BREAK_INTERVAL,
        DEFAULT_SHORT_BREAK_DURATION, DEFAULT_SHORT_BREAK_INTERVAL,
    };
    use std::error::Error;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();

        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn default_config_uses_expected_break_settings() {
        let config = Config::default();

        assert_eq!(config.breaks.after_active, DEFAULT_BREAK_AFTER_ACTIVE);
        assert_eq!(config.breaks.types.len(), 2);

        let short = &config.breaks.types["short"];
        assert_eq!(short.interval, DEFAULT_SHORT_BREAK_INTERVAL);
        assert_eq!(short.duration, DEFAULT_SHORT_BREAK_DURATION);
        assert_eq!(short.messages, vec![String::from("Rest your eyes")]);
        assert!(!short.autolock);

        let long = &config.breaks.types["long"];
        assert_eq!(long.interval, DEFAULT_LONG_BREAK_INTERVAL);
        assert_eq!(long.duration, DEFAULT_LONG_BREAK_DURATION);
        assert_eq!(long.messages, vec![String::from("Take a longer break")]);
        assert!(long.autolock);
    }

    #[test]
    fn default_config_uses_expected_disable_presets() {
        let config = Config::default();

        assert_eq!(config.disable_presets, DEFAULT_DISABLE_PRESETS);
    }

    #[test]
    fn rejects_zero_active_duration() {
        let mut config = Config::default();
        config.breaks.after_active = Duration::ZERO;

        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroBreakAfterActiveDuration)
        );
    }

    #[test]
    fn rejects_empty_break_types() {
        let mut config = Config::default();
        config.breaks.types.clear();

        assert_eq!(config.validate(), Err(ConfigError::EmptyBreakTypes));
    }

    #[test]
    fn rejects_empty_break_type_name() {
        let mut config = Config::default();
        config.breaks.types.clear();
        config.breaks.types.insert(
            String::new(),
            BreakTypeConfig {
                interval: 1,
                duration: Duration::from_secs(1),
                messages: vec![String::from("Rest")],
                autolock: false,
            },
        );

        assert_eq!(
            config.validate(),
            Err(ConfigError::InvalidBreakTypeName {
                name: String::new()
            })
        );
    }

    #[test]
    fn rejects_whitespace_padded_break_type_name() {
        let mut config = Config::default();
        config.breaks.types.clear();
        config.breaks.types.insert(
            String::from(" short"),
            BreakTypeConfig {
                interval: 1,
                duration: Duration::from_secs(1),
                messages: vec![String::from("Rest")],
                autolock: false,
            },
        );

        assert_eq!(
            config.validate(),
            Err(ConfigError::InvalidBreakTypeName {
                name: String::from(" short")
            })
        );
    }

    #[test]
    fn rejects_zero_break_interval() {
        let mut config = Config::default();
        config.breaks.types.insert(
            String::from("short"),
            BreakTypeConfig {
                interval: 0,
                duration: Duration::from_secs(1),
                messages: vec![String::from("Rest")],
                autolock: false,
            },
        );

        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroBreakInterval {
                name: String::from("short")
            })
        );
    }

    #[test]
    fn rejects_duplicate_break_interval() {
        let mut config = Config::default();
        config.breaks.types.insert(
            String::from("long"),
            BreakTypeConfig {
                interval: 1,
                duration: Duration::from_secs(1),
                messages: vec![String::from("Rest longer")],
                autolock: true,
            },
        );

        assert_eq!(
            config.validate(),
            Err(ConfigError::DuplicateBreakInterval {
                interval: 1,
                first_name: String::from("long"),
                duplicate_name: String::from("short")
            })
        );
    }

    #[test]
    fn rejects_zero_break_duration() {
        let mut config = Config::default();
        config.breaks.types.insert(
            String::from("long"),
            BreakTypeConfig {
                interval: DEFAULT_LONG_BREAK_INTERVAL,
                duration: Duration::ZERO,
                messages: vec![String::from("Rest longer")],
                autolock: true,
            },
        );

        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroBreakDuration {
                name: String::from("long")
            })
        );
    }

    #[test]
    fn rejects_empty_break_messages() {
        let mut config = Config::default();
        config.breaks.types.insert(
            String::from("short"),
            BreakTypeConfig {
                interval: DEFAULT_SHORT_BREAK_INTERVAL,
                duration: DEFAULT_SHORT_BREAK_DURATION,
                messages: Vec::new(),
                autolock: false,
            },
        );

        assert_eq!(
            config.validate(),
            Err(ConfigError::EmptyBreakMessages {
                name: String::from("short")
            })
        );
    }

    #[test]
    fn rejects_blank_break_message() {
        let mut config = Config::default();
        config.breaks.types.insert(
            String::from("short"),
            BreakTypeConfig {
                interval: DEFAULT_SHORT_BREAK_INTERVAL,
                duration: DEFAULT_SHORT_BREAK_DURATION,
                messages: vec![String::from("Look away"), String::from("   ")],
                autolock: false,
            },
        );

        assert_eq!(
            config.validate(),
            Err(ConfigError::EmptyBreakMessage {
                name: String::from("short"),
                index: 1
            })
        );
    }

    #[test]
    fn rejects_empty_disable_presets() {
        let mut config = Config::default();
        config.disable_presets.clear();

        assert_eq!(config.validate(), Err(ConfigError::EmptyDisablePresets));
    }

    #[test]
    fn rejects_zero_disable_preset() {
        let mut config = Config::default();
        config.disable_presets[1] = Duration::ZERO;

        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroDisablePreset { index: 1 })
        );
    }

    #[test]
    fn rejects_duplicate_disable_preset() {
        let mut config = Config::default();
        let duplicate = config.disable_presets[0];
        config.disable_presets[1] = duplicate;

        assert_eq!(
            config.validate(),
            Err(ConfigError::DuplicateDisablePreset {
                duration: duplicate
            })
        );
    }

    #[test]
    fn load_uses_defaults_when_implicit_config_is_missing() -> Result<(), Box<dyn Error>> {
        let test_dir = TestDir::new("missing-implicit")?;
        let xdg_home = test_dir.path().join("xdg");

        let config = Config::load_from_env(None, Some(xdg_home.into_os_string()), None)?;

        assert_eq!(config, Config::default());
        Ok(())
    }

    #[test]
    fn resteyes_config_takes_precedence_over_xdg_path() -> Result<(), Box<dyn Error>> {
        let test_dir = TestDir::new("env-precedence")?;
        let explicit_path = test_dir.path().join("explicit.yaml");
        let xdg_home = test_dir.path().join("xdg");
        let xdg_path = xdg_home.join("resteyes").join("config.yaml");

        write_file(
            &explicit_path,
            r"
breaks:
  after_active: '10m'
",
        )?;
        write_file(
            &xdg_path,
            r"
breaks:
  after_active: '30m'
",
        )?;

        let config = Config::load_from_env(
            Some(explicit_path.into_os_string()),
            Some(xdg_home.into_os_string()),
            None,
        )?;

        assert_eq!(config.breaks.after_active, Duration::from_secs(10 * 60));
        Ok(())
    }

    #[test]
    fn partial_yaml_overlays_defaults() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
breaks:
  after_active: '30m'
",
        )?;

        assert_eq!(config.breaks.after_active, Duration::from_secs(30 * 60));
        assert_eq!(config.breaks.types, Config::default().breaks.types);
        assert_eq!(config.disable_presets, DEFAULT_DISABLE_PRESETS);
        Ok(())
    }

    #[test]
    fn empty_yaml_uses_defaults() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str("")?;

        assert_eq!(config, Config::default());
        Ok(())
    }

    #[test]
    fn yaml_accepts_humantime_duration_strings() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
breaks:
  after_active: '20m'
  types:
    short:
      interval: 1
      duration: '20s'
      messages:
        - Rest your eyes
    long:
      interval: 4
      duration: '5m'
      messages:
        - Take a longer break
      autolock: true
disable_presets: ['30m', '1h', '2h', '3h']
",
        )?;

        assert_eq!(config.breaks.after_active, Duration::from_secs(1200));
        assert_eq!(
            config.breaks.types["short"].duration,
            Duration::from_secs(20)
        );
        assert_eq!(config.breaks.types["long"].interval, 4);
        assert_eq!(
            config.breaks.types["long"].duration,
            Duration::from_secs(5 * 60)
        );
        assert!(config.breaks.types["long"].autolock);
        assert_eq!(
            config.disable_presets,
            vec![
                Duration::from_secs(30 * 60),
                Duration::from_secs(60 * 60),
                Duration::from_secs(2 * 60 * 60),
                Duration::from_secs(3 * 60 * 60)
            ]
        );
        Ok(())
    }

    #[test]
    fn yaml_replaces_default_break_types() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
breaks:
  types:
    blink:
      interval: 1
      duration: '6s'
      messages:
        - Blink slowly
    short:
      interval: 50
      duration: '5m'
      messages:
        - Stand up
    long:
      interval: 1000
      duration: '1h'
      messages:
        - Leave the computer
      autolock: true
",
        )?;

        assert_eq!(
            config
                .breaks
                .types
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["blink", "long", "short"]
        );
        assert_eq!(config.breaks.types["blink"].interval, 1);
        assert_eq!(
            config.breaks.types["blink"].duration,
            Duration::from_secs(6)
        );
        assert!(!config.breaks.types["blink"].autolock);
        assert_eq!(config.breaks.types["short"].interval, 50);
        assert_eq!(config.breaks.types["long"].interval, 1000);
        assert!(config.breaks.types["long"].autolock);
        Ok(())
    }

    #[test]
    fn yaml_accepts_multiple_messages() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
breaks:
  types:
    stretch:
      interval: 1
      duration: '1m'
      messages:
        - Take a break
        - Stretch and look away
",
        )?;

        assert_eq!(
            config.breaks.types["stretch"].messages,
            vec![
                String::from("Take a break"),
                String::from("Stretch and look away")
            ]
        );
        Ok(())
    }

    #[test]
    fn yaml_maps_break_type_autolock_config() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
breaks:
  types:
    lock-screen:
      interval: 1
      duration: '5m'
      messages:
        - Time to lock
      autolock: true
",
        )?;

        assert!(config.breaks.types["lock-screen"].autolock);
        Ok(())
    }

    #[test]
    fn yaml_rejects_empty_break_types() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types: {}
",
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::EmptyBreakTypes,
                ..
            }
        ));
    }

    #[test]
    fn yaml_rejects_empty_break_type_name() {
        let error = expected_config_error(Config::from_yaml_str(
            r#"
breaks:
  types:
    "":
      interval: 1
      duration: "20s"
      messages:
        - Rest
"#,
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::InvalidBreakTypeName { name },
                ..
            } if name.is_empty()
        ));
    }

    #[test]
    fn yaml_rejects_whitespace_padded_break_type_name() {
        let error = expected_config_error(Config::from_yaml_str(
            r#"
breaks:
  types:
    "short ":
      interval: 1
      duration: "20s"
      messages:
        - Rest
"#,
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::InvalidBreakTypeName { name },
                ..
            } if name == "short "
        ));
    }

    #[test]
    fn yaml_rejects_zero_break_interval() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types:
    short:
      interval: 0
      duration: '20s'
      messages:
        - Rest
",
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::ZeroBreakInterval { name },
                ..
            } if name == "short"
        ));
    }

    #[test]
    fn yaml_rejects_duplicate_break_interval() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types:
    short:
      interval: 1
      duration: '20s'
      messages:
        - Rest
    long:
      interval: 1
      duration: '5m'
      messages:
        - Rest longer
",
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::DuplicateBreakInterval {
                    interval: 1,
                    first_name,
                    duplicate_name
                },
                ..
            } if first_name == "long" && duplicate_name == "short"
        ));
    }

    #[test]
    fn yaml_rejects_empty_message_list() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types:
    short:
      interval: 1
      duration: '20s'
      messages: []
",
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::EmptyBreakMessages { name },
                ..
            } if name == "short"
        ));
    }

    #[test]
    fn yaml_rejects_blank_message() {
        let error = expected_config_error(Config::from_yaml_str(
            r#"
breaks:
  types:
    short:
      interval: 1
      duration: "20s"
      messages:
        - " "
"#,
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::EmptyBreakMessage { name, index: 0 },
                ..
            } if name == "short"
        ));
    }

    #[test]
    fn yaml_rejects_malformed_input() {
        let error = expected_config_error(Config::from_yaml_str("breaks: ["));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_unknown_fields() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
unsupported: true
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_old_short_break_shape() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  short:
    duration: '20s'
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_missing_break_type_fields() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types:
    short:
      duration: '20s'
      messages:
        - Rest
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_invalid_duration_values() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types:
    short:
      interval: 1
      duration: 'soon'
      messages:
        - Rest
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_integer_duration_values() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types:
    short:
      interval: 1
      duration: 20
      messages:
        - Rest
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_validation_failures() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  types:
    short:
      interval: 1
      duration: '0s'
      messages:
        - Rest
",
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::ZeroBreakDuration { name },
                ..
            } if name == "short"
        ));
    }

    #[test]
    fn explicit_config_path_must_exist() -> Result<(), Box<dyn Error>> {
        let test_dir = TestDir::new("missing-explicit")?;
        let missing_path = test_dir.path().join("missing.yaml");

        let error = expected_config_error(Config::load_from_env(
            Some(missing_path.clone().into_os_string()),
            None,
            None,
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Read { path, .. } if path == missing_path
        ));
        Ok(())
    }

    fn expected_config_error(result: Result<Config, ConfigLoadError>) -> ConfigLoadError {
        match result {
            Ok(config) => panic!("expected config error, got {config:?}"),
            Err(error) => error,
        }
    }

    fn write_file(path: &Path, contents: &str) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, contents)
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> io::Result<Self> {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("resteyes-{name}-{}-{id}", std::process::id()));
            fs::create_dir_all(&path)?;

            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
