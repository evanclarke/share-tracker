//! Optional shared-credential access control: a single username + Argon2
//! password hash gates the whole HTTP surface when configured. This is
//! access control only, not multi-tenancy — the app stays single-taxpayer
//! (see `docs/SCHEMA.md`); there is one credential, not a users table.
//!
//! Disabled by default: [`config::Settings::auth`] is `None` unless `[auth]`
//! is present in the config file, and the server then serves the whole
//! surface exactly as before — so none of the ~1200 existing tests or the
//! deployment scripts need a credential. When configured, [`require_auth`]
//! (applied in `app::router`) gates every route except a small
//! unauthenticated allowlist (`GET`/`POST /login`, `GET /static/style.css` —
//! the login page must render and be usable before any credential is
//! presented) with either:
//! - a signed session cookie, minted by `POST /login` and read on every
//!   request; or
//! - `Authorization: Bearer <api_token>`, for the deployment scripts
//!   (`pkg/freebsd/update.sh`, `smoke-test.sh`) that call the HTTP API
//!   without a browser.
//!
//! **The cookie is self-contained: no session table, no migration.** Its
//! value is `<hex(username|expiry)>.<hex HMAC-SHA256>`, signed with a key
//! derived from the configured password hash
//! (`HMAC-SHA256(key = password_hash, msg = SESSION_CONTEXT)`). Two
//! consequences of that, both deliberate: sessions survive a server restart
//! (no random per-boot key to lose), and **changing the password invalidates
//! every existing session** (the derived key changes with it). A third,
//! less convenient one: **`POST /logout` cannot truly revoke a cookie**,
//! only tell the browser to stop sending it (`Max-Age=0`) — with no
//! server-side session store, there is nothing to mark the token invalid in,
//! so a copied-out cookie value stays cryptographically valid until its own
//! 30-day expiry even after "logging out". Accepted for a single-credential
//! hobbyist app (see `docs/API.md`'s Known limitations); a real revocation
//! list is the escalation if that ever stops being fine.
//!
//! **CSRF needs no token.** `SameSite=Lax` withholds the cookie from
//! cross-site requests, and every state-changing route in this app is
//! `POST`/`PUT`/`DELETE`; `GET` routes are read-only. The residual gap —
//! login-CSRF on `POST /login` itself — is recorded in `docs/API.md`'s Known
//! limitations as accepted for a single-credential app rather than papered
//! over with a token nothing else in the app needs.
//!
//! Argon2 throttles each individual guess (~30 ms/attempt on ordinary
//! hardware, i.e. tens of attempts/sec/core), but that is a per-attempt cost,
//! not a bound on how many guesses a source gets — and since the API became a
//! script/LLM surface as well as a browser one, an unattended client can spend
//! as long as it likes spending it. So failed logins are **also** bounded by a
//! per-source lockout ([`LockoutTracker`]), which reopens the earlier "no
//! separate lockout counter" scope decision deliberately:
//!
//! - **What it bounds**: consecutive *failed* `POST /login` attempts from one
//!   source. After [`LOCKOUT_BUDGET`] of them inside
//!   [`LOCKOUT_FAILURE_WINDOW`], further attempts from that source are refused
//!   outright for [`LOCKOUT_COOLDOWN`] — **without even checking the
//!   password**, so a correct password cannot be used to reset the lockout,
//!   which is the whole point. A success before the budget is reached clears
//!   that source's streak. The window is **idle-based**: each attempt pushes
//!   its end out, so only an absence of attempts expires a streak. The attempt
//!   is counted **at the gate**, atomically with the budget check and before
//!   the hash, so a burst of concurrent guesses cannot all read a count below
//!   the budget first — the budget is the budget, not the budget plus however
//!   many race it.
//! - **The key**: the request's peer address
//!   (`ConnectInfo<SocketAddr>`) — an IPv4 address, or an IPv6 **/64 prefix**
//!   (the allocation a host rotates inside for free), with the absent case — an
//!   in-process test client, or any deployment without
//!   `into_make_service_with_connect_info` — sharing one documented
//!   [`Source::Unknown`] bucket.
//! - **State bound**: at most [`LOCKOUT_MAX_SOURCES`] sources are tracked,
//!   oldest-seen evicted first ([`LockoutTracker`] carries the reasoning), so
//!   the table is a bucket an attacker cannot grow into a memory-exhaustion
//!   hole.
//! - **Reverse-proxy caveat, stated honestly**: in the documented nginx
//!   deployment the peer is **the proxy** (127.0.0.1) for every request, so
//!   every client shares one bucket — the lockout then still bounds
//!   *aggregate* guessing, but one attacker can lock out everybody, and nginx
//!   cannot fix it by forwarding a header because nothing here trusts one. A
//!   per-client limit in front of the app (the README's `limit_req` example)
//!   is the complementary control, not a substitute for this one; the bearer
//!   token is not part of the lockout, so scripted access survives it.
//! - **Under a flood, the bound wins over the lockout**: an attacker who
//!   contributes more than [`LOCKOUT_MAX_SOURCES`] distinct sources inside the
//!   failure window can have an actively locked-out entry evicted — the cap is
//!   the memory guarantee, and no bound on memory can also be a bound on every
//!   source. That is a deliberate trade (the alternative, refusing new sources
//!   once full, would let a flood block *legitimate* logins outright), and it
//!   costs real work — a distinct peer address per attempt — to buy back one
//!   source's guesses, which Argon2's per-guess cost still prices.
//! - **The answer**: `429 Too Many Requests` with a plain-text reason and a
//!   `Retry-After` naming the remaining seconds rounded up. A browser
//!   (`Accept: text/html`) gets the same `429` — the machine-visible refusal
//!   must not depend on a header the client chooses — carrying the rendered
//!   sign-in page as its body.
//!
//! This is **defence in depth against brute force, not a user account
//! system**: there is still one shared credential, no per-user state, no
//! permanent lock and no audit of who tried what beyond a `WARN` naming the
//! source (never the attempted password).
//!
//! Argon2 runs on login only; every other authenticated request costs one
//! HMAC verification. The lockout is checked before the hash is, so a refused
//! attempt costs nothing at all.
//!
//! What the lockout does **not** bound is how many verifies run *at once*: it
//! prices one source's guesses, and a flood arrives from many. That is
//! [`MAX_CONCURRENT_VERIFIES`]' job — the verify runs on the blocking pool
//! under a process-wide slot limit, so `Argon2::default()`'s 19 MiB and 30 ms
//! are multiplied by a constant this module chose rather than by however many
//! unauthenticated requests arrived together. An attempt that waits longer
//! than [`VERIFY_QUEUE_WAIT`] for a slot is refused with the same `429`.

use crate::infra::http::ApiError;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Router,
    extract::{ConnectInfo, FromRequest, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOGIN_HTML: &str = include_str!("auth/login.html");
const COOKIE_NAME: &str = "st_session";
const SESSION_CONTEXT: &[u8] = b"share-tracker session v1";
/// How long a session cookie is valid for. Not a config knob — one fewer
/// setting to get wrong; sign in again after 30 days.
const SESSION_LIFETIME_SECS: u64 = 30 * 24 * 60 * 60;

/// Consecutive failed `POST /login` attempts one source may make before it is
/// locked out. Not a config knob: the item is about behaviour, not another
/// setting to get wrong, and five is small enough that an attacker spending
/// Argon2's ~30 ms/guess gets nowhere while a person mistyping a password
/// twice is unaffected.
const LOCKOUT_BUDGET: u32 = 5;
/// How long a source's failure streak survives with no further attempt. A
/// streak older than this is dropped rather than counted toward the budget,
/// so isolated mistyped passwords days apart never accumulate into a lockout.
/// (An *active* lockout is bounded by [`LOCKOUT_COOLDOWN`] instead.)
const LOCKOUT_FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// How long a locked-out source is refused before it may try again. Deliberately
/// short: this is a single-credential hobbyist deployment whose owner may
/// legitimately have just mistyped the password five times, and the point is to
/// make online guessing impractical, not to lock the owner out of their own
/// portfolio.
const LOCKOUT_COOLDOWN: Duration = Duration::from_secs(5 * 60);
/// The most sources tracked at once. See [`LockoutTracker`] for why the state
/// is capped rather than merely expired.
const LOCKOUT_MAX_SOURCES: usize = 4096;

/// How many Argon2 verifies may be in flight at once, across every source.
///
/// The lockout bounds what *one* source can spend; this bounds what all of them
/// together can. `Argon2::default()` is m=19 MiB and ~30 ms of CPU, and the
/// verify used to run inline on the async handler — so N concurrent first-time
/// attempts from N different sources were N × 19 MiB of transient memory and N
/// blocked tokio workers, with nothing capping N. Two is enough for a
/// single-credential deployment (a second is there so the owner's own login is
/// not queued behind one stranger's) and caps the transient cost at ~38 MiB and
/// two threads of the blocking pool.
const MAX_CONCURRENT_VERIFIES: usize = 2;
/// How long a login waits for one of the [`MAX_CONCURRENT_VERIFIES`] slots
/// before it is refused `429` instead.
///
/// Queueing without a deadline would trade the memory bound for an unbounded
/// wait — a flood could park every pending login indefinitely, which for the
/// owner is indistinguishable from the server being down. Two seconds is long
/// enough to absorb a handful of overlapping attempts (each ~30 ms) and short
/// enough that a refusal is quick.
const VERIFY_QUEUE_WAIT: Duration = Duration::from_secs(2);

/// The verify slots [`MAX_CONCURRENT_VERIFIES`] hands out. Process-wide rather
/// than per-`Auth`: the bound being protected is the host's memory and blocking
/// pool, which every router in the process shares.
static VERIFY_SLOTS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(MAX_CONCURRENT_VERIFIES));

/// Verifies running right now, and the most that ever ran at once — the
/// concurrency bound as the process actually observed it, which is what
/// `the_concurrent_verifies_are_bounded` asserts on. Two relaxed atomics on a
/// path that already costs 30 ms of Argon2.
static VERIFIES_IN_FLIGHT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static VERIFIES_PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Per-source failed-login lockout
// ---------------------------------------------------------------------------

/// Where a login attempt came from. Two variants only: any deployment without
/// `into_make_service_with_connect_info` — the in-process test client among
/// them — collapses into the one [`Source::Unknown`] bucket, which is
/// documented behaviour rather than a silent fallback (a plain-HTTP listener
/// misconfigured without a socket still gets a working, if coarse, lockout).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Source {
    /// One IPv4 peer address — deliberately not the port. A TCP client picks a
    /// fresh ephemeral source port per connection, so keying on `IP:port` would
    /// hand an attacker a clean counter for every guess: reconnect, and the
    /// lockout is gone. Ignoring the port is what makes the lockout cost an
    /// attacker something; a NATed network still shares one bucket, which is
    /// the usual trade-off (fail2ban and nginx's `limit_req_zone` key on the
    /// bare address for the same reason).
    V4(Ipv4Addr),
    /// One IPv6 **/64 prefix**, with the low 64 bits zeroed.
    ///
    /// Keying an IPv6 peer on the full address would make the lockout free to
    /// defeat: a residential or VPS allocation is a /64, and rotating the
    /// interface identifier costs an attacker nothing while each attempt gets a
    /// fresh budget (and evicts a real entry from the 4096 cap). fail2ban and
    /// nginx's `limit_req_zone` key IPv6 on the /64 for exactly this reason,
    /// and `Source::of` is where that normalisation happens.
    V6Prefix(Ipv6Addr),
    /// No connect info on the request. A single shared bucket for every such
    /// request — the in-process test client, and any deployment whose listener
    /// was not built with `into_make_service_with_connect_info`.
    Unknown,
}

impl Source {
    fn of(peer: Option<SocketAddr>) -> Self {
        match peer.map(|addr| addr.ip()) {
            Some(IpAddr::V4(v4)) => Source::V4(v4),
            Some(IpAddr::V6(v6)) => {
                // Zero the interface identifier: the /64 is the allocation a
                // host is given, so every address inside it is one source.
                let mut octets = v6.octets();
                octets[8..].fill(0);
                Source::V6Prefix(Ipv6Addr::from(octets))
            }
            None => Source::Unknown,
        }
    }

    /// How the source is named in the log line. Never the attempted password
    /// (which is not even in scope where this is called).
    fn label(self) -> String {
        match self {
            Source::V4(ip) => ip.to_string(),
            Source::V6Prefix(ip) => format!("{ip}/64"),
            Source::Unknown => "<unknown peer>".to_string(),
        }
    }
}

/// Argon2's constant-time compare of `password` against a stored PHC string.
///
/// Free-standing so the [`Auth`]-borrowing sync path and the `spawn_blocking`
/// closure (which can only own its data) share one implementation of the
/// compare rather than each carrying a copy.
fn password_matches(stored_hash: &str, password: &str) -> bool {
    let Ok(hash) = PasswordHash::new(stored_hash) else {
        return false; // unreachable: validated in `Auth::new`
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

/// The `POST /login` handler: reads the peer and the browser/script split off
/// the request, then delegates to [`login_submit`].
///
/// It is an `async fn` taking the whole `Request` rather than a handler with
/// extractor arguments because one of the two values it needs — the peer
/// address — has **no optional extractor** in axum 0.8: `ConnectInfo` is
/// mandatory by design (`Option<ConnectInfo<_>>` needs an
/// `OptionalFromRequestParts` impl axum deliberately does not provide, and
/// both types are foreign so it cannot be added here). Reading the extension
/// directly keeps the peer optional — a listener built without
/// `into_make_service_with_connect_info`, and the in-process test client,
/// simply land in the documented [`Source::Unknown`] bucket — while consulting
/// exactly the value a real socket installs. The form body is still decoded
/// through axum's own `Form` extractor.
async fn login_handler(auth: Auth, base_path: String, req: Request) -> Response {
    // The peer is `ConnectInfo<SocketAddr>` — the extension
    // `into_make_service_with_connect_info::<SocketAddr>()` (see `main`)
    // installs, stored in the request's extensions as the bare `ConnectInfo`
    // value (`axum::Extension` inserts its inner `T`).
    let source = Source::of(
        req.extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| *addr),
    );
    // Read before the body is decoded, and before any lockout decision: a
    // browser (`Accept: text/html`) gets the rendered page on either failure
    // path, a script gets the machine-readable status.
    let html = wants_html(req.headers());
    // The form body is decoded through axum's own extractor, so an absent or
    // malformed body is answered exactly as it is on every other form route.
    let form = match axum::extract::Form::<LoginForm>::from_request(req, &()).await {
        Ok(axum::extract::Form(form)) => form,
        Err(rejection) => return rejection.into_response(),
    };
    login_submit(auth, base_path, source, html, form).await
}

/// What the tracker remembers about one source.
#[derive(Debug)]
struct SourceState {
    /// Consecutive failures in the current streak, and when the streak last
    /// advanced. The attempt that *reaches* the budget is counted and answered
    /// normally; the next one is the first refused.
    failures: u32,
    /// When the current streak last advanced. The failure window is
    /// **idle-based** — every attempt pushes this out — so a streak survives
    /// while attempts continue and expires only after the window passes with
    /// none, which is what the const documents.
    window_started: Instant,
    /// When this source was last seen, for the cap's oldest-first eviction.
    last_seen: Instant,
    /// When an active lockout ends; `None` when not locked out.
    locked_until: Option<Instant>,
}

/// The outcome of asking to attempt a login: allowed (and the attempt is now
/// counted against the source's budget), or refused because the source is
/// locked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Attempt {
    Allowed,
    Refused { retry_after_secs: u64 },
}

/// The bounded, in-memory failed-login tracker behind `POST /login`.
///
/// **Why bounded.** An attacker chooses the peer address of every attempt (and
/// rotates it freely), so a map keyed on the source would be a table an
/// attacker can grow until the process runs out of memory — a denial of service
/// handed over by the very control meant to prevent one. Two mechanisms close
/// that, and both are deliberate:
///
/// 1. every access prunes entries whose lockout has expired *and* whose failure
///    window has elapsed — so an idle source stops being tracked at all; and
/// 2. the map is hard-capped at [`LOCKOUT_MAX_SOURCES`] entries, evicting the
///    least-recently-seen source when a new one would exceed the cap.
///
/// Expiry alone is not enough (an attacker can create sources faster than they
/// expire, within the window), and the hard cap alone is not enough (it would
/// let a flood of one-shot sources evict a genuinely locked-out one), so the
/// two run together: the cap is the memory bound, and eviction prefers a stale
/// entry over a live one by virtue of being least-recently-seen.
///
/// Cloned with the [`Auth`] it lives on, sharing one table through the `Arc`;
/// the `Mutex` is held only across a few map operations, never across an
/// `await`.
#[derive(Clone, Debug)]
struct LockoutTracker {
    state: Arc<Mutex<TrackerState>>,
    policy: LockoutPolicy,
}

/// The lockout's tunables as one value, so the production consts and a test's
/// small numbers are the *same* state machine with different numbers rather
/// than two code paths.
#[derive(Clone, Copy, Debug)]
struct LockoutPolicy {
    /// Consecutive failures allowed before the cooldown starts.
    budget: u32,
    /// How long a failure streak survives with no further attempt.
    window: Duration,
    /// How long a locked-out source is refused.
    cooldown: Duration,
    /// The most sources tracked at once.
    max_sources: usize,
}

impl LockoutPolicy {
    /// The documented production policy — the consts at the top of the module.
    const fn production() -> Self {
        LockoutPolicy {
            budget: LOCKOUT_BUDGET,
            window: LOCKOUT_FAILURE_WINDOW,
            cooldown: LOCKOUT_COOLDOWN,
            max_sources: LOCKOUT_MAX_SOURCES,
        }
    }
}

#[derive(Debug)]
struct TrackerState {
    /// Keyed by source; capped at [`LockoutPolicy::max_sources`].
    sources: HashMap<Source, SourceState>,
}

impl LockoutTracker {
    /// The production policy — the documented consts above.
    fn new() -> Self {
        Self::with_policy(LockoutPolicy::production())
    }

    /// The same state machine over a caller's policy. Kept out of the public
    /// API — there is no deployment path to it — and used by
    /// [`Auth::with_lockout_policy`] so a test drives the identical code over a
    /// small budget and a sub-second cooldown instead of sleeping for minutes.
    fn with_policy(policy: LockoutPolicy) -> Self {
        LockoutTracker {
            state: Arc::new(Mutex::new(TrackerState {
                sources: HashMap::new(),
            })),
            policy,
        }
    }

    /// How many sources are currently tracked — the bound's observable, for
    /// the tests that drive more sources than the cap.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().sources.len()
    }

    /// The tracker's mutex, recovering the state from a poisoned lock rather
    /// than panicking: a panic in an unrelated handler must not make the whole
    /// surface permanently unservable, and the tracker's own operations cannot
    /// leave its invariants broken half-way.
    fn lock(&self) -> std::sync::MutexGuard<'_, TrackerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Rounds a remaining cooldown **up** to whole seconds for `Retry-After`:
    /// truncating 299.4 s to `299` invites a retry a fraction before the
    /// cooldown ends, which is refused again — a client that honours the header
    /// must not be lied to. Never returns 0 while any time remains.
    fn ceil_secs(remaining: Duration) -> u64 {
        remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
    }

    /// Reserve one attempt at the gate: refuse (without counting, and without
    /// ever reaching the caller's Argon2 verify) when the source is locked out;
    /// otherwise **count the attempt now**, under the same lock that made the
    /// decision.
    ///
    /// Counting at the gate is what bounds *concurrent* guesses. Checking the
    /// budget and incrementing it after the verify — which is ~30 ms of Argon2 —
    /// lets a burst of parallel attempts all read a count below the budget
    /// before any of them writes, so the effective budget becomes
    /// `budget + concurrency`, repeatable every cooldown. Reserving here makes
    /// it exactly the budget: the attempt that reaches it is still answered
    /// normally (the budget counts failures, so it takes that many to trip),
    /// and every attempt after it is refused before any hashing.
    fn begin_attempt(&self, source: Source) -> Attempt {
        let now = Instant::now();
        let mut state = self.lock();
        // Pruning is O(tracked sources); only a *new* source can grow the map,
        // so only that path pays for it. An existing entry's own expiry is
        // handled below, where it matters.
        if !state.sources.contains_key(&source) {
            Self::prune(&mut state, now, self.policy.window);
            if state.sources.len() >= self.policy.max_sources {
                Self::evict_oldest(&mut state);
            }
        }
        let entry = state.sources.entry(source).or_insert_with(|| SourceState {
            failures: 0,
            window_started: now,
            last_seen: now,
            locked_until: None,
        });
        entry.last_seen = now;
        if let Some(until) = entry.locked_until {
            if until > now {
                return Attempt::Refused {
                    retry_after_secs: Self::ceil_secs(until.saturating_duration_since(now)),
                };
            }
            // The cooldown has been served: back to a clean slate.
            entry.locked_until = None;
            entry.failures = 0;
        } else if now.saturating_duration_since(entry.window_started) >= self.policy.window {
            // The streak went idle for a whole window: it is dropped rather
            // than counted toward the budget.
            entry.failures = 0;
        }
        entry.failures += 1;
        entry.window_started = now;
        if entry.failures >= self.policy.budget {
            entry.locked_until = Some(now + self.policy.cooldown);
        }
        Attempt::Allowed
    }

    /// Clears a source's failure streak — a login that succeeded before the
    /// budget was reached is forgiven, so an ordinary mistyped attempt costs
    /// nothing permanent.
    fn record_success(&self, source: Source) {
        let mut state = self.lock();
        state.sources.remove(&source);
    }

    /// Drop entries that hold nothing worth bounding: a source that has no
    /// active lockout and whose failure window has elapsed can be forgotten
    /// entirely.
    fn prune(state: &mut TrackerState, now: Instant, window: Duration) {
        state.sources.retain(|_, entry| {
            let locked = entry.locked_until.is_some_and(|until| until > now);
            locked || now.saturating_duration_since(entry.window_started) < window
        });
    }

    /// Make room under the cap by dropping the least-recently-seen source. The
    /// live lockouts are what the cap must not lose preferentially, and the
    /// least-recently-seen entry is the one no request has touched for longest.
    fn evict_oldest(state: &mut TrackerState) {
        let Some(oldest) = state
            .sources
            .iter()
            .min_by_key(|(_, entry)| entry.last_seen)
            .map(|(source, _)| *source)
        else {
            return;
        };
        state.sources.remove(&oldest);
    }
}

/// The resolved `[auth]` configuration. Cheap to clone (a couple of
/// `String`s, a bool, a 32-byte key, and an `Arc` to the lockout table) —
/// cloned into every handler/middleware closure that needs it, the same way
/// `SharedFetcher` is. `Debug` exists so `config::Settings` (which derives it)
/// can keep doing so. `PartialEq` is hand-written — see its impl below — since
/// `Settings` derives that too and equality must not depend on how many failed
/// logins happened to be in flight.
#[derive(Clone, Debug)]
pub struct Auth {
    username: String,
    /// The PHC string (`$argon2id$...`), re-parsed on each password check —
    /// parsing a PHC string is cheap, and it means [`Auth`] never carries a
    /// type borrowing from itself.
    password_hash: String,
    api_token: Option<String>,
    /// Whether the session cookie carries `Secure` (HTTPS-only). Independent
    /// of this process's own scheme: behind the documented nginx reverse
    /// proxy, the *browser* talks HTTPS even though the backend socket is
    /// plain HTTP loopback, and `Secure` is judged by the browser's own
    /// connection. Defaults to `true`; set `secure_cookie = false` in
    /// `[auth]` only for a deliberately plain-HTTP setup (e.g. local testing).
    secure_cookie: bool,
    signing_key: [u8; 32],
    /// The bounded per-source failed-login lockout. Shared through its own
    /// `Arc` (not the derived `Clone` of an inner `HashMap`), so every clone
    /// of one `Auth` — the router closure, the test client — sees one table.
    lockouts: LockoutTracker,
}

/// Equality over the *configuration* only. The lockout table is live state
/// that changes with traffic, and `config::Settings`' `PartialEq` compares
/// resolved settings — a failed login is not a change of configuration, and
/// two `Auth`s built from the same `[auth]` table must compare equal however
/// many attempts either has seen.
impl PartialEq for Auth {
    fn eq(&self, other: &Self) -> bool {
        self.username == other.username
            && self.password_hash == other.password_hash
            && self.api_token == other.api_token
            && self.secure_cookie == other.secure_cookie
            && self.signing_key == other.signing_key
    }
}

impl Auth {
    /// Builds an `Auth` from resolved config values. Parses `password_hash`
    /// up front — an unusable hash aborts startup, the same philosophy as
    /// `config::normalise_base_path`: serving a login page that can never
    /// succeed is worse than not starting.
    pub fn new(
        username: String,
        password_hash: String,
        api_token: Option<String>,
        secure_cookie: bool,
    ) -> Result<Self, String> {
        PasswordHash::new(&password_hash)
            .map_err(|e| format!("invalid auth.password_hash: {e}"))?;
        let signing_key = derive_signing_key(&password_hash);
        Ok(Auth {
            username,
            password_hash,
            api_token,
            secure_cookie,
            signing_key,
            lockouts: LockoutTracker::new(),
        })
    }

    /// [`Auth::new`] over a test-sized lockout policy — the identical state
    /// machine with a small budget and a sub-second cooldown, so the
    /// lockout's tests exercise the real code without sleeping for minutes.
    /// `#[cfg(test)]` so the production policy on a real server can never be
    /// configured around the documented consts.
    #[cfg(test)]
    fn with_lockout_policy(
        username: String,
        password_hash: String,
        api_token: Option<String>,
        secure_cookie: bool,
        policy: LockoutPolicy,
    ) -> Result<Self, String> {
        let mut auth = Auth::new(username, password_hash, api_token, secure_cookie)?;
        auth.lockouts = LockoutTracker::with_policy(policy);
        Ok(auth)
    }

    /// Reserve an attempt for the given peer at the gate, *before* the password
    /// is checked: a locked-out source is refused without an Argon2 verify, a
    /// correct password cannot reset the lockout, and the attempt is counted
    /// atomically with the decision — see [`LockoutTracker::begin_attempt`].
    fn begin_login_attempt(&self, source: Source) -> Attempt {
        self.lockouts.begin_attempt(source)
    }

    fn record_login_success(&self, source: Source) {
        self.lockouts.record_success(source);
    }

    /// Hashes `password` as a fresh Argon2id PHC string, for `share-tracker
    /// hash-password` to print. Uses the library's own recommended
    /// parameters (`Argon2::default()`) rather than hand-tuned ones — this is
    /// a single-credential hobbyist deployment, not a target worth a bespoke
    /// cost parameter.
    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| format!("failed to hash password: {e}"))
    }

    /// 32 random bytes as lowercase hex, for `share-tracker gen-token` to
    /// print — reuses the CSPRNG argon2's `password-hash` feature already
    /// pulls in rather than adding a `rand` dependency just for this.
    pub fn generate_token() -> String {
        use argon2::password_hash::rand_core::RngCore;
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        hex_encode(&bytes)
    }

    /// Checks a login attempt. The username compare is not constant-time
    /// (usernames aren't secret); the password compare is constant-time via
    /// Argon2's own verifier.
    ///
    /// Synchronous, and the ~30 ms of Argon2 is the whole call — so the login
    /// handler reaches it through [`Self::verify_password_bounded`], never
    /// directly, and this is `#[cfg(test)]` because nothing else may: an
    /// ungated one would be an inline-Argon2 path a future caller could pick up
    /// by accident, as well as dead code in the non-test build.
    #[cfg(test)]
    fn verify_password(&self, username: &str, password: &str) -> bool {
        if username != self.username {
            return false;
        }
        password_matches(&self.password_hash, password)
    }

    /// [`Self::verify_password`] off the async runtime and under the
    /// process-wide slot limit: `Some(verified)`, or `None` when no slot came
    /// free inside [`VERIFY_QUEUE_WAIT`] and the attempt is to be refused.
    ///
    /// Two separate problems, one place. `spawn_blocking` keeps 30 ms of CPU off
    /// the async worker that would otherwise stop serving every other request
    /// on it; the semaphore keeps the *number* of those verifies — and so the
    /// 19 MiB each one allocates — from being whatever an unauthenticated flood
    /// chooses. The per-source lockout does not cover this: it prices one
    /// source's guesses, and a flood's whole point is to arrive from many.
    async fn verify_password_bounded(&self, username: &str, password: &str) -> Option<bool> {
        use std::sync::atomic::Ordering::Relaxed;
        let permit = tokio::time::timeout(VERIFY_QUEUE_WAIT, VERIFY_SLOTS.acquire())
            .await
            .ok()?
            .ok()?;
        let username_matches = username == self.username;
        let hash = self.password_hash.clone();
        let password = password.to_string();
        let verified = tokio::task::spawn_blocking(move || {
            let in_flight = VERIFIES_IN_FLIGHT.fetch_add(1, Relaxed) + 1;
            VERIFIES_PEAK.fetch_max(in_flight, Relaxed);
            // The same two steps `verify_password` takes, in the same order —
            // the username compare short-circuits, which is a stated decision
            // there (usernames are not secret) and not something moving the work
            // to another thread should quietly change.
            let matched = username_matches && password_matches(&hash, &password);
            VERIFIES_IN_FLIGHT.fetch_sub(1, Relaxed);
            matched
        })
        .await
        .unwrap_or(false);
        drop(permit);
        Some(verified)
    }

    /// Constant-time compare against the configured `api_token`; `false` if
    /// none is configured.
    fn verify_bearer(&self, token: &str) -> bool {
        use subtle::ConstantTimeEq;
        match &self.api_token {
            Some(expected) => token.as_bytes().ct_eq(expected.as_bytes()).into(),
            None => false,
        }
    }

    /// A fresh, signed cookie value for [`Self::username`], valid for
    /// [`SESSION_LIFETIME_SECS`] from now.
    fn mint_cookie(&self) -> String {
        let expiry = now_unix() + SESSION_LIFETIME_SECS;
        self.sign(&format!("{}|{expiry}", self.username))
    }

    fn sign(&self, payload: &str) -> String {
        let mut mac = hmac_with(&self.signing_key);
        mac.update(payload.as_bytes());
        let tag = mac.finalize().into_bytes();
        format!("{}.{}", hex_encode(payload.as_bytes()), hex_encode(&tag))
    }

    /// Verifies a cookie value end to end: well-formed, correctly signed
    /// (constant-time via `hmac`'s own `verify_slice`), not expired, and
    /// names this `Auth`'s configured username.
    fn verify_cookie(&self, cookie: &str) -> bool {
        let Some((payload_hex, tag_hex)) = cookie.split_once('.') else {
            return false;
        };
        let (Some(payload_bytes), Some(given_tag)) = (hex_decode(payload_hex), hex_decode(tag_hex))
        else {
            return false;
        };
        let mut mac = hmac_with(&self.signing_key);
        mac.update(&payload_bytes);
        if mac.verify_slice(&given_tag).is_err() {
            return false;
        }
        let Ok(payload) = String::from_utf8(payload_bytes) else {
            return false;
        };
        let Some((username, expiry)) = payload.split_once('|') else {
            return false;
        };
        let Ok(expiry) = expiry.parse::<u64>() else {
            return false;
        };
        username == self.username && expiry > now_unix()
    }
}

fn hmac_with(key: &[u8; 32]) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepts any key length")
}

fn derive_signing_key(password_hash: &str) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(password_hash.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(SESSION_CONTEXT);
    mac.finalize().into_bytes().into()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs()
}

/// Lowercase hex, matching `entities::attachment::checksum_hex`'s style.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(out, "{b:02x}").expect("writing to a String never fails");
    }
    out
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    // Bytewise `&s[i..i + 2]` slicing is only sound on an ASCII string: these
    // lengths are *bytes*, so an even-byte-length non-ASCII string (an emoji
    // is four bytes) would split a multi-byte char and panic. Callers today
    // pass header values already filtered by `HeaderValue::to_str()`, but the
    // invariant belongs here rather than two modules away.
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// The state the [`require_auth`] middleware closes over: the credential plus
/// the reverse-proxy prefix it needs to build an absolute `Location`/cookie
/// `Path` (the middleware runs on the *unprefixed* request — `nest` strips
/// the prefix before delegating — so it cannot recover `base_path` from the
/// request itself).
#[derive(Clone)]
pub struct AuthState {
    auth: Auth,
    base_path: String,
}

impl AuthState {
    pub fn new(auth: Auth, base_path: String) -> Self {
        AuthState { auth, base_path }
    }
}

/// Gate every request except the login/allowlisted paths behind a valid
/// session cookie or bearer token. Wired in with `axum::middleware::
/// from_fn_with_state` at `app::router` — see that function for why it must
/// be layered before the reverse-proxy `nest`.
pub async fn require_auth(State(state): State<AuthState>, req: Request, next: Next) -> Response {
    if is_allowlisted(req.uri().path()) || is_authorized(&state.auth, req.headers()) {
        return next.run(req).await;
    }
    unauthorized_response(&state, req.headers())
}

fn is_allowlisted(path: &str) -> bool {
    matches!(path, "/login" | "/static/style.css")
}

fn is_authorized(auth: &Auth, headers: &HeaderMap) -> bool {
    if let Some(token) = bearer_token(headers)
        && auth.verify_bearer(token)
    {
        return true;
    }
    if let Some(cookie) = session_cookie(headers)
        && auth.verify_cookie(cookie)
    {
        return true;
    }
    false
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (name, value) = kv.trim().split_once('=')?;
        (name == COOKIE_NAME).then_some(value)
    })
}

/// A browser navigation (the SPA shell, an attachment download, a CSV
/// export — none of which can carry an `Authorization` header) gets sent to
/// the login page; anything else (the SPA's own `fetch()`, curl, the
/// deployment scripts) gets a plain 401 it can act on programmatically.
fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"))
}

fn unauthorized_response(state: &AuthState, headers: &HeaderMap) -> Response {
    if wants_html(headers) {
        Redirect::to(&format!("{}/login", state.base_path)).into_response()
    } else {
        ApiError::unauthorized("no valid session cookie or bearer token").into_response()
    }
}

// ---------------------------------------------------------------------------
// Login / logout routes
// ---------------------------------------------------------------------------

#[derive(utoipa::ToSchema, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginForm {
    username: String,
    password: String,
}

/// `GET /login`, `POST /login` and `POST /logout` — merged into the app
/// alongside `web::router` only when `[auth]` is configured, so these routes
/// (and the `Auth` they need) simply don't exist otherwise.
///
/// `POST /login` keys its lockout on the request's peer address, which `main`
/// installs by serving through
/// `into_make_service_with_connect_info::<SocketAddr>()`. It is read
/// **optionally** (see [`login_handler`]): the in-process test client, and any
/// listener built without that call, has no such extension and still serves the
/// login page, sharing the one documented [`Source::Unknown`] bucket.
pub fn router(auth: Auth, base_path: &str) -> Router<SqlitePool> {
    let base_path = base_path.to_string();
    let get_base = base_path.clone();
    let post_auth = auth.clone();
    let post_base = base_path.clone();
    let logout_auth = auth;
    let logout_base = base_path;
    Router::new()
        .route(
            "/login",
            get(move || {
                let base_path = get_base.clone();
                async move { render_login(&base_path, None) }
            })
            .post(move |req: Request| {
                let auth = post_auth.clone();
                let base_path = post_base.clone();
                async move { login_handler(auth, base_path, req).await }
            }),
        )
        .route(
            "/logout",
            post(move || {
                let auth = logout_auth.clone();
                let base_path = logout_base.clone();
                async move { logout(auth, base_path).await }
            }),
        )
}

fn render_login(base_path: &str, error: Option<&str>) -> Html<String> {
    // `error` is always one of this module's own fixed strings, never
    // reflected user input, so no HTML-escaping is needed here.
    let error_html = match error {
        Some(msg) => format!(r#"<p class="error">{msg}</p>"#),
        None => String::new(),
    };
    Html(
        LOGIN_HTML
            .replace("{{BASE}}", base_path)
            .replace("{{ERROR}}", &error_html)
            // The same pre-paint colour-scheme script the SPA shell carries —
            // one const, so the login page can never disagree with the app it
            // leads to about which scheme is in force. It is also the only
            // JavaScript this page has: it loads no modules.
            .replace("{{THEME}}", crate::web::THEME_BOOT_SCRIPT),
    )
}

async fn login_submit(
    auth: Auth,
    base_path: String,
    source: Source,
    html: bool,
    form: LoginForm,
) -> Response {
    // The lockout gate comes **first**, before the password is even looked at:
    // that is what makes a correct password unable to reset an active lockout,
    // and it is also why a refused attempt costs no Argon2 work. The attempt is
    // counted here, atomically with the decision, so concurrent guesses cannot
    // slip past the budget between the check and the verify.
    if let Attempt::Refused { retry_after_secs } = auth.begin_login_attempt(source) {
        // Named by source, never by the attempted password — which is not in
        // scope here and must never reach a log. The source is either a socket
        // address or the fixed `<unknown peer>` sentinel, so nothing
        // attacker-chosen is written verbatim.
        tracing::warn!(
            source = %source.label(),
            retry_after_secs,
            "login refused: locked out after repeated failures"
        );
        let message = lockout_message(retry_after_secs);
        // Both answers are `429` — a machine-visible refusal an operator's log
        // or fail2ban rule can act on, which a `200` page for `Accept:
        // text/html` would hide. A browser still gets the rendered sign-in page
        // as the body rather than a bare plain-text reason.
        return if html {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, retry_after_secs.to_string())],
                render_login(&base_path, Some(&message)),
            )
                .into_response()
        } else {
            ApiError::too_many_requests(message, retry_after_secs).into_response()
        };
    }

    let Some(verified) = auth
        .verify_password_bounded(&form.username, &form.password)
        .await
    else {
        // Every verify slot was busy for VERIFY_QUEUE_WAIT. The same `429` the
        // lockout answers, for the same reason — too many login attempts, just
        // counted across sources rather than per source — with a one-second
        // Retry-After, since the queue drains in tens of milliseconds.
        tracing::warn!(
            source = %source.label(),
            "login refused: no Argon2 verify slot free"
        );
        let message = "too many login attempts are being verified at once; try again in a moment";
        return if html {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "1".to_string())],
                render_login(&base_path, Some(message)),
            )
                .into_response()
        } else {
            ApiError::too_many_requests(message.to_string(), 1).into_response()
        };
    };
    if verified {
        auth.record_login_success(source);
        // `?` (Debug quoting) rather than `%`: the username is request text,
        // and the default `fmt` subscriber writes a `%` field verbatim, so a
        // control character in it would split the log line.
        tracing::info!(username = ?form.username, "login succeeded");
        let cookie = build_set_cookie(&auth, &base_path, Some(auth.mint_cookie()));
        (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, home_path(&base_path)),
                (header::SET_COOKIE, cookie),
            ],
        )
            .into_response()
    } else {
        // The attempt was already counted at the gate (see `begin_login_attempt`),
        // so there is nothing more to record here.
        // Named by attempted username as well as source: the username is the
        // signal a human reading the log wants (which credential is being
        // guessed at), and the source is what the lockout keys on.
        //
        // `?` (Debug quoting) rather than `%`: this is the **pre-auth**
        // `POST /login` and the username is entirely attacker-chosen, so a
        // `%` field written verbatim would let `username=evil%0A…` append
        // lines to the log.
        tracing::warn!(
            source = %source.label(),
            username = ?form.username,
            "login failed: wrong username or password"
        );
        render_login(&base_path, Some("Incorrect username or password.")).into_response()
    }
}

/// The lockout's user-facing reason — the one string both answers carry: the
/// plain-text `429` body and, wrapped in the login page's error paragraph, the
/// browser's message. Built here so the two cannot drift.
///
/// It says how long to wait but never how many attempts are allowed: naming
/// the budget would only help someone probing it.
fn lockout_message(retry_after_secs: u64) -> String {
    format!("Too many failed sign-in attempts. Try again in {retry_after_secs} seconds.")
}

async fn logout(auth: Auth, base_path: String) -> Response {
    let cookie = build_set_cookie(&auth, &base_path, None);
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, home_path(&base_path)),
            (header::SET_COOKIE, cookie),
        ],
    )
        .into_response()
}

/// Where a successful login/logout lands: the app root, `"/"` at the default
/// empty `base_path` or the prefix itself under a reverse proxy (mirrors
/// `app::router`'s own redirect target for the bare-prefix form).
fn home_path(base_path: &str) -> String {
    if base_path.is_empty() {
        "/".to_string()
    } else {
        base_path.to_string()
    }
}

fn cookie_path(base_path: &str) -> &str {
    if base_path.is_empty() { "/" } else { base_path }
}

fn build_set_cookie(auth: &Auth, base_path: &str, value: Option<String>) -> String {
    let path = cookie_path(base_path);
    let secure = if auth.secure_cookie { "; Secure" } else { "" };
    match value {
        Some(v) => format!(
            "{COOKIE_NAME}={v}; Path={path}; Max-Age={SESSION_LIFETIME_SECS}; HttpOnly; SameSite=Lax{secure}"
        ),
        None => format!("{COOKIE_NAME}=; Path={path}; Max-Age=0; HttpOnly; SameSite=Lax{secure}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Auth {
        Auth::new(
            "evan".to_string(),
            Auth::hash_password("correct horse battery staple").unwrap(),
            Some("test-token".to_string()),
            true,
        )
        .unwrap()
    }

    #[test]
    fn correct_password_verifies_and_wrong_password_does_not() {
        let a = auth();
        assert!(a.verify_password("evan", "correct horse battery staple"));
        assert!(!a.verify_password("evan", "wrong password"));
        assert!(!a.verify_password("nobody", "correct horse battery staple"));
    }

    #[test]
    fn an_unparseable_password_hash_is_rejected_at_construction() {
        let err = Auth::new("evan".into(), "not a phc string".into(), None, true)
            .err()
            .unwrap();
        assert!(err.contains("password_hash"), "{err}");
    }

    #[test]
    fn a_minted_cookie_round_trips() {
        let a = auth();
        let cookie = a.mint_cookie();
        assert!(a.verify_cookie(&cookie));
    }

    #[test]
    fn a_tampered_cookie_signature_is_rejected() {
        let a = auth();
        let mut cookie = a.mint_cookie();
        // Flip the last hex character of the signature.
        let last = cookie.pop().unwrap();
        let flipped = if last == '0' { '1' } else { '0' };
        cookie.push(flipped);
        assert!(!a.verify_cookie(&cookie));
    }

    #[test]
    fn a_cookie_with_a_forged_payload_is_rejected() {
        // Re-signing a payload naming a different username must not verify
        // against an `Auth` configured for "evan" — the signature is over the
        // payload naming the username, not a shared secret the payload could
        // be swapped out from under.
        let a = auth();
        let forged = a.sign(&format!("someone-else|{}", now_unix() + 3600));
        assert!(!a.verify_cookie(&forged));
    }

    #[test]
    fn a_garbage_cookie_value_is_rejected_not_panicking() {
        let a = auth();
        for bad in ["", "no-dot-at-all", ".", "zz.zz", "abcd.efgh12"] {
            assert!(!a.verify_cookie(bad), "{bad:?} should not verify");
        }
    }

    #[test]
    fn an_expired_cookie_is_rejected() {
        let a = auth();
        let expired = a.sign(&format!("evan|{}", now_unix().saturating_sub(1)));
        assert!(!a.verify_cookie(&expired));
    }

    #[test]
    fn changing_the_password_invalidates_existing_sessions() {
        // The signing key is derived from the password hash, so a cookie
        // minted under the old hash must not verify against the new one —
        // this is the documented consequence of that design, pinned so a
        // future change to the derivation can't silently drop it.
        let old = auth();
        let cookie = old.mint_cookie();
        let new = Auth::new(
            "evan".to_string(),
            Auth::hash_password("a different password").unwrap(),
            None,
            true,
        )
        .unwrap();
        assert!(!new.verify_cookie(&cookie));
    }

    #[test]
    fn bearer_token_accepts_the_configured_value_and_rejects_others() {
        let a = auth();
        assert!(a.verify_bearer("test-token"));
        assert!(!a.verify_bearer("wrong-token"));
        assert!(!a.verify_bearer(""));
    }

    #[test]
    fn no_configured_token_rejects_every_bearer_value() {
        let a = Auth::new("evan".into(), Auth::hash_password("x").unwrap(), None, true).unwrap();
        assert!(!a.verify_bearer("anything"));
    }

    #[test]
    fn hex_round_trips() {
        let bytes = [0u8, 1, 255, 16, 128];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert_eq!(hex_decode("abc"), None); // odd length
        assert_eq!(hex_decode("zz"), None); // not hex
    }

    /// A non-ASCII string of even *byte* length must be rejected, not panic on
    /// a char-boundary slice: "😀" is four bytes, so `&s[0..2]` would split it.
    #[test]
    fn hex_decode_rejects_non_ascii_instead_of_panicking() {
        assert_eq!(hex_decode("😀"), None);
        assert_eq!(hex_decode("😀ab"), None);
        assert_eq!(hex_decode("é"), None); // two bytes, even length
    }

    #[test]
    fn generated_tokens_are_64_hex_chars_and_differ() {
        let a = Auth::generate_token();
        let b = Auth::generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// The documented tradeoff of a self-contained (no session table) cookie:
    /// a cookie minted before logout still verifies after it, because
    /// `POST /logout` only clears the browser's copy (`Max-Age=0`) — there is
    /// no server-side record to mark it revoked in. If this ever starts
    /// failing because a session store was added, the module doc's "cannot
    /// truly revoke" claim needs updating alongside it, not just this test.
    #[test]
    fn logout_does_not_revoke_a_previously_minted_cookie() {
        let a = auth();
        let cookie = a.mint_cookie();
        assert!(a.verify_cookie(&cookie), "sanity: freshly minted");
        // `logout` itself only builds a Set-Cookie header (exercised at the
        // API level in `api_tests`); nothing server-side changes as a result,
        // so the same cookie must still verify.
        assert!(a.verify_cookie(&cookie));
    }

    /// The failed-login username is entirely attacker-chosen on the pre-auth
    /// `POST /login`, and the default `fmt` subscriber writes a `%` field
    /// verbatim — so a value carrying a newline (`username=evil%0A…`) could
    /// append attacker-chosen lines to the log. Debug quoting (`?`) escapes
    /// the control character and keeps the attempt on one line.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn a_control_character_in_a_failed_login_username_cannot_split_the_log_line() {
        let response = login_submit(
            auth(),
            String::new(),
            Source::Unknown,
            false,
            LoginForm {
                username: "evil\nforged login succeeded".to_string(),
                // The credential is run through `Auth::verify_login` against the
                // Argon2 hash, so it can only ever fail — "wrong" is a test input,
                // not a credential. CodeQL's `rust/hard-coded-cryptographic-value`
                // flags this literal as a hard-coded password and cannot see that
                // the enclosing module is `#[cfg(test)]`; it is a reviewed false
                // positive, dismissed as "used in tests" on the code-scanning
                // alert. Rust has no inline `// codeql[...]` suppression to pin
                // that with, so the reasoning lives here and
                // `doc_checks::the_hard_coded_password_false_positive_is_documented`
                // keeps it — and this literal — from being silently removed.
                password: "wrong".to_string(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        logs_assert(|lines: &[&str]| {
            let failed: Vec<&str> = lines
                .iter()
                .copied()
                .filter(|line| line.contains("login failed"))
                .collect();
            match failed.as_slice() {
                [line] => {
                    if !line.contains(r#"username="evil\nforged login succeeded""#) {
                        return Err(format!(
                            "the attempted username must be Debug-quoted so a control \
                             character cannot split the line; got: {line}"
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "expected exactly one login-failed line, got {}: {other:?}",
                        other.len()
                    ));
                }
            }
            if let Some(forged) = lines.iter().find(|line| line.starts_with("forged")) {
                return Err(format!(
                    "the username's newline forged a log line: {forged:?}"
                ));
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod api_tests {
    use super::Auth;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::{StatusCode, header};
    use sqlx::SqlitePool;

    fn auth() -> Auth {
        Auth::new(
            "evan".to_string(),
            Auth::hash_password("hunter2").unwrap(),
            Some("test-token".to_string()),
            // `secure_cookie: false` — the in-process `oneshot` client never
            // negotiates TLS, and `Secure` only governs whether a real
            // browser sends the cookie back over plain HTTP, which nothing
            // here models either way.
            false,
        )
        .unwrap()
    }

    /// The whole application, auth-enabled — as `main` builds it when
    /// `[auth]` is configured. `app::router`'s own tests cover the
    /// `auth: None` default and a reverse-proxy `base_path`; this covers the
    /// gate itself.
    fn client(pool: &SqlitePool, auth: Auth) -> ApiClient {
        let fetcher = crate::entities::closing_price::test_support::QuoteStub::default().shared();
        let registry = crate::infra::scheduler::registry(
            pool.clone(),
            ":memory:".to_string(),
            None,
            None,
            fetcher.clone(),
            crate::entities::distribution_event::test_support::DistributionStub::default().shared(),
            None,
        );
        ApiClient::over(crate::app::router(
            "",
            pool.clone(),
            registry,
            fetcher,
            Some(auth),
        ))
    }

    /// `POST /login` with the right credentials, returning the `st_session`
    /// cookie's `name=value` pair ready to hand to [`ApiClient::with_header`].
    async fn log_in(c: &ApiClient) -> String {
        let resp = c
            .post_bytes(
                "/login",
                Some("application/x-www-form-urlencoded"),
                "username=evan&password=hunter2",
            )
            .await;
        assert_eq!(resp.status, StatusCode::SEE_OTHER, "body: {}", resp.text());
        let set_cookie = resp
            .headers
            .get(header::SET_COOKIE)
            .expect("login sets a cookie")
            .to_str()
            .unwrap();
        set_cookie.split(';').next().unwrap().to_string()
    }

    #[tokio::test]
    async fn an_unauthenticated_json_request_is_401() {
        let pool = test_pool().await;
        let c = client(&pool, auth());
        let resp = c.get("/exchanges").await;
        assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_unauthenticated_html_navigation_redirects_to_login() {
        let pool = test_pool().await;
        let c = client(&pool, auth()).with_header("Accept", "text/html");
        let resp = c.get("/").await;
        assert_eq!(resp.status, StatusCode::SEE_OTHER);
        assert_eq!(resp.headers.get(header::LOCATION).unwrap(), "/login");
    }

    #[tokio::test]
    async fn login_page_and_static_style_are_reachable_without_a_session() {
        let pool = test_pool().await;
        let c = client(&pool, auth());
        assert_eq!(c.get("/login").await.status, StatusCode::OK);
        assert_eq!(c.get("/static/style.css").await.status, StatusCode::OK);
    }

    /// The login page carries the app's colour-scheme bootstrap. It is the
    /// one page here that loads no JS modules at all, so without this inline
    /// script a dark-mode user meets a white page on every session expiry —
    /// and the placeholder must be substituted, not served.
    #[tokio::test]
    async fn the_login_page_applies_the_remembered_colour_scheme() {
        let pool = test_pool().await;
        let c = client(&pool, auth());
        let resp = c.get("/login").await;
        let body = resp.text();
        assert!(body.contains(crate::web::THEME_BOOT_SCRIPT));
        assert!(!body.contains("{{THEME}}"));
    }

    #[tokio::test]
    async fn wrong_password_re_renders_the_login_page_and_sets_no_cookie() {
        let pool = test_pool().await;
        let c = client(&pool, auth());
        let resp = c
            .post_bytes(
                "/login",
                Some("application/x-www-form-urlencoded"),
                "username=evan&password=wrong",
            )
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        assert!(resp.text().contains("Incorrect username or password."));
        assert!(resp.headers.get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn a_valid_session_cookie_unlocks_get_put_and_delete() {
        let pool = test_pool().await;
        let c = client(&pool, auth());
        let cookie = log_in(&c).await;
        let authed = c.with_header("Cookie", cookie);

        let exchange = serde_json::json!({
            "name": "Test Exchange",
            "country": "Testland",
            "currency": "AUD",
            "timezone": "UTC",
            "settlement_days": 2,
        });
        assert_eq!(
            authed.put_json("/exchanges/XTES", &exchange).await,
            StatusCode::CREATED
        );
        assert_eq!(authed.get("/exchanges/XTES").await.status, StatusCode::OK);
        assert_eq!(
            authed.delete("/exchanges/XTES").await.status,
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn logout_clears_the_cookie_client_side() {
        let pool = test_pool().await;
        let c = client(&pool, auth());
        let cookie = log_in(&c).await;
        let authed = c.with_header("Cookie", cookie);
        assert_eq!(authed.get("/exchanges").await.status, StatusCode::OK);

        let resp = authed.post_empty("/logout").await;
        assert_eq!(resp.status, StatusCode::SEE_OTHER);
        let cleared = resp
            .headers
            .get(header::SET_COOKIE)
            .expect("logout sets a clearing cookie")
            .to_str()
            .unwrap();
        assert!(cleared.starts_with("st_session=;"));
        assert!(cleared.contains("Max-Age=0"));
    }

    #[tokio::test]
    async fn bearer_token_unlocks_a_route_and_a_wrong_token_does_not() {
        let pool = test_pool().await;
        let c = client(&pool, auth());

        // `/jobs/no-such-job` 404s once past the auth gate — chosen (over a
        // real job) so this proves the gate opened without triggering an
        // actual scheduled job's side effects.
        let unauthed = c.post_empty("/jobs/no-such-job").await;
        assert_eq!(unauthed.status, StatusCode::UNAUTHORIZED);

        let wrong = c
            .clone()
            .with_header("Authorization", "Bearer wrong-token")
            .post_empty("/jobs/no-such-job")
            .await;
        assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

        let right = c
            .with_header("Authorization", "Bearer test-token")
            .post_empty("/jobs/no-such-job")
            .await;
        assert_eq!(right.status, StatusCode::NOT_FOUND);
    }
}

/// The bounded per-source failed-login lockout (REST API audit, 2026-09-24:
/// "Rate-limit / lock out `POST /login`", B6).
///
/// Two layers, deliberately distinct:
///
/// - the **tracker** unit-tested directly — the budget, the window, the
///   cooldown, the cap and per-source isolation are properties of that state
///   machine, and driving them through HTTP would test the same code with more
///   ceremony (and, for the cap, hundreds of connections);
/// - the **HTTP surface** tested through the whole application router, for the
///   parts only it can answer: which status and headers a refusal carries, that
///   a browser gets the rendered page instead, and that the request's peer
///   address is what selects the bucket.
///
/// No test sleeps for minutes. The policy is a value ([`LockoutPolicy`]), so
/// these use a three-failure budget and a ~1 s cooldown; the state machine
/// itself runs on real `Instant`s — there is no stubbed clock anywhere — and
/// the one test that must see a lockout *expire* sleeps just past that
/// cooldown (~1.2 s in total for the module).
#[cfg(test)]
mod lockout_tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::HeaderValue;
    use sqlx::SqlitePool;
    use std::net::SocketAddr;

    /// The small policy every test drives the production state machine with.
    /// The cooldown is a whole number of seconds plus a little: `Retry-After`
    /// truncates to seconds, so a sub-second cooldown would advertise `1` and
    /// make the expiry assertion's timing the only thing under test.
    const TEST_BUDGET: u32 = 3;
    const TEST_COOLDOWN: Duration = Duration::from_millis(1100);
    const TEST_WINDOW: Duration = Duration::from_secs(60);
    const TEST_CAP: usize = 4;

    fn policy() -> LockoutPolicy {
        LockoutPolicy {
            budget: TEST_BUDGET,
            window: TEST_WINDOW,
            cooldown: TEST_COOLDOWN,
            max_sources: TEST_CAP,
        }
    }

    fn auth_with(policy: LockoutPolicy) -> Auth {
        Auth::with_lockout_policy(
            "evan".to_string(),
            Auth::hash_password("hunter2").unwrap(),
            Some("test-token".to_string()),
            false,
            policy,
        )
        .unwrap()
    }

    /// A tracker on its own, for the unit-level properties.
    fn tracker() -> LockoutTracker {
        LockoutTracker::with_policy(policy())
    }

    /// A distinct source per `n`, varying the peer's **IP** — the lockout keys
    /// on the address, so tests that mean "another source" must vary the
    /// address, not the port (which the port test below proves is ignored).
    fn peer(n: u16) -> Source {
        let n = u32::from(n);
        Source::of(Some(SocketAddr::from((
            [203, 0, ((n / 256) % 256) as u8, (n % 256) as u8],
            40001,
        ))))
    }

    /// One address with two different ports, for `the_source_is_the_ip_not_the_port`.
    fn peer_on_port(port: u16) -> Source {
        Source::of(Some(SocketAddr::from(([203, 0, 113, 7], port))))
    }

    // ---- The tracker itself ------------------------------------------------

    /// The source is the peer's **IP**, so a client cannot shake off its
    /// failures by reconnecting: every connection from one host lands in the
    /// same bucket however the kernel numbers its ephemeral ports.
    #[test]
    fn the_source_is_the_ip_not_the_port() {
        assert_eq!(
            peer_on_port(40001),
            peer_on_port(40002),
            "two connections from one host are one source"
        );
        let tracker = tracker();
        for _ in 0..TEST_BUDGET {
            assert_eq!(tracker.begin_attempt(peer_on_port(40001)), Attempt::Allowed);
        }
        assert!(
            matches!(
                tracker.begin_attempt(peer_on_port(40002)),
                Attempt::Refused { .. }
            ),
            "a fresh port must not be a fresh budget"
        );
    }

    /// An IPv6 source is its **/64 prefix**, not the full address: rotating the
    /// interface identifier (free — it is the host's own allocation) must not
    /// buy a fresh budget, while a genuinely different /64 still gets its own.
    #[test]
    fn an_ipv6_source_is_its_64_prefix() {
        let addr = |s: &str, port| SocketAddr::new(s.parse::<IpAddr>().unwrap(), port);
        assert_eq!(
            Source::of(Some(addr("2001:db8:1:2::1", 40001))),
            Source::of(Some(addr("2001:db8:1:2:ffff:ffff:ffff:ffff", 40002))),
            "every address in one /64 is one source"
        );
        assert_ne!(
            Source::of(Some(addr("2001:db8:1:2::1", 40001))),
            Source::of(Some(addr("2001:db8:1:3::1", 40001))),
            "a different /64 is a different source"
        );
        assert_eq!(
            Source::of(Some(addr("2001:db8:1:2::1", 40001))).label(),
            "2001:db8:1:2::/64",
            "the log line must name the prefix the key is"
        );
        // And the budget is spent on the prefix, not one address in it.
        let tracker = tracker();
        for _ in 0..TEST_BUDGET {
            assert_eq!(
                tracker.begin_attempt(Source::of(Some(addr("2001:db8:1:2:1:2:3:4", 40001)))),
                Attempt::Allowed
            );
        }
        assert!(matches!(
            tracker.begin_attempt(Source::of(Some(addr("2001:db8:1:2::9", 40003)))),
            Attempt::Refused { .. }
        ));
    }

    /// The budget is a count of *consecutive failures*: the first `budget`
    /// attempts are answered normally, and the source is locked out from the
    /// moment the budget is exhausted.
    #[test]
    fn a_source_is_locked_out_only_once_the_budget_is_exhausted() {
        let tracker = tracker();
        for attempt in 1..=TEST_BUDGET {
            assert_eq!(
                tracker.begin_attempt(peer(1)),
                Attempt::Allowed,
                "attempt {attempt} is within the budget and must be allowed"
            );
        }
        assert!(
            matches!(tracker.begin_attempt(peer(1)), Attempt::Refused { .. }),
            "the attempt after the budget must start the cooldown"
        );
        // A refused attempt changes nothing: the remaining cooldown is
        // unchanged, so hammering a locked-out source cannot extend its lock.
        let Attempt::Refused {
            retry_after_secs: first,
        } = tracker.begin_attempt(peer(1))
        else {
            panic!("a locked-out source must stay refused");
        };
        let Attempt::Refused {
            retry_after_secs: second,
        } = tracker.begin_attempt(peer(1))
        else {
            panic!("a locked-out source must stay refused");
        };
        assert!(
            second <= first,
            "a refused attempt must not extend the lockout"
        );
    }

    /// The gate **counts every attempt it lets through**, so the budget cannot
    /// be beaten by racing it: had the count been written after the verify (as
    /// it was), a burst of parallel guesses would all read a count below the
    /// budget before any wrote and the effective budget would be
    /// `budget + concurrency`.
    #[test]
    fn the_gate_counts_each_attempt_rather_than_checking_then_acting() {
        let tracker = tracker();
        let allowed = (0..TEST_BUDGET * 4)
            .filter(|_| tracker.begin_attempt(peer(1)) == Attempt::Allowed)
            .count();
        assert_eq!(
            allowed, TEST_BUDGET as usize,
            "exactly the budget may pass the gate, however many race it"
        );
    }

    /// The concurrency bound, driven concurrently — which the
    /// [`the_gate_counts_each_attempt_rather_than_checking_then_acting`] pin
    /// above deliberately does not (it races the gate's arithmetic, in one
    /// thread, and says so).
    ///
    /// Eight wrong passwords from eight distinct sources are submitted at once
    /// through the real `login_submit`, so each reaches
    /// [`Auth::verify_password_bounded`]. Every one of them must be answered,
    /// and the peak number of Argon2 verifies actually in flight — recorded
    /// inside the `spawn_blocking` closure, not inferred — must never exceed
    /// [`MAX_CONCURRENT_VERIFIES`]. Before the semaphore, that peak was however
    /// many attempts arrived together, each holding 19 MiB.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_concurrent_verifies_are_bounded() {
        use std::sync::atomic::Ordering::Relaxed;
        // A shared process-wide watermark, so start from what is there rather
        // than assuming this test is the only one that has ever verified.
        VERIFIES_PEAK.store(0, Relaxed);
        // The production policy, not the test-sized one: eight distinct
        // sources each spend their first attempt, so no lockout is in play and
        // every one of them reaches the verify.
        let auth = auth_with(LockoutPolicy::production());
        let mut attempts = Vec::new();
        for n in 0..8u16 {
            let auth = auth.clone();
            attempts.push(tokio::spawn(async move {
                login_submit(
                    auth,
                    String::new(),
                    peer(n + 1),
                    false,
                    LoginForm {
                        username: "evan".to_string(),
                        password: format!("wrong {n}"),
                    },
                )
                .await
                .status()
            }));
        }
        for attempt in attempts {
            let status = attempt.await.expect("the attempt task completes");
            assert!(
                // A wrong password re-renders the page with its error (200 — the
                // documented outcome), and a busy verify queue is the 429.
                status == StatusCode::OK || status == StatusCode::TOO_MANY_REQUESTS,
                "a concurrent wrong password is answered, not dropped: {status}"
            );
        }
        let peak = VERIFIES_PEAK.load(Relaxed);
        assert!(peak > 0, "the watermark saw no verify at all");
        assert!(
            peak <= MAX_CONCURRENT_VERIFIES,
            "{peak} Argon2 verifies ran at once, over the {MAX_CONCURRENT_VERIFIES} slots — each \
             one is ~19 MiB of transient memory on an unauthenticated path"
        );
    }

    /// `Retry-After` rounds the remaining cooldown **up**: 1100 ms must
    /// advertise 2 whole seconds, never a truncated 1 that a client honouring
    /// the header would retry into a refusal.
    #[test]
    fn a_retry_after_rounds_the_remaining_cooldown_up() {
        let tracker = tracker();
        for _ in 0..TEST_BUDGET {
            let _ = tracker.begin_attempt(peer(1));
        }
        match tracker.begin_attempt(peer(1)) {
            Attempt::Refused { retry_after_secs } => assert_eq!(
                retry_after_secs, 2,
                "1100 ms of cooldown advertises 2 seconds, rounded up"
            ),
            Attempt::Allowed => panic!("the source is locked out"),
        }
    }

    /// One source's failures must not lock out another — the whole point of
    /// keying on the peer.
    #[test]
    fn one_sources_failures_do_not_lock_out_another() {
        let tracker = tracker();
        for _ in 0..TEST_BUDGET {
            let _ = tracker.begin_attempt(peer(1));
        }
        assert!(
            matches!(tracker.begin_attempt(peer(1)), Attempt::Refused { .. }),
            "source 1 is locked"
        );
        assert_eq!(
            tracker.begin_attempt(peer(2)),
            Attempt::Allowed,
            "source 2 has made no attempt and must not be locked out by source 1's"
        );
        assert_eq!(
            tracker.begin_attempt(Source::Unknown),
            Attempt::Allowed,
            "the no-peer bucket is its own source, not a shared one"
        );
    }

    /// A success clears the streak, so ordinary mistyping costs nothing.
    #[test]
    fn a_success_before_the_budget_clears_the_failure_streak() {
        let tracker = tracker();
        for _ in 0..TEST_BUDGET - 1 {
            let _ = tracker.begin_attempt(peer(1));
        }
        tracker.record_success(peer(1));
        // A fresh streak of budget-1 failures still leaves the source free: the
        // next (budget'th) attempt is answered, where a count that had survived
        // the success would already be refusing it.
        for _ in 0..TEST_BUDGET - 1 {
            assert_eq!(tracker.begin_attempt(peer(1)), Attempt::Allowed);
        }
        assert_eq!(
            tracker.begin_attempt(peer(1)),
            Attempt::Allowed,
            "the successful attempt must have reset the count"
        );
    }

    /// The lockout is a *cooldown*, not a permanent lock: once it has elapsed
    /// the source is free again and its streak has been forgotten.
    #[test]
    fn a_lockout_expires_back_to_a_clean_slate() {
        let tracker = tracker();
        for _ in 0..TEST_BUDGET {
            let _ = tracker.begin_attempt(peer(1));
        }
        assert!(matches!(
            tracker.begin_attempt(peer(1)),
            Attempt::Refused { .. }
        ));
        std::thread::sleep(TEST_COOLDOWN + Duration::from_millis(100));
        // A clean slate: a whole fresh budget is allowed before the lock trips
        // again, where a surviving count would have tripped on the first.
        for attempt in 1..=TEST_BUDGET {
            assert_eq!(
                tracker.begin_attempt(peer(1)),
                Attempt::Allowed,
                "attempt {attempt} after the cooldown is inside a fresh budget"
            );
        }
        assert!(matches!(
            tracker.begin_attempt(peer(1)),
            Attempt::Refused { .. }
        ));
    }

    /// The failure window is **idle-based**: each attempt pushes its end out,
    /// so a streak survives while attempts continue and expires only after the
    /// window passes with none. The two halves below pin both directions — a
    /// streak kept alive across gaps shorter than the window still reaches the
    /// budget, and one left idle for longer is dropped.
    ///
    /// The window is short and the gaps well inside it, so ordinary scheduler
    /// jitter cannot flip either assertion.
    #[test]
    fn the_failure_window_is_idle_based() {
        let mut policy = policy();
        policy.budget = 12;
        policy.window = Duration::from_millis(500);
        policy.cooldown = Duration::from_secs(60);

        // Kept alive: twelve attempts 60 ms apart span ~720 ms from the first —
        // past the window — so a window fixed at the first failure would have
        // dropped the early ones and never tripped.
        let tracker = LockoutTracker::with_policy(policy);
        for attempt in 1..=12 {
            assert_eq!(
                tracker.begin_attempt(peer(1)),
                Attempt::Allowed,
                "attempt {attempt} is inside every 500 ms window"
            );
            std::thread::sleep(Duration::from_millis(60));
        }
        assert!(
            matches!(tracker.begin_attempt(peer(1)), Attempt::Refused { .. }),
            "a streak kept alive by attempts must reach the budget"
        );

        // Dropped: one failure, then silence for longer than the window, leaves
        // a full fresh budget.
        let tracker = LockoutTracker::with_policy(policy);
        assert_eq!(tracker.begin_attempt(peer(1)), Attempt::Allowed);
        std::thread::sleep(Duration::from_millis(700));
        for attempt in 1..=12 {
            assert_eq!(
                tracker.begin_attempt(peer(1)),
                Attempt::Allowed,
                "attempt {attempt} of the fresh budget after an idle window"
            );
        }
        assert!(matches!(
            tracker.begin_attempt(peer(1)),
            Attempt::Refused { .. }
        ));
    }

    /// The state is **bounded**: driving many more distinct sources than the
    /// cap leaves the table at the cap, never at the number of sources seen.
    /// A lockout table an attacker can grow is a memory-exhaustion hole, and
    /// the attacker chooses the source of every attempt.
    #[test]
    fn the_tracker_never_grows_past_its_cap() {
        let tracker = tracker();
        for port in 1..=(TEST_CAP as u16 * 25) {
            let _ = tracker.begin_attempt(peer(port));
        }
        assert_eq!(
            tracker.len(),
            TEST_CAP,
            "the tracker must be capped at {TEST_CAP} sources, not grow with traffic"
        );
        // Eviction drops the least-recently-seen source, so the newest
        // sources are the ones that survive: a source that just failed is
        // still tracked (a burst of new sources cannot silently un-lock one
        // that is actively being attacked).
        let last = peer(TEST_CAP as u16 * 25);
        for _ in 0..TEST_BUDGET {
            let _ = tracker.begin_attempt(last);
        }
        assert!(
            matches!(tracker.begin_attempt(last), Attempt::Refused { .. }),
            "the most recently seen source must still be tracked"
        );
        assert!(tracker.len() <= TEST_CAP);
    }

    /// The documented consequence of capping the table: the bound wins over the
    /// lockout. A flood of distinct sources inside the failure window evicts an
    /// actively locked-out entry (there is no bound on memory that is also a
    /// bound on every source), and the tracker stays capped while it happens.
    /// This is pinned rather than hidden because the module doc and
    /// `docs/API.md` both state it.
    #[test]
    fn a_flood_of_new_sources_can_evict_a_locked_out_one() {
        let tracker = tracker();
        let locked = peer(1);
        for _ in 0..TEST_BUDGET {
            let _ = tracker.begin_attempt(locked);
        }
        assert!(
            matches!(tracker.begin_attempt(locked), Attempt::Refused { .. }),
            "sanity: locked out"
        );

        // More distinct sources than the cap, each seen after the locked one.
        for port in 100..100 + TEST_CAP as u16 + 1 {
            let _ = tracker.begin_attempt(peer(port));
        }
        assert_eq!(tracker.len(), TEST_CAP, "the cap still holds");
        assert_eq!(
            tracker.begin_attempt(locked),
            Attempt::Allowed,
            "the least-recently-seen source was evicted, as the docs say it can be"
        );
    }

    // ---- The HTTP surface --------------------------------------------------

    /// The whole application, auth on, over the tracker's small policy — as
    /// `main` builds it when `[auth]` is configured, except for the policy.
    fn client(pool: &SqlitePool, auth: Auth) -> ApiClient {
        let fetcher = crate::entities::closing_price::test_support::QuoteStub::default().shared();
        let registry = crate::infra::scheduler::registry(
            pool.clone(),
            ":memory:".to_string(),
            None,
            None,
            fetcher.clone(),
            crate::entities::distribution_event::test_support::DistributionStub::default().shared(),
            None,
        );
        ApiClient::over(crate::app::router(
            "",
            pool.clone(),
            registry,
            fetcher,
            Some(auth),
        ))
    }

    /// The login form body, and the browser's own `Accept` header — the pair
    /// the sign-in page submits.
    const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";
    fn login_body(password: &str) -> String {
        format!("username=evan&password={password}")
    }

    /// `POST /login` for the password given, as a person's browser sends it
    /// (`Accept: text/html`). Passwords in this module are test inputs that can
    /// only ever fail against the Argon2 hash, never credentials — the same
    /// CodeQL `rust/hard-coded-cryptographic-value` false positive documented
    /// on `a_control_character_in_a_failed_login_username_cannot_split_the_log_line`.
    async fn browser_login(c: &ApiClient, password: &str) -> crate::test_support::ApiResponse {
        c.clone()
            .with_header("Accept", "text/html")
            .post_bytes("/login", Some(FORM_CONTENT_TYPE), login_body(password))
            .await
    }

    /// `POST /login` as a script or LLM client sends it: no `Accept:
    /// text/html`, so a refusal is the machine-readable `429`.
    async fn script_login(c: &ApiClient, password: &str) -> crate::test_support::ApiResponse {
        c.post_bytes("/login", Some(FORM_CONTENT_TYPE), login_body(password))
            .await
    }

    /// The whole lockout sequence over one source, driven through HTTP:
    ///
    /// 1. the first `budget - 1` wrong attempts answer exactly what the
    ///    wrong-credentials path answers today (a `200` re-render);
    /// 2. the attempt that exhausts the budget is still answered normally —
    ///    the budget counts failures, so it takes that many to trip — and every
    ///    attempt after it answers `429` with the reason and a `Retry-After`;
    /// 3. during the lockout a **correct** password is still refused `429` —
    ///    the password is not even checked, which is the point;
    /// 4. once the cooldown elapses a correct password signs in (`303` with a
    ///    session cookie) and the counter is cleared, so a following wrong
    ///    attempt starts counting from one again.
    #[tokio::test]
    async fn the_login_lockout_refuses_by_source_with_429_and_retry_after() {
        let pool = test_pool().await;
        let client = client(&pool, auth_with(policy())).with_peer(([203, 0, 113, 7], 4001).into());

        for attempt in 1..=TEST_BUDGET {
            let resp = script_login(&client, "definitely-wrong").await;
            assert_eq!(
                resp.status,
                StatusCode::OK,
                "attempt {attempt} is within the budget and must answer the ordinary \
                 wrong-credentials response; body: {}",
                resp.text()
            );
            assert!(resp.text().contains("Incorrect username or password."));
            assert!(resp.headers.get(header::SET_COOKIE).is_none());
        }

        // The budget is now exhausted, so the *next* attempt is refused — and
        // this one is a failed attempt like any other, so the budget counts it
        // before the cooldown starts.
        let exhausting = script_login(&client, "definitely-wrong").await;
        assert_eq!(exhausting.status, StatusCode::TOO_MANY_REQUESTS);
        let retry_after = exhausting
            .headers
            .get(header::RETRY_AFTER)
            .expect("a 429 names how long to wait")
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(retry_after, "2");
        assert!(
            exhausting
                .text()
                .contains("Too many failed sign-in attempts."),
            "the 429 body must carry the reason: {}",
            exhausting.text()
        );
        assert!(exhausting.headers.get(header::SET_COOKIE).is_none());

        // The next attempt — and, crucially, one with the *right* password —
        // is still refused: a correct password must not reset an active
        // lockout, and a refusal never reaches the hash.
        for password in ["definitely-wrong", "hunter2"] {
            let resp = script_login(&client, password).await;
            assert_eq!(
                resp.status,
                StatusCode::TOO_MANY_REQUESTS,
                "a locked-out source must be refused whatever password it presents"
            );
            assert_eq!(
                resp.headers.get(header::RETRY_AFTER).unwrap(),
                &HeaderValue::from_static("2")
            );
        }

        // Wait out the (test-sized) cooldown: the correct password now signs
        // in, cookie and all.
        tokio::time::sleep(TEST_COOLDOWN + Duration::from_millis(250)).await;
        let signed_in = script_login(&client, "hunter2").await;
        assert_eq!(
            signed_in.status,
            StatusCode::SEE_OTHER,
            "the cooldown has elapsed; a correct password must sign in. body: {}",
            signed_in.text()
        );
        assert!(signed_in.headers.get(header::SET_COOKIE).is_some());

        // The success cleared the streak: exactly budget failures from the
        // same source are allowed again, and the attempt after them locks it
        // out — so the sign-in above reset the count rather than the cooldown
        // simply carrying it away. (The attempt that *reaches* the budget is
        // still answered normally; only the one after it is refused, which is
        // why the first assertion below expects OK from all `budget` of them.)
        for attempt in 1..=TEST_BUDGET {
            assert_eq!(
                script_login(&client, "definitely-wrong").await.status,
                StatusCode::OK,
                "attempt {attempt} of a freshly cleared budget must be answered normally"
            );
        }
        assert_eq!(
            script_login(&client, "definitely-wrong").await.status,
            StatusCode::TOO_MANY_REQUESTS,
            "a cleared streak must count from one, so the budget+1'th failure is refused"
        );
    }

    /// The lockout keys on the request's **peer address**, not on the
    /// connection at large: one source exhausting the budget leaves another
    /// entirely unaffected, and the peer reaches the handler through the same
    /// `ConnectInfo<SocketAddr>` extension a real listener installs.
    #[tokio::test]
    async fn the_lockout_is_per_source_not_per_server() {
        let pool = test_pool().await;
        let auth = auth_with(policy());
        let attacker = client(&pool, auth.clone()).with_peer(([203, 0, 113, 7], 4001).into());
        let innocent = client(&pool, auth).with_peer(([198, 51, 100, 9], 4002).into());

        for _ in 0..TEST_BUDGET {
            script_login(&attacker, "definitely-wrong").await;
        }
        assert_eq!(
            script_login(&attacker, "definitely-wrong").await.status,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            script_login(&innocent, "hunter2").await.status,
            StatusCode::SEE_OTHER,
            "another source's failures must not lock this one out"
        );
    }

    /// A successful login *before* the budget is exhausted clears the count,
    /// so two mistyped attempts followed by a successful one do not leave the
    /// source one failure from a lockout.
    #[tokio::test]
    async fn a_successful_login_before_the_budget_clears_the_count() {
        let pool = test_pool().await;
        let client = client(&pool, auth_with(policy())).with_peer(([203, 0, 113, 7], 4001).into());

        for _ in 0..TEST_BUDGET - 1 {
            assert_eq!(
                script_login(&client, "definitely-wrong").await.status,
                StatusCode::OK
            );
        }
        assert_eq!(
            script_login(&client, "hunter2").await.status,
            StatusCode::SEE_OTHER
        );
        // A full fresh budget-1 of failures is still allowed.
        for _ in 0..TEST_BUDGET - 1 {
            assert_eq!(
                script_login(&client, "definitely-wrong").await.status,
                StatusCode::OK,
                "the successful login must have reset the failure count"
            );
        }
    }

    /// The browser path: an HTML login POST during a lockout is refused
    /// **`429`** — so the refusal is machine-visible in an access log or a
    /// fail2ban rule however the client spells `Accept` — carrying the sign-in
    /// page as its body, never a bare plain-text reason a browser would render
    /// as text.
    #[tokio::test]
    async fn a_locked_out_browser_gets_a_429_carrying_the_login_page() {
        let pool = test_pool().await;
        let client = client(&pool, auth_with(policy())).with_peer(([203, 0, 113, 7], 4001).into());

        // The ordinary wrong-credentials browser path, for contrast: a 200
        // rendered page with its own message.
        let first = browser_login(&client, "definitely-wrong").await;
        assert_eq!(first.status, StatusCode::OK);
        assert!(first.text().contains("Incorrect username or password."));

        for _ in 2..=TEST_BUDGET {
            browser_login(&client, "definitely-wrong").await;
        }
        let locked = browser_login(&client, "hunter2").await;
        assert_eq!(
            locked.status,
            StatusCode::TOO_MANY_REQUESTS,
            "the refusal must be a 429 whatever the Accept header says"
        );
        assert_eq!(
            locked.headers.get(header::RETRY_AFTER).unwrap(),
            &HeaderValue::from_static("2"),
            "the browser path carries the same Retry-After as the script one"
        );
        let body = locked.text();
        assert!(
            body.contains("Too many failed sign-in attempts."),
            "the login page must carry the lockout message: {body}"
        );
        assert!(
            body.contains("<title>share-tracker — sign in</title>"),
            "the lockout message must be rendered in the sign-in page itself: {body}"
        );
        assert!(
            !body.contains("Incorrect username or password."),
            "the lockout page must not claim the credentials were wrong: {body}"
        );
        assert!(locked.headers.get(header::SET_COOKIE).is_none());

        // The exact response the documented Error-body contract's `429` row
        // (and its `doc_checks` pin) describe, from the code side.
        let refused = ApiError::too_many_requests(lockout_message(300), 300).into_response();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            refused.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8"
        );
        assert_eq!(refused.headers().get(header::RETRY_AFTER).unwrap(), "300");
    }

    /// A request with **no** connect info — the in-process client, and any
    /// listener not built with `into_make_service_with_connect_info` — is not
    /// rejected: it shares the one documented [`Source::Unknown`] bucket, so
    /// the lockout still works there, just coarsely.
    #[tokio::test]
    async fn requests_without_connect_info_share_one_documented_bucket() {
        let pool = test_pool().await;
        let client = client(&pool, auth_with(policy()));

        for _ in 0..TEST_BUDGET {
            assert_eq!(
                script_login(&client, "definitely-wrong").await.status,
                StatusCode::OK
            );
        }
        assert_eq!(
            script_login(&client, "hunter2").await.status,
            StatusCode::TOO_MANY_REQUESTS,
            "the absent-peer bucket is a source like any other"
        );
    }
}
