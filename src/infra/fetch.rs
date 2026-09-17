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
//!
//! The same module owns the other half of a fetch's safety: every import goes
//! through [`fetch_feed`], which bounds the whole call with [`FEED_TIMEOUT`]
//! and the body with [`MAX_FEED_BYTES`]. Before that, each of the three feeds
//! built its own client with no timeout and read `resp.text()` unbounded, so a
//! stalled or very large response parked the request task and buffered the
//! whole body. The seam ([`FeedFetcher`]) exists so the stalled and oversized
//! paths are testable without a socket.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

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

/// How long one outbound reference-data fetch may take, in total.
///
/// The feeds are small static documents served by their publisher, so a fetch
/// that has not finished in 30 seconds is not going to: it is a stalled
/// connection, and holding the request task — and, for a scheduled run, the
/// per-job lock every other trigger waits behind — only parks it. The bound is
/// applied twice, to the same figure:
///
/// * `reqwest::Client::builder().timeout(FEED_TIMEOUT)` in [`LiveFeedFetcher`]
///   — the transport bound, covering connect, TLS, headers and the whole body
///   read, so a server that stalls mid-body is cut off too;
/// * `tokio::time::timeout(FEED_TIMEOUT, …)` in [`fetch_feed`] — the same
///   bound over the whole [`FeedFetcher`] call, so it holds for *any*
///   implementation of the seam. That second one is what makes the stalled
///   path testable without a real socket: a test double that simply never
///   answers is cut off by the deadline rather than parking the test.
pub const FEED_TIMEOUT: Duration = Duration::from_secs(30);

/// The hard ceiling on one feed body.
///
/// The largest of the feeds (the DTIF digital-token registry JSON) is a few
/// MiB; 16 MiB leaves generous headroom for a registry that grows while still
/// bounding what one request can buffer. `resp.text()` buffered whatever the
/// far end sent, however large — a timeout alone would still let a fast,
/// talkative server park the task on memory.
pub const MAX_FEED_BYTES: usize = 16 * 1024 * 1024;

/// Why one feed fetch failed, in the terms an operator reads.
///
/// `Transport` carries the [`cause_chain`] rendering rather than the
/// `reqwest::Error` itself, so the reason under the outermost message survives
/// into `ImportError::Fetch` and on into `job_runs.error` (SCENARIOS T-06).
#[derive(Debug)]
pub enum FetchError {
    /// The request itself failed: DNS, connect, TLS, a non-2xx status, or a
    /// truncated body. Already rendered through [`cause_chain`].
    Transport(String),
    /// The whole fetch did not finish inside [`FEED_TIMEOUT`].
    Timeout { timeout: Duration },
    /// The body was longer than [`MAX_FEED_BYTES`].
    TooLarge { limit: usize },
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(rendered) => f.write_str(rendered),
            Self::Timeout { timeout } => {
                write!(f, "the feed did not answer within {}s", timeout.as_secs())
            }
            Self::TooLarge { limit } => {
                write!(f, "the feed body exceeded the {limit} byte limit")
            }
        }
    }
}

impl std::error::Error for FetchError {}

/// The result of one [`FeedFetcher::fetch`], boxed so the trait stays
/// object-safe (`dyn FeedFetcher`) exactly as `closing_price`'s `PriceFetcher`
/// is: the import handlers hold an `Arc<dyn FeedFetcher>` in an axum
/// `Extension`.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<String, FetchError>> + Send + 'a>>;

/// The outbound-feed seam: one GET, the whole body back.
///
/// The live implementation is [`LiveFeedFetcher`]; the tests inject a stub so
/// the stalled and oversized paths are exercised with no socket in play. Every
/// import that reaches a published feed goes through [`fetch_feed`], so the
/// timeout and the size cap cannot be bypassed by adding a fourth feed.
pub trait FeedFetcher: Send + Sync {
    /// GET `url`, optionally with HTTP Basic auth (the DTIF registry download
    /// requires credentials). The body comes back whole or not at all.
    fn fetch<'a>(&'a self, url: &'a str, basic_auth: Option<(&'a str, &'a str)>)
    -> FetchFuture<'a>;
}

/// The fetcher the entity routers install as an `Extension` and the import
/// paths use outside a test — see [`LiveFeedFetcher`].
pub type SharedFeedFetcher = Arc<dyn FeedFetcher>;

/// The live transport: a `reqwest` client built with [`FEED_TIMEOUT`], and a
/// body read that refuses to buffer more than [`MAX_FEED_BYTES`].
pub struct LiveFeedFetcher;

impl FeedFetcher for LiveFeedFetcher {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        basic_auth: Option<(&'a str, &'a str)>,
    ) -> FetchFuture<'a> {
        Box::pin(async move {
            let client = reqwest::Client::builder()
                .timeout(FEED_TIMEOUT)
                .build()
                .map_err(|e| FetchError::Transport(cause_chain(&e)))?;
            let mut request = client.get(url);
            if let Some((user, pass)) = basic_auth {
                request = request.basic_auth(user, Some(pass));
            }
            let mut response = request
                .send()
                .await
                .map_err(|e| FetchError::Transport(cause_chain(&e)))?
                .error_for_status()
                .map_err(|e| FetchError::Transport(cause_chain(&e)))?;

            // A declared length over the cap is refused before a body byte is
            // buffered. A chunked response declares none, which is what the
            // accumulating read below catches.
            if response
                .content_length()
                .is_some_and(|len| len > MAX_FEED_BYTES as u64)
            {
                return Err(FetchError::TooLarge {
                    limit: MAX_FEED_BYTES,
                });
            }

            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| FetchError::Transport(cause_chain(&e)))?
            {
                append_bounded(&mut body, &chunk, MAX_FEED_BYTES)?;
            }
            // `resp.text()` decoded with the declared charset, lossily; the
            // explicit UTF-8 decode keeps that tolerance. The cap, not the
            // decode, is the behaviour that changed.
            Ok(String::from_utf8_lossy(&body).into_owned())
        })
    }
}

/// Fetch one feed through `fetcher`, bounded by [`FEED_TIMEOUT`] and
/// [`MAX_FEED_BYTES`] — the single entry point every import uses.
pub async fn fetch_feed(
    fetcher: &dyn FeedFetcher,
    url: &str,
    basic_auth: Option<(&str, &str)>,
) -> Result<String, FetchError> {
    let body = tokio::time::timeout(FEED_TIMEOUT, fetcher.fetch(url, basic_auth))
        .await
        .map_err(|_| FetchError::Timeout {
            timeout: FEED_TIMEOUT,
        })??;
    // `LiveFeedFetcher` already enforced the cap as it read, so for the live
    // path this is belt-and-braces — but the seam's contract (a bounded body)
    // has to hold for every implementation, and an over-length body must not
    // reach a parser merely because a fetcher handed it over whole.
    if body.len() > MAX_FEED_BYTES {
        return Err(FetchError::TooLarge {
            limit: MAX_FEED_BYTES,
        });
    }
    Ok(body)
}

/// Append one body chunk, refusing to grow the body past `limit` bytes.
///
/// The check is against the total the append would produce, *before* any of
/// the chunk is written, so a body that crosses the line is refused rather
/// than buffered and then rejected. Split out from the response loop as a pure
/// function so the cap is unit-tested without a server.
fn append_bounded(body: &mut Vec<u8>, chunk: &[u8], limit: usize) -> Result<(), FetchError> {
    if body.len().saturating_add(chunk.len()) > limit {
        return Err(FetchError::TooLarge { limit });
    }
    body.extend_from_slice(chunk);
    Ok(())
}

/// Test doubles for the [`FeedFetcher`] seam, so the stalled and oversized
/// paths are exercised with no socket in play.
#[cfg(test)]
pub mod test_support {
    use super::*;

    /// A feed that never answers inside [`FEED_TIMEOUT`]: the shared deadline
    /// replies, not the stub. Sleeping four times the deadline (rather than
    /// forever) is what lets the regression check fail rather than hang — with
    /// the deadline removed the stub eventually returns an empty body and the
    /// import answers the parser's error instead of a `502`. In a
    /// `tokio::time::pause()`d test the wait costs no real time.
    pub struct StalledFeedFetcher;

    impl StalledFeedFetcher {
        pub fn shared() -> SharedFeedFetcher {
            Arc::new(Self)
        }
    }

    impl FeedFetcher for StalledFeedFetcher {
        fn fetch<'a>(
            &'a self,
            _url: &'a str,
            _basic_auth: Option<(&'a str, &'a str)>,
        ) -> FetchFuture<'a> {
            Box::pin(async move {
                tokio::time::sleep(FEED_TIMEOUT * 4).await;
                Ok(String::new())
            })
        }
    }

    /// A feed whose body is one byte over [`MAX_FEED_BYTES`].
    pub struct OversizedFeedFetcher;

    impl OversizedFeedFetcher {
        pub fn shared() -> SharedFeedFetcher {
            Arc::new(Self)
        }
    }

    impl FeedFetcher for OversizedFeedFetcher {
        fn fetch<'a>(
            &'a self,
            _url: &'a str,
            _basic_auth: Option<(&'a str, &'a str)>,
        ) -> FetchFuture<'a> {
            Box::pin(async move { Ok("x".repeat(MAX_FEED_BYTES + 1)) })
        }
    }
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

    /// The size cap is a rule about the *total*, checked before a chunk is
    /// written: a body that would cross the line is refused rather than
    /// buffered and then rejected. Pure, so the cap is pinned with no server.
    #[test]
    fn the_body_cap_refuses_the_chunk_that_would_cross_it() {
        let mut body = Vec::new();
        append_bounded(&mut body, b"1234", 8).expect("4 of the 8 bytes fit");
        append_bounded(&mut body, b"5678", 8).expect("exactly 8 is still inside the cap");
        assert_eq!(body, b"12345678");

        let error = append_bounded(&mut body, b"9", 8).expect_err("one byte past the cap");
        assert!(
            matches!(error, FetchError::TooLarge { limit: 8 }),
            "{error}"
        );
        assert_eq!(body, b"12345678", "the refused chunk was not buffered");

        // A single chunk larger than the whole budget is refused too, not just
        // a total built up chunk by chunk.
        let mut body = Vec::new();
        assert!(append_bounded(&mut body, b"123456789", 8).is_err());
        assert!(
            body.is_empty(),
            "nothing of an oversized first chunk landed"
        );
    }

    /// The seam's own guard: a fetcher that hands back an over-length body
    /// (the live one refuses it mid-read) is still refused here, before the
    /// body can reach a parser.
    #[tokio::test]
    async fn an_over_length_body_is_refused_at_the_seam() {
        let error = fetch_feed(
            &test_support::OversizedFeedFetcher,
            "https://example.invalid/feed",
            None,
        )
        .await
        .expect_err("a body over the cap must not be returned");

        assert!(
            matches!(error, FetchError::TooLarge { limit } if limit == MAX_FEED_BYTES),
            "{error}"
        );
    }

    /// The deadline really cuts a stalled fetch off — the stub sleeps four
    /// times [`FEED_TIMEOUT`]. The paused clock makes that cost no real time
    /// and, more importantly, lets the *absence* of the deadline show up as a
    /// completed fetch rather than a hung test.
    #[tokio::test]
    async fn a_stalled_fetch_is_cut_off_by_the_deadline() {
        tokio::time::pause();

        let error = fetch_feed(
            &test_support::StalledFeedFetcher,
            "https://example.invalid/feed",
            None,
        )
        .await
        .expect_err("the deadline replies before the stub does");

        assert!(
            matches!(error, FetchError::Timeout { timeout } if timeout == FEED_TIMEOUT),
            "{error}"
        );
    }
}
