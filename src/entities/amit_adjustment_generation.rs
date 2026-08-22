//! Generating an AMMA statement's per-parcel AMIT adjustments.
//!
//! Entering an AMMA statement creates nothing else: the `amit_adjustments`
//! rows that apply its per-unit `cost_base_adjustment` to each parcel are a
//! separate, per-parcel entry (FY2025 VDHG needs 30 of them). Typing them is
//! where the set goes wrong — a missed parcel overstates the cost base of the
//! units it covers, and CGT event E10's nil floor can turn an over-reduction
//! into a capital gain that was never made
//! (`docs/ato/amit-cost-base-adjustments.md`). This operation derives the set
//! instead: one adjustment per parcel open at the statement's
//! `tax_year_end_date`, on the statement's own listing and holding account.
//!
//! The parcels come from [`crate::domain::open_parcels::load`] as at that
//! date, so the "what was held?" question is answered by the same loader
//! every open-holdings report uses rather than a second walk. Each row is
//! written **through [`amit_adjustment::db_upsert_on`]** inside one
//! transaction, so the generated rows pass exactly the per-row invariants a
//! typed row does and land in the `row_history` audit trail the same way; a
//! partial set can never persist.
//!
//! Reconciling Σ against the statement's `units_held` is deliberately *not*
//! an invariant: a statement may state units at a date other than year end,
//! and a mismatch is a reconciliation question, not a reason to refuse a
//! write. The response carries both figures and their difference, and
//! [`crate::reports::amit_adjustment_cross_check`] keeps the statement
//! flagged until it is resolved.

use crate::domain::open_parcels;
use crate::entities::amit_adjustment::{self, AmitAdjustment};
use crate::entities::corporate_action;
use crate::infra::decimal::row_dec;
use crate::infra::http::ApiError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::post,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Default, Deserialize)]
pub struct GenerateBody {
    /// Delete the statement's existing adjustments and regenerate them in the
    /// same transaction. Without it, a statement that already has adjustments
    /// is refused — the common repair path is to correct the missed trade and
    /// re-run with `replace`.
    #[serde(default)]
    pub replace: bool,
    /// Compute the set and answer with it, writing nothing. Every refusal
    /// below still applies, so a preview answers the same 422 the write
    /// would: it is the confirm step's "are the current positions correct?"
    /// check, not a weaker one.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Serialize)]
pub struct GeneratedAdjustments {
    /// The adjustment rows created — or, for a preview, the rows that would
    /// be, carrying the ids they would take.
    pub created: Vec<AmitAdjustment>,
    /// Σ of the generated quantities re-based into the statement year's unit
    /// basis, so it is comparable with `units_held` (the stored quantities
    /// are in each parcel's as-acquired units).
    pub units_adjusted: Decimal,
    /// The statement's own `units_held`, verbatim.
    pub units_held: Decimal,
    /// `units_adjusted − units_held`. Non-zero is surfaced, never blocking.
    pub difference: Decimal,
    /// True when nothing was written.
    pub preview: bool,
    /// Units the statement covered that a **scrip-for-scrip exchange or
    /// demerger** has since carried into replacement parcels, which this
    /// operation does not attribute for you: those replacements' quantities are
    /// ratio-scaled and the data records no link between a replacement and the
    /// individual parcel it replaced, so choosing one would be a guess
    /// (SCENARIOS N-06). A **transfer** is attributed — its replacement carries
    /// exactly the units moved, under the source parcel's own acquisition date —
    /// so these entries are the residue. Enter one adjustment by hand against
    /// each named replacement parcel: that is accepted now, because the units
    /// are traceable back to this statement's account.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub unattributed: Vec<UnattributedUnits>,
}

/// One source parcel's units that a rollover carried away and generation left
/// for hand entry — see [`GeneratedAdjustments::unattributed`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnattributedUnits {
    /// The parcel the statement's account held at year end.
    pub source_trade_id: i64,
    /// Units of it the operation carried away, in the operation date's basis.
    pub units: Decimal,
    /// The replacement parcels of that operation, one of which now holds them.
    pub replacements: Vec<i64>,
}

#[derive(thiserror::Error, Debug)]
pub enum GenerateError {
    #[error("AMIT adjustment generation failed: {0}")]
    Db(#[from] sqlx::Error),
    #[error("no AMMA statement with that id")]
    StatementNotFound,
    /// The statement already has adjustments and `replace` was not set.
    #[error("this AMMA statement already has AMIT adjustments")]
    AlreadyGenerated,
    /// Nothing of the statement's listing was open in its holding account at
    /// the statement's year end. Generation derives its set from the parcels
    /// open at that date, so there is nothing for it to write — and writing
    /// an empty set would hide the two quite different situations that reach
    /// here: the position's trades have not been entered yet, or the holding
    /// was genuinely sold (or moved) away *during* the year, whose adjustment
    /// rows are entered by hand (SCENARIOS F-04). The refusal names both, and
    /// says where the hand-entered row goes in each case: against the parcels
    /// the units came from for a sale, and against the **replacement parcel**
    /// for a transfer/exchange/demerger, which is accepted because the units are
    /// traceable back to this statement's account (SCENARIOS N-06). A move
    /// *after* the year end needs none of this — the parcel is still open at the
    /// year end, and generation follows the operation itself.
    #[error("no open parcels at the statement's year end")]
    NothingHeld,
    #[error(transparent)]
    Upsert(#[from] amit_adjustment::UpsertError),
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/amma_statements/{id}/generate_adjustments", post(generate))
}

/// One rollover that carried units out of a parcel: the operation's date, the
/// units it took (in that date's basis, as the allocation records them), and the
/// replacement parcels it created.
struct Movement {
    date: NaiveDate,
    units: Decimal,
    /// Whether the operation was a holding-account transfer, whose replacement
    /// carries exactly the units moved.
    is_transfer: bool,
    /// The group's replacement parcels of the **same listing** (a demerger's
    /// demerged parcel is another listing and never adjustable by this
    /// statement), each with its quantity and carried acquisition date.
    replacements: Vec<i64>,
    replacement_facts: Vec<(i64, Decimal, Option<NaiveDate>)>,
}

impl Movement {
    /// The replacement parcel holding `units` of a parcel acquired on
    /// `acquired`, where that can be told without guessing: a transfer's
    /// replacement carries exactly those units under that acquisition date. A
    /// scrip exchange or demerger scales the quantity by its own ratio, so
    /// there is no such match and the caller reports the units instead.
    fn matching_replacement(&self, acquired: NaiveDate, units: Decimal) -> Option<i64> {
        if !self.is_transfer {
            return None;
        }
        self.replacement_facts
            .iter()
            .find(|(_, quantity, deemed)| *quantity == units && *deemed == Some(acquired))
            .map(|(id, _, _)| *id)
    }
}

/// Every rollover that consumed part of `parcel_id`, oldest first.
async fn db_rollovers_of(
    conn: &mut sqlx::SqliteConnection,
    parcel_id: i64,
) -> Result<Vec<Movement>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT s.date, pa.quantity_allocated, s.listing_id, \
                s.transfer_id, s.scrip_action_id, s.demerger_action_id \
         FROM parcel_allocations pa JOIN trades s ON s.id = pa.sale_trade_id \
         WHERE pa.purchase_trade_id = ? \
           AND (s.transfer_id IS NOT NULL OR s.scrip_action_id IS NOT NULL \
                OR s.demerger_action_id IS NOT NULL) \
         ORDER BY s.date, s.id",
    )
    .bind(parcel_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let listing_id: i64 = row.try_get("listing_id")?;
        let (column, group_id, is_transfer) =
            if let Some(id) = row.try_get::<Option<i64>, _>("transfer_id")? {
                ("transfer_id", id, true)
            } else if let Some(id) = row.try_get::<Option<i64>, _>("scrip_action_id")? {
                ("scrip_action_id", id, false)
            } else {
                let id: Option<i64> = row.try_get("demerger_action_id")?;
                ("demerger_action_id", id.unwrap_or_default(), false)
            };
        // The column name is one of the three literals above, never user input.
        let replacement_rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT id, quantity, deemed_acquisition_date FROM trades \
             WHERE {column} = ? AND trade_type IN ('Buy', 'DRP') AND listing_id = ? ORDER BY id"
        )))
        .bind(group_id)
        .bind(listing_id)
        .fetch_all(&mut *conn)
        .await?;
        let mut replacement_facts = Vec::with_capacity(replacement_rows.len());
        for r in &replacement_rows {
            replacement_facts.push((
                r.try_get("id")?,
                row_dec(r, "quantity")?,
                r.try_get("deemed_acquisition_date")?,
            ));
        }
        out.push(Movement {
            date: row.try_get("date")?,
            units: row_dec(row, "quantity_allocated")?,
            is_transfer,
            replacements: replacement_facts.iter().map(|(id, _, _)| *id).collect(),
            replacement_facts,
        });
    }
    Ok(out)
}

/// Generate (or preview) the statement's per-parcel adjustment set.
pub async fn db_generate(
    pool: &SqlitePool,
    statement_id: i64,
    body: &GenerateBody,
) -> Result<GeneratedAdjustments, GenerateError> {
    let mut tx = pool.begin().await?;

    let row = sqlx::query(
        "SELECT listing_id, holding_account_id, tax_year_end_date, units_held \
         FROM amma_statements WHERE id = ?",
    )
    .bind(statement_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(GenerateError::StatementNotFound);
    };
    let listing_id: i64 = row.try_get("listing_id")?;
    let holding_account_id: i64 = row.try_get("holding_account_id")?;
    let year_end: NaiveDate = row.try_get("tax_year_end_date")?;
    let units_held = row_dec(&row, "units_held")?;

    let existing: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM amit_adjustments WHERE amma_statement_id = ?")
            .bind(statement_id)
            .fetch_all(&mut *tx)
            .await?;
    if !existing.is_empty() && !body.replace {
        return Err(GenerateError::AlreadyGenerated);
    }

    // What was actually held at the statement's year end, from the shared
    // open-parcels loader, narrowed to the statement's own listing and
    // holding account (a registry issues one statement per holder account).
    let parcels: Vec<_> = open_parcels::load(&mut tx, Some(year_end))
        .await?
        .into_iter()
        .filter(|p| {
            p.parcel.listing_id == listing_id && p.parcel.holding_account_id == holding_account_id
        })
        .collect();
    if parcels.is_empty() {
        return Err(GenerateError::NothingHeld);
    }

    // Parcels either side of a split reach the year end on different unit
    // bases, and that is fine: the statement's per-unit figure is per unit as
    // the statement year saw them, and `amit_adjustment::reduction_for`
    // re-bases each parcel's stored as-acquired quantity into that basis
    // before multiplying. Splits are still needed here to store each
    // quantity in the parcel's own as-acquired basis.
    let splits = corporate_action::db_splits_for_listing(&mut *tx, listing_id).await?;

    if body.replace {
        // Same transaction as the regeneration below, so a failure mid-way
        // leaves the original set standing.
        sqlx::query("DELETE FROM amit_adjustments WHERE amma_statement_id = ?")
            .bind(statement_id)
            .execute(&mut *tx)
            .await?;
    }

    let mut next_id: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM amit_adjustments")
            .fetch_one(&mut *tx)
            .await?;
    let mut created = Vec::with_capacity(parcels.len());
    let mut units_adjusted = Decimal::ZERO;
    let mut unattributed = Vec::new();
    for parcel in &parcels {
        // `remaining_as_of` is in the year-end unit basis; the quantity
        // column stores as-acquired units (the basis `trade.quantity` caps).
        let quantity = corporate_action::as_acquired_quantity(
            parcel.remaining_as_of,
            &splits,
            parcel.parcel.date,
            year_end,
        );
        // Units of this parcel a rollover has since carried into a replacement
        // parcel cannot be adjusted here — the replacement took their cost base
        // with it (`amit_adjustment::UpsertError::UnitsCarriedIntoReplacement`),
        // and the statement's reduction has to follow them. This is the ordinary
        // case, not an edge one: an AMMA statement for a year ended 30 June
        // arrives in August or September, so any move in between lands in it
        // (SCENARIOS N-06).
        let moves = db_rollovers_of(&mut tx, parcel.parcel.id).await?;
        let mut source_quantity = quantity;
        for movement in &moves {
            let carried = corporate_action::as_acquired_quantity(
                movement.units,
                &splits,
                parcel.parcel.date,
                movement.date,
            );
            let from_this_statement = source_quantity.min(carried);
            if from_this_statement <= Decimal::ZERO {
                continue;
            }
            source_quantity -= from_this_statement;
            // A transfer's replacement carries exactly the units moved under the
            // source parcel's own acquisition date, so it is identified without
            // guessing. A scrip exchange or demerger scales the quantity by its
            // ratio and (for a demerger) creates two parcels, and nothing links
            // a replacement to the individual parcel it replaced — so those are
            // reported for hand entry rather than attributed by inference.
            match movement.matching_replacement(parcel.parcel.acquired(), movement.units) {
                Some(replacement) => {
                    let adjustment = AmitAdjustment {
                        id: next_id,
                        amma_statement_id: statement_id,
                        trade_id: replacement,
                        // Both figures are in the operation date's basis.
                        quantity: movement.units,
                    };
                    if !body.preview {
                        amit_adjustment::db_upsert_on(&mut tx, &adjustment).await?;
                    }
                    next_id += 1;
                    units_adjusted += corporate_action::split_adjusted_quantity(
                        from_this_statement,
                        &splits,
                        parcel.parcel.date,
                        Some(year_end),
                    );
                    created.push(adjustment);
                }
                None => unattributed.push(UnattributedUnits {
                    source_trade_id: parcel.parcel.id,
                    units: movement.units,
                    replacements: movement.replacements.clone(),
                }),
            }
        }
        if source_quantity > Decimal::ZERO {
            let adjustment = AmitAdjustment {
                id: next_id,
                amma_statement_id: statement_id,
                trade_id: parcel.parcel.id,
                quantity: source_quantity,
            };
            if !body.preview {
                amit_adjustment::db_upsert_on(&mut tx, &adjustment).await?;
            }
            next_id += 1;
            units_adjusted += corporate_action::split_adjusted_quantity(
                source_quantity,
                &splits,
                parcel.parcel.date,
                Some(year_end),
            );
            created.push(adjustment);
        }
    }

    if body.preview {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }

    Ok(GeneratedAdjustments {
        created,
        units_adjusted,
        units_held,
        difference: units_adjusted - units_held,
        preview: body.preview,
        unattributed,
    })
}

async fn generate(
    State(pool): State<SqlitePool>,
    Path(statement_id): Path<i64>,
    body: Option<Json<GenerateBody>>,
) -> Result<(StatusCode, Json<GeneratedAdjustments>), ApiError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let result = db_generate(&pool, statement_id, &body).await?;
    let status = if result.preview {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(result)))
}

impl From<GenerateError> for ApiError {
    fn from(e: GenerateError) -> Self {
        match e {
            GenerateError::StatementNotFound => {
                ApiError::not_found("no AMMA statement with that id")
            }
            GenerateError::AlreadyGenerated => ApiError::unprocessable(
                "this AMMA statement already has AMIT adjustments — re-run with \
                 \"replace\": true to delete and regenerate them",
            ),
            GenerateError::NothingHeld => ApiError::unprocessable(
                "no parcels of the statement's listing were open in its holding account at the \
                 statement's year end, so there is nothing to generate from — if trades are \
                 missing, enter them and run this again; if the holding was sold during the year, \
                 the statement still adjusts the units it covered, so enter one AMIT adjustment by \
                 hand against each parcel those units came from; and if it was transferred, \
                 exchanged or demerged away during the year, enter them against the replacement \
                 parcels that now hold those units, which is accepted because the units trace back \
                 to this account",
            ),
            GenerateError::Upsert(err) => err.into(),
            GenerateError::Db(err) => err.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::corporate_action::{ActionKind, CorporateAction};
    use crate::test_support::{self, ApiClient, allocate, dec, test_pool, ymd};

    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn amit_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .amit(true)
            .insert(pool)
            .await;
    }

    async fn amma(pool: &SqlitePool, id: i64, listing_id: i64, year_end: NaiveDate, units: &str) {
        test_support::amma(id, listing_id)
            .units(dec(units))
            .cost_base_adjustment(dec("-2.0257290076"))
            .with(|a| {
                a.tax_year_end_date = year_end;
                a.date_received = year_end + chrono::Duration::days(60);
            })
            .insert(pool)
            .await;
    }

    /// The HNDQ holding as the live database records it: two 2024 buys, two
    /// DRP allotments and a later buy inside FY2025, and one DRP allotment
    /// after 30 June 2025.
    async fn hndq_parcels(pool: &SqlitePool) {
        amit_listing(pool, 1, "HNDQ").await;
        for (id, date, qty, drp) in [
            (18, ymd(2024, 2, 28), "509", false),
            (19, ymd(2024, 3, 1), "1302", false),
            (61, ymd(2024, 7, 16), "32", true),
            (20, ymd(2024, 7, 30), "776", false),
            (64, ymd(2025, 1, 17), "1", true),
            (67, ymd(2025, 7, 16), "51", true),
        ] {
            let builder = if drp {
                test_support::drp(id, 1)
            } else {
                test_support::buy(id, 1)
            };
            builder.date(date).qty(dec(qty)).insert(pool).await;
        }
    }

    /// The empirical case the requirement is built on: generation reproduces
    /// the hand-entered HNDQ FY2024 and FY2025 sets exactly — including
    /// excluding the 2025-07-16 DRP from the FY2025 statement.
    #[tokio::test]
    async fn db_generation_reproduces_the_hand_entered_hndq_sets() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1811").await;
        amma(&pool, 7, 1, ymd(2025, 6, 30), "2620").await;

        let fy24 = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(
            fy24.created
                .iter()
                .map(|a| (a.trade_id, a.quantity))
                .collect::<Vec<_>>(),
            vec![(18, dec("509")), (19, dec("1302"))]
        );
        assert_eq!(fy24.units_adjusted, dec("1811"));
        assert_eq!(fy24.units_held, dec("1811"));
        assert_eq!(fy24.difference, Decimal::ZERO);
        assert!(!fy24.preview);

        let fy25 = db_generate(&pool, 7, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(
            fy25.created
                .iter()
                .map(|a| (a.trade_id, a.quantity))
                .collect::<Vec<_>>(),
            vec![
                (18, dec("509")),
                (19, dec("1302")),
                (20, dec("776")),
                (61, dec("32")),
                (64, dec("1")),
            ]
        );
        assert_eq!(fy25.units_adjusted, dec("2620"));
        assert_eq!(fy25.difference, Decimal::ZERO);

        // And the cross-check now reconciles both statements.
        assert!(
            crate::reports::amit_adjustment_cross_check::db_amit_adjustment_alerts(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A preview computes the same set and writes nothing.
    #[tokio::test]
    async fn db_preview_writes_nothing() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1811").await;

        let preview = db_generate(
            &pool,
            6,
            &GenerateBody {
                replace: false,
                preview: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(preview.created.len(), 2);
        assert_eq!(preview.units_adjusted, dec("1811"));
        assert!(preview.preview);

        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM amit_adjustments")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 0);
    }

    /// A mismatch against `units_held` is surfaced, not blocked — a statement
    /// may state units at a date other than year end.
    #[tokio::test]
    async fn db_units_held_mismatch_is_reported_not_refused() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1800").await;

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(result.units_adjusted, dec("1811"));
        assert_eq!(result.units_held, dec("1800"));
        assert_eq!(result.difference, dec("11"));
        // The rows were still written, and the cross-check keeps it flagged.
        assert_eq!(result.created.len(), 2);
        let alerts = crate::reports::amit_adjustment_cross_check::db_amit_adjustment_alerts(&pool)
            .await
            .unwrap();
        assert_eq!(alerts.len(), 1);
    }

    /// Generated rows go through `db_upsert`, so they are audited exactly as
    /// hand-entered ones are.
    #[tokio::test]
    async fn db_generated_rows_are_audited() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1811").await;
        db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();

        // Regenerating deletes and rewrites: the DELETEs are what the audit
        // trail records (an INSERT is not an audited event).
        db_generate(
            &pool,
            6,
            &GenerateBody {
                replace: true,
                preview: false,
            },
        )
        .await
        .unwrap();
        let audited: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM row_history WHERE table_name = 'amit_adjustments'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audited, 2);
    }

    /// A statement that already has adjustments is refused, and `replace`
    /// deletes and regenerates in one transaction.
    #[tokio::test]
    async fn db_existing_adjustments_are_refused_unless_replacing() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1811").await;
        db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();

        let err = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap_err();
        assert!(matches!(err, GenerateError::AlreadyGenerated));

        // The repair path: a trade entered after the statement, then re-run
        // with replace.
        test_support::buy(70, 1)
            .date(ymd(2024, 5, 1))
            .qty(dec("100"))
            .insert(&pool)
            .await;
        let redone = db_generate(
            &pool,
            6,
            &GenerateBody {
                replace: true,
                preview: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(redone.created.len(), 3);
        assert_eq!(redone.units_adjusted, dec("1911"));
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM amit_adjustments")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 3);
    }

    /// A statement for a position the system doesn't have is itself the
    /// error — an empty set would hide it.
    #[tokio::test]
    async fn db_no_open_parcels_is_refused() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2022, 2, 1))
            .qty(dec("100"))
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2023, 2, 1))
            .qty(dec("100"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("100")).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "100").await;

        let err = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap_err();
        assert!(matches!(err, GenerateError::NothingHeld));
    }

    /// Only the statement's own holding account is covered: another
    /// account's parcels of the same listing are not generated (and, when
    /// they are all there is, the statement is refused).
    #[tokio::test]
    async fn db_only_the_statements_holding_account_is_covered() {
        use crate::entities::holding_account::{self, HoldingAccount};
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(ymd(2024, 2, 1))
            .qty(dec("100"))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("50"))
            .with(|t| t.holding_account_id = 2)
            .insert(&pool)
            .await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "100").await;

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(result.created.len(), 1);
        assert_eq!(result.created[0].trade_id, 1);
    }

    /// SCENARIOS E-13: the fund splits *after* the statement's year end but
    /// before its adjustments are generated. The statement's per-unit figure
    /// is stated in the units held at its own year end, so the generation
    /// (and the reduction it produces) must ignore the later split entirely —
    /// the parcel's 100 units, not the 200 it has become by generation day.
    #[tokio::test]
    async fn db_a_split_after_the_year_end_does_not_change_the_reduction() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        test_support::amma(6, 1)
            .units(dec("100"))
            .cost_base_adjustment(dec("0.50"))
            .with(|a| {
                a.tax_year_end_date = ymd(2024, 6, 30);
                a.date_received = ymd(2024, 8, 29);
            })
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 8, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(
            result
                .created
                .iter()
                .map(|a| (a.trade_id, a.quantity))
                .collect::<Vec<_>>(),
            vec![(1, dec("100"))]
        );
        assert_eq!(result.units_adjusted, dec("100"));
        assert_eq!(result.difference, Decimal::ZERO);

        // 100 units × 50c — the post-split 200 units never enter it.
        let mut conn = pool.acquire().await.unwrap();
        let events = amit_adjustment::db_cost_base_reduction_events(&mut conn, None)
            .await
            .unwrap();
        let stated: Decimal = events[&1].iter().map(|e| e.amount()).sum();
        assert_eq!(stated, dec("50"));
    }

    /// A split between one covered parcel's acquisition and the year end
    /// leaves the parcels on different unit bases at that date — which is the
    /// basis the statement's per-unit figure is stated in. Both are covered,
    /// each stored in its own as-acquired units, and the **money** is right
    /// on each because `amit_adjustment::reduction_for` re-bases the stored
    /// quantity into the year-end basis before multiplying. (Asserting only
    /// the quantities is what let SCENARIOS B-24 through: the counts
    /// reconciled while the reductions were halved.)
    #[tokio::test]
    async fn db_a_split_across_covered_parcels_is_costed_on_the_year_end_basis() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 1, 15),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("50"))
            .price(dec("5"))
            .insert(&pool)
            .await;
        // FY2024: 250 units at the year end (200 post-split + 50), the fund's
        // cost base adjustment 50c per unit *of that year's units*.
        test_support::amma(6, 1)
            .units(dec("250"))
            .cost_base_adjustment(dec("0.50"))
            .with(|a| {
                a.tax_year_end_date = ymd(2024, 6, 30);
                a.date_received = ymd(2024, 8, 29);
            })
            .insert(&pool)
            .await;

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(
            result
                .created
                .iter()
                .map(|a| (a.trade_id, a.quantity))
                .collect::<Vec<_>>(),
            // Stored as-acquired, reported (and reconciled) in the year's basis.
            vec![(1, dec("100")), (2, dec("50"))]
        );
        assert_eq!(result.units_adjusted, dec("250"));
        assert_eq!(result.difference, Decimal::ZERO);

        // The money the fund actually took off: parcel 1's 100 as-acquired
        // units are 200 units at the year end, so $100 — not the $50 a naive
        // quantity × per-unit multiplication gives.
        let mut conn = pool.acquire().await.unwrap();
        let events = amit_adjustment::db_cost_base_reduction_events(&mut conn, None)
            .await
            .unwrap();
        let stated =
            |trade_id: i64| -> Decimal { events[&trade_id].iter().map(|e| e.amount()).sum() };
        assert_eq!(stated(1), dec("100"));
        assert_eq!(stated(2), dec("25"));
    }

    // API-level tests

    #[tokio::test]
    async fn api_generate_returns_201_with_the_created_rows() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1811").await;

        let resp = client(&pool)
            .post(
                "/amma_statements/6/generate_adjustments",
                &serde_json::json!({}),
            )
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["created"].as_array().unwrap().len(), 2);
        assert_eq!(body["units_adjusted"], "1811");
        assert_eq!(body["units_held"], "1811");
        assert_eq!(body["difference"], "0");
    }

    #[tokio::test]
    async fn api_preview_returns_200_and_writes_nothing() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1811").await;

        let resp = client(&pool)
            .post(
                "/amma_statements/6/generate_adjustments",
                &serde_json::json!({ "preview": true }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let body: serde_json::Value = resp.json();
        assert_eq!(body["preview"], true);
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM amit_adjustments")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 0);
    }

    #[tokio::test]
    async fn api_missing_statement_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool)
            .post(
                "/amma_statements/999/generate_adjustments",
                &serde_json::json!({}),
            )
            .await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
        assert!(resp.text().contains("no AMMA statement with that id"));
    }

    #[tokio::test]
    async fn api_each_refusal_returns_422_naming_the_reason() {
        let pool = test_pool().await;
        hndq_parcels(&pool).await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1811").await;
        // Nothing held: a statement on a listing with no parcels at all.
        amit_listing(&pool, 2, "VDHG").await;
        amma(&pool, 8, 2, ymd(2024, 6, 30), "10").await;

        let c = client(&pool);
        let nothing_held = c
            .post(
                "/amma_statements/8/generate_adjustments",
                &serde_json::json!({}),
            )
            .await;
        let (status, body) = nothing_held.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("no parcels"), "{body}");
        // SCENARIOS F-04: the refusal names *both* ways to arrive here — the
        // trades are missing, or the holding was sold/transferred away during
        // the year and its rows are entered by hand. Telling a user with a
        // correct, closed holding to "enter the missing trades" sent them
        // looking for data that was already there.
        assert!(body.contains("if trades are missing"), "{body}");
        assert!(body.contains("sold during the year"), "{body}");
        // And (SCENARIOS N-06) where the row goes in each case: the parcels the
        // units came from for a sale, the replacement parcels for a move — which
        // is what makes the advice followable, since the per-account rule accepts
        // a replacement parcel of the account the units came from.
        assert!(
            body.contains("transferred, exchanged or demerged away"),
            "{body}"
        );
        assert!(
            body.contains("against the replacement parcels that now hold those units"),
            "{body}"
        );
        assert!(body.contains("by hand"), "{body}");

        c.post(
            "/amma_statements/6/generate_adjustments",
            &serde_json::json!({}),
        )
        .await
        .expect_status(StatusCode::CREATED);
        let already = c
            .post(
                "/amma_statements/6/generate_adjustments",
                &serde_json::json!({}),
            )
            .await;
        let (status, body) = already.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("already has AMIT adjustments"), "{body}");

        let resp = c
            .post(
                "/amma_statements/6/generate_adjustments",
                &serde_json::json!({ "replace": true }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::CREATED);
    }

    /// SCENARIOS F-17, from generation's side — **and its answer, N-06**: the
    /// parcel was open at the statement's year end, so it is generated for, but
    /// a transfer has carried the units away since. This used to fail the whole
    /// generation on F-17's row-level refusal, which (with the per-account rule
    /// refusing the replacement parcel from the other side) left the statement's
    /// reduction with nowhere at all to be recorded — for the *ordinary* order of
    /// events, since an AMMA statement for a year ended 30 June arrives in
    /// spring. Generation now follows the transfer: the row is written against
    /// the replacement parcel, which holds exactly those units under the source
    /// parcel's own acquisition date.
    #[tokio::test]
    async fn db_a_rollover_after_the_year_end_generates_against_the_replacement() {
        use crate::entities::holding_account::{self, HoldingAccount};
        use crate::entities::sell::AllocationInput;
        use crate::entities::transfer::{self, TransferBody};

        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "100").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        // Moved in August, after the year end but before the statement was
        // entered — the common order, since the statement arrives in spring.
        transfer::db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: 1,
                date: ymd(2024, 8, 15),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("100"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();

        // The transfer-in Buy: transfer 1's replacement parcel.
        let replacement: i64 = sqlx::query_scalar(
            "SELECT id FROM trades WHERE transfer_id = 1 AND trade_type = 'Buy'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(result.created.len(), 1);
        assert_eq!(result.created[0].trade_id, replacement);
        assert_eq!(result.created[0].quantity, dec("100"));
        // The coverage figures still reconcile against the statement's units.
        assert_eq!(result.units_adjusted, dec("100"));
        assert_eq!(result.difference, Decimal::ZERO);
        assert!(result.unattributed.is_empty());

        // And the reduction reaches the units: this statement's per-unit figure
        // is a cost-base *increase* (a negative adjustment) of 2.0257290076 over
        // 100 units, so the replacement parcel's cost base is $1,000 plus
        // $202.5729007600 — the reduction applies to the parcel the units are in
        // now, which is the whole point.
        let parcels = crate::domain::open_parcels::load(&mut pool.acquire().await.unwrap(), None)
            .await
            .unwrap();
        let moved = parcels
            .iter()
            .find(|p| p.parcel.id == replacement)
            .expect("the replacement parcel is open");
        assert_eq!(moved.cost_base.amit_reduction, dec("-202.5729007600"));
        assert_eq!(moved.cost_base.adjusted, dec("1202.5729007600"));
    }

    /// SCENARIOS F-16: a parcel bought on 30 June itself was held at the
    /// year end — the boundary is inclusive, in the open-parcels loader and
    /// therefore here.
    #[tokio::test]
    async fn db_a_parcel_acquired_on_the_year_end_itself_is_covered() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        // FY2025, whose 30 June is a Monday: the parcel has to be acquired on
        // the year end *itself*, and a trade can only be dated on a day the
        // exchange traded (SCENARIOS S-08), so the year is picked to make the
        // two coincide.
        test_support::buy(1, 1)
            .date(ymd(2025, 6, 30))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        amma(&pool, 6, 1, ymd(2025, 6, 30), "100").await;

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(
            result
                .created
                .iter()
                .map(|a| (a.trade_id, a.quantity))
                .collect::<Vec<_>>(),
            vec![(1, dec("100"))]
        );
        assert_eq!(result.difference, Decimal::ZERO);
    }

    /// SCENARIOS F-05: units bought after the fund's last distribution period
    /// of the year. The statement's figure is per unit **held at its year
    /// end**, and the registry's `units_held` counts the late parcel too, so
    /// it is covered like any other — the year's whole cost-base movement is
    /// spread across every unit on hand at 30 June, not only the units that
    /// drew a distribution. Pinned because it is the convention the Σ against
    /// `units_held` depends on: excluding the late parcel would leave every
    /// such statement permanently short.
    #[tokio::test]
    async fn db_a_parcel_bought_after_the_last_distribution_is_still_covered() {
        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("1000"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        // Bought 20 June — after the 31 March distribution period, so this
        // parcel was attributed nothing by the fund.
        test_support::buy(2, 1)
            .date(ymd(2024, 6, 20))
            .qty(dec("200"))
            .price(dec("12"))
            .insert(&pool)
            .await;
        amma(&pool, 6, 1, ymd(2024, 6, 30), "1200").await;

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(
            result
                .created
                .iter()
                .map(|a| (a.trade_id, a.quantity))
                .collect::<Vec<_>>(),
            vec![(1, dec("1000")), (2, dec("200"))]
        );
        assert_eq!(result.units_adjusted, dec("1200"));
        assert_eq!(result.difference, Decimal::ZERO);
        assert!(
            crate::reports::amit_adjustment_cross_check::db_amit_adjustment_alerts(&pool)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// SCENARIOS F-17: the holding moves to another account mid-year. The
    /// transfer closes the original parcel and re-creates it in the
    /// destination, so the receiving account's statement covers the
    /// **replacement** parcel — the units are held there at the year end, and
    /// the replacement carries the original's cost base and acquisition date.
    #[tokio::test]
    async fn db_a_parcel_transferred_mid_year_is_covered_in_its_new_account() {
        use crate::entities::holding_account::{self, HoldingAccount};
        use crate::entities::sell::AllocationInput;
        use crate::entities::transfer::{self, TransferBody};

        let pool = test_pool().await;
        amit_listing(&pool, 1, "HNDQ").await;
        holding_account::db_upsert(
            &pool,
            &HoldingAccount {
                id: 2,
                name: "Second".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(ymd(2023, 8, 1))
            .qty(dec("100"))
            .price(dec("10"))
            .insert(&pool)
            .await;
        let group = transfer::db_transfer(
            &pool,
            1,
            &TransferBody {
                listing_id: 1,
                date: ymd(2024, 2, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("100"),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();
        let replacement = group.transfer_ins[0].id;

        test_support::amma(6, 1)
            .units(dec("100"))
            .cost_base_adjustment(dec("0.50"))
            .with(|a| {
                a.tax_year_end_date = ymd(2024, 6, 30);
                a.date_received = ymd(2024, 8, 29);
                a.holding_account_id = 2;
            })
            .insert(&pool)
            .await;

        let result = db_generate(&pool, 6, &GenerateBody::default())
            .await
            .unwrap();
        assert_eq!(
            result
                .created
                .iter()
                .map(|a| (a.trade_id, a.quantity))
                .collect::<Vec<_>>(),
            vec![(replacement, dec("100"))]
        );
        assert_eq!(result.difference, Decimal::ZERO);
        // …and the reduction lands on the replacement parcel: 100 × 50c off
        // the cost base it carried over.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(parcels[0].trade_id, replacement);
        assert_eq!(parcels[0].amit_cost_base_reduction, dec("50"));
        assert_eq!(parcels[0].remaining_cost_base, dec("950"));
    }
}
