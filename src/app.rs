//! Assembles the full axum application from the entity, report, and infra
//! routers. Kept separate from `main` so the wiring is unit-testable.
use axum::{Extension, Router};
use sqlx::SqlitePool;

use crate::entities::closing_price::SharedFetcher;
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
pub fn router(
    base_path: &str,
    pool: SqlitePool,
    registry: JobRegistry,
    fetcher: SharedFetcher,
) -> Router {
    let app = crate::entities::router()
        .merge(crate::reports::router())
        .merge(scheduler::router())
        .merge(crate::web::router(base_path))
        .with_state(pool)
        .layer(Extension(registry))
        .layer(Extension(fetcher));
    if base_path.is_empty() {
        return app;
    }
    // `nest` matches the prefix itself (`/share_tracker`) and everything below
    // it (`/share_tracker/listings`), but *not* the bare prefix with a trailing
    // slash: `/share_tracker/` leaves an empty remainder, which matches neither
    // pattern. That is precisely the URL a person types and the one an nginx
    // `location /share_tracker/` block forwards, so it cannot be left as a 404
    // — redirect it onto the prefix the nested router does serve. Temporary,
    // not permanent: `base_path` is configuration, and a browser must not cache
    // a redirect derived from it past a change.
    Router::new()
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
        .nest(base_path, app)
}

#[cfg(test)]
mod tests {
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;
    use sqlx::SqlitePool;

    /// The full application mounted under a reverse-proxy prefix, as `main`
    /// builds it when `base_path` is set.
    fn prefixed(pool: &SqlitePool) -> ApiClient {
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
        ))
    }

    #[tokio::test]
    async fn a_base_path_moves_the_whole_application_under_the_prefix() {
        let pool = test_pool().await;
        let client = prefixed(&pool);

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
        let client = prefixed(&pool);
        for uri in ["/", "/static/app.js", "/listings", "/jobs"] {
            assert_eq!(client.get(uri).await.status, StatusCode::NOT_FOUND, "{uri}");
        }
    }

    #[tokio::test]
    async fn an_empty_base_path_serves_everything_at_the_root() {
        // The default: byte-for-byte the pre-base-path behaviour.
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        for uri in ["/", "/static/app.js", "/listings", "/jobs"] {
            assert_eq!(client.get(uri).await.status, StatusCode::OK, "{uri}");
        }
    }
}
