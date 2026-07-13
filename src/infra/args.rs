use clap::Parser;

/// CLI flags. Every setting is optional here: defaults are applied by
/// `config::Settings::resolve` after merging with the config file (precedence
/// CLI > config file > built-in default), so a flag left off the command line
/// must be distinguishable from one explicitly set to the default value.
#[derive(Parser)]
#[command(about = "Australian share portfolio tracker", version)]
pub struct Args {
    /// Path to a TOML config file. Defaults to `/usr/local/etc/share-tracker.toml`
    /// when that exists; an explicitly given path must exist.
    #[arg(long)]
    pub config: Option<String>,
    /// Database file path [default: share-tracker.db]
    #[arg(long)]
    pub db: Option<String>,
    /// Directory the scheduled/triggered backups are written to. Defaults to
    /// beside the database file; set it to put backups on another volume so a
    /// disk failure can't take the database and its backups together.
    #[arg(long)]
    pub backup_dir: Option<String>,
    /// IP address to bind [default: 127.0.0.1 — localhost only, as the server
    /// has no authentication; use `0.0.0.0` to expose it to the network]
    #[arg(long)]
    pub host: Option<String>,
    /// Port to listen on [default: 3000]
    #[arg(long)]
    pub port: Option<u16>,
    /// Path to a cron schedule file overriding the built-in default (`schedule.cron`).
    #[arg(long)]
    pub schedule: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Default values (db path, host, port) are applied by
    // `config::Settings::resolve`, tested in `infra/config.rs`.

    #[test]
    fn no_flags_parse_to_none() {
        let args = Args::parse_from(["share-tracker"]);
        assert_eq!(args.config, None);
        assert_eq!(args.db, None);
        assert_eq!(args.backup_dir, None);
        assert_eq!(args.host, None);
        assert_eq!(args.port, None);
        assert_eq!(args.schedule, None);
    }

    #[test]
    fn custom_db_path() {
        let args = Args::parse_from(["share-tracker", "--db", "custom.db"]);
        assert_eq!(args.db.as_deref(), Some("custom.db"));
    }

    #[test]
    fn custom_host() {
        let args = Args::parse_from(["share-tracker", "--host", "0.0.0.0"]);
        assert_eq!(args.host.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn custom_backup_dir() {
        let args = Args::parse_from(["share-tracker", "--backup-dir", "/mnt/backups"]);
        assert_eq!(args.backup_dir.as_deref(), Some("/mnt/backups"));
    }

    #[test]
    fn custom_schedule_path() {
        let args = Args::parse_from(["share-tracker", "--schedule", "my.cron"]);
        assert_eq!(args.schedule.as_deref(), Some("my.cron"));
    }

    #[test]
    fn custom_config_path() {
        let args = Args::parse_from(["share-tracker", "--config", "/etc/st.toml"]);
        assert_eq!(args.config.as_deref(), Some("/etc/st.toml"));
    }

    /// `--version` reports the Cargo.toml version — the single source of truth
    /// the release workflow also reads, so the installed binary, the pkg
    /// version, and the release tag can never disagree.
    #[test]
    fn version_flag_reports_cargo_package_version() {
        use clap::CommandFactory;
        assert_eq!(
            Args::command().get_version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }
}
