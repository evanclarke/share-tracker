//! The cash-in-lieu disposal of a rounded corporate action's fraction:
//! `POST /corporate_actions/{id}/cash_in_lieu`.
//!
//! A ratio-driven action whose `fraction_rule` is `CashInLieu` rounds each
//! holding down onto its unit grid; the registry aggregates the fractions,
//! sells them and pays each holding its share. Until that payment is
//! recorded, the holding's fraction rides on one of its parcels
//! (`corporate_action_parcel_counts.in_lieu_units`, written by the action's
//! own rounding — `corporate_action::rounding`, or the exchange/demerger for
//! the replacement listing's units). This operation records the payment as
//! what it is for CGT purposes: a **disposal of the fraction** (the cash is
//! capital proceeds — `docs/ato/share-splits-and-consolidations.md`), written
//! as a Sell of exactly the stored fraction, dated the payment date, at the
//! cash the holding was paid, drawing on the parcel that carries it. The
//! fraction's share of that parcel's cost base comes out of the same
//! cost-base pipeline as any other disposal, so the realised-gains report
//! costs it (with the discount, if the parcel qualifies) and the parcel is
//! left at its count on the grid.
//!
//! One call per holding — the registry pays each holding its own cheque, which
//! is the figure the holder's statement prints. The Sell carries
//! `cash_in_lieu_action_id` (migration 0056): `PUT /sells` and
//! `PUT`/`DELETE /trades` refuse it, `DELETE /sells` is the undo, and the
//! action (and its units-received overrides) is frozen while it exists. The
//! price stored is the cash over the fraction; where the fraction is not a
//! terminating decimal the product reproduces the cash to `Decimal`'s last
//! digit, not beyond it.

use crate::entities::corporate_action::{self, ActionKind, FractionRule, UnitsAt};
use crate::entities::rollover;
use crate::entities::sell::{self, AllocationInput};
use crate::entities::trade::{self, Trade};
use crate::infra::currency::CurrencyCode;
use crate::infra::db::{WriteMode, write_tx};
use crate::infra::decimal::parse_dec;
use crate::infra::extract::{Json, Query};
use crate::infra::fx::{FxOverride, FxRates, RateMonth};
use crate::infra::http::{ApiError, DryRunQuery};
use crate::infra::id::{HoldingAccountId, ListingId, TradeId};
use crate::infra::respond::Written;
use axum::extract::{Path, State};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::{Row, SqlitePool};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The `POST /corporate_actions/{id}/cash_in_lieu` body: one holding's
/// payment for its fraction.
#[derive(utoipa::ToSchema, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CashInLieuBody {
    /// The holding account the registry paid.
    pub holding_account_id: HoldingAccountId,
    /// The payment date, on or after the action's own date — the disposal's
    /// date, so it sets the financial year and the discount clock.
    pub date: NaiveDate,
    /// The cash the holding was paid for its fraction, in the listing's
    /// currency (non-negative): the Sell's capital proceeds.
    #[serde(deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub proceeds: Decimal,
}

#[derive(thiserror::Error, Debug)]
pub enum CashInLieuError {
    #[error("cash-in-lieu write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// No corporate action with that id. Mapped to `404`.
    #[error("no corporate action with that id")]
    ActionNotFound,
    /// The action is not a ratio-driven one whose rule is `CashInLieu`.
    /// Mapped to `422`.
    #[error("that corporate action does not pay cash in lieu")]
    NotCashInLieu,
    /// The payment is dated before the action. Mapped to `422`.
    #[error("the payment is dated before the action")]
    BeforeAction,
    /// A cash-in-lieu Sell already records this holding's payment. Mapped to
    /// `422`.
    #[error("this holding's cash in lieu has already been recorded")]
    AlreadyPaid,
    /// No stored fraction for the holding: its count came out exact on the
    /// grid, it holds nothing the action converted, its account has no grid,
    /// or (for an exchange or demerger) the operation has not run. Mapped to
    /// `422`.
    #[error("the holding has no fraction left by the action")]
    NothingInLieu,
    /// The listing's currency has no ATO rate for the payment month. Mapped to
    /// `422`.
    #[error("no ATO FX rate for {currency} in {month}")]
    MissingFxRate {
        currency: CurrencyCode,
        month: RateMonth,
    },
    /// The Sell-side invariants failed. Mapped to `422`.
    #[error("the cash-in-lieu Sell was rejected: {0}")]
    Sell(#[source] sell::SellError),
}

impl From<sell::SellError> for CashInLieuError {
    // Every rejection but the database's is wrapped whole: the 422 wording
    // lives in the wrapper's `From<…> for ApiError` arm.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "a pass-through of every non-database SellError"
    )]
    fn from(e: sell::SellError) -> Self {
        match e {
            sell::SellError::Db(err) => CashInLieuError::Db(err),
            other => CashInLieuError::Sell(other),
        }
    }
}

impl From<CashInLieuError> for ApiError {
    fn from(e: CashInLieuError) -> Self {
        match e {
            CashInLieuError::ActionNotFound => {
                ApiError::not_found("no corporate action with that id")
            }
            CashInLieuError::NotCashInLieu => ApiError::unprocessable(
                "that corporate action does not pay cash in lieu — only a ShareSplit, \
                 BonusIssue, ScripForScrip or Demerger whose fraction_rule is CashInLieu does",
            ),
            CashInLieuError::BeforeAction => ApiError::unprocessable(
                "the cash-in-lieu payment is dated before its corporate action — the fraction \
                 it pays for does not exist until the action does",
            ),
            CashInLieuError::AlreadyPaid => ApiError::unprocessable(
                "this holding's cash in lieu has already been recorded — delete that Sell to \
                 record it again",
            ),
            CashInLieuError::NothingInLieu => ApiError::unprocessable(
                "this holding has no fraction left by the action: its rounded count was exact, \
                 it held nothing the action converted, its account has no unit grid to round \
                 onto, or (for an exchange or demerger) the operation has not been run yet",
            ),
            CashInLieuError::MissingFxRate { currency, month } => ApiError::Unprocessable(format!(
                "the listing is quoted in {currency} but no ATO/RBA rate has been imported \
                     for {currency} in {month} — import that month's RBA rates and record the \
                     payment then; without one the proceeds would be converted at parity"
            )),
            CashInLieuError::Sell(err) => {
                tracing::warn!(error = ?err, "cash-in-lieu Sell rejected by a sell invariant");
                if matches!(
                    err,
                    sell::SellError::Amounts(trade::AmountsError::FutureDate)
                ) {
                    ApiError::unprocessable(
                        "the cash-in-lieu payment is dated after today — record it on the day \
                         it is paid",
                    )
                } else {
                    ApiError::unprocessable(format!("the cash-in-lieu Sell is invalid: {err}"))
                }
            }
            CashInLieuError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new().routes(routes!(cash_in_lieu))
}

/// The holding's stored fraction: the parcel carrying it, that parcel's
/// listing, and the fraction in the action date's basis.
async fn db_fraction(
    conn: &mut sqlx::SqliteConnection,
    action_id: i64,
    account: HoldingAccountId,
) -> Result<Option<(TradeId, ListingId, Decimal)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT c.trade_id, c.in_lieu_units, t.listing_id \
         FROM corporate_action_parcel_counts c JOIN trades t ON t.id = c.trade_id \
         WHERE c.corporate_action_id = ? AND t.holding_account_id = ? ORDER BY c.trade_id",
    )
    .bind(action_id)
    .bind(account)
    .fetch_all(&mut *conn)
    .await?;
    for row in &rows {
        let in_lieu = parse_dec("in_lieu_units", row.try_get("in_lieu_units")?)?;
        if in_lieu > Decimal::ZERO {
            return Ok(Some((
                row.try_get("trade_id")?,
                row.try_get("listing_id")?,
                in_lieu,
            )));
        }
    }
    Ok(None)
}

/// Record one holding's cash in lieu, ending as `mode` says.
async fn execute(
    pool: &SqlitePool,
    action_id: i64,
    body: &CashInLieuBody,
    mode: WriteMode,
) -> Result<Trade, CashInLieuError> {
    let mut tx = write_tx(pool).await?;
    let action = corporate_action::db_get_tx(&mut *tx, action_id)
        .await?
        .ok_or(CashInLieuError::ActionNotFound)?;
    let rule = match action.kind {
        ActionKind::ShareSplit { fraction_rule, .. }
        | ActionKind::BonusIssue { fraction_rule, .. }
        | ActionKind::ScripForScrip { fraction_rule, .. }
        | ActionKind::Demerger { fraction_rule, .. } => Some(fraction_rule),
        ActionKind::ReturnOfCapital { .. }
        | ActionKind::RightsIssue { .. }
        | ActionKind::BuyBack { .. }
        | ActionKind::WorthlessShares { .. } => None,
    };
    if rule != Some(FractionRule::CashInLieu) {
        return Err(CashInLieuError::NotCashInLieu);
    }
    if body.date < action.date {
        return Err(CashInLieuError::BeforeAction);
    }
    let paid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM trades \
                       WHERE cash_in_lieu_action_id = ? AND holding_account_id = ?)",
    )
    .bind(action_id)
    .bind(body.holding_account_id)
    .fetch_one(&mut *tx)
    .await?;
    if paid {
        return Err(CashInLieuError::AlreadyPaid);
    }
    let (parcel, listing_id, fraction) = db_fraction(&mut tx, action_id, body.holding_account_id)
        .await?
        .ok_or(CashInLieuError::NothingInLieu)?;

    // The fraction is in the action date's unit basis; the allocation is in
    // the payment date's, across any re-basing between the two.
    let splits = corporate_action::db_splits_for_listing(&mut tx, listing_id).await?;
    let quantity = UnitsAt::new(fraction, action.date)
        .rebased(&splits.for_parcel(parcel), Some(body.date))
        .quantity();

    let currency: CurrencyCode = sqlx::query_scalar("SELECT currency FROM listings WHERE id = ?")
        .bind(listing_id)
        .fetch_one(&mut *tx)
        .await?;
    let fx = FxRates::load(&mut *tx).await?;
    let fx_rate = fx
        .resolve_rate(&currency, body.date, FxOverride::None)
        .map_err(|_| CashInLieuError::MissingFxRate {
            currency: currency.clone(),
            month: RateMonth::of(body.date),
        })?;
    let sell_body = rollover::closing_sell_body(
        body.date,
        listing_id,
        body.holding_account_id,
        body.proceeds / quantity,
        currency,
        fx_rate,
        vec![AllocationInput {
            purchase_trade_id: parcel,
            quantity_allocated: quantity,
        }],
    );
    let id = sell::upsert_sell_in_tx(
        &mut tx,
        sell::SellWrite {
            id: None,
            body: &sell_body,
            settlement: trade::Settlement::stated(body.date),
            provenance: Some(sell::SellProvenance::CashInLieuAction(action_id)),
        },
    )
    .await?;
    let sell = trade::db_get(&mut *tx, id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    mode.finish(tx).await?;
    Ok(sell)
}

#[utoipa::path(
    post,
    path = "/corporate_actions/{id}/cash_in_lieu",
    summary = "Record one holding's cash in lieu of a CashInLieu action's fraction as a Sell of that fraction, and return the Sell (?dry_run=true previews it: 200 with the same body, nothing stored).",
    params(DryRunQuery)
)]
async fn cash_in_lieu(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
    Query(query): Query<DryRunQuery>,
    Json(body): Json<CashInLieuBody>,
) -> Result<Written<Trade>, ApiError> {
    let sell = execute(&pool, action_id, &body, query.mode()).await?;
    Ok(Written::new(query.mode(), sell))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::open_parcels;
    use crate::reports::realised_gains;
    use crate::test_support::{self, ApiClient, dec, test_pool, ymd};
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    /// Listing 1 (and 2, a replacement listing), account 1 on whole units,
    /// and Buys of 50 and 50 units at $3 on 2024-01-02/03 (trades 1, 2).
    async fn holding(pool: &SqlitePool) -> ApiClient {
        test_support::listing(1).ticker("CIL").insert(pool).await;
        test_support::listing(2).ticker("NEW").insert(pool).await;
        let app = ApiClient::full(pool);
        app.put_ok(
            "/holding_accounts/1",
            &json!({ "name": "Default", "unit_scale": 0 }),
        )
        .await;
        for (id, day) in [(1, 2), (2, 3)] {
            test_support::buy(id, 1)
                .qty(dec("50"))
                .price(dec("3"))
                .date(ymd(2024, 1, day))
                .insert(pool)
                .await;
        }
        app
    }

    fn consolidation(rule: &str) -> Value {
        json!({
            "listing_id": 1, "date": "2024-06-03", "action_type": "ShareSplit",
            "split_new_units": "1", "split_old_units": "3", "fraction_rule": rule,
        })
    }

    fn payment(proceeds: &str) -> Value {
        json!({ "holding_account_id": 1, "date": "2024-06-20", "proceeds": proceeds })
    }

    async fn remaining(pool: &SqlitePool, on: NaiveDate) -> Vec<(i64, i64, Decimal)> {
        let mut rows: Vec<(i64, i64, Decimal)> =
            open_parcels::load(&mut pool.acquire().await.unwrap(), Some(on))
                .await
                .unwrap()
                .into_iter()
                .map(|p| {
                    (
                        p.parcel.listing_id.0,
                        p.parcel.id.0,
                        p.remaining_as_of.quantity(),
                    )
                })
                .collect();
        rows.sort_unstable();
        rows
    }

    /// 100 units consolidated 1-for-3, cash in lieu: 33 on the register
    /// (17 + 16), the 1/3 fraction carried on the parcel next in line until
    /// the payment, which sells exactly it — leaving 16 — at the cash paid,
    /// costed out of that parcel's cost base. The preview stores nothing, a
    /// second payment is refused, the action is frozen while it stands, and
    /// deleting the Sell is the undo.
    #[tokio::test]
    async fn a_split_paid_in_lieu_sells_exactly_the_fraction() {
        let pool = test_pool().await;
        let app = holding(&pool).await;
        app.put_ok("/corporate_actions/7", &consolidation("CashInLieu"))
            .await;
        let before = remaining(&pool, ymd(2024, 6, 3)).await;
        assert_eq!(before[0], (1, 1, dec("17")));
        assert_eq!(before[1].2.trunc(), dec("16"));
        assert!(before[1].2 > dec("16.3333") && before[1].2 < dec("16.3334"));

        let sell = app
            .assert_dry_run_previews(
                &pool,
                "/corporate_actions/7/cash_in_lieu",
                Some(&payment("2.00")),
            )
            .await;
        assert_eq!(sell["cash_in_lieu_action_id"], 7);
        let quantity: Decimal = sell["quantity"].as_str().unwrap().parse().unwrap();
        assert_eq!(quantity, before[1].2 - dec("16"));
        assert_eq!(
            remaining(&pool, ymd(2024, 6, 20)).await,
            [(1, 1, dec("17")), (1, 2, dec("16"))]
        );
        // The disposal: proceeds the cash (to Decimal's last digit), cost
        // base the fraction's share of parcel 2's $150 — 150 × (1/3) /
        // (16 + 1/3) ≈ $3.06.
        let gains = realised_gains::db_realised_gains(&pool).await.unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].proceeds.round_dp(10), dec("2.00"));
        assert_eq!(gains[0].cost_base.round_dp(2), dec("3.06"));

        app.post("/corporate_actions/7/cash_in_lieu", &payment("2.00"))
            .await
            .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
        app.put("/corporate_actions/7", &consolidation("Down"))
            .await
            .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
        let mut edited = sell.clone();
        edited["average_price"] = json!("7");
        app.put(
            format!("/sells/{}", sell["id"]),
            &test_support::writable_body(&edited, &[]),
        )
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);

        app.delete(format!("/sells/{}", sell["id"]))
            .await
            .expect_status(StatusCode::NO_CONTENT);
        assert_eq!(remaining(&pool, ymd(2024, 6, 20)).await, before);
        // The whole register then sells exactly.
        app.post(
            "/sells",
            &json!({
                "date": "2024-06-21", "listing_id": 1, "average_price": "9", "quantity": "33",
                "currency": "AUD", "brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1",
                "allocations": [
                    { "purchase_trade_id": 1, "quantity_allocated": "17" },
                    { "purchase_trade_id": 2, "quantity_allocated": "16" },
                ],
            }),
        )
        .await
        .expect_status(StatusCode::CREATED);
    }

    /// What is not a cash-in-lieu payment is refused before anything is
    /// written: another rule, a date before the action, a holding with no
    /// fraction (no grid), an unknown action.
    #[tokio::test]
    async fn what_is_not_a_cash_in_lieu_payment_is_refused() {
        let pool = test_pool().await;
        let app = holding(&pool).await;
        app.put_ok("/corporate_actions/7", &consolidation("Down"))
            .await;
        app.assert_dry_run_refuses_alike(
            &pool,
            "/corporate_actions/7/cash_in_lieu",
            Some(&payment("2")),
        )
        .await;
        app.put_ok("/corporate_actions/7", &consolidation("CashInLieu"))
            .await;
        let early = json!({ "holding_account_id": 1, "date": "2024-06-01", "proceeds": "2" });
        app.post("/corporate_actions/7/cash_in_lieu", &early)
            .await
            .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
        app.put_ok("/holding_accounts/2", &json!({ "name": "Wallet" }))
            .await;
        let other = json!({ "holding_account_id": 2, "date": "2024-06-20", "proceeds": "2" });
        let resp = app.post("/corporate_actions/7/cash_in_lieu", &other).await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.contains("no fraction"), "{body}");
        app.post("/corporate_actions/99/cash_in_lieu", &payment("2"))
            .await
            .expect_status(StatusCode::NOT_FOUND);
    }

    /// An exchange rounds each holding's replacement units onto the
    /// replacement listing's grid: 100 units 1-for-3 into listing 2, rounded
    /// down, are 17 + 16; paid in lieu, the fraction rides on the second
    /// replacement until the payment sells it on listing 2.
    #[tokio::test]
    async fn an_exchange_rounds_the_replacement_holding_and_pays_its_fraction() {
        let pool = test_pool().await;
        let app = holding(&pool).await;
        let scrip = |rule: &str| {
            json!({
                "listing_id": 1, "date": "2024-06-03", "action_type": "ScripForScrip",
                "scrip_listing_id": 2, "scrip_new_units": "1", "scrip_old_units": "3",
                "fraction_rule": rule,
            })
        };
        app.put_ok("/corporate_actions/8", &scrip("Down")).await;
        let exchanged: Value = app
            .post_empty("/corporate_actions/8/exchange")
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        let quantities: Vec<&str> = exchanged["replacements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["quantity"].as_str().unwrap())
            .collect();
        assert_eq!(quantities, ["17", "16"]);
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM corporate_action_parcel_counts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 2);
        // Undo the exchange (the counts go with its Buys), pay in lieu
        // instead.
        app.delete(format!("/sells/{}", exchanged["sell"]["id"]))
            .await
            .expect_status(StatusCode::NO_CONTENT);
        app.put_ok("/corporate_actions/8", &scrip("CashInLieu"))
            .await;
        app.post_empty("/corporate_actions/8/exchange")
            .await
            .expect_status(StatusCode::CREATED);
        let sell: Value = app
            .post("/corporate_actions/8/cash_in_lieu", &payment("1.50"))
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_eq!(sell["listing_id"], 2);
        let left = remaining(&pool, ymd(2024, 6, 20)).await;
        assert_eq!(
            left.iter().map(|r| r.2).collect::<Vec<_>>(),
            [dec("17"), dec("16")]
        );
    }

    /// A demerger rounds the demerged-entity leg the same way; the head
    /// parcels keep their own units.
    #[tokio::test]
    async fn a_demerger_rounds_the_demerged_leg() {
        let pool = test_pool().await;
        let app = holding(&pool).await;
        app.put_ok(
            "/corporate_actions/9",
            &json!({
                "listing_id": 1, "date": "2024-06-03", "action_type": "Demerger",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "3", "demerger_cost_base_pct": "20",
                "fraction_rule": "Up",
            }),
        )
        .await;
        let demerged: Value = app
            .post_empty("/corporate_actions/9/demerge")
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        let quantities = |key: &str| -> Vec<String> {
            demerged[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["quantity"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(quantities("head_replacements"), ["50", "50"]);
        assert_eq!(quantities("demerged_replacements"), ["17", "17"]);
    }
}
