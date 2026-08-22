//! Rendering an outbound-feed fetch failure so the recorded error says *why*.
//!
//! Every import that reaches a published feed (the RBA F11 rates, the ISO MIC
//! registry, the ISO 4217 / 24165 currency lists) stringifies its transport
//! failure into an `ImportError::Fetch(String)`, and that string is what the
//! operator eventually reads — in `job_runs.error`, the Jobs table's Error
//! column, the health banner, and the server log behind a `502`.
//!
//! A `reqwest::Error`'s own `Display` is only its outermost layer:
//!
//! ```text
//! error sending request for url (https://www.rba.gov.au/statistics/tables/csv/f11-data.csv)
//! ```
//!
//! which names the feed and says nothing about why it could not be reached.
//! The actual reason — `tcp connect error`, `Connection refused (os error 61)`,
//! a TLS or DNS failure — lives in the error's [`source`](std::error::Error::source)
//! chain, which `to_string()` never reaches (SCENARIOS T-06). So a fetch is
//! rendered through [`cause_chain`] instead, never `e.to_string()`.

/// Render an error and everything beneath it as one line, outermost first,
/// joined with `": "` — e.g.
///
/// ```text
/// error sending request for url (http://127.0.0.1:1/f11-data.csv): client error (Connect): tcp connect error: Connection refused (os error 61)
/// ```
///
/// A layer whose own message merely re-renders what it wraps (several wrapper
/// error types delegate `Display` straight to their source) is not repeated.
/// The walk is depth-bounded so a self-referencing `source()` cannot spin.
pub fn cause_chain(error: &dyn std::error::Error) -> String {
    /// Deeper than any transport error stack in practice; a guard, not a limit
    /// anything real is expected to reach.
    const MAX_DEPTH: usize = 12;

    let mut rendered = error.to_string();
    let mut source = error.source();
    for _ in 0..MAX_DEPTH {
        let Some(cause) = source else { break };
        let message = cause.to_string();
        if !rendered.ends_with(&message) {
            rendered.push_str(": ");
            rendered.push_str(&message);
        }
        source = cause.source();
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::fmt;

    /// A minimal error with an optional source, for building a chain by hand.
    #[derive(Debug)]
    struct Layer {
        message: String,
        source: Option<Box<Layer>>,
    }

    impl Layer {
        fn new(message: &str) -> Self {
            Self {
                message: message.to_string(),
                source: None,
            }
        }

        fn over(message: &str, source: Layer) -> Self {
            Self {
                message: message.to_string(),
                source: Some(Box::new(source)),
            }
        }
    }

    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl Error for Layer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source.as_ref().map(|s| s.as_ref() as &dyn Error)
        }
    }

    #[test]
    fn an_error_with_no_source_renders_as_its_own_message() {
        assert_eq!(
            cause_chain(&Layer::new("nothing beneath this")),
            "nothing beneath this"
        );
    }

    #[test]
    fn every_cause_beneath_the_error_is_rendered_outermost_first() {
        let chain = Layer::over(
            "error sending request for url (http://example.invalid/feed)",
            Layer::over(
                "client error (Connect)",
                Layer::over(
                    "tcp connect error",
                    Layer::new("Connection refused (os error 61)"),
                ),
            ),
        );
        assert_eq!(
            cause_chain(&chain),
            "error sending request for url (http://example.invalid/feed): client error (Connect): \
             tcp connect error: Connection refused (os error 61)"
        );
    }

    #[test]
    fn a_wrapper_that_only_re_renders_its_source_is_not_repeated() {
        // Several error types (yfinance-rs's own wrappers among them) delegate
        // Display straight through, which would otherwise double the message.
        let chain = Layer::over("boom", Layer::new("boom"));
        assert_eq!(cause_chain(&chain), "boom");
    }

    #[test]
    fn a_self_referencing_chain_terminates() {
        // Not constructible with `Layer` (it owns its source), so the bound is
        // exercised with a chain longer than MAX_DEPTH instead: rendering stops
        // rather than running away.
        let mut chain = Layer::new("l0");
        for i in 1..40 {
            chain = Layer::over(&format!("l{i}"), chain);
        }
        let rendered = cause_chain(&chain);
        assert!(rendered.starts_with("l39: l38: "), "{rendered}");
        assert_eq!(rendered.matches(": ").count(), 12, "{rendered}");
    }

    /// The reason this module exists, against the real error type: a refused
    /// connection's `Display` names only the URL, and the cause is a layer down.
    #[tokio::test]
    async fn a_reqwest_failure_renders_the_cause_its_own_display_hides() {
        let error = reqwest::get(crate::test_support::unreachable_url("f11-data.csv"))
            .await
            .expect_err("nothing is listening on that port");

        let top_level = error.to_string();
        let rendered = cause_chain(&error);
        assert!(
            !top_level.to_lowercase().contains("connect"),
            "reqwest's own Display started naming the cause: {top_level}"
        );
        assert!(
            rendered.starts_with(&top_level),
            "the chain must keep the outer message: {rendered}"
        );
        assert!(
            rendered.to_lowercase().contains("connect"),
            "the connect failure is not in the rendered chain: {rendered}"
        );
    }
}
