use clap::Parser;

#[derive(Parser)]
#[command(about = "Australian share portfolio tracker")]
pub struct Args {
    #[arg(long, default_value = "share-tracker.db")]
    pub db: String,
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
}
