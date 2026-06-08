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
pub fn router(pool: SqlitePool, registry: JobRegistry, fetcher: SharedFetcher) -> Router {
    crate::entities::router()
        .merge(crate::reports::router())
        .merge(scheduler::router())
        .merge(crate::web::router())
        .with_state(pool)
        .layer(Extension(registry))
        .layer(Extension(fetcher))
}
