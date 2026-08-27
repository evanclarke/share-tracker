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
//! Failed logins are throttled by Argon2 itself (~30 ms/attempt on ordinary
//! hardware, i.e. tens of attempts/sec/core): there is deliberately no
//! separate lockout counter for a single-credential hobbyist deployment.
//! Argon2 runs on login only; every other authenticated request costs one
//! HMAC verification.

use crate::infra::http::ApiError;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use sqlx::SqlitePool;
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

const LOGIN_HTML: &str = include_str!("auth/login.html");
const COOKIE_NAME: &str = "st_session";
const SESSION_CONTEXT: &[u8] = b"share-tracker session v1";
/// How long a session cookie is valid for. Not a config knob — one fewer
/// setting to get wrong; sign in again after 30 days.
const SESSION_LIFETIME_SECS: u64 = 30 * 24 * 60 * 60;

/// The resolved `[auth]` configuration. Cheap to clone (a couple of
/// `String`s, a bool, a 32-byte key) — cloned into every handler/middleware
/// closure that needs it, the same way `SharedFetcher` is. `Debug`/`PartialEq`
/// exist only so `config::Settings` (which derives both) can keep doing so;
/// the only place either fires is a test assertion's failure message.
#[derive(Clone, Debug, PartialEq)]
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
        })
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
    fn verify_password(&self, username: &str, password: &str) -> bool {
        if username != self.username {
            return false;
        }
        let Ok(hash) = PasswordHash::new(&self.password_hash) else {
            return false; // unreachable: validated in `new`
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
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
    if !s.len().is_multiple_of(2) {
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginForm {
    username: String,
    password: String,
}

/// `GET /login`, `POST /login` and `POST /logout` — merged into the app
/// alongside `web::router` only when `[auth]` is configured, so these routes
/// (and the `Auth` they need) simply don't exist otherwise.
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
            .post(
                move |axum::extract::Form(form): axum::extract::Form<LoginForm>| {
                    let auth = post_auth.clone();
                    let base_path = post_base.clone();
                    async move { login_submit(auth, base_path, form).await }
                },
            ),
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
            .replace("{{ERROR}}", &error_html),
    )
}

async fn login_submit(auth: Auth, base_path: String, form: LoginForm) -> Response {
    if auth.verify_password(&form.username, &form.password) {
        tracing::info!(username = %form.username, "login succeeded");
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
        // Named by attempted username rather than peer address: the app has
        // no `ConnectInfo` plumbing (added only for this it would also have
        // to reach the in-process test harness, which has no real socket),
        // and behind the documented reverse-proxy deployment the peer is
        // always 127.0.0.1 regardless — the attempted username is the more
        // useful signal here.
        tracing::warn!(username = %form.username, "login failed: wrong username or password");
        render_login(&base_path, Some("Incorrect username or password.")).into_response()
    }
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
            StatusCode::NO_CONTENT
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
