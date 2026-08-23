//! Assembles the full axum application from the entity, report, and infra
//! routers. Kept separate from `main` so the wiring is unit-testable.
use axum::{Extension, Router};
use sqlx::SqlitePool;

use crate::entities::closing_price::SharedFetcher;
use crate::infra::auth::Auth;
use crate::infra::scheduler::{self, JobRegistry};

/// Build the HTTP router: the web frontend plus all entity and report routes and
/// scheduler inspection, sharing the pool as state and the job registry as an
/// extension. The price fetcher is injected (not constructed here) and layered
/// as an extension, so the live `YahooFetcher` only reaches the router from
/// `main`; tests pass a stub `SharedFetcher` and never touch the network.
///
/// `base_path` is the reverse-proxy path prefix from
/// [`crate::infra::config::Settings`] — `""` (the default) mounts everything at
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
pub fn router(
    base_path: &str,
    pool: SqlitePool,
    registry: JobRegistry,
    fetcher: SharedFetcher,
    auth: Option<Auth>,
) -> Router {
    let mut app = crate::entities::router()
        .merge(crate::reports::router())
        .merge(scheduler::router())
        .merge(crate::web::router(base_path, auth.is_some()));
    if let Some(auth) = &auth {
        app = app.merge(crate::infra::auth::router(auth.clone(), base_path));
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
        Some(auth) => app.layer(axum::middleware::from_fn_with_state(
            crate::infra::auth::AuthState::new(auth, base_path.to_string()),
            crate::infra::auth::require_auth,
        )),
        None => app,
    };
    if base_path.is_empty() {
        return catching_panics(app);
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
            axum::routing::any({
                let target = base_path.to_string();
                move || {
                    let target = target.clone();
                    async move { axum::response::Redirect::temporary(&target) }
                }
            }),
        )
        .nest(base_path, app);
    catching_panics(app)
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
    app.layer(tower_http::catch_panic::CatchPanicLayer::custom(
        crate::infra::http::panic_response,
    ))
}

#[cfg(test)]
mod tests {
    use crate::infra::auth::Auth;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;
    use sqlx::SqlitePool;

    /// The full application mounted under a reverse-proxy prefix, as `main`
    /// builds it when `base_path` is set. `auth` mirrors `router`'s own
    /// parameter — `None` for the base-path-only tests below,
    /// `Some` for the one combining a prefix with `[auth]` configured.
    fn prefixed(pool: &SqlitePool, auth: Option<Auth>) -> ApiClient {
        let fetcher = crate::entities::closing_price::test_support::QuoteStub::default().shared();
        let registry = crate::infra::scheduler::registry(
            pool.clone(),
            ":memory:".to_string(),
            None,
            None,
            fetcher.clone(),
        );
        ApiClient::over(super::router(
            "/share_tracker",
            pool.clone(),
            registry,
            fetcher,
            auth,
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
        let app =
            super::catching_panics(axum::Router::new().route("/boom", axum::routing::get(boom)));

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
        crate::test_support::listing(9).insert(&pool).await;
        crate::test_support::insert_parcel_bypassing_checks(
            &pool,
            9600,
            9,
            crate::test_support::ymd(2024, 3, 15),
            "1000000000000000",
            "1000000000000000",
        )
        .await;

        let client = ApiClient::full(&pool);
        let resp = client.get("/portfolio/open-parcels").await;
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
            resp.headers.get(axum::http::header::LOCATION).unwrap(),
            "/share_tracker"
        );
        // A static asset, an entity route, a report route, and the scheduler
        // routes — one from each merged router, so nesting covers all of them.
        for uri in [
            "/share_tracker/static/app.js",
            "/share_tracker/listings",
            "/share_tracker/reports/health",
            "/share_tracker/jobs",
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

    fn test_auth() -> Auth {
        Auth::new(
            "evan".to_string(),
            Auth::hash_password("hunter2").unwrap(),
            None,
            false,
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
            resp.headers.get(axum::http::header::LOCATION).unwrap(),
            "/share_tracker/login"
        );

        let login = client
            .post_bytes(
                "/share_tracker/login",
                Some("application/x-www-form-urlencoded"),
                "username=evan&password=hunter2",
            )
            .await;
        assert_eq!(login.status, StatusCode::SEE_OTHER);
        assert_eq!(
            login.headers.get(axum::http::header::LOCATION).unwrap(),
            "/share_tracker"
        );
        let set_cookie = login
            .headers
            .get(axum::http::header::SET_COOKIE)
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
    /// the entity, report, scheduler and web routers merged in `router`,
    /// mirroring `a_base_path_moves_the_whole_application_under_the_prefix`'s
    /// own route list but proving each one is actually gated rather than
    /// just reachable.
    #[tokio::test]
    async fn auth_gates_one_route_from_every_merged_router() {
        let pool = test_pool().await;
        let client = prefixed(&pool, Some(test_auth()));
        for uri in [
            "/share_tracker/static/app.js",
            "/share_tracker/listings",
            "/share_tracker/reports/health",
            "/share_tracker/jobs",
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
                "username=evan&password=hunter2",
            )
            .await;
        let cookie = login
            .headers
            .get(axum::http::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let authed = client.with_header("Cookie", cookie);
        for uri in [
            "/share_tracker/static/app.js",
            "/share_tracker/listings",
            "/share_tracker/reports/health",
            "/share_tracker/jobs",
        ] {
            assert_eq!(authed.get(uri).await.status, StatusCode::OK, "{uri}");
        }
    }
}
