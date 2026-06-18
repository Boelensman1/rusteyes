use serde::{Deserialize, de};
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

pub const DEFAULT_SHORT_BREAK_AFTER_ACTIVE: Duration = Duration::from_secs(20 * 60);
pub const DEFAULT_SHORT_BREAK_DURATION: Duration = Duration::from_secs(20);
pub const DEFAULT_LONG_BREAK_AFTER_ACTIVE: Duration = Duration::from_secs(60 * 60);
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
    pub autolock: AutolockConfig,
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
        self.breaks.short.validate(BreakKind::Short)?;
        self.breaks.long.validate(BreakKind::Long)?;
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
            autolock: AutolockConfig::default(),
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
    pub short: BreakConfig,
    pub long: BreakConfig,
}

impl Default for Breaks {
    fn default() -> Self {
        Self {
            short: BreakConfig {
                after_active: DEFAULT_SHORT_BREAK_AFTER_ACTIVE,
                duration: DEFAULT_SHORT_BREAK_DURATION,
                messages: vec![String::from("Rest your eyes")],
            },
            long: BreakConfig {
                after_active: DEFAULT_LONG_BREAK_AFTER_ACTIVE,
                duration: DEFAULT_LONG_BREAK_DURATION,
                messages: vec![String::from("Take a longer break")],
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakConfig {
    pub after_active: Duration,
    pub duration: Duration,
    pub messages: Vec<String>,
}

impl BreakConfig {
    fn validate(&self, kind: BreakKind) -> Result<(), ConfigError> {
        if self.after_active.is_zero() {
            return Err(ConfigError::ZeroActiveDuration { kind });
        }

        if self.duration.is_zero() {
            return Err(ConfigError::ZeroBreakDuration { kind });
        }

        if self.messages.is_empty() {
            return Err(ConfigError::EmptyBreakMessages { kind });
        }

        for (index, message) in self.messages.iter().enumerate() {
            if message.trim().is_empty() {
                return Err(ConfigError::EmptyBreakMessage { kind, index });
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutolockConfig {
    pub after_short_break: bool,
    pub after_long_break: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPathMode {
    Required,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakKind {
    Short,
    Long,
}

impl fmt::Display for BreakKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Short => formatter.write_str("short"),
            Self::Long => formatter.write_str("long"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    ZeroActiveDuration { kind: BreakKind },
    ZeroBreakDuration { kind: BreakKind },
    EmptyBreakMessages { kind: BreakKind },
    EmptyBreakMessage { kind: BreakKind, index: usize },
    EmptyDisablePresets,
    ZeroDisablePreset { index: usize },
    DuplicateDisablePreset { duration: Duration },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroActiveDuration { kind } => {
                write!(
                    formatter,
                    "{kind} break active duration must be greater than zero"
                )
            }
            Self::ZeroBreakDuration { kind } => {
                write!(formatter, "{kind} break duration must be greater than zero")
            }
            Self::EmptyBreakMessages { kind } => {
                write!(formatter, "{kind} break messages must not be empty")
            }
            Self::EmptyBreakMessage { kind, index } => {
                write!(formatter, "{kind} break message {index} must not be empty")
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
    autolock: Option<PartialAutolockConfig>,
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

        if let Some(autolock) = self.autolock {
            autolock.apply_to(&mut config.autolock);
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialBreaks {
    short: Option<PartialBreakConfig>,
    long: Option<PartialBreakConfig>,
}

impl PartialBreaks {
    fn apply_to(self, breaks: &mut Breaks) {
        if let Some(short) = self.short {
            short.apply_to(&mut breaks.short);
        }

        if let Some(long) = self.long {
            long.apply_to(&mut breaks.long);
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialBreakConfig {
    after_active: Option<ConfigDuration>,
    duration: Option<ConfigDuration>,
    messages: Option<Vec<String>>,
}

impl PartialBreakConfig {
    fn apply_to(self, break_config: &mut BreakConfig) {
        if let Some(after_active) = self.after_active {
            break_config.after_active = after_active.into_duration();
        }

        if let Some(duration) = self.duration {
            break_config.duration = duration.into_duration();
        }

        if let Some(messages) = self.messages {
            break_config.messages = messages;
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialAutolockConfig {
    after_short_break: Option<bool>,
    after_long_break: Option<bool>,
}

impl PartialAutolockConfig {
    fn apply_to(self, autolock: &mut AutolockConfig) {
        if let Some(after_short_break) = self.after_short_break {
            autolock.after_short_break = after_short_break;
        }

        if let Some(after_long_break) = self.after_long_break {
            autolock.after_long_break = after_long_break;
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
        AutolockConfig, BreakKind, Config, ConfigError, ConfigLoadError, DEFAULT_DISABLE_PRESETS,
        DEFAULT_LONG_BREAK_AFTER_ACTIVE, DEFAULT_LONG_BREAK_DURATION,
        DEFAULT_SHORT_BREAK_AFTER_ACTIVE, DEFAULT_SHORT_BREAK_DURATION,
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

        assert_eq!(
            config.breaks.short.after_active,
            DEFAULT_SHORT_BREAK_AFTER_ACTIVE
        );
        assert_eq!(config.breaks.short.duration, DEFAULT_SHORT_BREAK_DURATION);
        assert_eq!(
            config.breaks.short.messages,
            vec![String::from("Rest your eyes")]
        );
        assert_eq!(
            config.breaks.long.after_active,
            DEFAULT_LONG_BREAK_AFTER_ACTIVE
        );
        assert_eq!(config.breaks.long.duration, DEFAULT_LONG_BREAK_DURATION);
        assert_eq!(
            config.breaks.long.messages,
            vec![String::from("Take a longer break")]
        );
    }

    #[test]
    fn default_config_uses_expected_disable_presets() {
        let config = Config::default();

        assert_eq!(config.disable_presets, DEFAULT_DISABLE_PRESETS);
    }

    #[test]
    fn default_config_does_not_autolock_after_breaks() {
        let config = Config::default();

        assert_eq!(config.autolock, AutolockConfig::default());
    }

    #[test]
    fn rejects_zero_active_duration() {
        let mut config = Config::default();
        config.breaks.short.after_active = Duration::ZERO;

        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroActiveDuration {
                kind: BreakKind::Short
            })
        );
    }

    #[test]
    fn rejects_zero_break_duration() {
        let mut config = Config::default();
        config.breaks.long.duration = Duration::ZERO;

        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroBreakDuration {
                kind: BreakKind::Long
            })
        );
    }

    #[test]
    fn rejects_empty_break_messages() {
        let mut config = Config::default();
        config.breaks.short.messages.clear();

        assert_eq!(
            config.validate(),
            Err(ConfigError::EmptyBreakMessages {
                kind: BreakKind::Short
            })
        );
    }

    #[test]
    fn rejects_blank_break_message() {
        let mut config = Config::default();
        config.breaks.short.messages = vec![String::from("Look away"), String::from("   ")];

        assert_eq!(
            config.validate(),
            Err(ConfigError::EmptyBreakMessage {
                kind: BreakKind::Short,
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
  short:
    duration: '31s'
",
        )?;
        write_file(
            &xdg_path,
            r"
breaks:
  short:
    duration: '41s'
",
        )?;

        let config = Config::load_from_env(
            Some(explicit_path.into_os_string()),
            Some(xdg_home.into_os_string()),
            None,
        )?;

        assert_eq!(config.breaks.short.duration, Duration::from_secs(31));
        Ok(())
    }

    #[test]
    fn partial_yaml_overlays_defaults() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
breaks:
  short:
    duration: '45s'
    messages:
      - Blink slowly
",
        )?;

        assert_eq!(
            config.breaks.short.after_active,
            DEFAULT_SHORT_BREAK_AFTER_ACTIVE
        );
        assert_eq!(config.breaks.short.duration, Duration::from_secs(45));
        assert_eq!(
            config.breaks.short.messages,
            vec![String::from("Blink slowly")]
        );
        assert_eq!(
            config.breaks.long.after_active,
            DEFAULT_LONG_BREAK_AFTER_ACTIVE
        );
        assert_eq!(
            config.breaks.long.messages,
            vec![String::from("Take a longer break")]
        );
        assert_eq!(config.disable_presets, DEFAULT_DISABLE_PRESETS);
        assert_eq!(config.autolock, AutolockConfig::default());
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
  short:
    after_active: '20m'
    duration: '20s'
  long:
    after_active: '1h'
    duration: '5m'
disable_presets: ['30m', '1h', '2h', '3h']
",
        )?;

        assert_eq!(config.breaks.short.after_active, Duration::from_secs(1200));
        assert_eq!(config.breaks.short.duration, Duration::from_secs(20));
        assert_eq!(
            config.breaks.long.after_active,
            Duration::from_secs(60 * 60)
        );
        assert_eq!(config.breaks.long.duration, Duration::from_secs(5 * 60));
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
    fn yaml_accepts_multiple_messages() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
breaks:
  long:
    messages:
      - Take a longer break
      - Stretch and look away
",
        )?;

        assert_eq!(
            config.breaks.long.messages,
            vec![
                String::from("Take a longer break"),
                String::from("Stretch and look away")
            ]
        );
        Ok(())
    }

    #[test]
    fn yaml_maps_autolock_config() -> Result<(), Box<dyn Error>> {
        let config = Config::from_yaml_str(
            r"
autolock:
  after_short_break: true
  after_long_break: true
",
        )?;

        assert!(config.autolock.after_short_break);
        assert!(config.autolock.after_long_break);
        Ok(())
    }

    #[test]
    fn yaml_rejects_empty_message_list() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  short:
    messages: []
",
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::EmptyBreakMessages {
                    kind: BreakKind::Short
                },
                ..
            }
        ));
    }

    #[test]
    fn yaml_rejects_blank_message() {
        let error = expected_config_error(Config::from_yaml_str(
            r#"
breaks:
  short:
    messages:
      - " "
"#,
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::EmptyBreakMessage {
                    kind: BreakKind::Short,
                    index: 0
                },
                ..
            }
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
lock:
  after_short_break: true
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_invalid_duration_values() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  short:
    duration: 'soon'
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_integer_duration_values() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  short:
    duration: 20
",
        ));

        assert!(matches!(error, ConfigLoadError::Parse { .. }));
    }

    #[test]
    fn yaml_rejects_validation_failures() {
        let error = expected_config_error(Config::from_yaml_str(
            r"
breaks:
  short:
    duration: '0s'
",
        ));

        assert!(matches!(
            error,
            ConfigLoadError::Invalid {
                error: ConfigError::ZeroBreakDuration {
                    kind: BreakKind::Short
                },
                ..
            }
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
