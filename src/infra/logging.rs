use std::io::IsTerminal;
use tracing_subscriber::EnvFilter;

fn build_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

pub fn init() {
    tracing_subscriber::fmt()
        .with_env_filter(build_filter())
        // Color only when stdout is a terminal. Under daemon(8) stdout is a
        // pipe into the log file (-o), and tracing's default-on ANSI would
        // litter it with escape codes.
        .with_ansi(std::io::stdout().is_terminal())
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::filter::LevelFilter;

    #[tracing_test::traced_test]
    #[test]
    fn info_messages_are_emitted() {
        tracing::info!("test info output");
        assert!(logs_contain("test info output"));
    }

    #[test]
    fn default_level_is_info_when_rust_log_unset() {
        unsafe { std::env::remove_var("RUST_LOG") };
        let filter = build_filter();
        assert_eq!(filter.max_level_hint(), Some(LevelFilter::INFO));
    }

    #[test]
    fn rust_log_env_var_controls_level() {
        // Verify the filter respects the same directive format used by RUST_LOG
        let debug_filter: EnvFilter = "debug".parse().unwrap();
        assert_eq!(debug_filter.max_level_hint(), Some(LevelFilter::DEBUG));

        let warn_filter: EnvFilter = "warn".parse().unwrap();
        assert_eq!(warn_filter.max_level_hint(), Some(LevelFilter::WARN));
    }
}
