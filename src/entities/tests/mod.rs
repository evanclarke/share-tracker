//! The cross-entity tests: contracts every entity route keeps, pinned over
//! the whole of `entities::router` rather than one entity's own module. Moved
//! verbatim out of the parent's inline `mod tests`, one file per contract.

mod delete;
mod list;
mod put;

use super::*;
use crate::entities::route_tables::{LIST_ROUTES, PUT_ROUTES, PutOutcome};
use crate::infra::id::ListingId;
use crate::test_support::{ApiClient, test_pool};
use crate::{entities, test_support};
use axum::http::StatusCode;
use std::collections::BTreeMap;
