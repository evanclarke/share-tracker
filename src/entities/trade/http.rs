//! HTTP surface for trades: the router and its handlers. Sells and DRP
//! trades are rejected here — they are only created via their dedicated
//! endpoints (`PUT /sells/:id`, `POST /income/:id/reinvest`) so their
//! invariants (allocations, residual chain) always hold.

use super::{
    DeleteOutcome, Settlement, Trade, TradeBody, TradeType, db_delete, db_get, db_list, db_upsert,
    resolve_brokerage,
};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use sqlx::SqlitePool;

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/trades", get(list))
        .route("/trades/{id}", get(get_one).put(upsert).delete(delete))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Trade>>, ApiError> {
    let trades = db_list(&pool).await?;
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

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<TradeBody>,
) -> Result<StatusCode, ApiError> {
    // Sells must be created via PUT /sells/{id} so they are persisted together
    // with a full set of parcel allocations (no uncovered Sell can exist).
    if body.trade_type == TradeType::Sell {
        return Err(ApiError::unprocessable(
            "a Sell must be created via PUT /sells/:id so it carries its parcel allocations",
        ));
    }
    // DRP trades are only ever created via POST /income/:id/reinvest, which
    // links the shares back to their funding distribution and threads the
    // residual carry-forward chain. A free-form DRP here would be an orphan
    // parcel (no income link, zero residuals) that could shadow that chain —
    // and editing a reinvest-created DRP through this endpoint would silently
    // zero its residual columns (the form doesn't carry them). Reject both.
    if body.trade_type == TradeType::DRP {
        return Err(ApiError::unprocessable(
            "a DRP trade is created via POST /income/:id/reinvest so it stays linked to its \
             distribution and residual chain",
        ));
    }
    // Which of the two wrote the stored settlement date is recorded with it:
    // a supplied value is the taxpayer's own assertion and is never rewritten,
    // a computed one is re-derived by the `settlement-recompute` job once the
    // calendar it was computed against is completed (SCENARIOS S-04/S-05).
    let settlement =
        Settlement::resolve(&pool, id, body.listing_id, body.date, body.settlement_date).await?;
    // A GST-inclusive brokerage entry is split here, at the API boundary, so
    // the stored columns (and `Trade` itself) are always ex-GST + GST.
    let (brokerage, gst_on_brokerage) = resolve_brokerage(
        body.brokerage_includes_gst,
        body.brokerage,
        body.gst_on_brokerage,
    );
    let trade = Trade {
        id,
        trade_type: body.trade_type,
        date: body.date,
        settlement_date: settlement.date,
        settlement_date_source: settlement.source,
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
    };
    db_upsert(&pool, &trade).await?;
    Ok(StatusCode::NO_CONTENT)
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
