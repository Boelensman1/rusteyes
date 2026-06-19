use super::{
    BreakTypeConfig, Config, ConfigError, ConfigLoadError, DEFAULT_BREAK_AFTER_ACTIVE,
    DEFAULT_DISABLE_PRESETS, DEFAULT_LOCK_COMMAND, DEFAULT_LONG_BREAK_DURATION,
    DEFAULT_LONG_BREAK_INTERVAL, DEFAULT_SHORT_BREAK_DURATION, DEFAULT_SHORT_BREAK_INTERVAL,
    LockConfig, MIN_SYNC_SHARED_SECRET_LENGTH, SharedSecret, SyncConfig,
};
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const VALID_SHARED_SECRET: &str = "0123456789abcdef0123456789abcdef";

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
fn default_config_uses_expected_lock_command() {
    let config = Config::default();

    assert_eq!(
        config.lock.command,
        DEFAULT_LOCK_COMMAND
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
}

#[test]
fn default_config_uses_expected_sync_settings() {
    let config = Config::default();

    assert!(!config.sync.enabled);
    assert!(config.sync.shared_secret.is_none());
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
fn rejects_empty_lock_command() {
    let mut config = Config::default();
    config.lock.command.clear();

    assert_eq!(config.validate(), Err(ConfigError::EmptyLockCommand));
}

#[test]
fn rejects_blank_lock_program() {
    let mut config = Config::default();
    config.lock.command[0] = String::from("   ");

    assert_eq!(config.validate(), Err(ConfigError::BlankLockProgram));
}

#[test]
fn rejects_enabled_sync_without_shared_secret() {
    let mut config = Config::default();
    config.sync.enabled = true;

    assert_eq!(config.validate(), Err(ConfigError::MissingSyncSharedSecret));
}

#[test]
fn rejects_blank_sync_shared_secret() {
    let mut config = Config::default();
    config.sync.shared_secret = Some(SharedSecret::new(String::from("   ")));

    assert_eq!(config.validate(), Err(ConfigError::BlankSyncSharedSecret));
}

#[test]
fn rejects_whitespace_padded_sync_shared_secret() {
    let mut config = Config::default();
    config.sync.shared_secret = Some(SharedSecret::new(format!(" {VALID_SHARED_SECRET}")));

    assert_eq!(
        config.validate(),
        Err(ConfigError::WhitespacePaddedSyncSharedSecret)
    );
}

#[test]
fn rejects_short_sync_shared_secret() {
    let mut config = Config::default();
    let short_secret = "a".repeat(MIN_SYNC_SHARED_SECRET_LENGTH - 1);
    config.sync.shared_secret = Some(SharedSecret::new(short_secret.clone()));

    assert_eq!(
        config.validate(),
        Err(ConfigError::ShortSyncSharedSecret {
            min_length: MIN_SYNC_SHARED_SECRET_LENGTH,
            actual_length: short_secret.chars().count()
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
    assert_eq!(config.lock, LockConfig::default());
    assert_eq!(config.sync, SyncConfig::default());
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
lock:
  command: ['test-locker', '--lock-now']
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
    assert_eq!(
        config.lock.command,
        vec![String::from("test-locker"), String::from("--lock-now")]
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
fn yaml_accepts_lock_command_override() -> Result<(), Box<dyn Error>> {
    let config = Config::from_yaml_str(
        r"
lock:
  command: ['xdg-screensaver', 'lock']
",
    )?;

    assert_eq!(
        config.lock.command,
        vec![String::from("xdg-screensaver"), String::from("lock")]
    );
    Ok(())
}

#[test]
fn yaml_accepts_sync_config() -> Result<(), Box<dyn Error>> {
    let config = Config::from_yaml_str(
        r"
sync:
  enabled: true
  shared_secret: '0123456789abcdef0123456789abcdef'
",
    )?;

    assert!(config.sync.enabled);
    assert_eq!(
        config.sync.shared_secret.as_ref().map(SharedSecret::as_str),
        Some(VALID_SHARED_SECRET)
    );
    Ok(())
}

#[test]
fn sync_shared_secret_debug_output_is_redacted() -> Result<(), Box<dyn Error>> {
    let config = Config::from_yaml_str(
        r"
sync:
  enabled: true
  shared_secret: '0123456789abcdef0123456789abcdef'
",
    )?;

    let debug_output = format!("{config:?}");

    assert!(debug_output.contains("<redacted>"));
    assert!(!debug_output.contains(VALID_SHARED_SECRET));
    Ok(())
}

#[test]
fn yaml_rejects_empty_lock_command() {
    let error = expected_config_error(Config::from_yaml_str(
        r"
lock:
  command: []
",
    ));

    assert!(matches!(
        error,
        ConfigLoadError::Invalid {
            error: ConfigError::EmptyLockCommand,
            ..
        }
    ));
}

#[test]
fn yaml_rejects_blank_lock_program() {
    let error = expected_config_error(Config::from_yaml_str(
        r#"
lock:
  command: ["   ", "lock"]
"#,
    ));

    assert!(matches!(
        error,
        ConfigLoadError::Invalid {
            error: ConfigError::BlankLockProgram,
            ..
        }
    ));
}

#[test]
fn yaml_rejects_enabled_sync_without_shared_secret() {
    let error = expected_config_error(Config::from_yaml_str(
        r"
sync:
  enabled: true
",
    ));

    assert!(matches!(
        error,
        ConfigLoadError::Invalid {
            error: ConfigError::MissingSyncSharedSecret,
            ..
        }
    ));
}

#[test]
fn yaml_rejects_blank_sync_shared_secret() {
    let error = expected_config_error(Config::from_yaml_str(
        r#"
sync:
  shared_secret: "   "
"#,
    ));

    assert!(matches!(
        error,
        ConfigLoadError::Invalid {
            error: ConfigError::BlankSyncSharedSecret,
            ..
        }
    ));
}

#[test]
fn yaml_rejects_whitespace_padded_sync_shared_secret() {
    let error = expected_config_error(Config::from_yaml_str(
        r#"
sync:
  shared_secret: " 0123456789abcdef0123456789abcdef"
"#,
    ));

    assert!(matches!(
        error,
        ConfigLoadError::Invalid {
            error: ConfigError::WhitespacePaddedSyncSharedSecret,
            ..
        }
    ));
}

#[test]
fn yaml_rejects_short_sync_shared_secret() {
    let error = expected_config_error(Config::from_yaml_str(
        r"
sync:
  shared_secret: short-secret
",
    ));

    assert!(matches!(
        error,
        ConfigLoadError::Invalid {
            error: ConfigError::ShortSyncSharedSecret {
                min_length: MIN_SYNC_SHARED_SECRET_LENGTH,
                actual_length: 12
            },
            ..
        }
    ));
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
fn yaml_rejects_unknown_sync_fields() {
    let error = expected_config_error(Config::from_yaml_str(
        r"
sync:
  peer_id: workstation
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
