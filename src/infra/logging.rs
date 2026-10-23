use std::io::IsTerminal;
use std::{env, io};
use tracing_subscriber::EnvFilter;

/// The filter for a `RUST_LOG` value: its directives, or `info` when it is
/// unset or unparseable. Taking the value rather than reading the environment
/// is what lets the default be tested without mutating the process
/// environment, which is unsound while the multithreaded test harness runs.
fn build_filter(rust_log: Option<&str>) -> EnvFilter {
    rust_log
        .and_then(|directives| EnvFilter::try_new(directives).ok())
        .unwrap_or_else(|| EnvFilter::new("info"))
}

pub fn init() {
    tracing_subscriber::fmt()
        .with_env_filter(build_filter(
            env::var(EnvFilter::DEFAULT_ENV).ok().as_deref(),
        ))
        // Color only when stdout is a terminal. Under daemon(8) stdout is a
        // pipe into the log file (-o), and tracing's default-on ANSI would
        // litter it with escape codes.
        .with_ansi(io::stdout().is_terminal())
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use tracing_subscriber::filter::LevelFilter;

    #[tracing_test::traced_test]
    #[test]
    fn info_messages_are_emitted() {
        tracing::info!("test info output");
        assert!(logs_contain("test info output"));
    }

    #[test]
    fn default_level_is_info_when_rust_log_unset() {
        assert_eq!(build_filter(None).max_level_hint(), Some(LevelFilter::INFO));
    }

    #[test]
    fn rust_log_controls_level() {
        assert_eq!(
            build_filter(Some("debug")).max_level_hint(),
            Some(LevelFilter::DEBUG)
        );
        assert_eq!(
            build_filter(Some("warn")).max_level_hint(),
            Some(LevelFilter::WARN)
        );
    }

    #[test]
    fn an_unparseable_rust_log_falls_back_to_info() {
        assert_eq!(
            build_filter(Some("not=a=directive")).max_level_hint(),
            Some(LevelFilter::INFO)
        );
    }

    /// `unsafe` is confined to `test_support`'s busy-handler FFI, whose blocks
    /// each carry a `SAFETY:` comment. A test that mutated the environment
    /// (`std::env::remove_var`) used to be the other one: edition 2024 made
    /// that `unsafe` because it is unsound while other threads run, and
    /// `cargo test` always has other threads, so no honest `SAFETY:` comment
    /// could be written for it.
    #[test]
    fn unsafe_code_is_confined_to_test_support() {
        let offenders: Vec<&str> = test_support::rust_sources()
            .iter()
            .filter(|source| source.path != "test_support.rs")
            .filter(|source| {
                source
                    .code
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .any(|word| word == "unsafe")
            })
            .map(|source| source.path.as_str())
            .collect();
        assert!(
            offenders.is_empty(),
            "`unsafe` outside test_support.rs: {offenders:?}"
        );
    }
}
