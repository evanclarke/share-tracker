//! Assembles the full axum application from the entity, report, and infra
//! routers. Kept separate from `main` so the wiring is unit-testable.
use axum::{Extension, Router};
use sqlx::SqlitePool;

use crate::entities::closing_price;
use crate::infra::scheduler::{self, JobRegistry};

/// Build the HTTP router: the web frontend plus all entity and report routes and
/// scheduler inspection, sharing the pool as state and the job registry as an
/// extension. The live Yahoo price fetcher is injected the same way (entity
/// tests layer a stub `SharedFetcher` instead).
pub fn router(pool: SqlitePool, registry: JobRegistry) -> Router {
    let fetcher: closing_price::SharedFetcher =
        std::sync::Arc::new(closing_price::YahooFetcher::default());
    crate::entities::router()
        .merge(crate::reports::router())
        .merge(scheduler::router())
        .merge(crate::web::router())
        .with_state(pool)
        .layer(Extension(registry))
        .layer(Extension(fetcher))
}
