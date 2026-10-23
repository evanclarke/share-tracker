//! Shared fixtures for the scheduler tests, and the topic modules over them.
//! Moved verbatim out of the module's single inline `mod tests`; what lives
//! here is what more than one topic uses, and a fixture only one topic calls
//! lives in that topic's file.

mod backup;
mod failures;
mod history;
mod listing_and_locks;
mod parsing;
mod properties;
mod stored_schedule;
mod triggering;

use super::*;
use crate::entities::closing_price::SharedFetcher;
use crate::entities::closing_price::test_support::QuoteStub;
use crate::entities::distribution_event::SharedDistributionFetcher;
use crate::entities::distribution_event::test_support::DistributionStub;
use crate::entities::rba_fx_rate::ImportError;
use crate::infra::fetch;
use crate::infra::scheduler::registry::JobParams;
use crate::reports::health;
use crate::test_support;
use crate::test_support::{ApiClient, await_condition, file_pool, test_pool};
use axum::Extension;
use axum::http::StatusCode;
use chrono::{DateTime, Local, TimeZone, Utc};
use chrono_tz::{America, Australia, Tz};
use croner::Cron;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex, atomic};
use std::time::{Duration, Instant as StdInstant};
use std::{fs, path};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, oneshot};
use tokio::time::Instant;
use tokio::{task, time};

/// An offline price-source stub so building a registry never constructs the
/// live `YahooFetcher` (these tests trigger only the `backup` job, but the
/// stub guarantees no test path can reach the network).
fn stub_fetcher() -> SharedFetcher {
    QuoteStub::default().shared()
}

fn stub_distributions() -> SharedDistributionFetcher {
    DistributionStub::default().shared()
}

async fn test_registry() -> (JobRegistry, SqlitePool, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db").to_string_lossy().to_string();
    let pool = file_pool(&db_path).await;
    (
        registry(
            pool.clone(),
            db_path.clone(),
            None,
            None,
            stub_fetcher(),
            stub_distributions(),
            None,
        ),
        pool,
        dir,
        db_path,
    )
}
