//! Assembles the full axum application from the entity, report, and infra
//! routers. Kept separate from `main` so the wiring is unit-testable.
use crate::infra::auth::AuthState;
use crate::infra::request_guard::RequestGuard;
use crate::infra::{auth, http, request_guard, security_headers};
use crate::{api_spec, entities, reports, web};
use axum::response::Redirect;
use axum::{Extension, Router, middleware, routing};
use sqlx::SqlitePool;
use tower_http::catch_panic::CatchPanicLayer;
use utoipa_axum::router::OpenApiRouter;

use crate::entities::closing_price::SharedFetcher;
use crate::infra::auth::{Auth, AuthMode};
use crate::infra::config::BasePath;
use crate::infra::scheduler::{self, JobRegistry};

/// Build the HTTP router: the web frontend plus all entity and report routes and
/// scheduler inspection, sharing the pool as state and the job registry as an
/// extension. The price fetcher is injected (not constructed here) and layered
/// as an extension, so the live `YahooFetcher` only reaches the router from
/// `main`; tests pass a stub `SharedFetcher` and never touch the network.
///
/// `base_path` is the reverse-proxy path prefix from
/// [`crate::infra::config::Settings`] — [`BasePath::ROOT`] (the default) mounts everything at
/// the root exactly as before, and e.g. `/share_tracker` nests the *whole*
/// application under that prefix. Nesting rather than having the proxy strip
/// the prefix keeps routing and the URLs the frontend emits in agreement: the
/// app serves the same paths the browser asks for, so a prefixed deployment is
/// reachable (and testable) without a proxy in front of it.
///
/// `auth` is `None` (the default) when `[auth]` is absent from config: the
/// whole surface then serves exactly as it did before `infra::auth` existed —
/// no `/login` route, no gate on anything else. `Some` merges the login/logout
/// routes and applies `infra::auth::require_auth` in front of everything else
/// merged here (including the web frontend and its `/static` assets, except
/// the small unauthenticated allowlist `require_auth` itself carves out).
///
/// `allowed_hosts` is the config file's list of extra names the server may be
/// reached by when `[auth]` is absent — see [`crate::infra::request_guard`],
/// which is layered outside everything else (the login page included) whatever
/// `auth` is.
pub fn router(
    base_path: &BasePath,
    pool: SqlitePool,
    registry: JobRegistry,
    fetcher: SharedFetcher,
    auth: Option<Auth>,
    allowed_hosts: &[String],
) -> Router {
    let mode = AuthMode::of(auth.as_ref());
    let guard = RequestGuard::new(mode, allowed_hosts);
    // One registration per route: `split_for_parts` hands back the axum router
    // and the OpenAPI operations utoipa generated from the very same
    // `#[utoipa::path]` handlers, which `api_spec` completes into the served
    // document — so a route cannot be served undocumented, or documented
    // unserved.
    let (app, generated) = api_routes(base_path, mode).split_for_parts();
    let mut app = app.merge(api_spec::router(generated, base_path, mode));
    if let Some(auth) = &auth {
        // What the login routes need — the credential and the base path the
        // redirects are built from — as the extension their handlers read.
        app = app.layer(Extension(AuthState::new(auth.clone(), base_path.clone())));
    }
    let app = app
        .with_state(pool)
        .layer(Extension(registry))
        .layer(Extension(fetcher));
    // Applied after the two `Extension` layers (order between `Extension`
    // layers never matters) and, critically, before the `nest` below: `nest`
    // strips `base_path` from the request before delegating to `app`, so
    // `require_auth` — layered onto `app` itself — sees the *unprefixed*
    // path, matching the plain `"/login"`/`"/static/style.css"` strings its
    // allowlist checks against.
    let app = match auth {
        Some(auth) => app.layer(middleware::from_fn_with_state(
            AuthState::new(auth, base_path.clone()),
            auth::require_auth,
        )),
        None => app,
    };
    if base_path.is_root() {
        return catching_panics(guarded(app, guard));
    }
    // `nest` matches the prefix itself (`/share_tracker`) and everything below
    // it (`/share_tracker/listings`), but *not* the bare prefix with a trailing
    // slash: `/share_tracker/` leaves an empty remainder, which matches neither
    // pattern. That is precisely the URL a person types and the one an nginx
    // `location /share_tracker/` block forwards, so it cannot be left as a 404
    // — redirect it onto the prefix the nested router does serve. Temporary,
    // not permanent: `base_path` is configuration, and a browser must not cache
    // a redirect derived from it past a change.
    let app = Router::new()
        .route(
            &format!("{base_path}/"),
            routing::any({
                let target = base_path.to_string();
                move || {
                    let target = target.clone();
                    async move { Redirect::temporary(&target) }
                }
            }),
        )
        .nest(base_path.as_str(), app);
    catching_panics(guarded(app, guard))
}

/// Every documented route the application serves — the entity, report,
/// scheduler and web-frontend routers, plus the login routes when `[auth]` is
/// configured — as the one `OpenApiRouter` [`router`] serves and
/// `api_spec` documents. The login routes are merged without the
/// [`AuthState`](crate::infra::auth::AuthState) they read, which [`router`]
/// layers on, so the document can be generated without a credential.
pub fn api_routes(base_path: &BasePath, mode: AuthMode) -> OpenApiRouter<SqlitePool> {
    let routes = entities::router()
        .merge(reports::router())
        .merge(scheduler::router())
        .merge(web::router(base_path, mode));
    match mode {
        AuthMode::Required => routes.merge(auth::routes()),
        AuthMode::Open => routes,
    }
}

/// The request guard ([`crate::infra::request_guard`]) around the whole
/// served surface: outside `require_auth` and the base-path redirect, so a
/// cross-site write or an unlisted `Host` is refused before anything else
/// runs, and inside only the panic net.
///
/// Outside the guard in turn sit the browser-hardening response headers
/// ([`crate::infra::security_headers`]), so a refusal carries them too.
fn guarded(app: Router, guard: RequestGuard) -> Router {
    app.layer(middleware::from_fn_with_state(guard, request_guard::guard))
        .layer(middleware::map_response(security_headers::apply))
}

/// The outermost layer of both shapes [`router`] returns: a panicking handler
/// answers a logged, empty-bodied `500` instead of dropping the connection
/// with no status at all (SCENARIOS W-b — a mistyped parcel whose cost-base
/// arithmetic overflowed `rust_decimal` killed every portfolio read, and the
/// web UI could only show a bare network error naming nothing).
///
/// Outermost deliberately: applied here rather than to the inner `app`, it
/// also covers `require_auth` and the base-path redirect route, so no part of
/// the served surface can panic its way past it. The response itself is
/// [`crate::infra::http::panic_response`], which matches
/// `ApiError::Internal`'s convention exactly — `tracing::error!` naming the
/// panic, nothing of it in the body.
///
/// A panic is still a bug, not a supported outcome: this is the net under the
/// arithmetic fixes, not a substitute for them.
fn catching_panics(app: Router) -> Router {
    app.layer(CatchPanicLayer::custom(http::panic_response))
}

#[cfg(test)]
mod tests {
    use crate::entities::closing_price::test_support::QuoteStub;
    use crate::entities::distribution_event::test_support::DistributionStub;
    use crate::infra::auth::{Auth, CookieSecurity};
    use crate::infra::config::BasePath;
    use crate::infra::scheduler;
    use crate::test_support::{ApiClient, ApiResponse, TEST_PASSWORD, test_pool};
    use crate::{test_support, web};
    use axum::http::{StatusCode, header};
    use axum::routing;
    use sqlx::SqlitePool;

    /// The full application mounted under a reverse-proxy prefix, as `main`
    /// builds it when `base_path` is set. `auth` mirrors `router`'s own
    /// parameter — `None` for the base-path-only tests below,
    /// `Some` for the one combining a prefix with `[auth]` configured.
    fn prefixed(pool: &SqlitePool, auth: Option<Auth>) -> ApiClient {
        let fetcher = QuoteStub::default().shared();
        let registry = scheduler::registry(
            pool.clone(),
            ":memory:".to_owned(),
            None,
            None,
            fetcher.clone(),
            DistributionStub::default().shared(),
            None,
        );
        ApiClient::over(super::router(
            &BasePath::of("/share_tracker"),
            pool.clone(),
            registry,
            fetcher,
            auth,
            &[],
        ))
    }

    /// SCENARIOS W-b, the layer on its own: a handler that panics answers the
    /// same logged, empty-bodied `500` an `ApiError::Internal` does, instead
    /// of unwinding out of the connection with no status at all.
    ///
    /// The panicking route is built here rather than wired into the
    /// production router — nothing serving real data should be able to panic
    /// on purpose — but the layer under test is [`super::catching_panics`]
    /// itself, the one call `router` makes, so the two can't drift.
    ///
    /// Expect one `deliberate panic in a test route` line on stderr while
    /// this runs: that is Rust's default panic hook, which still fires before
    /// the unwind is caught. The test passing is the point, not the line.
    #[tokio::test]
    async fn a_panicking_handler_answers_500_rather_than_dropping_the_connection() {
        async fn boom() -> StatusCode {
            panic!("deliberate panic in a test route")
        }
        let app = super::catching_panics(axum::Router::new().route("/boom", routing::get(boom)));

        let resp = ApiClient::over(app).get("/boom").await;
        assert_eq!(resp.status, StatusCode::INTERNAL_SERVER_ERROR);
        // Empty, like every other `500`: a panic payload can carry anything.
        assert_eq!(resp.text(), "");
    }

    /// SCENARIOS W-b end to end, through the application `main` serves. The
    /// finding's own trade — a fat-fingered run of zeros in both
    /// `average_price` and `quantity` — has a `price × quantity` of 1e30, past
    /// `rust_decimal`'s ~7.9228e28 ceiling. That product *is* the parcel's
    /// cost base, so no reordering can produce it and `Parcel::initial_cost`
    /// still panics — but the read answers a `500` the UI can show a message
    /// for, where before the connection was reset and curl reported `000`.
    ///
    /// W-e has since closed the *write* half: `trade::check_amounts` refuses
    /// such a trade `422`, so the row can only reach the database from a build
    /// that predates the guard — which is what
    /// `insert_parcel_bypassing_checks` reproduces here. The panic layer is
    /// what such a database still relies on, so this stays the test of it.
    #[tokio::test]
    async fn an_unrepresentable_parcel_cost_base_is_a_500_not_a_dead_connection() {
        let pool = test_pool().await;
        test_support::listing(9).insert(&pool).await;
        test_support::insert_parcel_bypassing_checks(
            &pool,
            9600,
            9,
            test_support::ymd(2024, 3, 15),
            "1000000000000000",
            "1000000000000000",
        )
        .await;

        let client = ApiClient::full(&pool);
        let resp = client.get("/portfolio/open_parcels").await;
        assert_eq!(resp.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(resp.text(), "");
        // The control the finding measured: the reports that never form the
        // product were answering all along, and still do.
        assert_eq!(client.get("/reports/health").await.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_base_path_moves_the_whole_application_under_the_prefix() {
        let pool = test_pool().await;
        let client = prefixed(&pool, None);

        // The SPA shell at the prefix itself.
        let resp = client.get("/share_tracker").await;
        assert_eq!(resp.status, StatusCode::OK);
        assert!(resp.text().contains("<title>share-tracker</title>"));
        // …and the trailing-slash form — what a person types, and what an nginx
        // `location /share_tracker/` block forwards — redirected onto it rather
        // than 404ing on `nest`'s empty remainder.
        let resp = client.get("/share_tracker/").await;
        assert_eq!(resp.status, StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            resp.headers.get(header::LOCATION).unwrap(),
            "/share_tracker"
        );
        // A static asset, an entity route, a report route, the scheduler
        // routes, and the OpenAPI description — one from each merged router,
        // so nesting covers all of them.
        for uri in [
            "/share_tracker/static/app.js",
            "/share_tracker/listings",
            "/share_tracker/reports/health",
            "/share_tracker/jobs",
            "/share_tracker/openapi.json",
        ] {
            assert_eq!(client.get(uri).await.status, StatusCode::OK, "{uri}");
        }
    }

    #[tokio::test]
    async fn the_unprefixed_paths_are_gone_when_a_base_path_is_set() {
        // Nothing is served at the root: the proxy passes the prefix through,
        // so a request without it is not this application's.
        let pool = test_pool().await;
        let client = prefixed(&pool, None);
        for uri in ["/", "/static/app.js", "/listings", "/jobs"] {
            assert_eq!(client.get(uri).await.status, StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn an_empty_base_path_serves_everything_at_the_root() {
        // The default: byte-for-byte the pre-base-path behaviour. Also pins
        // the `auth: None` default end to end: `ApiClient::full` passes
        // `None`, so every route (including ones an auth-enabled deployment
        // would gate) is open with no credential in play.
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        for uri in ["/", "/static/app.js", "/listings", "/jobs"] {
            assert_eq!(client.get(uri).await.status, StatusCode::OK, "{uri}");
        }
    }

    #[tokio::test]
    async fn no_auth_configured_means_no_login_route_at_all() {
        // `infra::auth::router` is only merged in when `auth` is `Some` — so
        // with the default `None`, `/login` isn't a 401/redirect, it simply
        // doesn't exist.
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        assert_eq!(client.get("/login").await.status, StatusCode::NOT_FOUND);
    }

    /// The anti-framing headers (`infra::security_headers`) on the three
    /// kinds of response a framing page could aim at: the app shell, a JSON
    /// route, and — with `[auth]` on, under a base path — the login page and
    /// the unauthenticated refusal in front of everything else.
    #[tokio::test]
    async fn every_response_refuses_to_be_framed() {
        fn assert_hardened(resp: &ApiResponse, what: &str) {
            let header = |name| {
                resp.headers
                    .get(name)
                    .unwrap_or_else(|| panic!("{what}: no {name}"))
                    .to_str()
                    .unwrap()
                    .to_owned()
            };
            assert_eq!(header("x-frame-options"), "DENY", "{what}");
            assert_eq!(header("x-content-type-options"), "nosniff", "{what}");
            let csp = header("content-security-policy");
            for directive in [
                "default-src 'self'",
                "frame-ancestors 'none'",
                "object-src 'none'",
            ] {
                assert!(csp.contains(directive), "{what}: {csp}");
            }
        }
        let pool = test_pool().await;
        let open = ApiClient::full(&pool);
        let shell = open.get("/").await;
        assert_eq!(shell.status, StatusCode::OK);
        assert_hardened(&shell, "the app shell");
        let json = open.get("/listings").await;
        assert_eq!(json.status, StatusCode::OK);
        assert_hardened(&json, "a JSON route");
        // A request the guard refuses never reaches a handler, and is still
        // covered: the layer sits outside the guard.
        let refused = ApiClient::full(&pool)
            .with_header("Host", "evil.example")
            .get("/listings")
            .await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_hardened(&refused, "a guard refusal");

        let gated = prefixed(&pool, Some(test_auth()));
        let login = gated.get("/share_tracker/login").await;
        assert_eq!(login.status, StatusCode::OK);
        assert_hardened(&login, "the login page");
        let unauthenticated = gated.get("/share_tracker/listings").await;
        assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);
        assert_hardened(&unauthenticated, "an unauthenticated refusal");
    }

    /// The inline theme bootstrap both pages carry is the one inline script
    /// the policy allows, by hash — the shell and the login page each embed
    /// the exact text the policy's hash was computed over.
    #[tokio::test]
    async fn the_policy_allows_the_inline_theme_script_both_pages_embed() {
        let pool = test_pool().await;
        let gated = prefixed(&pool, Some(test_auth()));
        let login = gated.get("/share_tracker/login").await;
        let shell = ApiClient::full(&pool).get("/").await;
        for resp in [&login, &shell] {
            assert!(resp.text().contains(web::THEME_BOOT_SCRIPT));
            let csp = resp.headers["content-security-policy"].to_str().unwrap();
            assert!(csp.contains("script-src 'self' 'sha256-"), "{csp}");
        }
    }

    /// The one response the app frames itself — an attachment rendered in
    /// the edit form's document pane — may be framed by this origin, and by
    /// no other.
    #[tokio::test]
    async fn an_attachment_may_be_framed_by_the_app_itself() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1).insert(&pool).await;
        let boundary = "X-TEST-BOUNDARY";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"trade_id\"\r\n\r\n1\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.pdf\"\r\n\
             Content-Type: application/pdf\r\n\r\n%PDF-1.4\r\n--{boundary}--\r\n"
        );
        let client = ApiClient::full(&pool);
        let created = client
            .post_bytes(
                "/attachments",
                Some(&format!("multipart/form-data; boundary={boundary}")),
                body,
            )
            .await
            .expect_status(StatusCode::CREATED)
            .json::<serde_json::Value>();
        let resp = client
            .get(format!(
                "/attachments/{}/content?disposition=inline",
                created["id"]
            ))
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(resp.headers["x-frame-options"], "SAMEORIGIN");
        assert_eq!(
            resp.headers["content-security-policy"],
            "frame-ancestors 'self'"
        );
    }

    fn test_auth() -> Auth {
        Auth::new(
            "evan".to_owned(),
            Auth::hash_password(&TEST_PASSWORD).unwrap(),
            None,
            CookieSecurity::PlainHttp,
        )
        .unwrap()
    }

    /// The two deployment knobs (`base_path`, `[auth]`) combine correctly:
    /// the redirect a browser gets when unauthenticated lands on the
    /// *prefixed* login route, and the session cookie minted on login is
    /// scoped (`Path=`) to the prefix rather than the root — both built from
    /// `base_path` inside `infra::auth`, since `require_auth` itself only
    /// ever sees the request with the prefix already stripped by `nest`.
    #[tokio::test]
    async fn an_authenticated_base_path_deployment_redirects_and_scopes_the_cookie_to_the_prefix() {
        let pool = test_pool().await;
        let client = prefixed(&pool, Some(test_auth())).with_header("Accept", "text/html");

        let resp = client.get("/share_tracker/listings").await;
        assert_eq!(resp.status, StatusCode::SEE_OTHER);
        assert_eq!(
            resp.headers.get(header::LOCATION).unwrap(),
            "/share_tracker/login"
        );

        let login = client
            .post_bytes(
                "/share_tracker/login",
                Some("application/x-www-form-urlencoded"),
                format!("username=evan&password={}", *TEST_PASSWORD),
            )
            .await;
        assert_eq!(login.status, StatusCode::SEE_OTHER);
        assert_eq!(
            login.headers.get(header::LOCATION).unwrap(),
            "/share_tracker"
        );
        let set_cookie = login
            .headers
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            set_cookie.contains("Path=/share_tracker;"),
            "cookie not scoped to the prefix: {set_cookie}"
        );
    }

    /// `require_auth` is layered onto the merged router before `nest`, so it
    /// covers everything nested under the prefix — one route from each of
    /// the entity, report, scheduler, api_spec and web routers merged in
    /// `router`, mirroring
    /// `a_base_path_moves_the_whole_application_under_the_prefix`'s own route
    /// list but proving each one is actually gated rather than just reachable.
    /// `/openapi.json` is in the list deliberately: the description is behind
    /// `[auth]` like every other route, not on the login page's allowlist.
    #[tokio::test]
    async fn auth_gates_one_route_from_every_merged_router() {
        let pool = test_pool().await;
        let client = prefixed(&pool, Some(test_auth()));
        for uri in [
            "/share_tracker/static/app.js",
            "/share_tracker/listings",
            "/share_tracker/reports/health",
            "/share_tracker/jobs",
            "/share_tracker/openapi.json",
        ] {
            assert_eq!(
                client.get(uri).await.status,
                StatusCode::UNAUTHORIZED,
                "{uri}"
            );
        }

        let login = client
            .post_bytes(
                "/share_tracker/login",
                Some("application/x-www-form-urlencoded"),
                format!("username=evan&password={}", *TEST_PASSWORD),
            )
            .await;
        let cookie = login
            .headers
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let authed = client.with_header("Cookie", cookie);
        for uri in [
            "/share_tracker/static/app.js",
            "/share_tracker/listings",
            "/share_tracker/reports/health",
            "/share_tracker/jobs",
            "/share_tracker/openapi.json",
        ] {
            assert_eq!(authed.get(uri).await.status, StatusCode::OK, "{uri}");
        }
    }

    /// Every non-test function that still takes a `bool` parameter, with the
    /// reason its meaning is visible at the call site anyway. A `bool` whose
    /// sense is invisible where it is passed (`Written::new(true, …)`, the
    /// auth flag `web::router(…, false)`) is an enum instead — `WriteMode`,
    /// `AuthMode`, `LiveQuotes`, `CookieSecurity` — so a new one fails
    /// [`no_function_takes_an_unclassified_bool_parameter`] until it is
    /// converted or classified here.
    const BOOL_PARAMETERS_ALLOWED: &[(&str, &str, &str)] = &[
        (
            "infra/http.rs",
            "of",
            "`Upsert::of` is the one adapter from the existence query's answer \
             (`crud_exists`) to the `Upsert` every write then carries",
        ),
        (
            "entities/closing_price/live.rs",
            "requested",
            "`LiveQuotes::requested` is the one adapter from a report request's \
             serde `live` field to the enum the pricing path takes",
        ),
        (
            "infra/http.rs",
            "deleted",
            "every caller passes a local named `found`, the answer of the delete it just ran",
        ),
        (
            "infra/http.rs",
            "deleted_with_body",
            "private twin of `deleted`; its one caller binds the delete's answer as `found`",
        ),
        (
            "api_spec.rs",
            "operation_id",
            "private; its one caller passes a local named `answers_array`",
        ),
        (
            "infra/email.rs",
            "align",
            "private; called with `table.numeric[i]`, the column's own named flag",
        ),
        (
            "domain/deduction_destination.rs",
            "destination",
            "the domain takes the fact rather than the entity type (CLAUDE.md, \
             `src/domain/`); every caller passes `….is_debt_deduction()`",
        ),
        (
            "domain/tax_year.rs",
            "amit_in_tax_year",
            "callers pass the listing's stored `amit` column by name",
        ),
        (
            "entities/rights_sale.rs",
            "resolve_fx_rate",
            "private; its one caller passes `amounts.converts_nothing()`",
        ),
        (
            "entities/trade/checks.rs",
            "resolve_brokerage",
            "both callers pass the request body's `brokerage_includes_gst` field",
        ),
        (
            "reports/net_capital_gain.rs",
            "reduction_for_units",
            "private; its one caller passes the cohort's named `disposal` field",
        ),
        (
            "reports/performance.rs",
            "build_row",
            "private; its callers pass the locals `open` / `any_open`",
        ),
    ];

    /// The `(file, fn)` of every function in non-test code with a parameter
    /// typed `bool`.
    fn functions_taking_a_bool() -> Vec<(String, String)> {
        let test_only = |path: &str| {
            path == "test_support.rs"
                || path.ends_with("/test_support.rs")
                || path == "ato_examples.rs"
                || path == "doc_checks.rs"
                || path.contains("/tests/")
        };
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let mut found = Vec::new();
        for source in test_support::rust_sources() {
            if test_only(&source.path) {
                continue;
            }
            let code = source.production.as_str();
            let mut rest = code;
            while let Some(at) = rest.find("fn ") {
                let preceded_by_ident = rest[..at].chars().next_back().is_some_and(ident);
                rest = &rest[at + 3..];
                if preceded_by_ident {
                    continue;
                }
                let name: String = rest.chars().take_while(|&c| ident(c)).collect();
                let Some(open) = rest.find('(') else { break };
                // A signature reaches its parameter list before any body or `;`.
                if name.is_empty() || rest[..open].contains(['{', ';']) {
                    continue;
                }
                let mut depth = 0;
                let close = rest[open..]
                    .char_indices()
                    .find(|&(_, c)| {
                        match c {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        depth == 0
                    })
                    .map(|(i, _)| open + i)
                    .expect("a parameter list closes");
                let params = &rest[open + 1..close];
                let takes_bool = params.split(',').any(|param| {
                    param
                        .split_once(':')
                        .is_some_and(|(_, ty)| ty.trim() == "bool")
                });
                if takes_bool {
                    found.push((source.path.clone(), name));
                }
            }
        }
        found.sort();
        found
    }

    /// See [`BOOL_PARAMETERS_ALLOWED`]: a `bool` parameter is either an enum or
    /// classified there with its reason, and no classification outlives its
    /// function.
    #[test]
    fn no_function_takes_an_unclassified_bool_parameter() {
        let found = functions_taking_a_bool();
        let allowed = |path: &str, name: &str| {
            BOOL_PARAMETERS_ALLOWED
                .iter()
                .any(|&(p, n, _)| p == path && n == name)
        };
        let unclassified: Vec<_> = found
            .iter()
            .filter(|(path, name)| !allowed(path, name))
            .collect();
        assert!(
            unclassified.is_empty(),
            "a `bool` parameter whose meaning the call site cannot see — take an enum \
             (`WriteMode`, `AuthMode`, …) or classify it in BOOL_PARAMETERS_ALLOWED: \
             {unclassified:?}"
        );
        let stale: Vec<_> = BOOL_PARAMETERS_ALLOWED
            .iter()
            .filter(|&&(p, n, _)| !found.iter().any(|(path, name)| path == p && name == n))
            .collect();
        assert!(
            stale.is_empty(),
            "classified but no longer taking a `bool`: {stale:?}"
        );
    }

    /// The `fn parse`/`fn from_str` methods taking a `&str` that sit in an
    /// `impl` block other than a `FromStr` one, as `(line, name)`: an ad-hoc
    /// constructor from text that `str::parse` cannot reach. Clippy's
    /// `should_implement_trait` catches only a `pub` method literally named
    /// `from_str` with the trait's exact signature, which is how a private
    /// `WorthlessEvent::from_str` and four `parse`s slipped past it.
    fn inherent_parse_methods(code: &str) -> Vec<(usize, String)> {
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        // One frame per open brace: `Some(is_from_str)` for an `impl` block,
        // `None` for any other block.
        let mut blocks: Vec<Option<bool>> = Vec::new();
        let mut header_start = 0;
        let mut found = Vec::new();
        for (at, c) in code.char_indices() {
            match c {
                '{' => {
                    let header = &code[header_start..at];
                    let tokens: Vec<&str> = header.split_whitespace().collect();
                    let is_impl = tokens
                        .iter()
                        .any(|t| *t == "impl" || t.starts_with("impl<"))
                        && !tokens.contains(&"fn");
                    blocks.push(is_impl.then(|| header.contains("FromStr for")));
                    header_start = at + 1;
                }
                '}' => {
                    blocks.pop();
                    header_start = at + 1;
                }
                ';' => header_start = at + 1,
                'f' if code[at..].starts_with("fn ")
                    && !code[..at].chars().next_back().is_some_and(ident) =>
                {
                    let rest = &code[at + 3..];
                    let name: String = rest.chars().take_while(|&c| ident(c)).collect();
                    if name != "parse" && name != "from_str" {
                        continue;
                    }
                    let Some(open) = rest.find('(') else { continue };
                    let first = rest[open + 1..].split([',', ')']).next().unwrap_or("");
                    let takes_str = first.split_once(':').is_some_and(|(_, ty)| {
                        ty.trim().starts_with('&') && ty.trim().ends_with("str")
                    });
                    if takes_str && blocks.last() == Some(&Some(false)) {
                        let line = code[..at].matches('\n').count() + 1;
                        found.push((line, name));
                    }
                }
                _ => {}
            }
        }
        found
    }

    #[test]
    fn the_parse_method_scan_tells_an_inherent_method_from_a_from_str_impl() {
        let code = "\
            impl Foo {\n\
                pub fn parse(raw: &str) -> Option<Self> { None }\n\
                fn from_str(s: &'static str) -> Result<Self, E> { todo!() }\n\
                fn parse_all(raw: &str) -> Vec<Self> { vec![] }\n\
                fn parse(&self, raw: &str) -> u8 { 0 }\n\
            }\n\
            impl std::str::FromStr for Foo {\n\
                type Err = E;\n\
                fn from_str(s: &str) -> Result<Self, E> { if s.is_empty() { todo!() } else { todo!() } }\n\
            }\n\
            fn parse(schedule: &str) -> Vec<Entry> { vec![] }\n\
            fn f(x: impl Iterator) { let _ = { 1 }; }\n\
            mod tests { fn parse(json: &str) -> u8 { 0 } }\n";
        assert_eq!(
            inherent_parse_methods(code),
            [(2, "parse".to_owned()), (3, "from_str".to_owned())]
        );
    }

    /// A type that is made from text implements `FromStr`, so callers write
    /// `s.parse::<T>()` and the conversion's error is a real `Err` type —
    /// never an inherent `T::parse(&str)` or a private `T::from_str` that
    /// `str::parse` cannot reach (the Google Rust style guide's
    /// `should_implement_trait`). Free functions named `parse` (a whole
    /// schedule file into entries) are not constructors and are not flagged.
    #[test]
    fn no_type_has_an_inherent_parse_method_instead_of_from_str() {
        let found: Vec<_> = test_support::rust_sources()
            .iter()
            .flat_map(|source| {
                inherent_parse_methods(&source.code)
                    .into_iter()
                    .map(|(line, name)| format!("{}:{line} {name}", source.path))
            })
            .collect();
        assert!(
            found.is_empty(),
            "implement `std::str::FromStr` instead of an inherent parse method: {found:?}"
        );
    }
}
