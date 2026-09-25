use crate::infra::db::write_tx;
use crate::infra::decimal::Money;
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AmmaStatement {
    pub id: i64,
    pub listing_id: i64,
    /// End of the Australian financial year the statement attributes — always a
    /// 30 June date, enforced at write time. Every AMMA-keyed report buckets the
    /// statement into the FY identified by this date's calendar year (the
    /// `domain::tax_year` convention), so any other date would land the
    /// statement in the wrong year silently.
    pub tax_year_end_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    pub units_held: Decimal,
    pub date_received: NaiveDate,
    #[sqlx(try_from = "Money")]
    pub australian_interest: Decimal,
    #[sqlx(try_from = "Money")]
    pub australian_dividends_unfranked: Decimal,
    #[sqlx(try_from = "Money")]
    pub franked_dividends: Decimal,
    #[sqlx(try_from = "Money")]
    pub franking_credits: Decimal,
    #[sqlx(try_from = "Money")]
    pub net_rent: Decimal,
    #[sqlx(try_from = "Money")]
    pub foreign_income: Decimal,
    /// Part C's foreign income tax offset on foreign **income** — claimable in
    /// full (subject to the year's A$1,000 de-minimis). Foreign tax paid on
    /// the statement's foreign *capital gains* is the separate line below.
    #[sqlx(try_from = "Money")]
    pub foreign_tax_credits: Decimal,
    /// Part C's foreign income tax offset applicable to the statement's
    /// **capital gains**, as the trustee reports it: **grossed up**, not
    /// reduced for any discount applied at trust level (the AMMA guidance
    /// notes are explicit that FITO is not reduced there). Where only part of
    /// a foreign capital gain is assessable — the Division 115 discount being
    /// the ordinary case — the ATO requires the foreign tax to be apportioned
    /// to the assessable part, and that reduction is the *investor's* step:
    /// `reports::tax_summary` does it, over the three CGT gain figures below
    /// (`docs/ato/fito-capital-gains-apportionment.md`, SCENARIOS M-12).
    ///
    /// Additional to [`Self::foreign_tax_credits`], not a split of it
    /// (migration 0032 defaults it to `0`), because the two are separate Part
    /// C lines claimed differently. A statement whose Part C reports one
    /// combined figure needs its capital-gains portion moved across by hand —
    /// nothing can infer the split from the total.
    #[sqlx(try_from = "Money")]
    pub foreign_tax_credits_capital_gains: Decimal,
    #[sqlx(try_from = "Money")]
    pub other_income: Decimal,
    #[sqlx(try_from = "Money")]
    pub cgt_discount_gains: Decimal,
    #[sqlx(try_from = "Money")]
    pub cgt_indexation_gains: Decimal,
    #[sqlx(try_from = "Money")]
    pub cgt_other_gains: Decimal,
    #[sqlx(try_from = "Money")]
    pub capital_losses_applied: Decimal,
    /// Informational only. Tax-deferred amounts are a reported AMMA statement line, but
    /// they do NOT directly drive the member's cost base adjustment — the ATO's annual
    /// AMIT cost base net amount (`cost_base_adjustment` below) already reflects them.
    /// See `docs/ato/amit-cost-base-adjustments.md`. Not consumed by any calculation.
    #[sqlx(try_from = "Money")]
    pub tax_deferred_amount: Decimal,
    /// Informational only. As with `tax_deferred_amount`, tax-free amounts are reported
    /// on the statement but are not a direct cost-base driver; they are broadly reflected
    /// in `cost_base_adjustment`. See `docs/ato/amit-cost-base-adjustments.md`.
    #[sqlx(try_from = "Money")]
    pub tax_free_amount: Decimal,
    /// The AMIT cost base net amount **per unit** for the year — the sole driver of the
    /// cost base adjustment applied to affected parcels (see
    /// `amit_adjustment::db_cost_base_reductions`). A positive value reduces the cost base;
    /// a negative value increases it (upward adjustments are permitted under the AMIT
    /// regime). See `docs/ato/amit-cost-base-adjustments.md`.
    #[sqlx(try_from = "Money")]
    pub cost_base_adjustment: Decimal,
    #[sqlx(try_from = "Money")]
    pub tfn_withholding_tax: Decimal,
    /// ISO 4217 currency the attributed amounts are denominated in. The tax summary
    /// converts non-AUD amounts to AUD via the ATO rate for this currency and the
    /// month of `tax_year_end_date` (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// The holding account the statement covers (a registry issues one AMMA
    /// statement per holder account; see `entities::holding_account`).
    /// Defaults to the seeded default account when omitted from a request.
    pub holding_account_id: i64,
}

#[derive(utoipa::ToSchema, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmmaStatementBody {
    pub listing_id: i64,
    pub tax_year_end_date: NaiveDate,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub units_held: Decimal,
    pub date_received: NaiveDate,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub australian_interest: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub australian_dividends_unfranked: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub franked_dividends: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub franking_credits: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub net_rent: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub foreign_income: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub foreign_tax_credits: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub foreign_tax_credits_capital_gains: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub other_income: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub cgt_discount_gains: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub cgt_indexation_gains: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub cgt_other_gains: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub capital_losses_applied: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub tax_deferred_amount: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub tax_free_amount: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub cost_base_adjustment: Decimal,
    #[serde(default, deserialize_with = "crate::infra::decimal::strict_decimal")]
    pub tfn_withholding_tax: Decimal,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
}

fn default_currency() -> String {
    "AUD".to_string()
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    /// `tax_year_end_date` is not a 30 June date (carries the rejected date).
    /// An AMMA statement attributes a full Australian financial year, and every
    /// AMMA-keyed report buckets it into the FY named by this date's calendar
    /// year — a mid-year date would silently land in the wrong FY. Mapped to `422`.
    #[error("tax_year_end_date {0} is not a 30 June date")]
    NotFinancialYearEnd(NaiveDate),
    /// A capital-gains foreign tax figure with no capital gains to apportion
    /// it over. The amount is claimable only in the proportion of the gains
    /// it was paid on that is assessable here
    /// (`docs/ato/fito-capital-gains-apportionment.md`), so with no gains
    /// stated there is no proportion to compute and the whole figure would
    /// pass through unreduced. Mapped to `422`.
    #[error("foreign_tax_credits_capital_gains {0} is stated with no capital gains")]
    CapitalGainsTaxWithoutGains(Decimal),
    /// The statement's currency is not its listing's. An AMMA attributes the
    /// income and capital gains of *that listed trust*, and the tax summary
    /// converts every component at this currency's rate for the month of
    /// `tax_year_end_date` — so a statement in another currency reports a
    /// fund's year in money the fund does not pay. The rule
    /// [`ess_statement`](crate::entities::ess_statement) applies to a
    /// statement's per-share market value and
    /// [`trade`](crate::entities::trade) to a parcel's price. Mapped to `422`.
    #[error("the AMMA statement is in {statement} but its listing is quoted in {listing}")]
    CurrencyNotListings { statement: String, listing: String },
    /// A negative component (carries the field name). Every figure an AMMA
    /// attributes is the fund's own member component, and the guidance notes
    /// state the rule outright: "An AMIT or attribution CCIV sub-fund trust
    /// attribution amount cannot be a negative"
    /// (`docs/ato/amma-statement-guidance-notes.md`, Part B). A negative would
    /// silently reduce the year's assessable income, and a negative
    /// `cgt_other_gains` is worse than that: `reports::net_capital_gain` reads
    /// it straight into the year's non-discountable gain bucket, where a
    /// negative bucket leaves `net_other` at zero but manufactures a
    /// *carried-forward capital loss* — so a later year's real gain is netted
    /// to nothing and label 18A is understated with no figure ever looking
    /// wrong. `cost_base_adjustment` is deliberately exempt from this sweep:
    /// it is the AMIT cost base net amount, signed by design — a positive
    /// value reduces the cost base and a negative one increases it (an upward
    /// adjustment, `docs/ato/amit-cost-base-adjustments.md`, CGT event E10).
    /// Mapped to `422`.
    #[error("{0} cannot be negative")]
    NegativeAmount(&'static str),
    #[error("AMMA statement write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// A create committed but the row could not be read back. Not reachable in
    /// practice — the row is committed and unread only if something deletes it
    /// in the same instant — but a create that answers with no row would be
    /// worse than a loud failure, so it is its own variant rather than an
    /// `unwrap`. Mapped to `500`.
    #[error("the AMMA statement row was created but could not be read back")]
    VanishedAfterCreate,
}

impl From<UpsertError> for ApiError {
    fn from(err: UpsertError) -> Self {
        match err {
            UpsertError::NotFinancialYearEnd(date) => ApiError::unprocessable(format!(
                "tax_year_end_date {date} is not a 30 June date — an AMMA statement \
                 covers the Australian financial year ending 30 June, and reports \
                 attribute it to the year of that date"
            )),
            UpsertError::CapitalGainsTaxWithoutGains(amount) => ApiError::unprocessable(format!(
                "foreign_tax_credits_capital_gains of {amount} is stated but the statement reports \
                 no capital gains — that figure is claimable only in proportion to the part of the \
                 gains it was paid on that is assessable here, so it needs the CGT gain lines it \
                 belongs to. Foreign tax on the statement's foreign *income* goes in \
                 foreign_tax_credits instead"
            )),
            UpsertError::CurrencyNotListings { statement, listing } => {
                ApiError::unprocessable(format!(
                    "this AMMA statement is recorded in {statement} but its listing is quoted in \
                     {listing} — the statement attributes that fund's own year, so enter it in \
                     {listing} (a statement in another currency is converted before entry, or the \
                     wrong listing was picked)"
                ))
            }
            UpsertError::NegativeAmount(field) => ApiError::unprocessable(format!(
                "{field} cannot be negative — an AMMA statement's figures are the fund's own \
                 attributed amounts, which are never below zero, and a negative capital gain \
                 would become a fictitious carried-forward loss that nets away a later year's \
                 real gain. The one signed field is cost_base_adjustment, the AMIT cost base \
                 net amount, where a negative value is the upward (shortfall) adjustment"
            )),
            UpsertError::Db(err) => err.into(),
            // A committed create whose row could not be read back: a real
            // server-side fault, so it answers `500` and is logged rather than
            // dressed up as a client error.
            err @ UpsertError::VanishedAfterCreate => ApiError::internal(err),
        }
    }
}

impl CrudEntity for AmmaStatement {
    type Key = i64;
    const TABLE: &'static str = "amma_statements";
    const COLUMNS: &'static str = "id, listing_id, tax_year_end_date, units_held, date_received, \
         australian_interest, australian_dividends_unfranked, franked_dividends, \
         franking_credits, net_rent, foreign_income, foreign_tax_credits, \
         foreign_tax_credits_capital_gains, other_income, \
         cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
         tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
         currency, holding_account_id";
    const ORDER_BY: &'static str = "tax_year_end_date, id";
    const NOUN: &'static str = "AMMA statement";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route(
            "/amma_statements",
            get(http::list_handler::<AmmaStatement>).post(create),
        )
        .route(
            "/amma_statements/{id}",
            get(http::get_handler::<AmmaStatement>)
                .put(upsert)
                // Deleting a statement still referenced by AMIT adjustments
                // violates an FK → 422.
                .delete(http::delete_handler::<AmmaStatement>),
        )
}

#[cfg(test)]
pub async fn db_list(pool: &SqlitePool) -> Result<Vec<AmmaStatement>, sqlx::Error> {
    http::crud_list(pool).await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<AmmaStatement>, sqlx::Error> {
    http::crud_get(pool, id).await
}

pub async fn db_upsert(pool: &SqlitePool, stmt: &AmmaStatement) -> Result<(), UpsertError> {
    write(pool, Some(stmt.id), stmt).await?;
    Ok(())
}

/// `POST /amma_statements` — create without naming an id, and return the row
/// the database assigned one to.
pub async fn db_create(
    pool: &SqlitePool,
    stmt: &AmmaStatement,
) -> Result<AmmaStatement, UpsertError> {
    let id = write(pool, None, stmt).await?;
    db_get(pool, id)
        .await?
        .ok_or(UpsertError::VanishedAfterCreate)
}

/// Write an AMMA statement, allocating its id when `id` is `None`.
///
/// Every validation below is shared by both callers, so a create and an upsert
/// can never drift: `id = Some` is the long-standing `PUT
/// /amma_statements/:id` upsert, and `id = None` is the `POST
/// /amma_statements` create, which leaves the id to the database.
async fn write(
    pool: &SqlitePool,
    id: Option<i64>,
    stmt: &AmmaStatement,
) -> Result<i64, UpsertError> {
    // No component of the statement may be negative: every figure is the
    // fund's own attributed amount, and the ATO's AMMA guidance notes state
    // the rule outright — "An AMIT or attribution CCIV sub-fund trust
    // attribution amount cannot be a negative"
    // (`docs/ato/amma-statement-guidance-notes.md`, Part B). Checked before
    // the other rules so a negative figure gets the message naming its field.
    //
    // `cost_base_adjustment` is deliberately *not* in this sweep: it is the
    // AMIT cost base net amount, signed by design — positive reduces the cost
    // base, negative increases it (an upward adjustment under Subdivision
    // 276-H, `docs/ato/amit-cost-base-adjustments.md`, CGT event E10).
    // `units_held` is not an attribution amount either, but a negative unit
    // count is likewise unrepresentable, and the sibling income path refuses
    // its own quantity (`securities_held`) the same way.
    for (field, value) in [
        ("units_held", stmt.units_held),
        ("australian_interest", stmt.australian_interest),
        (
            "australian_dividends_unfranked",
            stmt.australian_dividends_unfranked,
        ),
        ("franked_dividends", stmt.franked_dividends),
        ("franking_credits", stmt.franking_credits),
        ("net_rent", stmt.net_rent),
        ("foreign_income", stmt.foreign_income),
        ("foreign_tax_credits", stmt.foreign_tax_credits),
        (
            "foreign_tax_credits_capital_gains",
            stmt.foreign_tax_credits_capital_gains,
        ),
        ("other_income", stmt.other_income),
        ("cgt_discount_gains", stmt.cgt_discount_gains),
        ("cgt_indexation_gains", stmt.cgt_indexation_gains),
        ("cgt_other_gains", stmt.cgt_other_gains),
        ("capital_losses_applied", stmt.capital_losses_applied),
        ("tax_deferred_amount", stmt.tax_deferred_amount),
        ("tax_free_amount", stmt.tax_free_amount),
        ("tfn_withholding_tax", stmt.tfn_withholding_tax),
    ] {
        if value < Decimal::ZERO {
            return Err(UpsertError::NegativeAmount(field));
        }
    }
    // An AMMA statement is an annual attribution over an income year, and this
    // model has only the one FY-end shape: 30 June. Every reader buckets the
    // statement with `domain::tax_year::tax_year_for`, so a hand-entered row
    // at another date would still land in a coherent FY — but the statement
    // itself would be for a period the AMIT regime does not report, so the
    // write path refuses it.
    if (stmt.tax_year_end_date.month(), stmt.tax_year_end_date.day()) != (6, 30) {
        return Err(UpsertError::NotFinancialYearEnd(stmt.tax_year_end_date));
    }
    // A capital-gains foreign tax figure needs the gains it was paid on: the
    // tax summary apportions it over them (SCENARIOS M-12), and with none
    // stated there is nothing to apportion — the figure would be claimed in
    // full, which is exactly the over-claim the split exists to fix.
    if stmt.foreign_tax_credits_capital_gains != Decimal::ZERO
        && stmt.cgt_discount_gains == Decimal::ZERO
        && stmt.cgt_indexation_gains == Decimal::ZERO
        && stmt.cgt_other_gains == Decimal::ZERO
    {
        return Err(UpsertError::CapitalGainsTaxWithoutGains(
            stmt.foreign_tax_credits_capital_gains,
        ));
    }
    let mut tx = write_tx(pool).await?;

    // A create (`id` is `None`, from `POST /amma_statements`) omits the id
    // column altogether, so the database's `AUTOINCREMENT` sequence assigns
    // one it has never issued — a server-computed id could hand the new row a
    // deleted row's id and, with it, that row's `row_history` trail
    // (SCENARIOS U-a). The upsert branch is otherwise byte-identical, so a
    // `PUT` on an explicit id stays the upsert it has always been.
    let query = crate::insert_with_optional_id!(
        "INSERT INTO amma_statements \
         (id, listing_id, tax_year_end_date, units_held, date_received, \
          australian_interest, australian_dividends_unfranked, franked_dividends, \
          franking_credits, net_rent, foreign_income, foreign_tax_credits, \
         foreign_tax_credits_capital_gains, other_income, \
          cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
          tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
          currency, holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                      = excluded.listing_id, \
             tax_year_end_date               = excluded.tax_year_end_date, \
             units_held                      = excluded.units_held, \
             date_received                   = excluded.date_received, \
             australian_interest             = excluded.australian_interest, \
             australian_dividends_unfranked  = excluded.australian_dividends_unfranked, \
             franked_dividends               = excluded.franked_dividends, \
             franking_credits                = excluded.franking_credits, \
             net_rent                        = excluded.net_rent, \
             foreign_income                  = excluded.foreign_income, \
             foreign_tax_credits             = excluded.foreign_tax_credits, \
             foreign_tax_credits_capital_gains = excluded.foreign_tax_credits_capital_gains, \
             other_income                    = excluded.other_income, \
             cgt_discount_gains              = excluded.cgt_discount_gains, \
             cgt_indexation_gains            = excluded.cgt_indexation_gains, \
             cgt_other_gains                 = excluded.cgt_other_gains, \
             capital_losses_applied          = excluded.capital_losses_applied, \
             tax_deferred_amount             = excluded.tax_deferred_amount, \
             tax_free_amount                 = excluded.tax_free_amount, \
             cost_base_adjustment            = excluded.cost_base_adjustment, \
             tfn_withholding_tax             = excluded.tfn_withholding_tax, \
             currency                        = excluded.currency, \
             holding_account_id              = excluded.holding_account_id",
        "INSERT INTO amma_statements \
         (listing_id, tax_year_end_date, units_held, date_received, \
          australian_interest, australian_dividends_unfranked, franked_dividends, \
          franking_credits, net_rent, foreign_income, foreign_tax_credits, \
         foreign_tax_credits_capital_gains, other_income, \
          cgt_discount_gains, cgt_indexation_gains, cgt_other_gains, capital_losses_applied, \
          tax_deferred_amount, tax_free_amount, cost_base_adjustment, tfn_withholding_tax, \
          currency, holding_account_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                      = excluded.listing_id, \
             tax_year_end_date               = excluded.tax_year_end_date, \
             units_held                      = excluded.units_held, \
             date_received                   = excluded.date_received, \
             australian_interest             = excluded.australian_interest, \
             australian_dividends_unfranked  = excluded.australian_dividends_unfranked, \
             franked_dividends               = excluded.franked_dividends, \
             franking_credits                = excluded.franking_credits, \
             net_rent                        = excluded.net_rent, \
             foreign_income                  = excluded.foreign_income, \
             foreign_tax_credits             = excluded.foreign_tax_credits, \
             foreign_tax_credits_capital_gains = excluded.foreign_tax_credits_capital_gains, \
             other_income                    = excluded.other_income, \
             cgt_discount_gains              = excluded.cgt_discount_gains, \
             cgt_indexation_gains            = excluded.cgt_indexation_gains, \
             cgt_other_gains                 = excluded.cgt_other_gains, \
             capital_losses_applied          = excluded.capital_losses_applied, \
             tax_deferred_amount             = excluded.tax_deferred_amount, \
             tax_free_amount                 = excluded.tax_free_amount, \
             cost_base_adjustment            = excluded.cost_base_adjustment, \
             tfn_withholding_tax             = excluded.tfn_withholding_tax, \
             currency                        = excluded.currency, \
             holding_account_id              = excluded.holding_account_id",
        id,
    );
    let result = query
        .bind(stmt.listing_id)
        .bind(stmt.tax_year_end_date)
        .bind(Money(stmt.units_held))
        .bind(stmt.date_received)
        .bind(Money(stmt.australian_interest))
        .bind(Money(stmt.australian_dividends_unfranked))
        .bind(Money(stmt.franked_dividends))
        .bind(Money(stmt.franking_credits))
        .bind(Money(stmt.net_rent))
        .bind(Money(stmt.foreign_income))
        .bind(Money(stmt.foreign_tax_credits))
        .bind(Money(stmt.foreign_tax_credits_capital_gains))
        .bind(Money(stmt.other_income))
        .bind(Money(stmt.cgt_discount_gains))
        .bind(Money(stmt.cgt_indexation_gains))
        .bind(Money(stmt.cgt_other_gains))
        .bind(Money(stmt.capital_losses_applied))
        .bind(Money(stmt.tax_deferred_amount))
        .bind(Money(stmt.tax_free_amount))
        .bind(Money(stmt.cost_base_adjustment))
        .bind(Money(stmt.tfn_withholding_tax))
        .bind(&stmt.currency)
        .bind(stmt.holding_account_id)
        .execute(&mut *tx)
        .await?;
    // An id-less INSERT was given one by the database; an upsert wrote the
    // explicit one it was handed. Either way the caller gets the id the row
    // actually holds.
    let assigned_id = id.unwrap_or_else(|| result.last_insert_rowid());

    // The statement's currency must be the listing's, as a trade's must
    // (SCENARIOS M-08). Checked after the write, like the trade path's twin,
    // so an unrecognised currency code meets its own foreign-key rejection
    // first.
    if let Some(listing) =
        crate::entities::trade::listing_currency_mismatch(&mut tx, stmt.listing_id, &stmt.currency)
            .await?
    {
        return Err(UpsertError::CurrencyNotListings {
            statement: stmt.currency.clone(),
            listing,
        });
    }

    tx.commit().await?;
    Ok(assigned_id)
}

/// The row a request body describes. `id` is the path's on an upsert and
/// ignored on a create (the database assigns one), so both entry points build
/// their `AmmaStatement` through this one mapping and cannot drift.
fn amma_from_body(id: i64, body: AmmaStatementBody) -> AmmaStatement {
    AmmaStatement {
        id,
        listing_id: body.listing_id,
        tax_year_end_date: body.tax_year_end_date,
        units_held: body.units_held,
        date_received: body.date_received,
        australian_interest: body.australian_interest,
        australian_dividends_unfranked: body.australian_dividends_unfranked,
        franked_dividends: body.franked_dividends,
        franking_credits: body.franking_credits,
        net_rent: body.net_rent,
        foreign_income: body.foreign_income,
        foreign_tax_credits: body.foreign_tax_credits,
        foreign_tax_credits_capital_gains: body.foreign_tax_credits_capital_gains,
        other_income: body.other_income,
        cgt_discount_gains: body.cgt_discount_gains,
        cgt_indexation_gains: body.cgt_indexation_gains,
        cgt_other_gains: body.cgt_other_gains,
        capital_losses_applied: body.capital_losses_applied,
        tax_deferred_amount: body.tax_deferred_amount,
        tax_free_amount: body.tax_free_amount,
        cost_base_adjustment: body.cost_base_adjustment,
        tfn_withholding_tax: body.tfn_withholding_tax,
        currency: body.currency,
        holding_account_id: body.holding_account_id,
    }
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<AmmaStatementBody>,
) -> Result<StatusCode, ApiError> {
    db_upsert(&pool, &amma_from_body(id, body))
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(ApiError::from)
}

/// `POST /amma_statements` — create the statement without naming an id. The
/// database assigns one (see [`write`]), and the created row is returned so the
/// caller can act on its id at once — attaching the AMMA's generated AMIT
/// adjustments to it, for one — with no `max(id) + 1` guess between the two
/// calls.
async fn create(
    State(pool): State<SqlitePool>,
    Json(body): Json<AmmaStatementBody>,
) -> Result<(StatusCode, Json<AmmaStatement>), ApiError> {
    let created = db_create(&pool, &amma_from_body(0, body)).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, dec, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VAF")
            .name("Vanguard Australian Fixed Interest ETF")
            .amit(true)
            .insert(pool)
            .await;
    }

    fn sample_amma() -> AmmaStatement {
        test_support::amma(1, 1)
            .units(dec("1000"))
            .cost_base_adjustment(dec("0.0023"))
            .with(|a| {
                a.australian_interest = dec("12.50");
                a.australian_dividends_unfranked = dec("5.25");
                a.tax_deferred_amount = dec("2.30");
                a.tax_free_amount = dec("1.10");
            })
            .build()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(
            got.tax_year_end_date,
            NaiveDate::from_ymd_opt(2024, 6, 30).unwrap()
        );
        assert_eq!(got.units_held, "1000".parse::<Decimal>().unwrap());
        assert_eq!(got.australian_interest, "12.50".parse::<Decimal>().unwrap());
        assert_eq!(
            got.australian_dividends_unfranked,
            "5.25".parse::<Decimal>().unwrap()
        );
        assert_eq!(got.tax_deferred_amount, "2.30".parse::<Decimal>().unwrap());
        assert_eq!(got.tax_free_amount, "1.10".parse::<Decimal>().unwrap());
        assert_eq!(
            got.cost_base_adjustment,
            "0.0023".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn db_cost_base_adjustment_calculation() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        // total cost base reduction = cost_base_adjustment per unit * units_held
        let total_adjustment = got.cost_base_adjustment * got.units_held;
        assert_eq!(total_adjustment, "2.3".parse::<Decimal>().unwrap());
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let mut updated = sample_amma();
        updated.australian_interest = "99.99".parse().unwrap();
        db_upsert(&pool, &updated).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.australian_interest, "99.99".parse::<Decimal>().unwrap());
    }

    /// `POST /amma_statements` allocates its own id — the create that makes the
    /// `max(id) + 1` dance unnecessary (SCENARIOS U-a).
    #[tokio::test]
    async fn post_amma_creates_a_row_with_a_database_assigned_id() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let client = client(&pool);

        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1000",
            "date_received": "2024-08-15",
            "australian_interest": "12.50",
        });
        let response = client.post("/amma_statements", &body).await;
        let (status, text) = response.status_and_body();
        assert_eq!(status, StatusCode::CREATED, "body: {text:?}");

        let created: AmmaStatement = response.json();
        assert_ne!(created.id, 0, "a create must not land on id 0");
        assert_eq!(created.units_held, dec("1000"));

        // The returned row is the stored one, and it is readable under its own
        // id — so the caller can act on the id at once instead of re-reading
        // the collection to discover it.
        let read_back = db_get(&pool, created.id).await.unwrap().unwrap();
        assert_eq!(read_back.id, created.id);
        assert_eq!(read_back.listing_id, 1);
        assert_eq!(read_back.australian_interest, dec("12.50"));
    }

    /// The bug this endpoint exists to kill: with an id-less create there is no
    /// guessed id to collide with, so two creates are two rows — before, a
    /// second `max(id) + 1`-style PUT on a taken id silently overwrote the
    /// first through the upsert's `ON CONFLICT ... DO UPDATE`.
    #[tokio::test]
    async fn post_amma_twice_stores_two_distinct_rows() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let client = client(&pool);

        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "date_received": "2024-08-15",
        });
        let first: AmmaStatement = client
            .post("/amma_statements", &body)
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        let second: AmmaStatement = client
            .post("/amma_statements", &body)
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_ne!(
            first.id, second.id,
            "each create must get its own id, not overwrite the previous row"
        );

        let all: Vec<AmmaStatement> = client.get_json("/amma_statements").await;
        assert_eq!(all.len(), 2, "both creates are stored: {all:?}");
    }

    /// A create never re-issues an id a deleted row held: an id handed out
    /// twice inherits the previous occupant's `row_history` trail, and
    /// `AUTOINCREMENT` guarantees it only while the INSERT omits the id column
    /// — which is exactly what the create path does (SCENARIOS U-a, migration
    /// 0045).
    #[tokio::test]
    async fn post_amma_never_reuses_a_deleted_rows_id() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let client = client(&pool);

        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "date_received": "2024-08-15",
        });
        let first: AmmaStatement = client
            .post("/amma_statements", &body)
            .await
            .expect_status(StatusCode::CREATED)
            .json();

        // Edit it, so its trail is non-empty, then delete it — the id is now
        // free and carries history that must never be inherited.
        client
            .put(
                &format!("/amma_statements/{}", first.id),
                &serde_json::json!({
                    "listing_id": 1,
                    "tax_year_end_date": "2024-06-30",
                    "date_received": "2024-08-20",
                    "units_held": "1000",
                }),
            )
            .await
            .expect_status(StatusCode::NO_CONTENT);
        client
            .delete(&format!("/amma_statements/{}", first.id))
            .await
            .expect_status(StatusCode::NO_CONTENT);

        let second: AmmaStatement = client
            .post("/amma_statements", &body)
            .await
            .expect_status(StatusCode::CREATED)
            .json();
        assert_ne!(
            first.id, second.id,
            "the create took the deleted row's id back, inheriting its audit history"
        );
    }

    /// A create shares every write-time validation with the upsert, so a bad
    /// statement is refused and nothing is stored — the create path is not a
    /// way round the 30 June rule.
    #[tokio::test]
    async fn post_amma_rejects_like_the_upsert_and_stores_nothing() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let client = client(&pool);

        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-12-31",
            "date_received": "2024-08-15",
        });
        let response = client.post("/amma_statements", &body).await;
        let (status, text) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {text:?}");
        assert!(
            text.contains("30 June"),
            "the shared FY-end wording is reused: {text:?}"
        );

        let all: Vec<AmmaStatement> = client.get_json("/amma_statements").await;
        assert!(all.is_empty(), "a rejected create stores nothing: {all:?}");
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1000",
            "date_received": "2024-08-15",
            "australian_interest": "12.50",
            "tax_deferred_amount": "2.30",
            "cost_base_adjustment": "0.0023"
        });
        let resp = client(&pool).put("/amma_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.australian_interest, "12.50".parse::<Decimal>().unwrap());
        assert_eq!(
            got.cost_base_adjustment,
            "0.0023".parse::<Decimal>().unwrap()
        );
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let resp = client(&pool).get("/amma_statements").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<AmmaStatement> = resp.json();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/amma_statements/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &sample_amma()).await.unwrap();
        let resp = client(&pool).delete("/amma_statements/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/amma_statements/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    /// `tax_year_end_date` must be a 30 June FY end: an AMMA attribution is an
    /// annual statement over an income year, and this model carries only the
    /// 30 June shape. Readers bucket it with `domain::tax_year::tax_year_for`,
    /// so a 2024-12-31 row would land in FY2025 — coherent with the rest of
    /// that FY — but would claim a period the regime does not report, so the
    /// write path refuses it (2026-07-12 review: the 30 June assumption was
    /// never validated; 2026-09-17 review: the bucketing no longer rests on
    /// this check).
    #[tokio::test]
    async fn api_non_june_30_year_end_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for date in ["2024-12-31", "2024-06-29", "2024-07-01"] {
            let body = serde_json::json!({
                "listing_id": 1,
                "tax_year_end_date": date,
                "date_received": "2024-08-15"
            });
            let resp = client(&pool).put("/amma_statements/1", &body).await;
            assert_eq!(
                resp.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{date} must be rejected"
            );
            let detail = resp.text().to_string();
            assert!(
                detail.contains(date) && detail.contains("30 June"),
                "{date}: detail must carry the date and the rule, got: {detail}"
            );
            assert!(
                db_get(&pool, 1).await.unwrap().is_none(),
                "{date}: nothing persisted"
            );
        }
    }

    /// 30 June is accepted for any year — the rule pins the day, not the year.
    #[tokio::test]
    async fn db_june_30_of_any_year_accepted() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for (id, year) in [(1, 2019), (2, 2025)] {
            let stmt = test_support::amma(id, 1)
                .with(|a| a.tax_year_end_date = NaiveDate::from_ymd_opt(year, 6, 30).unwrap())
                .build();
            db_upsert(&pool, &stmt).await.unwrap();
        }
        assert_eq!(db_list(&pool).await.unwrap().len(), 2);
    }

    /// No component of an AMMA statement may be negative: every figure is the
    /// fund's own attributed amount, and the guidance notes state it outright
    /// — "An AMIT or attribution CCIV sub-fund trust attribution amount cannot
    /// be a negative" (`docs/ato/amma-statement-guidance-notes.md`, Part B).
    /// Refused `422` naming the field, with nothing stored (2026-09-17 review:
    /// negatives were accepted, and a negative `cgt_other_gains` produced a
    /// fictitious carried-forward capital loss while a negative gain with a
    /// positive `foreign_tax_credits_capital_gains` produced a negative FITO).
    /// The one signed field, `cost_base_adjustment`, is covered separately.
    #[tokio::test]
    async fn api_negative_component_returns_422_naming_the_field() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for field in [
            "units_held",
            "australian_interest",
            "australian_dividends_unfranked",
            "franked_dividends",
            "franking_credits",
            "net_rent",
            "foreign_income",
            "foreign_tax_credits",
            "foreign_tax_credits_capital_gains",
            "other_income",
            "cgt_discount_gains",
            "cgt_indexation_gains",
            "cgt_other_gains",
            "capital_losses_applied",
            "tax_deferred_amount",
            "tax_free_amount",
            "tfn_withholding_tax",
        ] {
            let mut body = serde_json::json!({
                "listing_id": 1,
                "tax_year_end_date": "2024-06-30",
                "date_received": "2024-08-15",
            });
            body[field] = serde_json::json!("-1");
            let resp = client(&pool).put("/amma_statements/1", &body).await;
            assert_eq!(
                resp.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "negative {field} must be rejected"
            );
            let detail = resp.text().to_string();
            assert!(
                detail.contains(field) && detail.contains("cannot be negative"),
                "negative {field}: detail must name the field, got: {detail}"
            );
            assert!(
                db_get(&pool, 1).await.unwrap().is_none(),
                "negative {field}: nothing persisted"
            );
        }
    }

    /// The counterpart to the refusal: a statement whose components are all
    /// positive or zero is still accepted, and round-trips unchanged.
    #[tokio::test]
    async fn api_positive_and_zero_components_are_accepted() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1000",
            "date_received": "2024-08-15",
            "australian_interest": "12.50",
            "australian_dividends_unfranked": "5.25",
            "franked_dividends": "8.00",
            "franking_credits": "3.43",
            "net_rent": "1.00",
            "foreign_income": "2.00",
            "foreign_tax_credits": "0.50",
            "foreign_tax_credits_capital_gains": "0.25",
            "other_income": "0.10",
            "cgt_discount_gains": "100",
            "cgt_indexation_gains": "10",
            "cgt_other_gains": "20",
            "capital_losses_applied": "5",
            "tax_deferred_amount": "2.30",
            "tax_free_amount": "1.10",
            "cost_base_adjustment": "0",
            "tfn_withholding_tax": "0.75",
        });
        client(&pool)
            .put("/amma_statements/1", &body)
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.australian_interest, dec("12.50"));
        assert_eq!(got.cgt_other_gains, dec("20"));
        assert_eq!(got.tfn_withholding_tax, dec("0.75"));
        assert_eq!(got.cost_base_adjustment, Decimal::ZERO);

        // An all-zero statement is equally valid.
        let zero = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2025-06-30",
            "date_received": "2025-08-15",
        });
        client(&pool)
            .put("/amma_statements/2", &zero)
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(got.units_held, Decimal::ZERO);
        assert_eq!(got.cost_base_adjustment, Decimal::ZERO);
    }

    /// `cost_base_adjustment` is the AMIT cost base net amount, signed by
    /// design: a negative value is the **upward** (shortfall) adjustment that
    /// increases the cost base (`docs/ato/amit-cost-base-adjustments.md`,
    /// Subdivision 276-H, CGT event E10; ATO worked example 28 in
    /// `src/ato_examples.rs`). It must survive the negative-component sweep.
    #[tokio::test]
    async fn api_negative_cost_base_adjustment_is_still_accepted() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1",
            "date_received": "2024-08-15",
            "cost_base_adjustment": "-10",
        });
        client(&pool)
            .put("/amma_statements/1", &body)
            .await
            .expect_status(StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.cost_base_adjustment, dec("-10"));
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "tax_year_end_date": "2024-06-30",
            "units_held": "1234.567890123",
            "date_received": "2024-08-15",
            "australian_interest": "9.876543210",
            "cost_base_adjustment": "0.001234567890"
        });
        let resp = client(&pool).put("/amma_statements/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let resp = client(&pool).get("/amma_statements/1").await;
        let got: AmmaStatement = resp.json();
        assert_eq!(got.units_held, "1234.567890123".parse::<Decimal>().unwrap());
        assert_eq!(
            got.australian_interest,
            "9.876543210".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            got.cost_base_adjustment,
            "0.001234567890".parse::<Decimal>().unwrap()
        );
    }

    /// An AMMA statement is recorded in its listing's own currency (SCENARIOS
    /// M-08): it attributes *that fund's* year, and the tax summary converts
    /// every component at this currency's rate for the month of
    /// `tax_year_end_date` — so a statement in another currency reports the
    /// fund's year in money the fund does not pay. The rule the trade path
    /// applies to a parcel's price, and the ESS path to a per-share value.
    #[tokio::test]
    async fn api_amma_currency_must_be_the_listings() {
        let pool = test_pool().await;
        test_support::listing(1)
            .mic("XNYS")
            .ticker("VTS")
            .name("VTS")
            .currency("USD")
            .amit(true)
            .insert(&pool)
            .await;
        let body = |currency: &str| {
            serde_json::json!({
                "listing_id": 1,
                "tax_year_end_date": "2024-06-30",
                "units_held": "100",
                "date_received": "2024-08-15",
                "currency": currency,
            })
        };
        let resp = client(&pool).put("/amma_statements/1", &body("AUD")).await;
        let (status, detail) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("recorded in AUD") && detail.contains("quoted in USD"),
            "the refusal names both currencies: {detail}"
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_none(),
            "nothing persisted"
        );

        // In the listing's own currency it goes through.
        client(&pool)
            .put("/amma_statements/1", &body("USD"))
            .await
            .expect_status(StatusCode::NO_CONTENT);
    }
}
