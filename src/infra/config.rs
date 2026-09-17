//! Server configuration: an optional TOML config file merged with CLI args.
//!
//! Precedence is CLI flag > config-file value > built-in default. The file is
//! loaded from `/usr/local/etc/share-tracker.toml` when present (where the
//! FreeBSD package installs it — see `pkg/freebsd/`); `--config PATH` overrides
//! the location and must then exist. Unknown keys and invalid TOML abort
//! startup: this is a financial-records server, and silently falling back to a
//! default database because of a typo is worse than not starting.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Where the config file is looked for when `--config` is not given.
pub const DEFAULT_CONFIG_PATH: &str = "/usr/local/etc/share-tracker.toml";
pub const DEFAULT_DB: &str = "share-tracker.db";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 3000;
/// The price-change threshold the alert email uses when `[email]` names none:
/// a 5% move in one close is unusual enough to be worth reading about and
/// common enough that the alert proves itself working.
pub const DEFAULT_PRICE_ALERT_PCT: &str = "5";

/// The config file's schema. Every field is optional — the file only states
/// what it wants to change. `deny_unknown_fields` makes a misspelt key an
/// error instead of a silently ignored line.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub db: Option<String>,
    pub backup_dir: Option<String>,
    pub backup_command: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub base_path: Option<String>,
    pub schedule: Option<String>,
    pub auth: Option<AuthConfig>,
    pub email: Option<EmailConfig>,
}

/// The optional `[email]` table: where the two scheduled report emails are
/// sent from and to, and how the SMTP connection is made (see
/// `infra::email`). Absent — the default — there is no mailer at all and both
/// jobs record a run that sent nothing.
///
/// Config-file only, for the same reason `[auth]` is: an SMTP password on the
/// command line is visible to anyone on the host via `ps`.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailConfig {
    pub smtp_host: String,
    /// Defaults to the conventional port for `encryption` — 465 implicit,
    /// 587 STARTTLS, 25 unencrypted.
    pub smtp_port: Option<u16>,
    /// `implicit` (the default), `starttls`, or `none`.
    pub encryption: Option<String>,
    /// Both or neither: a username with no password cannot authenticate, and
    /// resolution rejects the half-filled pair rather than connecting
    /// anonymously and failing weekly.
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    /// At least one recipient — an empty list is a configured mailer that can
    /// never deliver, which resolution rejects.
    pub to: Vec<String>,
    /// Prepended to every subject, e.g. `[share-tracker]`, so the mail can be
    /// filtered on one string.
    pub subject_prefix: Option<String>,
    /// The price-alert threshold in percent; defaults to
    /// [`DEFAULT_PRICE_ALERT_PCT`].
    ///
    /// Read as a **string or an integer**, never a TOML float: a float would
    /// cross `f64` on its way to `Decimal`, and `2.5` is exact there only by
    /// luck — `0.1` is not. The money rules forbid `f64` anywhere near a
    /// figure that is compared against a stored price, and this one is.
    #[serde(default, deserialize_with = "percent")]
    pub price_alert_pct: Option<Decimal>,
}

/// Read a percentage written as a TOML string (`"2.5"`) or integer (`5`),
/// rejecting a float with a message saying which forms are accepted — see
/// [`EmailConfig::price_alert_pct`] for why a float is not one of them.
fn percent<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Decimal>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Text(String),
    }
    let raw = Option::<Raw>::deserialize(deserializer).map_err(|_| {
        serde::de::Error::custom(
            "price_alert_pct must be written as a string (\"2.5\") or a whole number (5), never \
             as a decimal number — a TOML float cannot represent every threshold exactly",
        )
    })?;
    match raw {
        None => Ok(None),
        Some(Raw::Int(n)) => Ok(Some(Decimal::from(n))),
        Some(Raw::Text(text)) => text.trim().parse::<Decimal>().map(Some).map_err(|e| {
            serde::de::Error::custom(format!("invalid price_alert_pct {text:?}: {e}"))
        }),
    }
}

/// The optional `[auth]` table: a single shared credential gating the whole
/// HTTP surface (see `infra::auth`). Deliberately config-file only — there
/// are no `--auth-*` CLI flags, because a password hash or bearer token on
/// argv is visible to anyone on the host via `ps`; this also matches the
/// FreeBSD rc.d script, which only ever passes `--config`.
#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub username: String,
    /// An Argon2id PHC string — generate one with `share-tracker
    /// hash-password`, never a plain password.
    pub password_hash: String,
    /// Bearer token accepted alongside a session cookie, for the deployment
    /// scripts (`pkg/freebsd/update.sh`, `smoke-test.sh`) that call the HTTP
    /// API without a browser. Generate one with `share-tracker gen-token`.
    pub api_token: Option<String>,
    /// Whether the session cookie carries `Secure`. Defaults to `true` (see
    /// `infra::auth::Auth`); set `false` only for a deliberately plain-HTTP
    /// setup.
    pub secure_cookie: Option<bool>,
}

/// The fully resolved settings the server runs with.
#[derive(Debug, PartialEq)]
pub struct Settings {
    pub db: String,
    pub backup_dir: Option<String>,
    pub backup_command: Option<String>,
    pub host: String,
    pub port: u16,
    /// URL path prefix the whole application is mounted under, normalised to
    /// either `""` (the root — the default) or a leading-slash, no-trailing-slash
    /// path like `/share_tracker`. See [`normalise_base_path`].
    pub base_path: String,
    pub schedule: Option<String>,
    /// `None` (the default) serves the whole application exactly as before —
    /// see `infra::auth`. `Some` only when `[auth]` is present in the config
    /// file; there is no CLI flag for it.
    pub auth: Option<super::auth::Auth>,
    /// `None` (the default) means no email is sent: both scheduled email jobs
    /// stay registered and record a run that did nothing, rather than failing
    /// weekly on a deployment that never asked for mail. `Some` only when
    /// `[email]` is present in the config file; there are no CLI flags for it.
    pub email: Option<super::email::EmailSettings>,
}

impl Settings {
    /// Merge CLI args over config-file values over built-in defaults.
    ///
    /// Fallible only because of `base_path`: an unusable prefix is a typo in a
    /// deployment config, and serving the whole app at a subtly wrong path is
    /// worse than not starting (the same reasoning as `deny_unknown_fields`).
    pub fn resolve(args: super::args::Args, file: ConfigFile) -> Result<Settings, String> {
        let email = file.email.map(resolve_email).transpose()?;
        let auth = file
            .auth
            .map(|a| {
                super::auth::Auth::new(
                    a.username,
                    a.password_hash,
                    a.api_token,
                    a.secure_cookie.unwrap_or(true),
                )
            })
            .transpose()?;
        Ok(Settings {
            db: args.db.or(file.db).unwrap_or_else(|| DEFAULT_DB.into()),
            backup_dir: args.backup_dir.or(file.backup_dir),
            backup_command: args.backup_command.or(file.backup_command),
            host: args
                .host
                .or(file.host)
                .unwrap_or_else(|| DEFAULT_HOST.into()),
            port: args.port.or(file.port).unwrap_or(DEFAULT_PORT),
            base_path: normalise_base_path(args.base_path.or(file.base_path).as_deref())?,
            schedule: args.schedule.or(file.schedule),
            auth,
            email,
        })
    }
}

/// Validate the `[email]` table into the settings the mailer is built from.
///
/// Everything that can be wrong with it is caught here, at startup: an
/// unparseable address, an empty recipient list, a half-filled credential
/// pair, an unknown encryption mode, a non-positive threshold. A weekly job is
/// the worst place to discover any of them — the failure surfaces a week late,
/// to nobody, in a log.
fn resolve_email(config: EmailConfig) -> Result<super::email::EmailSettings, String> {
    use super::email::{EmailSettings, Encryption};

    let mailbox = |raw: &str, field: &str| {
        raw.parse::<lettre::message::Mailbox>()
            .map_err(|e| format!("invalid email {field} {raw:?}: {e}"))
    };
    let encryption = match &config.encryption {
        Some(raw) => Encryption::parse(raw)?,
        None => Encryption::Implicit,
    };
    let credentials = match (config.username, config.password) {
        (Some(username), Some(password)) => Some((username, password)),
        (None, None) => None,
        // Half a credential pair never authenticates. Rejecting is the same
        // call `deny_unknown_fields` makes: a config that cannot do what it
        // plainly means to do must not start.
        (Some(_), None) => {
            return Err("email username is set without a password".to_string());
        }
        (None, Some(_)) => {
            return Err("email password is set without a username".to_string());
        }
    };
    if config.to.is_empty() {
        return Err("email to is empty: name at least one recipient".to_string());
    }
    let to = config
        .to
        .iter()
        .map(|raw| mailbox(raw, "to"))
        .collect::<Result<Vec<_>, _>>()?;
    let price_alert_pct = config.price_alert_pct.unwrap_or_else(|| {
        DEFAULT_PRICE_ALERT_PCT
            .parse()
            .expect("the built-in default threshold parses")
    });
    if price_alert_pct <= Decimal::ZERO {
        return Err(format!(
            "invalid price_alert_pct {price_alert_pct}: the threshold must be greater than zero \
             (a zero or negative one would alert on every close)"
        ));
    }
    Ok(EmailSettings {
        smtp_host: config.smtp_host,
        smtp_port: config
            .smtp_port
            .unwrap_or_else(|| encryption.default_port()),
        encryption,
        credentials,
        from: mailbox(&config.from, "from")?,
        to,
        subject_prefix: config.subject_prefix,
        price_alert_pct,
    })
}

/// Normalise a configured reverse-proxy path prefix to the one form the rest of
/// the code assumes: `""` (mounted at the root) or `/a/b` — leading slash, no
/// trailing slash. Absent, empty and `"/"` all mean the root, so a config file
/// can spell "no prefix" any of the obvious ways.
///
/// Each segment is restricted to unreserved URL characters (RFC 3986
/// `A-Z a-z 0-9 - . _ ~`): a prefix is concatenated with API paths by the
/// frontend and matched by axum's router, so a space, `?`, `#` or `%` in it
/// would produce URLs that mean something other than intended. Rejecting is
/// deliberate — the alternative is a server that starts and then serves a UI
/// whose every request 404s.
pub fn normalise_base_path(raw: Option<&str>) -> Result<String, String> {
    let trimmed = raw.unwrap_or("").trim();
    let stripped = trimmed.trim_matches('/');
    if stripped.is_empty() {
        return Ok(String::new());
    }
    let valid = |seg: &str| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
    };
    if !stripped.split('/').all(valid) {
        return Err(format!(
            "invalid base_path {trimmed:?}: expected a URL path prefix like \"/share_tracker\" \
             (path segments of letters, digits, '-', '.', '_' or '~')"
        ));
    }
    Ok(format!("/{stripped}"))
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
    use std::os::unix::fs::PermissionsExt;

    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read config file {path}: {e}"))?;
    let warning = std::fs::metadata(path)
        .ok()
        .and_then(|metadata| mode_warning(path, metadata.permissions().mode()));
    if let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
    toml::from_str(&text).map_err(|e| format!("invalid config file {path}: {e}"))
}

/// The warning to log for a config file whose mode grants the group or other
/// class any access, `None` when it is owner-only.
///
/// The file holds every secret the deployment has: `[auth].password_hash` is
/// the direct input to the session-signing key (`infra::auth` derives it from
/// the PHC string), so any local user who can read the file can mint a valid
/// `st_session` cookie for any expiry without knowing the password, and
/// `[auth].api_token` is full read/write API access. The check is split out of
/// [`read`] so it is unit-testable without a file on disk; the packaging half
/// (installing the sample `0600` and tightening the live copy) lives in
/// `pkg/freebsd/` and is pinned by `doc_checks::freebsd_packaging`. The server
/// warns but never changes the mode itself — silently rewriting a file the
/// operator owns is worse than telling them.
fn mode_warning(path: &str, mode: u32) -> Option<String> {
    if mode & 0o077 == 0 {
        return None;
    }
    Some(format!(
        "config file {path} is readable or writable by users other than its owner (mode {:04o}); \
         it holds [auth] secrets — chmod 600 {path}",
        mode & 0o7777
    ))
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
            Settings::resolve(Args::parse_from(["share-tracker"]), ConfigFile::default())
                .expect("defaults resolve");
        assert_eq!(
            settings,
            Settings {
                db: "share-tracker.db".into(),
                backup_dir: None,
                backup_command: None,
                host: "127.0.0.1".into(),
                port: 3000,
                base_path: String::new(),
                schedule: None,
                auth: None,
                email: None,
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
            backup_command = "scp {BACKUP_FILE} host:/backups/"
            host = "0.0.0.0"
            port = 8080
            schedule = "/usr/local/etc/share-tracker.cron"
            "#,
        );
        let settings =
            Settings::resolve(Args::parse_from(["share-tracker"]), file).expect("resolves");
        assert_eq!(settings.db, "/var/db/share-tracker/share-tracker.db");
        assert_eq!(
            settings.backup_dir.as_deref(),
            Some("/var/db/share-tracker/backups")
        );
        assert_eq!(
            settings.backup_command.as_deref(),
            Some("scp {BACKUP_FILE} host:/backups/")
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
        let settings = Settings::resolve(args, file).expect("resolves");
        assert_eq!(settings.db, "cli.db");
        assert_eq!(settings.port, 9999);
        // A flag not given still takes the file's value.
        assert_eq!(settings.host, "0.0.0.0");
    }

    #[test]
    fn cli_backup_command_overrides_config_file() {
        let file = parse("backup_command = \"rsync {BACKUP_FILE} old-dest:/\"");
        let args = Args::parse_from([
            "share-tracker",
            "--backup-command",
            "rsync {BACKUP_FILE} new-dest:/",
        ]);
        let settings = Settings::resolve(args, file).expect("resolves");
        assert_eq!(
            settings.backup_command.as_deref(),
            Some("rsync {BACKUP_FILE} new-dest:/")
        );
    }

    #[test]
    fn partial_config_file_keeps_defaults_for_the_rest() {
        let settings = Settings::resolve(Args::parse_from(["share-tracker"]), parse("port = 8080"))
            .expect("resolves");
        assert_eq!(settings.port, 8080);
        assert_eq!(settings.db, "share-tracker.db");
        assert_eq!(settings.host, "127.0.0.1");
    }

    #[test]
    fn base_path_defaults_to_the_root() {
        // Absent, empty and "/" all mean "mounted at the root", so an operator
        // can spell "no prefix" any of the obvious ways without it becoming a
        // one-segment prefix.
        for raw in [None, Some(""), Some("  "), Some("/"), Some("//")] {
            assert_eq!(normalise_base_path(raw), Ok(String::new()), "{raw:?}");
        }
    }

    #[test]
    fn base_path_is_normalised_to_leading_slash_no_trailing_slash() {
        for raw in [
            "/share_tracker",
            "share_tracker",
            "/share_tracker/",
            "  /share_tracker/  ",
        ] {
            assert_eq!(
                normalise_base_path(Some(raw)),
                Ok("/share_tracker".to_string()),
                "{raw:?}"
            );
        }
        // Multi-segment prefixes are fine — a proxy may mount under /apps/x.
        assert_eq!(
            normalise_base_path(Some("apps/share-tracker/")),
            Ok("/apps/share-tracker".to_string())
        );
    }

    #[test]
    fn unusable_base_path_is_rejected_naming_the_value() {
        // Characters that would change what the concatenated URL means, or an
        // empty interior segment: better to refuse to start than to serve a UI
        // whose every request 404s.
        for raw in ["/share tracker", "/a?b", "/a#b", "/a%2fb", "/a//b"] {
            let err = normalise_base_path(Some(raw)).expect_err("an unusable prefix is rejected");
            assert!(err.contains("base_path"), "{raw:?}: {err}");
            assert!(
                err.contains(raw.trim()),
                "names the bad value — {raw:?}: {err}"
            );
        }
    }

    #[test]
    fn cli_base_path_overrides_config_file() {
        let file = parse("base_path = \"/from_file\"");
        let args = Args::parse_from(["share-tracker", "--base-path", "/from_cli/"]);
        let settings = Settings::resolve(args, file).expect("resolves");
        assert_eq!(settings.base_path, "/from_cli");
        // …and the file's value applies on its own, normalised the same way.
        let settings = Settings::resolve(
            Args::parse_from(["share-tracker"]),
            parse("base_path = \"share_tracker/\""),
        )
        .expect("resolves");
        assert_eq!(settings.base_path, "/share_tracker");
    }

    #[test]
    fn a_bad_base_path_fails_resolution() {
        let err = Settings::resolve(
            Args::parse_from(["share-tracker"]),
            parse("base_path = \"/share tracker\""),
        )
        .expect_err("rejects an unusable prefix");
        assert!(err.contains("share tracker"), "{err}");
    }

    #[test]
    fn auth_table_resolves_into_settings() {
        let hash = crate::infra::auth::Auth::hash_password("hunter2").unwrap();
        let file = parse(&format!(
            r#"
            [auth]
            username = "evan"
            password_hash = "{hash}"
            api_token = "abc123"
            secure_cookie = false
            "#,
        ));
        let settings =
            Settings::resolve(Args::parse_from(["share-tracker"]), file).expect("resolves");
        assert!(settings.auth.is_some());
    }

    #[test]
    fn no_auth_table_leaves_settings_auth_none() {
        let settings = Settings::resolve(Args::parse_from(["share-tracker"]), parse("port = 8080"))
            .expect("resolves");
        assert!(settings.auth.is_none());
    }

    #[test]
    fn an_unparseable_password_hash_fails_resolution_naming_the_field() {
        let file = parse(
            r#"
            [auth]
            username = "evan"
            password_hash = "not a phc string"
            "#,
        );
        let err = Settings::resolve(Args::parse_from(["share-tracker"]), file)
            .err()
            .unwrap();
        assert!(err.contains("password_hash"), "{err}");
    }

    #[test]
    fn auth_table_rejects_unknown_keys() {
        let err = toml::from_str::<ConfigFile>(
            r#"
            [auth]
            username = "evan"
            password_hash = "x"
            typo_field = "x"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("typo_field"), "{err}");
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
    fn a_world_accessible_config_warns_but_a_tight_one_does_not() {
        // Owner-only modes are the point: nothing is said.
        for mode in [0o600, 0o400, 0o700] {
            assert_eq!(
                mode_warning("/usr/local/etc/share-tracker.toml", mode),
                None,
                "{mode:04o} must not warn"
            );
        }
        // Any group or other access warns, naming the file, the offending mode
        // and the fix — the file holds the password hash the session-signing
        // key is derived from and the API token.
        for mode in [0o640, 0o644, 0o604, 0o660, 0o777, 0o407, 0o664] {
            let warning = mode_warning("/usr/local/etc/share-tracker.toml", mode)
                .unwrap_or_else(|| panic!("{mode:04o} must warn"));
            assert!(
                warning.contains("/usr/local/etc/share-tracker.toml"),
                "names the file: {warning}"
            );
            assert!(
                warning.contains(&format!("{mode:04o}")),
                "names the mode: {warning}"
            );
            assert!(warning.contains("chmod 600"), "says the fix: {warning}");
        }
        // Only the low twelve bits are the mode; the file-type bits above them
        // must not read as group/other access.
        assert_eq!(mode_warning("c.toml", 0o100_600), None);
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
        // backup_command has no universal safe default (the destination is
        // site-specific), so the sample documents it commented out rather than
        // active — assert it stays out of the parsed (active) settings, and
        // that the documented example line hasn't silently drifted from the
        // real key name.
        assert!(sample.backup_command.is_none());
        assert!(
            SAMPLE.contains("# backup_command = "),
            "sample should document backup_command as a commented-out example"
        );
        // base_path is likewise commented out: the server is mounted at the
        // root unless it is being proxied onto a sub-path, so the shipped file
        // documents the setting without activating a prefix nobody asked for.
        assert!(sample.base_path.is_none());
        assert!(
            SAMPLE.contains("# base_path = "),
            "sample should document base_path as a commented-out example"
        );
        // [auth] is commented out too: the server is open by default, and a
        // commented example (rather than a fabricated active hash) is the
        // only honest way to document the setting without shipping a
        // password nobody chose.
        assert!(sample.auth.is_none());
        assert!(
            SAMPLE.contains("# [auth]"),
            "sample should document the [auth] table as a commented-out example"
        );
        // …and [email] for the same reason: with no table the server sends
        // nothing, which is the right default, so the sample documents the
        // settings without turning mail on for a host that never asked.
        assert!(sample.email.is_none());
        assert!(
            SAMPLE.contains("# [email]"),
            "sample should document the [email] table as a commented-out example"
        );
        // Every [email] key appears in the commented example, so a renamed or
        // added setting breaks the build rather than the deployment. Read off
        // the struct's own field list would be ideal; short of that, this is
        // the list the resolver reads.
        for key in [
            "smtp_host",
            "smtp_port",
            "encryption",
            "username",
            "password",
            "from",
            "to",
            "subject_prefix",
            "price_alert_pct",
        ] {
            assert!(
                SAMPLE.contains(&format!("# {key} = ")),
                "sample should document the [email] setting {key}"
            );
        }
    }

    // ---- [email] -------------------------------------------------------

    fn email_settings(table: &str) -> Result<crate::infra::email::EmailSettings, String> {
        Settings::resolve(Args::parse_from(["share-tracker"]), parse(table))
            .map(|s| s.email.expect("an [email] table resolves"))
    }

    const MINIMAL_EMAIL: &str = r#"
        [email]
        smtp_host = "smtp.example.com"
        from = "share-tracker@example.com"
        to = ["you@example.com"]
    "#;

    #[test]
    fn a_minimal_email_table_takes_the_documented_defaults() {
        let email = email_settings(MINIMAL_EMAIL).expect("resolves");
        assert_eq!(email.smtp_host, "smtp.example.com");
        // Implicit TLS on submissions, and the 5% threshold — the two defaults
        // the sample and the README both state.
        assert_eq!(email.encryption, crate::infra::email::Encryption::Implicit);
        assert_eq!(email.smtp_port, 465);
        assert_eq!(email.price_alert_pct, Decimal::from(5));
        assert_eq!(email.credentials, None);
        assert_eq!(email.subject_prefix, None);
        assert_eq!(email.to.len(), 1);
    }

    #[test]
    fn the_port_follows_the_encryption_mode_unless_it_is_named() {
        for (mode, port) in [("implicit", 465), ("starttls", 587), ("none", 25)] {
            let email = email_settings(&format!(
                "{MINIMAL_EMAIL}
encryption = \"{mode}\"
"
            ))
            .expect("resolves");
            assert_eq!(email.smtp_port, port, "{mode}");
        }
        let email = email_settings(&format!(
            "{MINIMAL_EMAIL}
smtp_port = 2525
"
        ))
        .expect("resolves");
        assert_eq!(email.smtp_port, 2525);
    }

    #[test]
    fn an_unknown_encryption_mode_fails_startup_naming_it() {
        let err = email_settings(&format!(
            "{MINIMAL_EMAIL}
encryption = \"tsl\"
"
        ))
        .expect_err("a typo is rejected");
        assert!(err.contains("tsl"), "{err}");
        assert!(err.contains("starttls"), "lists the valid modes: {err}");
    }

    #[test]
    fn a_malformed_address_fails_startup_naming_the_field() {
        let err = email_settings(
            r#"
            [email]
            smtp_host = "smtp.example.com"
            from = "not an address"
            to = ["you@example.com"]
            "#,
        )
        .expect_err("an unparseable from is rejected");
        assert!(err.contains("from"), "{err}");
        assert!(err.contains("not an address"), "names the value: {err}");

        let err = email_settings(
            r#"
            [email]
            smtp_host = "smtp.example.com"
            from = "share-tracker@example.com"
            to = ["you@example.com", "@nope"]
            "#,
        )
        .expect_err("an unparseable recipient is rejected");
        assert!(err.contains("to"), "{err}");
    }

    #[test]
    fn an_empty_recipient_list_fails_startup() {
        // A configured mailer that can never deliver is a typo, not a
        // deployment that wants no email — that one leaves [email] out.
        let err = email_settings(
            r#"
            [email]
            smtp_host = "smtp.example.com"
            from = "share-tracker@example.com"
            to = []
            "#,
        )
        .expect_err("no recipient is rejected");
        assert!(err.contains("recipient"), "{err}");
    }

    #[test]
    fn half_a_credential_pair_fails_startup() {
        for (half, expected) in [
            ("username = \"me\"", "password"),
            ("password = \"hunter2\"", "username"),
        ] {
            let err = email_settings(&format!(
                "{MINIMAL_EMAIL}
{half}
"
            ))
            .expect_err("half a credential pair never authenticates");
            assert!(err.contains(expected), "{err}");
        }
        let email = email_settings(&format!(
            "{MINIMAL_EMAIL}
username = \"me\"
password = \"hunter2\"
"
        ))
        .expect("both halves resolve");
        assert_eq!(
            email.credentials,
            Some(("me".to_string(), "hunter2".to_string()))
        );
    }

    #[test]
    fn the_alert_threshold_reads_a_string_or_a_whole_number_but_never_a_float() {
        for (written, expected) in [("\"2.5\"", "2.5"), ("5", "5"), ("\"0.25\"", "0.25")] {
            let email = email_settings(&format!(
                "{MINIMAL_EMAIL}
price_alert_pct = {written}
"
            ))
            .expect("resolves");
            assert_eq!(
                email.price_alert_pct,
                expected.parse::<Decimal>().unwrap(),
                "{written}"
            );
        }
        // A TOML float would cross f64 on its way to Decimal, which the money
        // rules forbid anywhere near a figure compared against a stored price.
        let err = toml::from_str::<ConfigFile>(&format!(
            "{MINIMAL_EMAIL}
price_alert_pct = 2.5
"
        ))
        .expect_err("a float threshold is rejected")
        .to_string();
        assert!(err.contains("price_alert_pct"), "{err}");
        assert!(err.contains("string"), "says how to write it: {err}");
    }

    #[test]
    fn a_non_positive_threshold_fails_startup() {
        for written in ["\"0\"", "\"-5\""] {
            let err = email_settings(&format!(
                "{MINIMAL_EMAIL}
price_alert_pct = {written}
"
            ))
            .expect_err("a threshold that alerts on everything is rejected");
            assert!(err.contains("price_alert_pct"), "{written}: {err}");
        }
    }

    #[test]
    fn no_email_table_leaves_settings_email_none() {
        let settings = Settings::resolve(Args::parse_from(["share-tracker"]), parse("port = 8080"))
            .expect("resolves");
        assert!(settings.email.is_none());
    }

    #[test]
    fn the_email_table_rejects_unknown_keys() {
        let err = toml::from_str::<ConfigFile>(&format!(
            "{MINIMAL_EMAIL}
smpt_port = 465
"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("smpt_port"), "{err}");
    }
}
