use std::fmt;
use std::time::Duration;

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
    pub lock: LockConfig,
}

impl Config {
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            breaks: Breaks::default(),
            disable_presets: DEFAULT_DISABLE_PRESETS.to_vec(),
            lock: LockConfig::default(),
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
                message: String::from("Rest your eyes"),
            },
            long: BreakConfig {
                after_active: DEFAULT_LONG_BREAK_AFTER_ACTIVE,
                duration: DEFAULT_LONG_BREAK_DURATION,
                message: String::from("Take a longer break"),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakConfig {
    pub after_active: Duration,
    pub duration: Duration,
    pub message: String,
}

impl BreakConfig {
    fn validate(&self, kind: BreakKind) -> Result<(), ConfigError> {
        if self.after_active.is_zero() {
            return Err(ConfigError::ZeroActiveDuration { kind });
        }

        if self.duration.is_zero() {
            return Err(ConfigError::ZeroBreakDuration { kind });
        }

        if self.message.trim().is_empty() {
            return Err(ConfigError::EmptyBreakMessage { kind });
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockConfig {
    pub after_short_break: bool,
    pub after_long_break: bool,
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
    EmptyBreakMessage { kind: BreakKind },
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
            Self::EmptyBreakMessage { kind } => {
                write!(formatter, "{kind} break message must not be empty")
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

#[cfg(test)]
mod tests {
    use super::{
        BreakKind, Config, ConfigError, DEFAULT_DISABLE_PRESETS, DEFAULT_LONG_BREAK_AFTER_ACTIVE,
        DEFAULT_LONG_BREAK_DURATION, DEFAULT_SHORT_BREAK_AFTER_ACTIVE,
        DEFAULT_SHORT_BREAK_DURATION,
    };
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
        assert_eq!(config.breaks.short.message, "Rest your eyes");
        assert_eq!(
            config.breaks.long.after_active,
            DEFAULT_LONG_BREAK_AFTER_ACTIVE
        );
        assert_eq!(config.breaks.long.duration, DEFAULT_LONG_BREAK_DURATION);
        assert_eq!(config.breaks.long.message, "Take a longer break");
    }

    #[test]
    fn default_config_uses_expected_disable_presets() {
        let config = Config::default();

        assert_eq!(config.disable_presets, DEFAULT_DISABLE_PRESETS);
    }

    #[test]
    fn default_config_does_not_lock_after_breaks() {
        let config = Config::default();

        assert!(!config.lock.after_short_break);
        assert!(!config.lock.after_long_break);
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
    fn rejects_empty_break_message() {
        let mut config = Config::default();
        config.breaks.short.message = String::from("   ");

        assert_eq!(
            config.validate(),
            Err(ConfigError::EmptyBreakMessage {
                kind: BreakKind::Short
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
}
