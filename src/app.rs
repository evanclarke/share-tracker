//! Assembles the full axum application from the entity, report, and infra
//! routers. Kept separate from `main` so the wiring is unit-testable.
use axum::{Extension, Router};
use sqlx::SqlitePool;

use crate::infra::scheduler::{self, JobRegistry};

/// Build the HTTP router: the web frontend plus all entity and report routes and
/// scheduler inspection, sharing the pool as state and the job registry as an
/// extension.
pub fn router(pool: SqlitePool, registry: JobRegistry) -> Router {
    crate::entities::router()
        .merge(crate::reports::router())
        .merge(scheduler::router())
        .merge(crate::web::router())
        .with_state(pool)
        .layer(Extension(registry))
}
