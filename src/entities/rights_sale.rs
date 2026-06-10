//! Atomic rights sale/lapse: dispose of a `RightsIssue`'s rights themselves
//! (see `docs/ato/rights-issues.md` Example 39 and
//! `docs/ato/retail-premiums.md` TR 2017/4).
//!
//! Selling renounceable rights (or letting them lapse, or receiving a retail
//! premium for not taking them up) is a CGT event on the **rights**, not on
//! the original shares — the holding is unchanged, so the disposal is its own
//! row (`rights_sales`), never a Sell trade: a Sell would consume share
//! parcels. The realised-gains report reads these rows alongside ordinary
//! Sells (`source = RightsSale`) and the net-capital-gain report nets them
//! with everything else.
//!
//! Free rights are taken to have been **acquired when the original shares
//! were acquired**, so each sale carries allocations anchoring the sold
//! rights to original parcels (Buy/DRP trades of the listing dated before the
//! record date): the allocation's 12-month discount clock runs from its
//! parcel's (possibly deemed) acquisition date. Unlike a Sell's
//! `parcel_allocations`, these allocations consume nothing.
//!
//! Cost base: nil for rights issued free; `rights_cost` carries the total
//! paid to acquire the disposed rights (the purchased-rights case), which the
//! report apportions over the allocations — so nil proceeds on a paid right
//! (a lapse) realises a capital loss, while a lapsed free right is a nil/nil
//! non-event that still consumes entitlement.
//!
//! Caps, validated at write time in one transaction:
//! - **total**: rights used against the action — exercises plus sales, via
//!   the shared `rights_exercise::db_rights_used` — may not exceed the
//!   holding's record-date entitlement;
//! - **per parcel**: rights anchored to a parcel (across all sales of the
//!   action) may not exceed the entitlement that parcel's record-date units
//!   earned, so a sale can't borrow an older parcel's acquisition date for
//!   the discount.
//!
//! To keep the validated figures honest a rights sale is immutable (no PUT —
//! delete it and re-enter), the action is frozen while sales reference it
//! (`entities::corporate_action`), and an anchoring parcel Buy is frozen
//! against `PUT`/`DELETE /trades` while referenced (`entities::trade`).

use crate::entities::corporate_action::{
    self, ActionKind, sold_in_acquired_units, split_adjusted_quantity,
};
use crate::entities::rights_exercise::{db_held_at_record_date, db_rights_used, entitled_units};
use crate::entities::trade::TradeType;
use crate::infra::decimal::{parse_dec, row_dec};
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct RightsSale {
    pub id: i64,
    pub rights_action_id: i64,
    /// Sale (or lapse/expiry) date.
    pub date: NaiveDate,
    /// Rights disposed of, in record-date (as-issued) rights units.
    pub units: Decimal,
    /// Per-right capital proceeds in the issue's currency; 0 = lapse.
    pub proceeds_per_right: Decimal,
    /// Total paid to acquire the disposed rights (0 for rights issued free).
    pub rights_cost: Decimal,
    pub fx_rate: Decimal,
    pub holding_account_id: i64,
    pub allocations: Vec<RightsSaleAllocation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RightsSaleAllocation {
    pub purchase_trade_id: i64,
    pub units: Decimal,
}

fn sale_from_row(row: &SqliteRow) -> Result<RightsSale, sqlx::Error> {
    Ok(RightsSale {
        id: row.try_get("id")?,
        rights_action_id: row.try_get("rights_action_id")?,
        date: row.try_get("date")?,
        units: row_dec(row, "units")?,
        proceeds_per_right: row_dec(row, "proceeds_per_right")?,
        rights_cost: row_dec(row, "rights_cost")?,
        fx_rate: row_dec(row, "fx_rate")?,
        holding_account_id: row.try_get("holding_account_id")?,
        allocations: Vec::new(),
    })
}

#[derive(Debug, Deserialize)]
pub struct AllocationInput {
    pub purchase_trade_id: i64,
    pub units: Decimal,
}

#[derive(Debug, Deserialize)]
pub struct SellRightsBody {
    /// Sale (or lapse) date. Must not precede the issue's record date.
    pub date: NaiveDate,
    /// Rights disposed of (strictly positive, record-date rights units).
    pub units: Decimal,
    /// Per-right proceeds in the action's currency (defaults to 0 — a lapse).
    #[serde(default)]
    pub proceeds_per_right: Option<Decimal>,
    /// Total amount paid to acquire the disposed rights, in the action's
    /// currency (defaults to 0 — rights issued free have a nil cost base).
    #[serde(default)]
    pub rights_cost: Option<Decimal>,
    /// Optional foreign-per-AUD override (defaults to 1; reports prefer the
    /// ATO rate and fall back to this — see `infra::fx`).
    #[serde(default)]
    pub fx_rate: Option<Decimal>,
    /// The holding account the disposal is reported under. Defaults to the
    /// seeded default account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    /// Which original parcels the sold rights are anchored to; must sum
    /// exactly to `units`.
    pub allocations: Vec<AllocationInput>,
}

#[derive(Debug)]
pub enum SellRightsError {
    Db(sqlx::Error),
    /// No corporate action with that id.
    ActionNotFound,
    /// The action is not a RightsIssue.
    NotARightsIssue,
    /// `units` is not strictly positive.
    NonPositiveUnits,
    /// `proceeds_per_right` is negative.
    NegativeProceeds,
    /// `rights_cost` is negative.
    NegativeRightsCost,
    /// The sale date precedes the issue's record date.
    BeforeRecordDate,
    /// The allocations are empty, non-positive, or do not sum to `units`.
    AllocationsDontSum,
    /// An allocation's parcel is missing, not a Buy/DRP of the issue's
    /// listing, or not held before the record date (so it earned no rights).
    NotAnOriginalParcel,
    /// Rights anchored to a parcel (across the action's sales) exceed the
    /// entitlement that parcel's record-date units earned.
    ExceedsParcelEntitlement,
    /// Total rights used — exercises plus sales — would exceed the
    /// entitlement earned by the units held at the record date.
    ExceedsEntitlement,
}

impl From<sqlx::Error> for SellRightsError {
    fn from(e: sqlx::Error) -> Self {
        SellRightsError::Db(e)
    }
}

impl From<SellRightsError> for ApiError {
    fn from(e: SellRightsError) -> Self {
        match e {
            SellRightsError::ActionNotFound => {
                ApiError::not_found("no corporate action with that id")
            }
            SellRightsError::NotARightsIssue => {
                ApiError::unprocessable("that corporate action is not a rights issue")
            }
            SellRightsError::NonPositiveUnits => {
                ApiError::unprocessable("the number of rights sold must be greater than zero")
            }
            SellRightsError::NegativeProceeds => {
                ApiError::unprocessable("the proceeds per right cannot be negative")
            }
            SellRightsError::NegativeRightsCost => {
                ApiError::unprocessable("the rights cost cannot be negative")
            }
            SellRightsError::BeforeRecordDate => {
                ApiError::unprocessable("the sale date is before the issue's record date")
            }
            SellRightsError::AllocationsDontSum => ApiError::unprocessable(
                "the parcel allocations must be positive and sum to the rights sold",
            ),
            SellRightsError::NotAnOriginalParcel => ApiError::unprocessable(
                "an allocated parcel is not a Buy/DRP of the issue's listing held before the \
                 record date",
            ),
            SellRightsError::ExceedsParcelEntitlement => ApiError::unprocessable(
                "the rights anchored to a parcel exceed the entitlement its record-date units \
                 earned",
            ),
            SellRightsError::ExceedsEntitlement => ApiError::unprocessable(
                "the rights sold and exercised exceed the entitlement earned by the holding at \
                 the record date",
            ),
            SellRightsError::Db(err) => err.into(),
        }
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/corporate_actions/{id}/sell_rights", post(sell_rights))
        .route("/rights_sales", get(list))
        .route("/rights_sales/{id}", get(get_one).delete(delete_one))
}

/// Record a rights sale/lapse with its parcel anchoring, atomically.
pub async fn db_sell_rights(
    pool: &SqlitePool,
    action_id: i64,
    body: &SellRightsBody,
) -> Result<RightsSale, SellRightsError> {
    if body.units <= Decimal::ZERO {
        return Err(SellRightsError::NonPositiveUnits);
    }
    let proceeds_per_right = body.proceeds_per_right.unwrap_or(Decimal::ZERO);
    if proceeds_per_right < Decimal::ZERO {
        return Err(SellRightsError::NegativeProceeds);
    }
    let rights_cost = body.rights_cost.unwrap_or(Decimal::ZERO);
    if rights_cost < Decimal::ZERO {
        return Err(SellRightsError::NegativeRightsCost);
    }
    if body.allocations.is_empty()
        || body.allocations.iter().any(|a| a.units <= Decimal::ZERO)
        || body.allocations.iter().map(|a| a.units).sum::<Decimal>() != body.units
    {
        return Err(SellRightsError::AllocationsDontSum);
    }

    let mut tx = pool.begin().await?;

    let action = match corporate_action::db_get_tx(&mut *tx, action_id).await? {
        Some(a) => a,
        None => return Err(SellRightsError::ActionNotFound),
    };
    let (rights_units, rights_held_units) = match &action.kind {
        ActionKind::RightsIssue {
            rights_units,
            rights_held_units,
            ..
        } => (*rights_units, *rights_held_units),
        _ => return Err(SellRightsError::NotARightsIssue),
    };
    let record_date = action.date;
    if body.date < record_date {
        return Err(SellRightsError::BeforeRecordDate);
    }

    let splits = corporate_action::db_splits_for_listing(&mut *tx, action.listing_id).await?;

    // Total cap, shared with the exercise operation: every right used against
    // the action — exercised or sold — comes out of one entitlement.
    let held = db_held_at_record_date(&mut tx, action.listing_id, record_date, &splits).await?;
    let entitled = entitled_units(held, rights_units, rights_held_units);
    let used = db_rights_used(&mut tx, action_id, record_date, &splits).await? + body.units;
    if used > entitled {
        return Err(SellRightsError::ExceedsEntitlement);
    }

    // Per-parcel cap: each anchoring parcel must have earned the rights
    // anchored to it (cumulatively, across the action's sales), per its units
    // held when the record date arrived. In-request totals accumulate so a
    // request can't split one parcel over two allocations to dodge the cap.
    let mut in_request: HashMap<i64, Decimal> = HashMap::new();
    for alloc in &body.allocations {
        let parcel =
            sqlx::query("SELECT trade_type, listing_id, date, quantity FROM trades WHERE id = ?")
                .bind(alloc.purchase_trade_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(parcel) = parcel else {
            return Err(SellRightsError::NotAnOriginalParcel);
        };
        let parcel_listing: i64 = parcel.try_get("listing_id")?;
        let parcel_date: NaiveDate = parcel.try_get("date")?;
        if !matches!(
            parcel.try_get::<TradeType, _>("trade_type")?,
            TradeType::Buy | TradeType::DRP
        ) || parcel_listing != action.listing_id
            || parcel_date >= record_date
        {
            return Err(SellRightsError::NotAnOriginalParcel);
        }
        let parcel_qty = parse_dec("quantity", parcel.try_get("quantity")?)?;

        // The parcel's units still held when the record date arrived: its
        // as-acquired quantity minus units consumed by sales dated before the
        // record date, re-based to record-date units.
        let sold_rows = sqlx::query(
            "SELECT s.date AS sale_date, pa.quantity_allocated \
             FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
             WHERE pa.purchase_trade_id = ? AND s.date < ?",
        )
        .bind(alloc.purchase_trade_id)
        .bind(record_date)
        .fetch_all(&mut *tx)
        .await?;
        let mut sold: Vec<(NaiveDate, Decimal)> = Vec::with_capacity(sold_rows.len());
        for row in &sold_rows {
            sold.push((
                row.try_get("sale_date")?,
                parse_dec("quantity_allocated", row.try_get("quantity_allocated")?)?,
            ));
        }
        let remaining = parcel_qty - sold_in_acquired_units(&sold, &splits, parcel_date);
        let at_record = split_adjusted_quantity(remaining, &splits, parcel_date, Some(record_date));
        let parcel_entitled = entitled_units(at_record, rights_units, rights_held_units);

        let prior: Vec<String> = sqlx::query_scalar(
            "SELECT rsa.units FROM rights_sale_allocations rsa \
             JOIN rights_sales rs ON rs.id = rsa.rights_sale_id \
             WHERE rs.rights_action_id = ? AND rsa.purchase_trade_id = ?",
        )
        .bind(action_id)
        .bind(alloc.purchase_trade_id)
        .fetch_all(&mut *tx)
        .await?;
        let mut anchored = *in_request
            .get(&alloc.purchase_trade_id)
            .unwrap_or(&Decimal::ZERO);
        for units in prior {
            anchored += parse_dec("units", units)?;
        }
        anchored += alloc.units;
        if anchored > parcel_entitled {
            return Err(SellRightsError::ExceedsParcelEntitlement);
        }
        in_request.insert(alloc.purchase_trade_id, anchored);
    }

    let fx_rate = body.fx_rate.unwrap_or(Decimal::ONE);
    let result = sqlx::query(
        "INSERT INTO rights_sales \
         (rights_action_id, date, units, proceeds_per_right, rights_cost, fx_rate, \
          holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(action_id)
    .bind(body.date)
    .bind(body.units.to_string())
    .bind(proceeds_per_right.to_string())
    .bind(rights_cost.to_string())
    .bind(fx_rate.to_string())
    .bind(body.holding_account_id)
    .execute(&mut *tx)
    .await?;
    let new_id = result.last_insert_rowid();
    for alloc in &body.allocations {
        sqlx::query(
            "INSERT INTO rights_sale_allocations (rights_sale_id, purchase_trade_id, units) \
             VALUES (?, ?, ?)",
        )
        .bind(new_id)
        .bind(alloc.purchase_trade_id)
        .bind(alloc.units.to_string())
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // Read the freshly created sale back so the response is exactly what was
    // stored.
    db_get(pool, new_id)
        .await?
        .ok_or(SellRightsError::Db(sqlx::Error::RowNotFound))
}

async fn attach_allocations(
    pool: &SqlitePool,
    sales: &mut [RightsSale],
) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(
        "SELECT rights_sale_id, purchase_trade_id, units FROM rights_sale_allocations ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let mut by_sale: HashMap<i64, Vec<RightsSaleAllocation>> = HashMap::new();
    for row in &rows {
        by_sale
            .entry(row.try_get("rights_sale_id")?)
            .or_default()
            .push(RightsSaleAllocation {
                purchase_trade_id: row.try_get("purchase_trade_id")?,
                units: row_dec(row, "units")?,
            });
    }
    for sale in sales {
        sale.allocations = by_sale.remove(&sale.id).unwrap_or_default();
    }
    Ok(())
}

pub async fn db_list(pool: &SqlitePool) -> Result<Vec<RightsSale>, sqlx::Error> {
    let rows = sqlx::query("SELECT * FROM rights_sales ORDER BY date, id")
        .fetch_all(pool)
        .await?;
    let mut sales = rows
        .iter()
        .map(sale_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    attach_allocations(pool, &mut sales).await?;
    Ok(sales)
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<RightsSale>, sqlx::Error> {
    let row = sqlx::query("SELECT * FROM rights_sales WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let mut sales = vec![sale_from_row(&row)?];
    attach_allocations(pool, &mut sales).await?;
    Ok(sales.pop())
}

/// Delete a rights sale (its allocations cascade), freeing the entitlement it
/// consumed.
pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM rights_sales WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn sell_rights(
    State(pool): State<SqlitePool>,
    Path(action_id): Path<i64>,
    Json(body): Json<SellRightsBody>,
) -> Result<(StatusCode, Json<RightsSale>), ApiError> {
    let sale = db_sell_rights(&pool, action_id, &body).await?;
    Ok((StatusCode::CREATED, Json(sale)))
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<RightsSale>>, ApiError> {
    Ok(Json(db_list(&pool).await?))
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<RightsSale>, ApiError> {
    db_get(&pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("no rights sale with that id"))
}

async fn delete_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if db_delete(&pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found("no rights sale with that id"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action::CorporateAction;
    use crate::entities::{listing, rights_exercise, trade};
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64) {
        test_support::listing(id)
            .ticker("RTS")
            .name("Rights Test Co")
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    async fn insert_buy(pool: &SqlitePool, id: i64, date: NaiveDate, qty: &str) {
        test_support::buy(id, 1)
            .date(date)
            .settlement(date)
            .qty(qty.parse().unwrap())
            .price("2.00".parse().unwrap())
            .insert(pool)
            .await;
    }

    /// A 1-for-4 rights issue at $1.80, record date `date`.
    async fn insert_rights_issue(pool: &SqlitePool, id: i64, date: NaiveDate) {
        corporate_action::db_upsert(
            pool,
            &CorporateAction {
                id,
                listing_id: 1,
                date,
                kind: ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: "1.80".parse().unwrap(),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
    }

    fn body(date: NaiveDate, units: &str, parcel: i64) -> SellRightsBody {
        SellRightsBody {
            date,
            units: units.parse().unwrap(),
            proceeds_per_right: Some("0.20".parse().unwrap()),
            rights_cost: None,
            fx_rate: None,
            holding_account_id: 1,
            allocations: vec![AllocationInput {
                purchase_trade_id: parcel,
                units: units.parse().unwrap(),
            }],
        }
    }

    // DB-level tests

    #[tokio::test]
    async fn sell_rights_persists_the_sale_with_its_anchoring() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        let sale = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1))
            .await
            .unwrap();
        assert_eq!(sale.rights_action_id, 10);
        assert_eq!(sale.date, d(2024, 7, 20));
        assert_eq!(sale.units, Decimal::from(250));
        assert_eq!(sale.proceeds_per_right, "0.20".parse::<Decimal>().unwrap());
        assert_eq!(sale.rights_cost, Decimal::ZERO);
        assert_eq!(sale.allocations.len(), 1);
        assert_eq!(sale.allocations[0].purchase_trade_id, 1);
        assert_eq!(sale.allocations[0].units, Decimal::from(250));

        // The holding is untouched: selling rights consumes no share parcel.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].remaining_quantity, Decimal::from(1000));
    }

    /// The entitlement is one pool shared with exercises, in both directions:
    /// rights already sold block an exercise and vice versa.
    #[tokio::test]
    async fn entitlement_cap_is_shared_with_exercises() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await; // entitled to 250

        // Exercise 100, sell 150 — the entitlement is exactly consumed.
        rights_exercise::db_exercise(
            &pool,
            10,
            &rights_exercise::ExerciseBody {
                date: d(2024, 7, 10),
                units: "100".parse().unwrap(),
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
            },
        )
        .await
        .unwrap();
        db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "150", 1))
            .await
            .unwrap();

        // No room left on either path.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 21), "1", 1)).await;
        assert!(matches!(err, Err(SellRightsError::ExceedsEntitlement)));
        let err = rights_exercise::db_exercise(
            &pool,
            10,
            &rights_exercise::ExerciseBody {
                date: d(2024, 7, 21),
                units: Decimal::ONE,
                rights_cost: None,
                fx_rate: None,
                holding_account_id: 1,
            },
        )
        .await;
        assert!(matches!(
            err,
            Err(rights_exercise::ExerciseError::ExceedsEntitlement)
        ));

        // Deleting the sale frees its share of the entitlement again.
        let sale_id: i64 = sqlx::query_scalar("SELECT id FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(db_delete(&pool, sale_id).await.unwrap());
        db_sell_rights(&pool, 10, &body(d(2024, 7, 22), "150", 1))
            .await
            .unwrap();
    }

    /// A parcel only anchors the rights its own record-date units earned —
    /// the cap can't be dodged by splitting one parcel across allocations or
    /// across successive sales.
    #[tokio::test]
    async fn per_parcel_anchoring_is_capped_at_the_parcels_entitlement() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        // Two parcels: 800 units (entitled 200) and 200 units (entitled 50).
        insert_buy(&pool, 1, d(2023, 1, 15), "800").await;
        insert_buy(&pool, 2, d(2024, 6, 1), "200").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        // Anchoring 201 rights to the old parcel (to borrow its >12-month
        // acquisition date) is rejected — in one allocation or split in two.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "201", 1)).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ExceedsParcelEntitlement)
        ));
        let mut split = body(d(2024, 7, 20), "201", 1);
        split.allocations = vec![
            AllocationInput {
                purchase_trade_id: 1,
                units: "200".parse().unwrap(),
            },
            AllocationInput {
                purchase_trade_id: 1,
                units: Decimal::ONE,
            },
        ];
        let err = db_sell_rights(&pool, 10, &split).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ExceedsParcelEntitlement)
        ));

        // 200 + 50 across the two parcels is the whole entitlement.
        let mut ok = body(d(2024, 7, 20), "250", 1);
        ok.allocations = vec![
            AllocationInput {
                purchase_trade_id: 1,
                units: "200".parse().unwrap(),
            },
            AllocationInput {
                purchase_trade_id: 2,
                units: "50".parse().unwrap(),
            },
        ];
        db_sell_rights(&pool, 10, &ok).await.unwrap();

        // A later sale finds the entitlement exhausted (the total cap fires
        // before the per-parcel one).
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 21), "1", 2)).await;
        assert!(matches!(err, Err(SellRightsError::ExceedsEntitlement)));
    }

    /// Units sold out of a parcel before the record date earned no rights:
    /// the parcel's anchoring cap reflects its record-date remainder. A
    /// second untouched parcel keeps the *total* entitlement roomy, so the
    /// rejection is specifically the per-parcel cap.
    #[tokio::test]
    async fn parcel_cap_reflects_units_sold_before_the_record_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_buy(&pool, 3, d(2024, 2, 1), "1000").await;
        test_support::sell(2, 1)
            .date(d(2024, 5, 1))
            .qty("600".parse().unwrap())
            .price("2.00".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::allocate(&pool, 1, 2, 1, "600".parse().unwrap()).await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;

        // Parcel 1 has 400 of its 1000 left at the record date → it anchors
        // at most 100, even though the holding's total entitlement is 350.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "101", 1)).await;
        assert!(matches!(
            err,
            Err(SellRightsError::ExceedsParcelEntitlement)
        ));
        db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "100", 1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn invalid_sales_are_rejected_and_nothing_persisted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_buy(&pool, 2, d(2024, 7, 1), "400").await; // ex-rights — earned nothing
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 11,
                listing_id: 1,
                date: d(2024, 7, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        let err = db_sell_rights(&pool, 999, &body(d(2024, 7, 20), "10", 1)).await;
        assert!(matches!(err, Err(SellRightsError::ActionNotFound)));
        let err = db_sell_rights(&pool, 11, &body(d(2024, 7, 20), "10", 1)).await;
        assert!(matches!(err, Err(SellRightsError::NotARightsIssue)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 6, 30), "10", 1)).await;
        assert!(matches!(err, Err(SellRightsError::BeforeRecordDate)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "0", 1)).await;
        assert!(matches!(err, Err(SellRightsError::NonPositiveUnits)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "-5", 1)).await;
        assert!(matches!(err, Err(SellRightsError::NonPositiveUnits)));

        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.proceeds_per_right = Some("-0.20".parse().unwrap());
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(err, Err(SellRightsError::NegativeProceeds)));

        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.rights_cost = Some("-1".parse().unwrap());
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(err, Err(SellRightsError::NegativeRightsCost)));

        // Allocations must be present, positive, and sum to the units.
        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.allocations.clear();
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(err, Err(SellRightsError::AllocationsDontSum)));
        let mut bad = body(d(2024, 7, 20), "10", 1);
        bad.allocations[0].units = "9".parse().unwrap();
        let err = db_sell_rights(&pool, 10, &bad).await;
        assert!(matches!(err, Err(SellRightsError::AllocationsDontSum)));

        // Anchoring to a missing parcel, or to one dated on the record date
        // (ex-rights), is rejected.
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "10", 999)).await;
        assert!(matches!(err, Err(SellRightsError::NotAnOriginalParcel)));
        let err = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "10", 2)).await;
        assert!(matches!(err, Err(SellRightsError::NotAnOriginalParcel)));

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "no rejected sale may persist a row");
    }

    /// The action the sales were validated against is frozen while they
    /// reference it, exactly like exercise trades.
    #[tokio::test]
    async fn referenced_action_cannot_be_edited_or_deleted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let sale = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1))
            .await
            .unwrap();

        let action = corporate_action::db_get(&pool, 10).await.unwrap().unwrap();
        let err = corporate_action::db_upsert(&pool, &action).await;
        assert!(matches!(
            err,
            Err(corporate_action::WriteError::ReferencedByTrade)
        ));
        let err = corporate_action::db_delete(&pool, 10).await;
        assert!(err.is_err(), "the rights_sales FK must block the delete");

        // Removing the sale unfreezes the action.
        assert!(db_delete(&pool, sale.id).await.unwrap());
        corporate_action::db_upsert(&pool, &action).await.unwrap();
        assert!(corporate_action::db_delete(&pool, 10).await.unwrap());
    }

    /// An anchoring parcel Buy is frozen against free-form trade edits and
    /// individual deletion while the sale references it — its date and
    /// quantity were what the anchoring caps validated.
    #[tokio::test]
    async fn anchoring_parcel_is_frozen_while_referenced() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let sale = db_sell_rights(&pool, 10, &body(d(2024, 7, 20), "250", 1))
            .await
            .unwrap();

        let parcel = trade::db_get(&pool, 1).await.unwrap().unwrap();
        let err = trade::db_upsert(&pool, &parcel).await;
        assert!(matches!(err, Err(trade::UpsertError::RightsAnchorParcel)));
        assert_eq!(
            trade::db_delete(&pool, 1).await.unwrap(),
            trade::DeleteOutcome::Referenced
        );

        // Deleting the sale unfreezes the parcel.
        assert!(db_delete(&pool, sale.id).await.unwrap());
        trade::db_upsert(&pool, &parcel).await.unwrap();
    }

    // API-level tests

    #[tokio::test]
    async fn api_sell_rights_returns_201_then_lists_gets_and_deletes() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let app = router().with_state(pool.clone());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/corporate_actions/10/sell_rights")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "date": "2024-07-20",
                            "units": "250",
                            "proceeds_per_right": "0.20",
                            "allocations": [
                                { "purchase_trade_id": 1, "units": "250" },
                            ],
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let sale: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(sale["units"], "250");
        assert_eq!(sale["proceeds_per_right"], "0.20");
        assert_eq!(sale["allocations"][0]["purchase_trade_id"], 1);
        let id = sale["id"].as_i64().unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/rights_sales")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let sales: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(sales.as_array().unwrap().len(), 1);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/rights_sales/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/rights_sales/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/rights_sales/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_invalid_sales_return_404_or_422_with_a_reason() {
        let pool = test_pool().await;
        insert_listing(&pool, 1).await;
        insert_buy(&pool, 1, d(2024, 1, 15), "1000").await;
        insert_rights_issue(&pool, 10, d(2024, 7, 1)).await;
        let app = router().with_state(pool.clone());

        let cases: Vec<(i64, serde_json::Value, StatusCode)> = vec![
            (
                999,
                serde_json::json!({
                    "date": "2024-07-20", "units": "10",
                    "allocations": [{ "purchase_trade_id": 1, "units": "10" }],
                }),
                StatusCode::NOT_FOUND,
            ),
            (
                10,
                serde_json::json!({
                    "date": "2024-07-20", "units": "251",
                    "allocations": [{ "purchase_trade_id": 1, "units": "251" }],
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                10,
                serde_json::json!({
                    "date": "2024-07-20", "units": "10",
                    "allocations": [{ "purchase_trade_id": 1, "units": "9" }],
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                10,
                serde_json::json!({
                    "date": "2024-06-30", "units": "10",
                    "allocations": [{ "purchase_trade_id": 1, "units": "10" }],
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ];
        for (action_id, json, expected) in cases {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/corporate_actions/{action_id}/sell_rights"))
                        .header("content-type", "application/json")
                        .body(Body::from(json.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), expected);
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            assert!(
                !bytes.is_empty(),
                "a {expected} rejection must carry a reason body"
            );
        }

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rights_sales")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }
}
