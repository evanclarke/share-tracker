use clap::Parser;

#[derive(Parser)]
#[command(about = "Australian share portfolio tracker")]
pub struct Args {
    #[arg(long, default_value = "share-tracker.db")]
    pub db: String,
    /// IP address to bind. Defaults to `0.0.0.0` (all interfaces, reachable from
    /// other machines on the network); use `127.0.0.1` to restrict to localhost.
    #[arg(long, default_value = "0.0.0.0")]
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
    fn default_host_is_all_interfaces() {
        let args = Args::parse_from(["share-tracker"]);
        assert_eq!(args.host, "0.0.0.0");
        // The default must parse to a bindable address.
        assert!(args.host.parse::<std::net::IpAddr>().is_ok());
    }

    #[test]
    fn custom_host() {
        let args = Args::parse_from(["share-tracker", "--host", "127.0.0.1"]);
        assert_eq!(args.host, "127.0.0.1");
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
