use clap::Parser;

#[derive(Parser)]
#[command(about = "Australian share portfolio tracker")]
pub struct Args {
    #[arg(long, default_value = "share-tracker.db")]
    pub db: String,
    /// Directory the scheduled/triggered backups are written to. Defaults to
    /// beside the database file; set it to put backups on another volume so a
    /// disk failure can't take the database and its backups together.
    #[arg(long)]
    pub backup_dir: Option<String>,
    /// IP address to bind. Defaults to `127.0.0.1` (localhost only — the server
    /// has no authentication); use `0.0.0.0` to expose it to the network.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, default_value_t = 3000)]
    pub port: u16,
    /// Path to a cron schedule file overriding the built-in default (`schedule.cron`).
    #[arg(long)]
    pub schedule: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_db_path() {
        let args = Args::parse_from(["share-tracker"]);
        assert_eq!(args.db, "share-tracker.db");
    }

    #[test]
    fn custom_db_path() {
        let args = Args::parse_from(["share-tracker", "--db", "custom.db"]);
        assert_eq!(args.db, "custom.db");
    }

    #[test]
    fn default_host_is_localhost_only() {
        // The server has no authentication, so the safe default is to listen
        // on the loopback interface only; exposing it is the explicit opt-in.
        let args = Args::parse_from(["share-tracker"]);
        assert_eq!(args.host, "127.0.0.1");
        // The default must parse to a bindable address.
        assert!(args.host.parse::<std::net::IpAddr>().is_ok());
    }

    #[test]
    fn custom_host() {
        let args = Args::parse_from(["share-tracker", "--host", "0.0.0.0"]);
        assert_eq!(args.host, "0.0.0.0");
    }

    #[test]
    fn default_backup_dir_is_none() {
        let args = Args::parse_from(["share-tracker"]);
        assert_eq!(args.backup_dir, None);
    }

    #[test]
    fn custom_backup_dir() {
        let args = Args::parse_from(["share-tracker", "--backup-dir", "/mnt/backups"]);
        assert_eq!(args.backup_dir.as_deref(), Some("/mnt/backups"));
    }

    #[test]
    fn default_schedule_is_none() {
        let args = Args::parse_from(["share-tracker"]);
        assert_eq!(args.schedule, None);
    }

    #[test]
    fn custom_schedule_path() {
        let args = Args::parse_from(["share-tracker", "--schedule", "my.cron"]);
        assert_eq!(args.schedule.as_deref(), Some("my.cron"));
    }
}
