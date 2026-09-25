//! HTTP surface for trades: the router and its handlers. Sells and DRP
//! trades are rejected here — they are only created via their dedicated
//! endpoints (`PUT /sells/:id`, `POST /income/:id/reinvest`) so their
//! invariants (allocations, residual chain) always hold.

use super::{
    DeleteOutcome, Trade, TradeBody, TradeListQuery, TradeType, db_create, db_delete, db_get,
    db_list_filtered, db_upsert_resolving_settlement, model::SettlementDateSource,
    resolve_brokerage,
};
use crate::infra::http::{self, ApiError, UpsertResponse};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
};
use sqlx::SqlitePool;

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/trades", get(list).post(create))
        .route("/trades/{id}", get(get_one).put(upsert).delete(delete))
}

/// The trade list is hand-written only because a trade is *presented* through
/// [`Trade::present`] (its GST-inclusive brokerage recombined); the filtering
/// itself is the shared [`http::crud_list_filtered`], over the same
/// [`TradeListQuery`] every other entity's list takes.
async fn list(
    State(pool): State<SqlitePool>,
    Query(filter): Query<TradeListQuery>,
) -> Result<Json<Vec<Trade>>, ApiError> {
    let trades = db_list_filtered(&pool, &filter).await?;
    Ok(Json(trades.into_iter().map(Trade::present).collect()))
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Trade>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(|t| Json(t.present()))
        .ok_or(ApiError::NotFound)
}

/// The row a request body describes. `id` is the path's on an upsert and
/// ignored on a create (the database assigns one), so both entry points build
/// their `Trade` through this one mapping and cannot drift.
///
/// The two refusals live here rather than in the write because they are about
/// the *body's* kind, which only the HTTP boundary knows. Sells must be created
/// via `PUT /sells/{id}` so they are persisted together with a full set of
/// parcel allocations (no uncovered Sell can exist). DRP trades are only ever
/// created via `POST /income/:id/reinvest`, which links the shares back to
/// their funding distribution and threads the residual carry-forward chain. A
/// free-form DRP here would be an orphan parcel (no income link, zero
/// residuals) that could shadow that chain — and editing a reinvest-created DRP
/// through this endpoint would silently zero its residual columns (the form
/// doesn't carry them). Reject both.
fn trade_from_body(id: i64, body: TradeBody) -> Result<Trade, ApiError> {
    if body.trade_type == TradeType::Sell {
        return Err(ApiError::unprocessable(
            "a Sell must be created via PUT /sells/:id so it carries its parcel allocations",
        ));
    }
    if body.trade_type == TradeType::DRP {
        return Err(ApiError::unprocessable(
            "a DRP trade is created via POST /income/:id/reinvest so it stays linked to its \
             distribution and residual chain",
        ));
    }
    // The stored settlement date's placeholder. Which of the two wrote it — the
    // taxpayer (a supplied value, never rewritten) or the exchange calendar (a
    // computed one, re-derived by the `settlement-recompute` job once the
    // calendar it was computed against is completed, SCENARIOS S-04/S-05) — is
    // decided inside the write's own transaction
    // (`db_upsert_resolving_settlement`/`db_create`), not here, so a concurrent
    // write cannot change the stored row between the classifying read and the
    // write. The two fields below are only the placeholder that lets the
    // pre-transaction figure checks validate a supplied date (and passes on the
    // trade date when one will be computed).
    let settlement_date = body.settlement_date.unwrap_or(body.date);
    // A GST-inclusive brokerage entry is split here, at the API boundary, so
    // the stored columns (and `Trade` itself) are always ex-GST + GST.
    let (brokerage, gst_on_brokerage) = resolve_brokerage(
        body.brokerage_includes_gst,
        body.brokerage,
        body.gst_on_brokerage,
    );
    Ok(Trade {
        id,
        trade_type: body.trade_type,
        date: body.date,
        settlement_date,
        settlement_date_source: SettlementDateSource::Stated,
        listing_id: body.listing_id,
        average_price: body.average_price,
        quantity: body.quantity,
        currency: body.currency,
        brokerage,
        gst_on_brokerage,
        brokerage_includes_gst: body.brokerage_includes_gst,
        brokerage_currency: body.brokerage_currency,
        fx_rate: body.fx_rate,
        spot_fx_rate: body.spot_fx_rate,
        contract_note_ref: body.contract_note_ref,
        statement_total: body.statement_total,
        residual_brought_forward: body.residual_brought_forward,
        residual_carried_forward: body.residual_carried_forward,
        residual_paid_out: body.residual_paid_out,
        rights_action_id: None,
        buyback_action_id: None,
        scrip_action_id: None,
        demerger_action_id: None,
        worthless_action_id: None,
        deemed_acquisition_date: None,
        holding_account_id: body.holding_account_id,
        transfer_id: None,
        ess_statement_id: None,
        inheritance_id: None,
    })
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<TradeBody>,
) -> Result<UpsertResponse<Trade>, ApiError> {
    let supplied = body.settlement_date;
    let trade = trade_from_body(id, body)?;
    let outcome = db_upsert_resolving_settlement(&pool, &trade, supplied).await?;
    http::upsert_response::<Trade>(&pool, outcome, id).await
}

/// `POST /trades` — create the row without naming an id. The database assigns
/// one (see `db::write`), and the created row is returned so the caller can use
/// its id immediately with no `max(id) + 1` guess between the two calls. The
/// create runs the full write-time validation (settlement resolution,
/// holding-account/currency checks, rollover guards, parcel re-basing) exactly
/// as the upsert does, so it is not a way round any of it.
async fn create(
    State(pool): State<SqlitePool>,
    Json(body): Json<TradeBody>,
) -> Result<(StatusCode, Json<Trade>), ApiError> {
    let supplied = body.settlement_date;
    let trade = trade_from_body(0, body)?;
    let created = db_create(&pool, &trade, supplied).await?;
    Ok((StatusCode::CREATED, Json(created.present())))
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no trade with that id")),
        DeleteOutcome::Referenced => Err(ApiError::unprocessable(
            "this trade is referenced by a sale allocation, AMIT adjustment, reinvestment, or \
             a scrip-for-scrip/demerger group — remove those first (e.g. delete the Sell)",
        )),
    }
}
