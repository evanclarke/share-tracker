//! Server configuration: an optional TOML config file merged with CLI args.
//!
//! Precedence is CLI flag > config-file value > built-in default. The file is
//! loaded from `/usr/local/etc/share-tracker.toml` when present (where the
//! FreeBSD package installs it — see `pkg/freebsd/`); `--config PATH` overrides
//! the location and must then exist. Unknown keys and invalid TOML abort
//! startup: this is a financial-records server, and silently falling back to a
//! default database because of a typo is worse than not starting.

use serde::Deserialize;

/// Where the config file is looked for when `--config` is not given.
pub const DEFAULT_CONFIG_PATH: &str = "/usr/local/etc/share-tracker.toml";
pub const DEFAULT_DB: &str = "share-tracker.db";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 3000;

/// The config file's schema. Every field is optional — the file only states
/// what it wants to change. `deny_unknown_fields` makes a misspelt key an
/// error instead of a silently ignored line.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub db: Option<String>,
    pub backup_dir: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub schedule: Option<String>,
}

/// The fully resolved settings the server runs with.
#[derive(Debug, PartialEq)]
pub struct Settings {
    pub db: String,
    pub backup_dir: Option<String>,
    pub host: String,
    pub port: u16,
    pub schedule: Option<String>,
}

impl Settings {
    /// Merge CLI args over config-file values over built-in defaults.
    pub fn resolve(args: super::args::Args, file: ConfigFile) -> Settings {
        Settings {
            db: args.db.or(file.db).unwrap_or_else(|| DEFAULT_DB.into()),
            backup_dir: args.backup_dir.or(file.backup_dir),
            host: args
                .host
                .or(file.host)
                .unwrap_or_else(|| DEFAULT_HOST.into()),
            port: args.port.or(file.port).unwrap_or(DEFAULT_PORT),
            schedule: args.schedule.or(file.schedule),
        }
    }
}

/// Load the config file: an explicit `--config` path must exist; the default
/// path is optional (absent file = empty config, today's flag-only behaviour).
pub fn load(explicit: Option<&str>) -> Result<ConfigFile, String> {
    match explicit {
        Some(path) => read(path),
        None if std::path::Path::new(DEFAULT_CONFIG_PATH).exists() => read(DEFAULT_CONFIG_PATH),
        None => Ok(ConfigFile::default()),
    }
}

fn read(path: &str) -> Result<ConfigFile, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config file {path}: {e}"))?;
    toml::from_str(&text).map_err(|e| format!("invalid config file {path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::args::Args;
    use clap::Parser;

    /// The sample config the FreeBSD package installs. Parsing it here means a
    /// renamed or removed setting breaks the build instead of the deployment.
    const SAMPLE: &str = include_str!("../../pkg/freebsd/share-tracker.toml.sample");

    fn parse(toml: &str) -> ConfigFile {
        toml::from_str(toml).expect("valid config")
    }

    #[test]
    fn defaults_when_no_flags_and_no_file() {
        let settings =
            Settings::resolve(Args::parse_from(["share-tracker"]), ConfigFile::default());
        assert_eq!(
            settings,
            Settings {
                db: "share-tracker.db".into(),
                backup_dir: None,
                host: "127.0.0.1".into(),
                port: 3000,
                schedule: None,
            }
        );
        // The default host must parse to a bindable address (the server has no
        // authentication, so the safe default is loopback only).
        assert!(settings.host.parse::<std::net::IpAddr>().is_ok());
    }

    #[test]
    fn config_file_values_apply_when_no_flags() {
        let file = parse(
            r#"
            db = "/var/db/share-tracker/share-tracker.db"
            backup_dir = "/var/db/share-tracker/backups"
            host = "0.0.0.0"
            port = 8080
            schedule = "/usr/local/etc/share-tracker.cron"
            "#,
        );
        let settings = Settings::resolve(Args::parse_from(["share-tracker"]), file);
        assert_eq!(settings.db, "/var/db/share-tracker/share-tracker.db");
        assert_eq!(
            settings.backup_dir.as_deref(),
            Some("/var/db/share-tracker/backups")
        );
        assert_eq!(settings.host, "0.0.0.0");
        assert_eq!(settings.port, 8080);
        assert_eq!(
            settings.schedule.as_deref(),
            Some("/usr/local/etc/share-tracker.cron")
        );
    }

    #[test]
    fn cli_flags_override_config_file() {
        let file = parse("db = \"file.db\"\nport = 8080\nhost = \"0.0.0.0\"");
        let args = Args::parse_from(["share-tracker", "--db", "cli.db", "--port", "9999"]);
        let settings = Settings::resolve(args, file);
        assert_eq!(settings.db, "cli.db");
        assert_eq!(settings.port, 9999);
        // A flag not given still takes the file's value.
        assert_eq!(settings.host, "0.0.0.0");
    }

    #[test]
    fn partial_config_file_keeps_defaults_for_the_rest() {
        let settings = Settings::resolve(Args::parse_from(["share-tracker"]), parse("port = 8080"));
        assert_eq!(settings.port, 8080);
        assert_eq!(settings.db, "share-tracker.db");
        assert_eq!(settings.host, "127.0.0.1");
    }

    #[test]
    fn unknown_key_is_rejected() {
        // A typo must fail loudly, never silently fall back to a default.
        let err = toml::from_str::<ConfigFile>("prot = 8080").unwrap_err();
        assert!(err.to_string().contains("prot"), "names the bad key: {err}");
    }

    #[test]
    fn invalid_toml_is_rejected() {
        assert!(toml::from_str::<ConfigFile>("port = ").is_err());
    }

    #[test]
    fn explicit_config_path_must_exist() {
        let err = load(Some("/nonexistent/share-tracker.toml")).unwrap_err();
        assert!(err.contains("/nonexistent/share-tracker.toml"));
    }

    #[test]
    fn load_reads_an_explicit_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "port = 4444\n").expect("write");
        let file = load(Some(path.to_str().expect("utf-8 path"))).expect("loads");
        assert_eq!(file.port, Some(4444));
    }

    #[test]
    fn load_rejects_bad_file_with_path_in_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not toml at all [").expect("write");
        let err = load(Some(path.to_str().expect("utf-8 path"))).unwrap_err();
        assert!(err.contains("config.toml"), "names the file: {err}");
    }

    #[test]
    fn shipped_sample_config_parses_and_exercises_every_setting() {
        let sample: ConfigFile = toml::from_str(SAMPLE).expect("sample config parses");
        // Every setting appears in the sample (commented-out ones don't count),
        // so the shipped file documents the full schema and drifts loudly.
        assert!(sample.db.is_some());
        assert!(sample.backup_dir.is_some());
        assert!(sample.host.is_some());
        assert!(sample.port.is_some());
        assert!(sample.schedule.is_some());
        // The sample points the service at the package's data directory.
        assert_eq!(sample.db.unwrap(), "/var/db/share-tracker/share-tracker.db");
    }
}
