use crate::infra::decimal::{Money, OptMoney};
use crate::infra::http::{self, ApiError, CrudEntity};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Income {
    pub id: i64,
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    pub ex_date: Option<NaiveDate>,
    #[sqlx(try_from = "Money")]
    pub franked_amount: Decimal,
    #[sqlx(try_from = "Money")]
    pub unfranked_amount: Decimal,
    #[sqlx(try_from = "Money")]
    pub foreign_source_income: Decimal,
    #[sqlx(try_from = "Money")]
    pub foreign_tax_paid: Decimal,
    #[sqlx(try_from = "Money")]
    pub tfn_withholding_tax: Decimal,
    #[sqlx(try_from = "Money")]
    pub franking_credits: Decimal,
    /// The **LIC capital gain amount** printed on a listed investment
    /// company's dividend statement — the *attributable part* the LIC advises,
    /// entered verbatim, **not** the deduction claimed from it
    /// (`docs/ato/lic-capital-gain-deduction.md`). The deduction is 50% of it
    /// for the resident individual this system reports for, computed by
    /// [`Income::lic_capital_gain_deduction`]; entering an already-halved
    /// figure here halves it twice.
    #[sqlx(try_from = "Money")]
    pub lic_capital_gain_amount: Decimal,
    /// The portion of `unfranked_amount` the payer declared to be **conduit
    /// foreign income** (CFI, Subdiv 802-A ITAA 1997) — a **memo figure**, not
    /// an amount of its own: it is recorded *within* `unfranked_amount`, never
    /// on top of it, and a value exceeding that amount is rejected (`422`).
    ///
    /// The convention matters because it decides what is assessable. CFI is
    /// non-assessable non-exempt only for a **foreign resident** member; for
    /// the Australian-resident individual this system reports for (the tax
    /// summary's `taxpayer_basis`) an unfranked dividend declared to be CFI is
    /// ordinary assessable income — the ATO's AMMA guidance notes put it in
    /// "Dividends: unfranked amount declared to be CFI", "which forms part of
    /// the non-primary production income"
    /// (`docs/ato/amma-statement-guidance-notes.md`, Part B item 13U). Holding
    /// it inside `unfranked_amount` is what makes it assessable exactly once:
    /// every report totals `unfranked_amount` and reads this field for
    /// reference only, so adding it again would double-count the same dollars.
    ///
    /// A statement that prints its unfranked amount split into a CFI line and a
    /// non-CFI line is therefore entered with `unfranked_amount` as the **sum**
    /// of the two and this field as the CFI line alone. Reported as its own
    /// memo column by the [annual tax report](crate::reports::tax_report), so
    /// the entered figure can still be tied back to the statement.
    #[sqlx(try_from = "Money")]
    pub conduit_foreign_income: Decimal,
    pub trust_income: bool,
    /// Trust distributions only: the date the holder became presently entitled
    /// (in practice the distribution period's end, printed on the statement).
    /// Trust income is assessed in the year of present entitlement regardless
    /// of when it is paid (ATO QC 23087, `docs/ato/trust-income-timing.md`),
    /// so when set the tax summary attributes the row by this date instead of
    /// `date_paid`. Rejected (`422`) on a non-trust row — a dividend is always
    /// assessed by payment.
    pub entitlement_date: Option<NaiveDate>,
    /// Provenance link to the DRP trade this distribution was reinvested
    /// into, managed solely by the reinvest operation: set by
    /// `POST /income/:id/reinvest`, cleared by `DELETE /income/:id/reinvest`
    /// (`entities::drp_reinvestment`). Read-only through CRUD — `PUT /income`
    /// carries no such field and [`db_upsert`] never writes the column (an
    /// edit preserves an existing link), so a client can neither forge a link
    /// to an arbitrary trade nor silently orphan the DRP trade by omission.
    /// While set, the linked trade is frozen (`trade::db_upsert`/`db_delete`
    /// reject it) and `DELETE /income/:id` is refused — undo the reinvestment
    /// first.
    pub reinvestment_trade_id: Option<i64>,
    /// ISO 4217 currency the amounts are denominated in. The tax summary converts
    /// non-AUD amounts to AUD via the ATO rate for this currency and the month of
    /// the assessment date — `date_paid`, or `entitlement_date` when that governs
    /// a trust row (see `infra::fx::to_aud`). Defaults to AUD.
    pub currency: String,
    /// Provenance link from a buy-back dividend-component row to the buy-back
    /// Sell trade it was created with (`None` for every other row). Set only
    /// by `POST /corporate_actions/:id/participate`
    /// (`entities::buyback_participation`). A row carrying it is managed by
    /// the participation: `PUT`/`DELETE /income` reject it (`422`), and it is
    /// removed together with the Sell by `DELETE /sells/:id`.
    pub buyback_trade_id: Option<i64>,
    /// The holding account the distribution was paid to (see
    /// `entities::holding_account`): decides whose DRP enrolment applies and
    /// which account a reinvestment trade lands in. Defaults to the seeded
    /// default account when omitted from a request.
    pub holding_account_id: i64,
    /// Optional per-share figure from the registry statement, supplied only
    /// together with `securities_held`: their product, cent-rounded, must
    /// equal the gross cash components (see `check_per_share`). Informational
    /// / validation-only — no report uses it (mirrors
    /// `trades.statement_total`).
    #[sqlx(try_from = "OptMoney")]
    pub amount_per_security: Option<Decimal>,
    /// See `amount_per_security` — the statement's securities-held count.
    #[sqlx(try_from = "OptMoney")]
    pub securities_held: Option<Decimal>,
    /// Non-AMIT trust statements only: the statement's "tax-deferred amount" —
    /// a non-assessable payment that is a CGT event E4 cost-base reduction
    /// (`docs/ato/cgt-non-assessable-payments.md`). Informational: the E4
    /// reduction itself is entered as a `ReturnOfCapital` corporate action and
    /// no calculation reads this figure — the E4 cross-check report
    /// (`reports::e4_cross_check`) flags a row whose non-zero amount has no
    /// matching same-FY action. Trust rows only, never negative (`422`
    /// otherwise, mirrored by a schema CHECK).
    #[sqlx(try_from = "OptMoney")]
    pub tax_deferred_amount: Option<Decimal>,
}

impl Income {
    /// True when the distribution carries no Australian-sourced component —
    /// every one of `franked_amount`, `unfranked_amount`, `franking_credits`,
    /// `lic_capital_gain_amount`, `conduit_foreign_income` (a memo subset of
    /// `unfranked_amount`, so it can only be non-zero when that is — an
    /// Australian company's income, only *labelled* conduit, which is why
    /// `tax_summary` keeps it out of the foreign total) and
    /// `tfn_withholding_tax` is zero.
    /// A foreign company's dividend (e.g. a US-listed RSU holding) is entered
    /// this way: `foreign_source_income` and `foreign_tax_paid` alone.
    ///
    /// This is the "nothing to declare at Item 11 / Item 13" test the
    /// Australian-side income tables use to skip a row that would otherwise
    /// print as all zeros — such a row belongs under Item 20 (foreign income)
    /// instead. Read on the row's own native-currency figures, so the answer
    /// never depends on an FX rate being importable.
    ///
    /// Two corollaries follow from the fields it looks at. A row with no
    /// amounts at all counts as foreign-only: there is nothing to declare on
    /// the Australian side either way. And `tax_deferred_amount` is ignored —
    /// it is a non-assessable payment (a CGT event E4 cost-base reduction),
    /// not income of any source.
    pub fn is_foreign_only(&self) -> bool {
        [
            self.franked_amount,
            self.unfranked_amount,
            self.franking_credits,
            self.lic_capital_gain_amount,
            self.conduit_foreign_income,
            self.tfn_withholding_tax,
        ]
        .iter()
        .all(|amount| amount.is_zero())
    }

    /// The **LIC capital gain deduction** claimable from this dividend, in the
    /// row's own currency: **50%** of the [`lic_capital_gain_amount`] the LIC
    /// advised (`docs/ato/lic-capital-gain-deduction.md` — Ben's $50
    /// attributable part is a $25 deduction at question D8).
    ///
    /// The 50% is the **individual** rate, per the taxpayer basis every tax
    /// figure here assumes (a complying superannuation entity or life insurance
    /// company deducts 33⅓%; a trust or partnership 50% — see `docs/API.md`'s
    /// Known limitations). *The* place the halving happens: the tax summary's
    /// D8 line and the annual tax report's per-dividend column both read it, so
    /// a per-dividend figure can never disagree with the year's total.
    ///
    /// [`lic_capital_gain_amount`]: Income::lic_capital_gain_amount
    pub fn lic_capital_gain_deduction(&self) -> Decimal {
        self.lic_capital_gain_amount / Decimal::TWO
    }

    /// The date the distribution is assessed on — the date every FY-keyed
    /// report buckets it by (via [`crate::domain::tax_year::tax_year_for`])
    /// and the date whose month resolves its FX rate.
    ///
    /// Trust income is assessed in the year of *present entitlement*, not of
    /// payment (ATO QC 23087, `docs/ato/trust-income-timing.md`): a June trust
    /// distribution paid in mid-July belongs to the FY just ended. When a
    /// trust row records its `entitlement_date`, every component of the row is
    /// attributed by it. A dividend is always assessed by `date_paid` — the
    /// column is rejected on a non-trust row at write time, so the
    /// `trust_income` guard here only restates that invariant.
    pub fn assessment_date(&self) -> NaiveDate {
        match self.entitlement_date {
            Some(d) if self.trust_income => d,
            _ => self.date_paid,
        }
    }

    /// The date the distribution went ex — the date the entitlement to it was
    /// fixed — falling back to a trust row's `entitlement_date` and then to
    /// the payment date when the statement didn't record one.
    ///
    /// Distinct from [`Self::assessment_date`]: that is *when the income is
    /// taxed*, this is *who was entitled to it*. The two rules the entitlement
    /// side drives both read it — whether a DRP enrolment period covered the
    /// holding when the distribution went ex
    /// (`entities::drp_reinvestment`, since participation is fixed at the
    /// record date), and the start of the franking holding-period window the
    /// at-risk test measures (`reports::franking`).
    ///
    /// The middle step is what makes a trust distribution testable at all
    /// (SCENARIOS G-20). Units go ex at the end of the distribution period,
    /// and `entitlement_date` *is* that period's end — so on a statement that
    /// prints no ex date it is the entitlement anchor, and the payment date
    /// (often weeks later) is not. Anchoring on payment was silently
    /// anti-conservative for franking: a disposal between the real ex date and
    /// the payment date left the units unentitled in the walk, so the credits
    /// the holding-period rule denies were claimed in full.
    ///
    /// The last step remains the payment date, the latest the entitlement can
    /// have been fixed; [`Self::ex_date_recorded`] says when the answer rests
    /// on it, so a franking test that could not really be run is surfaced
    /// rather than passing quietly.
    pub fn ex_or_pay_date(&self) -> NaiveDate {
        self.ex_date
            .or(self.trust_entitlement_date())
            .unwrap_or(self.date_paid)
    }

    /// The entitlement date of a trust row, `None` on any other row — the
    /// middle step of [`Self::ex_or_pay_date`]. (Write-time validation keeps
    /// the column off non-trust rows, so the guard only restates that
    /// invariant, as in [`Self::assessment_date`].)
    fn trust_entitlement_date(&self) -> Option<NaiveDate> {
        self.entitlement_date.filter(|_| self.trust_income)
    }

    /// Whether [`Self::ex_or_pay_date`] is a date the statement actually
    /// fixed the entitlement on (a recorded `ex_date`, or a trust row's
    /// `entitlement_date`) rather than the payment-date fallback.
    ///
    /// False means the franking holding-period walk is anchored on a date up
    /// to weeks after the shares really went ex, so its answer is not
    /// reliable — a disposal in between is invisible to it. The franking
    /// at-risk report carries such a dividend as `untested_no_ex_date`
    /// (SCENARIOS G-11), which is why an empty report can be read as an
    /// all-clear.
    ///
    /// A buy-back's dividend component (`buyback_trade_id` set) is true
    /// without either date: it arises from the tender itself, so its
    /// `date_paid` — the participation date, written by the operation, not by
    /// a user — *is* the day the entitlement was fixed, and the walk measures
    /// exactly the days the tendered units were held before it (SCENARIOS
    /// E-31). Nothing there is falling back.
    pub fn ex_date_recorded(&self) -> bool {
        self.ex_date.is_some()
            || self.trust_entitlement_date().is_some()
            || self.buyback_trade_id.is_some()
    }

    /// The distribution's gross cash components in its own currency:
    /// `franked_amount + unfranked_amount + foreign_source_income`.
    ///
    /// Franking credits are notional rather than cash, and foreign tax /  TFN
    /// amounts are withheld *from* this gross rather than added to it — so
    /// neither belongs in the sum (see [`Self::net_cash_received`] for the
    /// figure net of what was withheld). This is the gross the per-share check
    /// reconciles the statement's `amount_per_security × securities_held`
    /// against, the AMIT cash cross-check totals, and the activity ledger
    /// prints.
    pub fn gross_cash_income(&self) -> Decimal {
        self.franked_amount + self.unfranked_amount + self.foreign_source_income
    }

    /// Cash actually received from the distribution:
    /// [`Self::gross_cash_income`] less the foreign tax and TFN amounts
    /// withheld at source. Franking credits are a tax offset rather than cash
    /// and never appear in it.
    ///
    /// This is what a DRP has available to reinvest
    /// (`entities::drp_reinvestment`) and what the performance report counts
    /// as an income cash flow — one definition, so the two can't drift.
    pub fn net_cash_received(&self) -> Decimal {
        self.gross_cash_income() - self.foreign_tax_paid - self.tfn_withholding_tax
    }
}

#[derive(Debug, Deserialize)]
pub struct IncomeBody {
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    #[serde(default)]
    pub ex_date: Option<NaiveDate>,
    #[serde(default)]
    pub franked_amount: Decimal,
    #[serde(default)]
    pub unfranked_amount: Decimal,
    #[serde(default)]
    pub foreign_source_income: Decimal,
    #[serde(default)]
    pub foreign_tax_paid: Decimal,
    #[serde(default)]
    pub tfn_withholding_tax: Decimal,
    #[serde(default)]
    pub franking_credits: Decimal,
    #[serde(default)]
    pub lic_capital_gain_amount: Decimal,
    #[serde(default)]
    pub conduit_foreign_income: Decimal,
    #[serde(default)]
    pub trust_income: bool,
    /// See `Income::entitlement_date` — trust rows only.
    #[serde(default)]
    pub entitlement_date: Option<NaiveDate>,
    // No `reinvestment_trade_id`: the DRP link is provenance managed by the
    // reinvest operation (see `Income::reinvestment_trade_id`) — a body value
    // is ignored, and an edit preserves an existing link.
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Defaults to the seeded default holding account when omitted.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
    /// Optional statement cross-check; see `Income::amount_per_security`.
    #[serde(default)]
    pub amount_per_security: Option<Decimal>,
    #[serde(default)]
    pub securities_held: Option<Decimal>,
    /// See `Income::tax_deferred_amount` — trust rows only, ≥ 0.
    #[serde(default)]
    pub tax_deferred_amount: Option<Decimal>,
}

fn default_currency() -> String {
    "AUD".to_string()
}

impl CrudEntity for Income {
    type Key = i64;
    const TABLE: &'static str = "income";
    const COLUMNS: &'static str = "id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
         foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
         lic_capital_gain_amount, conduit_foreign_income, trust_income, entitlement_date, \
         reinvestment_trade_id, currency, buyback_trade_id, holding_account_id, \
         amount_per_security, securities_held, tax_deferred_amount";
    const ORDER_BY: &'static str = "date_paid, id";
    const NOUN: &'static str = "income";
}

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/income", get(http::list_handler::<Income>))
        .route(
            "/income/{id}",
            get(http::get_handler::<Income>).put(upsert).delete(delete),
        )
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Income>, sqlx::Error> {
    http::crud_get(pool, id).await
}

#[derive(thiserror::Error, Debug)]
pub enum UpsertError {
    #[error("income write failed: {0}")]
    Db(#[from] sqlx::Error),
    /// The existing row is a buy-back dividend component (`buyback_trade_id`
    /// set): its figures derive from the buy-back's terms, so free-form edits
    /// are rejected. Delete the buy-back Sell via `DELETE /sells/:id` (which
    /// removes this row too) and re-participate instead. Mapped to `422`.
    #[error("this income row is a buy-back dividend component and cannot be edited")]
    BuyBackIncome,
    /// The existing row has been reinvested (`reinvestment_trade_id` set) and
    /// the edit moves a figure the reinvestment was computed or validated
    /// against — the listing, holding account, currency, the dates fixing the
    /// entitlement, or a cash component. Carries the field name. Undo the
    /// reinvestment (`DELETE /income/:id/reinvest`) first. Mapped to `422`.
    #[error("{0} cannot be changed while this distribution is reinvested")]
    ReinvestedIncome(&'static str),
    /// The supplied per-share figures failed the cross-check. Mapped to `422`.
    #[error("the per-share cross-check failed: {0}")]
    PerShare(#[source] PerShareError),
    /// An `entitlement_date` was supplied on a non-trust row. A dividend is
    /// assessed when paid or credited — present entitlement only shifts the
    /// assessment year of trust distributions (`docs/ato/trust-income-timing.md`).
    /// Mapped to `422`.
    #[error("entitlement_date only applies to trust distributions")]
    EntitlementDateOnNonTrust,
    /// A `tax_deferred_amount` was supplied on a non-trust row. Tax-deferred
    /// amounts are a unit-trust statement concept (CGT event E4,
    /// `docs/ato/cgt-non-assessable-payments.md`) — a company's equivalent is
    /// a return of capital, entered as the corporate action directly. Mapped
    /// to `422`.
    #[error("tax_deferred_amount only applies to trust distributions")]
    TaxDeferredOnNonTrust,
    /// A negative `tax_deferred_amount` — the statement figure is a payment
    /// received, never below zero. Mapped to `422`.
    #[error("tax_deferred_amount cannot be negative")]
    TaxDeferredNegative,
    /// The row's listing is an AMIT (`listings.amit`) but `trust_income` is
    /// false. An AMIT is an attribution managed investment *trust* — its cash
    /// distribution advice is entered as a trust row (cash-only: the AMMA
    /// statement is the assessable record). Mapped to `422`.
    #[error("this listing is an AMIT — its distributions are trust income")]
    AmitNonTrust,
    /// A non-zero notional tax component (`franking_credits`,
    /// `lic_capital_gain_amount`, or `conduit_foreign_income`) on an AMIT
    /// listing's row. An AMIT cash advice is not a tax document — the fund's
    /// attribution (credits, LIC deduction, CFI) is reported by its AMMA
    /// statement, and the tax summary reads it from there alone; a value here
    /// would be stored but never used. Carries the offending field name.
    /// Mapped to `422`.
    #[error("{0} cannot be entered on an AMIT distribution")]
    AmitNotionalComponent(&'static str),
    /// A `tax_deferred_amount` on an AMIT listing's row. An AMIT's cost-base
    /// movement is the AMMA statement's `cost_base_adjustment` (entered as
    /// AMIT adjustments, CGT event E10) — the E4 tax-deferred mechanism is
    /// for non-AMIT trusts. Mapped to `422`.
    #[error("tax_deferred_amount does not apply to an AMIT")]
    AmitTaxDeferred,
    /// A negative money figure (carries the field name). Every income figure
    /// — cash and notional components, withholding, and the per-share
    /// cross-check figures — is the statement's own amount, never below
    /// zero; a negative would silently reduce the year's totals in every
    /// report. Mapped to `422`.
    #[error("{0} cannot be negative")]
    NegativeAmount(&'static str),
    /// `conduit_foreign_income` exceeds `unfranked_amount`. The CFI figure is
    /// a memo *subset* of the unfranked amount (it is assessable to a resident
    /// through that field — see [`Income::conduit_foreign_income`]), so it can
    /// never be the larger of the two: a row where it is has almost certainly
    /// had the statement's CFI line keyed as an amount of its own, which
    /// understates the year's income. Carries both figures so the rejection
    /// can name the ceiling. Mapped to `422`.
    #[error("conduit_foreign_income {cfi} exceeds unfranked_amount {unfranked}")]
    ConduitExceedsUnfranked { cfi: Decimal, unfranked: Decimal },
    /// A franking credit on a non-trust row with no franked dividend behind
    /// it. The credit is attached to the franked part of a distribution, so
    /// without one there is nothing it could have come from — the same rule a
    /// buy-back's terms already carry (`entities::corporate_action`). Mapped
    /// to `422`.
    #[error("franking_credits {0} has no franked dividend behind it")]
    FrankingCreditWithoutDividend(Decimal),
    /// A franking credit above the maximum a company could have attached to
    /// the row's franked amount (`domain::franking_credit`, from
    /// `docs/ato/allocating-franking-credits.md`). Carries the ceiling so the
    /// rejection can name it. Mapped to `422`.
    #[error(
        "franking_credits {credits} exceeds the maximum {ceiling} for a franked amount of {franked}"
    )]
    FrankingCreditAboveMaximum {
        credits: Decimal,
        franked: Decimal,
        ceiling: Decimal,
    },
}

/// Why the supplied per-share figures failed to reconcile (both map to 422).
#[derive(thiserror::Error, Debug, PartialEq)]
pub enum PerShareError {
    /// Exactly one of `amount_per_security` / `securities_held` was supplied —
    /// the cross-check needs both (or neither).
    #[error("amount_per_security and securities_held must be supplied together")]
    SuppliedAlone,
    /// amount_per_security × securities_held, cent-rounded, does not equal
    /// the gross cash components (carried so the rejection can say what the
    /// statement figures actually multiply to).
    #[error("the per-share figures multiply to {product}, which is not the gross cash components")]
    ProductMismatch { product: Decimal },
}

/// Cross-check the optional per-share statement figures against the entered
/// amounts: amount_per_security × securities_held, rounded to the cent (half
/// away from zero, matching statements), must equal the gross cash components
/// `franked + unfranked + foreign_source_income` — franking credits are
/// notional and TFN withholding is deducted from (not part of) the gross.
/// Comparison is numeric (`Decimal` equality ignores trailing zeros). Neither
/// supplied means the figures weren't recorded — nothing to check.
fn check_per_share(income: &Income) -> Result<(), PerShareError> {
    let (aps, held) = match (income.amount_per_security, income.securities_held) {
        (None, None) => return Ok(()),
        (Some(aps), Some(held)) => (aps, held),
        _ => return Err(PerShareError::SuppliedAlone),
    };
    let product = (aps * held)
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    if product != income.gross_cash_income() {
        return Err(PerShareError::ProductMismatch { product });
    }
    Ok(())
}

/// Human-readable body for a per-share 422 (shown by the web UI).
pub(crate) fn per_share_detail(e: &PerShareError) -> String {
    match e {
        PerShareError::SuppliedAlone => {
            "amount_per_security and securities_held must be supplied together \
             — provide both or neither"
                .to_string()
        }
        PerShareError::ProductMismatch { product } => {
            format!(
                "per-share figures do not reconcile: amount_per_security × \
                 securities_held computes to {product}, which must equal \
                 franked + unfranked + foreign source income"
            )
        }
    }
}

pub async fn db_upsert(pool: &SqlitePool, income: &Income) -> Result<(), UpsertError> {
    // No money figure on the row may be negative: statements report positive
    // (or zero) amounts, and a negative would silently reduce the year's
    // totals in every report. Checked before the per-share cross-check so a
    // negative per-share figure gets the clearer message.
    for (field, value) in [
        ("franked_amount", Some(income.franked_amount)),
        ("unfranked_amount", Some(income.unfranked_amount)),
        ("foreign_source_income", Some(income.foreign_source_income)),
        ("foreign_tax_paid", Some(income.foreign_tax_paid)),
        ("tfn_withholding_tax", Some(income.tfn_withholding_tax)),
        ("franking_credits", Some(income.franking_credits)),
        (
            "lic_capital_gain_amount",
            Some(income.lic_capital_gain_amount),
        ),
        (
            "conduit_foreign_income",
            Some(income.conduit_foreign_income),
        ),
        ("amount_per_security", income.amount_per_security),
        ("securities_held", income.securities_held),
    ] {
        if value.is_some_and(|v| v < Decimal::ZERO) {
            return Err(UpsertError::NegativeAmount(field));
        }
    }
    check_per_share(income).map_err(UpsertError::PerShare)?;
    if income.entitlement_date.is_some() && !income.trust_income {
        return Err(UpsertError::EntitlementDateOnNonTrust);
    }
    if let Some(td) = income.tax_deferred_amount {
        if !income.trust_income {
            return Err(UpsertError::TaxDeferredOnNonTrust);
        }
        if td < Decimal::ZERO {
            return Err(UpsertError::TaxDeferredNegative);
        }
    }

    let mut tx = pool.begin().await?;

    let existing: Option<Income> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM income WHERE id = ?",
        <Income as CrudEntity>::COLUMNS
    )))
    .bind(income.id)
    .fetch_optional(&mut *tx)
    .await?;

    // A buy-back dividend-component row is immutable here: it was created
    // from its action's terms by the participation operation. (The INSERT
    // below never sets buyback_trade_id, so a normal row can't become one
    // either.)
    if let Some(existing) = &existing {
        if existing.buyback_trade_id.is_some() {
            return Err(UpsertError::BuyBackIncome);
        }
        // A reinvested distribution's load-bearing figures are frozen while
        // the link stands. The reinvest operation validated each of them —
        // the enrolment covering the entitlement date, in this listing and
        // this account, and the cash the units and residual were computed
        // from — and the DRP trade it created records the answers. Letting
        // them move afterwards reintroduces states the operation itself
        // refuses (SCENARIOS I-07): a link across listings, a trade in
        // another account's residual chain, a reinvestment resting on an
        // enrolment that no longer covers it, or a parcel costed from cash
        // that no longer exists — breaking the identity the whole operation
        // rests on, that the acquisition cost *is* the dividend applied
        // (`docs/ato/cgt-dividend-reinvestment-plans.md`). Undo the
        // reinvestment (`DELETE /income/:id/reinvest`), edit, redo.
        //
        // The same shape as a Buy's guards against an allocating Sell
        // (`trade::db_upsert`, SCENARIOS A-09/A-13). Notional and memo
        // figures — franking credits, the LIC and CFI amounts, the per-share
        // cross-check pair — are not part of the reinvestment and stay
        // editable.
        if existing.reinvestment_trade_id.is_some() {
            let frozen = [
                ("listing_id", existing.listing_id != income.listing_id),
                (
                    "holding_account_id",
                    existing.holding_account_id != income.holding_account_id,
                ),
                ("currency", existing.currency != income.currency),
                ("date_paid", existing.date_paid != income.date_paid),
                ("ex_date", existing.ex_date != income.ex_date),
                (
                    "entitlement_date",
                    existing.entitlement_date != income.entitlement_date,
                ),
                ("trust_income", existing.trust_income != income.trust_income),
                (
                    "franked_amount",
                    existing.franked_amount != income.franked_amount,
                ),
                (
                    "unfranked_amount",
                    existing.unfranked_amount != income.unfranked_amount,
                ),
                (
                    "foreign_source_income",
                    existing.foreign_source_income != income.foreign_source_income,
                ),
                (
                    "foreign_tax_paid",
                    existing.foreign_tax_paid != income.foreign_tax_paid,
                ),
                (
                    "tfn_withholding_tax",
                    existing.tfn_withholding_tax != income.tfn_withholding_tax,
                ),
            ];
            if let Some((field, _)) = frozen.iter().find(|(_, changed)| *changed) {
                return Err(UpsertError::ReinvestedIncome(field));
            }
        }
    }

    // AMIT listings take cash-only income rows: the row funds the DRP chain
    // (cash components, source withholding) but the AMMA statement is the only
    // assessable record, so the notional tax components must be entered there
    // — a value here would be stored and silently never reported. An unknown
    // listing falls through to the INSERT's FK rejection.
    let listing_amit: Option<(bool, Option<chrono::NaiveDate>)> =
        sqlx::query_as("SELECT amit, amit_from FROM listings WHERE id = ?")
            .bind(income.listing_id)
            .fetch_optional(&mut *tx)
            .await?;
    // A fund that converted mid-history is an AMIT only from its first AMIT
    // income year: the pre-conversion years' rows are ordinary trust
    // distributions, notional components and tax-deferred amounts included
    // (SCENARIOS F-23). `listing::amit_in_tax_year` is the shared rule.
    let row_is_amit = listing_amit.is_some_and(|(amit, amit_from)| {
        crate::entities::listing::amit_in_tax_year(
            amit,
            amit_from,
            crate::domain::tax_year::tax_year_for(income.assessment_date()),
        )
    });
    if row_is_amit {
        if !income.trust_income {
            return Err(UpsertError::AmitNonTrust);
        }
        for (field, value) in [
            ("franking_credits", income.franking_credits),
            ("lic_capital_gain_amount", income.lic_capital_gain_amount),
            ("conduit_foreign_income", income.conduit_foreign_income),
        ] {
            if value != Decimal::ZERO {
                return Err(UpsertError::AmitNotionalComponent(field));
            }
        }
        if income.tax_deferred_amount.is_some() {
            return Err(UpsertError::AmitTaxDeferred);
        }
    }

    // A company's franking credit is bounded by what it could have attached
    // to the franked amount (`domain::franking_credit`). Trust rows are out of
    // scope — the "franked distributions from trusts" component can be reduced
    // by the trust's own deductions while the member still claims the full
    // credit (AMMA guidance notes, Part B item 13Q) — and an AMIT row rejects
    // credits outright above, so this only ever sees a company dividend.
    if !income.trust_income && income.franking_credits > Decimal::ZERO {
        if income.franked_amount.is_zero() {
            return Err(UpsertError::FrankingCreditWithoutDividend(
                income.franking_credits,
            ));
        }
        if let Some(ceiling) = crate::domain::franking_credit::credit_above_ceiling(
            income.franked_amount,
            income.franking_credits,
            income.date_paid,
        ) {
            return Err(UpsertError::FrankingCreditAboveMaximum {
                credits: income.franking_credits,
                franked: income.franked_amount,
                ceiling,
            });
        }
    }

    // The CFI figure is a memo subset of the unfranked amount, so it cannot
    // exceed it (see `Income::conduit_foreign_income`). Checked after the AMIT
    // block deliberately: an AMIT row must carry no CFI at all, and that
    // rejection names the reason — the AMMA statement is the tax record — which
    // is more use than the ceiling wording here.
    if income.conduit_foreign_income > income.unfranked_amount {
        return Err(UpsertError::ConduitExceedsUnfranked {
            cfi: income.conduit_foreign_income,
            unfranked: income.unfranked_amount,
        });
    }

    // `reinvestment_trade_id` is deliberately absent from both the column
    // list (a new row starts unlinked) and the ON CONFLICT SET (an edit
    // preserves an existing link): the DRP link is written only by the
    // reinvest operation, in its own transaction.
    sqlx::query(
        "INSERT INTO income \
         (id, listing_id, date_paid, ex_date, franked_amount, unfranked_amount, \
          foreign_source_income, foreign_tax_paid, tfn_withholding_tax, franking_credits, \
          lic_capital_gain_amount, conduit_foreign_income, trust_income, entitlement_date, \
          currency, holding_account_id, amount_per_security, \
          securities_held, tax_deferred_amount) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
             listing_id                 = excluded.listing_id, \
             date_paid                  = excluded.date_paid, \
             ex_date                    = excluded.ex_date, \
             franked_amount             = excluded.franked_amount, \
             unfranked_amount           = excluded.unfranked_amount, \
             foreign_source_income      = excluded.foreign_source_income, \
             foreign_tax_paid           = excluded.foreign_tax_paid, \
             tfn_withholding_tax        = excluded.tfn_withholding_tax, \
             franking_credits           = excluded.franking_credits, \
             lic_capital_gain_amount = excluded.lic_capital_gain_amount, \
             conduit_foreign_income     = excluded.conduit_foreign_income, \
             trust_income               = excluded.trust_income, \
             entitlement_date           = excluded.entitlement_date, \
             currency                   = excluded.currency, \
             holding_account_id         = excluded.holding_account_id, \
             amount_per_security        = excluded.amount_per_security, \
             securities_held            = excluded.securities_held, \
             tax_deferred_amount        = excluded.tax_deferred_amount",
    )
    .bind(income.id)
    .bind(income.listing_id)
    .bind(income.date_paid)
    .bind(income.ex_date)
    .bind(Money(income.franked_amount))
    .bind(Money(income.unfranked_amount))
    .bind(Money(income.foreign_source_income))
    .bind(Money(income.foreign_tax_paid))
    .bind(Money(income.tfn_withholding_tax))
    .bind(Money(income.franking_credits))
    .bind(Money(income.lic_capital_gain_amount))
    .bind(Money(income.conduit_foreign_income))
    .bind(income.trust_income)
    .bind(income.entitlement_date)
    .bind(&income.currency)
    .bind(income.holding_account_id)
    .bind(OptMoney(income.amount_per_security))
    .bind(OptMoney(income.securities_held))
    .bind(OptMoney(income.tax_deferred_amount))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Outcome of a delete request, so the handler can map to the right status.
#[derive(Debug, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    NotFound,
    /// The row is a buy-back dividend component (`buyback_trade_id` set) —
    /// deleting it alone would leave the buy-back Sell without its dividend
    /// side. Delete the Sell via `DELETE /sells/:id` instead (which removes
    /// this row too). Mapped to `422`.
    BuyBackIncome,
    /// The row has a reinvestment trade (`reinvestment_trade_id` set) —
    /// deleting it alone would orphan the DRP trade (a parcel with no funding
    /// distribution, invisible to `trade::db_delete`'s reinvestment guard).
    /// Undo the reinvestment first (`DELETE /income/:id/reinvest`). Mapped to
    /// `422`.
    ReinvestedIncome,
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<DeleteOutcome, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let links: Option<(Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT buyback_trade_id, reinvestment_trade_id FROM income WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match links {
        None => return Ok(DeleteOutcome::NotFound),
        Some((Some(_), _)) => return Ok(DeleteOutcome::BuyBackIncome),
        Some((_, Some(_))) => return Ok(DeleteOutcome::ReinvestedIncome),
        Some((None, None)) => {}
    }

    sqlx::query("DELETE FROM income WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(DeleteOutcome::Deleted)
}

async fn upsert(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
    Json(body): Json<IncomeBody>,
) -> Result<StatusCode, ApiError> {
    let income = Income {
        id,
        listing_id: body.listing_id,
        date_paid: body.date_paid,
        ex_date: body.ex_date,
        franked_amount: body.franked_amount,
        unfranked_amount: body.unfranked_amount,
        foreign_source_income: body.foreign_source_income,
        foreign_tax_paid: body.foreign_tax_paid,
        tfn_withholding_tax: body.tfn_withholding_tax,
        franking_credits: body.franking_credits,
        lic_capital_gain_amount: body.lic_capital_gain_amount,
        conduit_foreign_income: body.conduit_foreign_income,
        trust_income: body.trust_income,
        entitlement_date: body.entitlement_date,
        // Provenance links are never client-settable; db_upsert doesn't
        // write them anyway (an edit preserves an existing DRP link).
        reinvestment_trade_id: None,
        currency: body.currency,
        buyback_trade_id: None,
        holding_account_id: body.holding_account_id,
        amount_per_security: body.amount_per_security,
        securities_held: body.securities_held,
        tax_deferred_amount: body.tax_deferred_amount,
    };
    db_upsert(&pool, &income).await?;
    Ok(StatusCode::NO_CONTENT)
}

impl From<UpsertError> for ApiError {
    fn from(e: UpsertError) -> Self {
        match e {
            // Managed by the buy-back participation → 422.
            UpsertError::BuyBackIncome => ApiError::unprocessable(
                "this income row is a buy-back dividend component and cannot be edited — \
                 it is managed by the buy-back participation",
            ),
            // Frozen while the DRP link stands — and the undo that frees it
            // is one call, so the rejection names it rather than leaving the
            // user to find it.
            UpsertError::ReinvestedIncome(field) => ApiError::unprocessable(format!(
                "{field} cannot be changed while this distribution is reinvested — its DRP trade \
                 was created from it. Undo the reinvestment (DELETE /income/:id/reinvest, or the \
                 Undo reinvest action), make the change, then reinvest again"
            )),
            // The cross-check rejection says what the statement figures
            // multiply to, so a typo is findable without a calculator.
            UpsertError::PerShare(detail) => ApiError::unprocessable(per_share_detail(&detail)),
            UpsertError::EntitlementDateOnNonTrust => ApiError::unprocessable(
                "entitlement_date only applies to trust distributions — a dividend is \
                 assessed when paid; tick trust income or clear the entitlement date",
            ),
            UpsertError::TaxDeferredOnNonTrust => ApiError::unprocessable(
                "tax_deferred_amount only applies to trust distributions — a company's \
                 non-assessable payment is entered as a ReturnOfCapital corporate action \
                 instead; tick trust income or clear the tax-deferred amount",
            ),
            UpsertError::TaxDeferredNegative => ApiError::unprocessable(
                "tax_deferred_amount cannot be negative — it is a payment received per \
                 the trust's statement",
            ),
            UpsertError::AmitNonTrust => ApiError::unprocessable(
                "this listing is an AMIT — its distributions are trust income; tick \
                 trust income (the cash row funds DRP reinvestment, while the fund's \
                 AMMA statement carries the assessable figures)",
            ),
            UpsertError::AmitNotionalComponent(field) => ApiError::unprocessable(format!(
                "{field} cannot be entered on an AMIT distribution — the cash advice \
                 is not a tax record; the fund's attributed components belong on its \
                 AMMA statement, which the tax summary reads instead"
            )),
            UpsertError::AmitTaxDeferred => ApiError::unprocessable(
                "tax_deferred_amount does not apply to an AMIT — its cost-base \
                 movement is the AMMA statement's cost_base_adjustment, entered as \
                 AMIT adjustments (CGT event E10), not an E4 tax-deferred amount",
            ),
            UpsertError::NegativeAmount(field) => ApiError::unprocessable(format!(
                "{field} cannot be negative — income figures are the statement's own \
                 positive (or zero) amounts"
            )),
            UpsertError::ConduitExceedsUnfranked { cfi, unfranked } => {
                ApiError::unprocessable(format!(
                    "conduit foreign income {cfi} cannot exceed the unfranked amount \
                     {unfranked} — the CFI figure is the part of the unfranked dividend \
                     the payer declared to be conduit foreign income, recorded within \
                     the unfranked amount rather than in addition to it (to an Australian \
                     resident it is assessable, and it is counted through the unfranked \
                     amount); enter the statement's full unfranked amount, CFI included"
                ))
            }
            UpsertError::FrankingCreditWithoutDividend(credits) => {
                ApiError::unprocessable(format!(
                    "franking credits of {credits} have no franked dividend behind them — \
                     a credit is attached to the franked part of a distribution, so enter \
                     the statement's franked amount as well (a trust distribution's credits \
                     go on a row ticked as trust income)"
                ))
            }
            UpsertError::FrankingCreditAboveMaximum {
                credits,
                franked,
                ceiling,
            } => ApiError::unprocessable(format!(
                "franking credits of {credits} exceed the {ceiling} maximum a company could \
                 attach to a franked amount of {franked} (the franked amount × 30/70 at the \
                 30% company tax rate, less at a base-rate entity's) — check the statement's \
                 franked amount and credit are not transposed or read from the wrong line; \
                 only the maximum is claimable in any case"
            )),
            UpsertError::Db(err) => err.into(),
        }
    }
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match db_delete(&pool, id).await? {
        DeleteOutcome::Deleted => Ok(StatusCode::NO_CONTENT),
        DeleteOutcome::NotFound => Err(ApiError::not_found("no income with that id")),
        // Managed by the buy-back participation → 422.
        DeleteOutcome::BuyBackIncome => Err(ApiError::unprocessable(
            "this income row is a buy-back dividend component — delete the buy-back Sell \
             instead, which removes it too",
        )),
        // Deleting the distribution alone would orphan its DRP trade → 422.
        DeleteOutcome::ReinvestedIncome => Err(ApiError::unprocessable(
            "this distribution has a reinvestment trade — undo the reinvestment first \
             (DELETE /income/:id/reinvest), then delete the row",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{self, ApiClient, test_pool, ymd};
    use rust_decimal::Decimal;

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_test_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VAS")
            .name("Vanguard Australian Shares ETF")
            .insert(pool)
            .await;
    }

    async fn insert_test_trade(pool: &SqlitePool) -> i64 {
        test_support::drp(1, 1)
            .date(ymd(2024, 3, 15))
            .settlement(ymd(2024, 3, 15))
            .qty(Decimal::from(2))
            .price(Decimal::from(95))
            .insert(pool)
            .await;
        1
    }

    fn dividend_income() -> Income {
        test_support::income(1, 1, ymd(2024, 3, 15))
            .with(|i| {
                i.ex_date = Some(ymd(2024, 3, 1));
                i.franked_amount = Decimal::from(70);
                i.unfranked_amount = Decimal::from(30);
                i.franking_credits = Decimal::from(30);
            })
            .build()
    }

    /// A trust row records its assessment date as the date of present
    /// entitlement (ATO QC 23087) — so a June distribution paid in mid-July is
    /// assessed in the FY just ended, not the one that has begun. A dividend
    /// is always assessed by payment, and a trust row without an entitlement
    /// date falls back to it. Every FY-keyed report shares this one rule.
    #[test]
    fn trust_income_is_assessed_by_present_entitlement_not_payment() {
        let trust = |entitlement| {
            test_support::income(1, 1, ymd(2024, 7, 15))
                .with(|i: &mut Income| {
                    i.trust_income = true;
                    i.entitlement_date = entitlement;
                })
                .build()
        };
        assert_eq!(
            trust(Some(ymd(2024, 6, 30))).assessment_date(),
            ymd(2024, 6, 30),
            "the entitlement date governs the whole row"
        );
        assert_eq!(
            trust(None).assessment_date(),
            ymd(2024, 7, 15),
            "no entitlement date recorded: assessed on payment"
        );
        // A dividend goes by payment even were the column somehow set (write
        // time rejects it on a non-trust row).
        let dividend = test_support::income(1, 1, ymd(2024, 7, 15))
            .with(|i| i.entitlement_date = Some(ymd(2024, 6, 30)))
            .build();
        assert_eq!(dividend.assessment_date(), ymd(2024, 7, 15));
    }

    /// Gross cash is the three cash components; franking credits are notional
    /// and the two withholdings are deducted from (not part of) it — the
    /// distinction between the gross the per-share check reconciles against
    /// and the net cash a DRP reinvests.
    #[test]
    fn gross_cash_excludes_credits_and_net_cash_deducts_withholdings() {
        let income = test_support::income(1, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.franked_amount = Decimal::from(70);
                i.unfranked_amount = Decimal::from(20);
                i.foreign_source_income = Decimal::from(10);
                i.franking_credits = Decimal::from(30);
                i.foreign_tax_paid = Decimal::from(3);
                i.tfn_withholding_tax = Decimal::from(5);
            })
            .build();
        assert_eq!(income.gross_cash_income(), Decimal::from(100));
        assert_eq!(income.net_cash_received(), Decimal::from(92));
    }

    /// Entitlement is fixed when the shares go ex, so `ex_or_pay_date` is the
    /// recorded ex date — and the payment date only when the statement gave
    /// none. It is not [`Income::assessment_date`]: that answers when the
    /// income is taxed, which for a trust row can be a different date again.
    #[test]
    fn ex_or_pay_date_prefers_the_ex_date_and_is_not_the_assessment_date() {
        let paid = ymd(2024, 7, 15);
        let with_ex = test_support::income(1, 1, paid)
            .with(|i| i.ex_date = Some(ymd(2024, 6, 20)))
            .build();
        assert_eq!(with_ex.ex_or_pay_date(), ymd(2024, 6, 20));

        let without_ex = test_support::income(1, 1, paid).build();
        assert_eq!(without_ex.ex_or_pay_date(), paid);

        // A trust row can have all three dates differ: entitled on the ex
        // date, assessed on the entitlement date, paid later still.
        let trust = test_support::income(1, 1, paid)
            .with(|i| {
                i.trust_income = true;
                i.ex_date = Some(ymd(2024, 6, 20));
                i.entitlement_date = Some(ymd(2024, 6, 30));
            })
            .build();
        assert_eq!(trust.ex_or_pay_date(), ymd(2024, 6, 20));
        assert_eq!(trust.assessment_date(), ymd(2024, 6, 30));
    }

    /// SCENARIOS G-20. A trust statement rarely prints an ex date, but its
    /// entitlement date *is* the distribution period's end — the day the units
    /// went ex — so it anchors the entitlement in place of the payment date,
    /// which can be weeks later. `ex_date_recorded` says which rows rest on
    /// the payment-date fallback and are therefore not really testable.
    #[test]
    fn a_trust_rows_entitlement_date_anchors_the_entitlement_before_the_pay_date() {
        let paid = ymd(2025, 7, 20);
        let trust = test_support::income(1, 1, paid)
            .with(|i| {
                i.trust_income = true;
                i.entitlement_date = Some(ymd(2025, 6, 30));
            })
            .build();
        assert_eq!(trust.ex_or_pay_date(), ymd(2025, 6, 30));
        assert!(trust.ex_date_recorded());

        // A recorded ex date still wins over it.
        let both = test_support::income(1, 1, paid)
            .with(|i| {
                i.trust_income = true;
                i.ex_date = Some(ymd(2025, 7, 1));
                i.entitlement_date = Some(ymd(2025, 6, 30));
            })
            .build();
        assert_eq!(both.ex_or_pay_date(), ymd(2025, 7, 1));

        // Neither date: the payment-date fallback, flagged as such.
        let bare = test_support::income(1, 1, paid).build();
        assert_eq!(bare.ex_or_pay_date(), paid);
        assert!(!bare.ex_date_recorded());

        // A buy-back's dividend component is fixed by the tender itself, so
        // its payment date is not a fallback (SCENARIOS E-31).
        let buyback = test_support::income(1, 1, paid)
            .with(|i| i.buyback_trade_id = Some(7))
            .build();
        assert_eq!(buyback.ex_or_pay_date(), paid);
        assert!(buyback.ex_date_recorded());
    }

    /// `is_foreign_only` is the Australian-side content test the annual tax
    /// report's Item 11 table skips a row by: only a row whose every
    /// Australian-sourced component is zero counts as foreign-only, and each
    /// such component on its own is enough to disqualify it. Read off the
    /// native-currency figures, so a foreign row with no imported FX rate
    /// still classifies.
    #[test]
    fn foreign_only_requires_every_australian_component_to_be_zero() {
        // A US-listed holding's dividend, entered the way the statement reads.
        let foreign = test_support::income(1, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.currency = "USD".to_string();
                i.foreign_source_income = Decimal::from(100);
                i.foreign_tax_paid = Decimal::from(15);
            })
            .build();
        assert!(foreign.is_foreign_only());
        // An ordinary franked dividend is not.
        assert!(!dividend_income().is_foreign_only());

        // The same foreign row with one Australian-side component added.
        let plus = |set: fn(&mut Income)| {
            let mut income = foreign.clone();
            set(&mut income);
            income
        };
        for (field, income) in [
            ("franked_amount", plus(|i| i.franked_amount = Decimal::ONE)),
            (
                "unfranked_amount",
                plus(|i| i.unfranked_amount = Decimal::ONE),
            ),
            (
                "franking_credits",
                plus(|i| i.franking_credits = Decimal::ONE),
            ),
            (
                "lic_capital_gain_amount",
                plus(|i| i.lic_capital_gain_amount = Decimal::ONE),
            ),
            (
                "conduit_foreign_income",
                plus(|i| i.conduit_foreign_income = Decimal::ONE),
            ),
            (
                "tfn_withholding_tax",
                plus(|i| i.tfn_withholding_tax = Decimal::ONE),
            ),
        ] {
            assert!(
                !income.is_foreign_only(),
                "{field} is Australian-side content"
            );
        }

        // Corollaries: a row with no amounts at all has nothing to declare on
        // the Australian side either, and a tax-deferred amount is a
        // non-assessable payment rather than income of any source.
        let empty = test_support::income(2, 1, ymd(2024, 3, 31)).build();
        assert!(empty.is_foreign_only());
        let deferred = test_support::income(3, 1, ymd(2024, 3, 31))
            .with(|i| {
                i.trust_income = true;
                i.tax_deferred_amount = Some(Decimal::from(50));
            })
            .build();
        assert!(deferred.is_foreign_only());
    }

    // DB-level tests

    #[tokio::test]
    async fn db_dividend_income_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.franked_amount, Decimal::from(70));
        assert_eq!(got.unfranked_amount, Decimal::from(30));
        assert_eq!(got.franking_credits, Decimal::from(30));
        assert_eq!(
            got.ex_date,
            Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap())
        );
        assert!(!got.trust_income);
        assert!(got.reinvestment_trade_id.is_none());
    }

    #[tokio::test]
    async fn db_trust_distribution_insert_and_retrieve() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let dist = Income {
            holding_account_id: 1,
            id: 2,
            listing_id: 1,
            date_paid: NaiveDate::from_ymd_opt(2024, 6, 30).unwrap(),
            ex_date: None,
            franked_amount: Decimal::ZERO,
            unfranked_amount: Decimal::from(50),
            foreign_source_income: Decimal::from(10),
            foreign_tax_paid: "1.5".parse().unwrap(),
            tfn_withholding_tax: Decimal::ZERO,
            franking_credits: Decimal::ZERO,
            lic_capital_gain_amount: Decimal::from(5),
            conduit_foreign_income: Decimal::from(3),
            trust_income: true,
            entitlement_date: None,
            reinvestment_trade_id: None,
            currency: "AUD".to_string(),
            buyback_trade_id: None,
            amount_per_security: None,
            securities_held: None,
            tax_deferred_amount: None,
        };
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(got.trust_income);
        assert_eq!(got.foreign_source_income, Decimal::from(10));
        assert_eq!(got.conduit_foreign_income, Decimal::from(3));
        assert_eq!(got.lic_capital_gain_amount, Decimal::from(5));
    }

    #[tokio::test]
    async fn db_upsert_never_writes_the_reinvestment_link() {
        // The DRP link is provenance managed by the reinvest operation:
        // db_upsert must neither set it on insert (a forged link) nor clear
        // it on update (orphaning the DRP trade) — whatever the struct says.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade_id = insert_test_trade(&pool).await;
        let mut inc = test_support::income(3, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap())
            .with(|i| i.reinvestment_trade_id = Some(trade_id))
            .build();
        db_upsert(&pool, &inc).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(
            got.reinvestment_trade_id, None,
            "a client-supplied link must not be stored"
        );

        // Link it the way the reinvest operation does, then re-upsert with
        // the field absent (None) — the link must survive the edit.
        sqlx::query("UPDATE income SET reinvestment_trade_id = ? WHERE id = 3")
            .bind(trade_id)
            .execute(&pool)
            .await
            .unwrap();
        inc.reinvestment_trade_id = None;
        // An editable field: the cash components and the entitlement dates are
        // frozen while the link stands (`UpsertError::ReinvestedIncome`), but
        // a notional figure is no part of the reinvestment.
        inc.lic_capital_gain_amount = Decimal::from(40);
        db_upsert(&pool, &inc).await.unwrap();
        let got = db_get(&pool, 3).await.unwrap().unwrap();
        assert_eq!(
            got.reinvestment_trade_id,
            Some(trade_id),
            "an edit must preserve the existing link"
        );
        assert_eq!(got.lic_capital_gain_amount, Decimal::from(40));
    }

    /// SCENARIOS I-07. Everything the reinvest operation validated against —
    /// the listing and account whose enrolment it checked, the dates that
    /// fixed the entitlement, the cash it computed units and residual from —
    /// is frozen while the DRP link stands, the way a Buy's date and account
    /// are frozen while a Sell allocates from it. Each refusal names the
    /// field and points at the undo; nothing is persisted.
    #[tokio::test]
    async fn a_reinvested_distribution_freezes_what_the_reinvestment_used() {
        let pool = test_pool().await;
        crate::test_support::listing(1).insert(&pool).await;
        crate::test_support::listing(2)
            .ticker("XYZ")
            .insert(&pool)
            .await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        crate::entities::drp_enrolment::db_upsert(
            &pool,
            &crate::entities::drp_enrolment::DrpEnrolment {
                id: 1,
                listing_id: 1,
                holding_account_id: 1,
                enrolment_date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                unenrolment_date: None,
                residual_handling: Default::default(),
            },
        )
        .await
        .unwrap();
        let original = test_support::income(1, 1, NaiveDate::from_ymd_opt(2024, 3, 15).unwrap())
            .with(|i| {
                i.ex_date = Some(NaiveDate::from_ymd_opt(2024, 3, 1).unwrap());
                i.unfranked_amount = Decimal::from(100);
            })
            .build();
        db_upsert(&pool, &original).await.unwrap();
        crate::entities::drp_reinvestment::db_reinvest(
            &pool,
            1,
            &crate::entities::drp_reinvestment::ReinvestBody {
                reinvestment_price: Decimal::from(7),
                units: None,
                fx_rate: None,
                date: None,
            },
        )
        .await
        .unwrap();

        for (field, edit) in [
            (
                "listing_id",
                (|i: &mut Income| i.listing_id = 2) as fn(&mut Income),
            ),
            ("holding_account_id", |i: &mut Income| {
                i.holding_account_id = 2
            }),
            ("currency", |i: &mut Income| i.currency = "USD".to_string()),
            ("date_paid", |i: &mut Income| {
                i.date_paid = NaiveDate::from_ymd_opt(2024, 4, 15).unwrap()
            }),
            ("ex_date", |i: &mut Income| {
                i.ex_date = Some(NaiveDate::from_ymd_opt(2023, 3, 1).unwrap())
            }),
            ("unfranked_amount", |i: &mut Income| {
                i.unfranked_amount = Decimal::from(200)
            }),
            ("tfn_withholding_tax", |i: &mut Income| {
                i.tfn_withholding_tax = Decimal::from(10)
            }),
        ] {
            let mut edited = original.clone();
            edit(&mut edited);
            let err = db_upsert(&pool, &edited).await.unwrap_err();
            assert!(
                matches!(&err, UpsertError::ReinvestedIncome(f) if *f == field),
                "editing {field}: {err:?}"
            );
        }
        // Nothing moved, and the DRP trade is where it was.
        let stored = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(stored.listing_id, 1);
        assert_eq!(stored.unfranked_amount, Decimal::from(100));
        assert_eq!(
            stored.date_paid,
            NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
        );

        // The 422 names the field and the undo that frees it.
        let response = client(&pool)
            .put(
                "/income/1",
                &serde_json::json!({
                    "listing_id": 1, "date_paid": "2024-03-15", "ex_date": "2024-03-01",
                    "unfranked_amount": "200", "currency": "AUD",
                }),
            )
            .await;
        let (status, body) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("unfranked_amount"), "{body}");
        assert!(body.contains("/reinvest"), "{body}");

        let mut edited = original.clone();
        edited.unfranked_amount = Decimal::from(200);

        // Undo the reinvestment and the same edit goes through.
        crate::entities::drp_reinvestment::db_unreinvest(&pool, 1)
            .await
            .unwrap();
        db_upsert(&pool, &edited).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().unfranked_amount,
            Decimal::from(200)
        );
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    // API-level tests

    #[tokio::test]
    async fn api_upsert_and_get() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "date_paid": "2024-03-15",
            "ex_date": "2024-03-01",
            "franked_amount": 70.0,
            "unfranked_amount": 30.0,
            "franking_credits": 30.0
        });
        let resp = client(&pool).put("/income/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.franked_amount, Decimal::from(70));
    }

    #[tokio::test]
    async fn api_list_returns_ok() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let resp = client(&pool).get("/income").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<Income> = resp.json();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn api_get_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/income/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_delete_existing_returns_no_content() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let resp = client(&pool).delete("/income/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_reinvestment_link_is_not_client_writable() {
        // The DRP link is provenance: a body value is ignored (not stored,
        // not an error), and an edit of a reinvested row preserves the
        // existing link even though the body never carries the field.
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade_id = insert_test_trade(&pool).await;
        let app = client(&pool);

        let resp = app
            .put(
                "/income/1",
                &serde_json::json!({
                    "listing_id": 1,
                    "date_paid": "2024-03-15",
                    "franked_amount": 70.0,
                    "reinvestment_trade_id": trade_id
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.reinvestment_trade_id, None,
            "a client-supplied link must be ignored"
        );

        // Link it the way the reinvest operation does, then edit the row —
        // the link must survive (the old contract silently cleared it). The
        // edit is to a figure the reinvestment did not use: the cash
        // components and entitlement dates are frozen while it stands
        // (`a_reinvested_distribution_freezes_what_the_reinvestment_used`).
        sqlx::query("UPDATE income SET reinvestment_trade_id = ? WHERE id = 1")
            .bind(trade_id)
            .execute(&pool)
            .await
            .unwrap();
        let resp = app
            .put(
                "/income/1",
                &serde_json::json!({
                    "listing_id": 1,
                    "date_paid": "2024-03-15",
                    "franked_amount": 70.0,
                    "lic_capital_gain_amount": 12.0
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.reinvestment_trade_id, Some(trade_id));
        assert_eq!(got.lic_capital_gain_amount, Decimal::from(12));
    }

    #[tokio::test]
    async fn api_delete_reinvested_income_returns_422() {
        // Deleting the distribution alone would orphan its DRP trade — the
        // reinvestment must be undone first (DELETE /income/:id/reinvest).
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trade_id = insert_test_trade(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        sqlx::query("UPDATE income SET reinvestment_trade_id = ? WHERE id = 1")
            .bind(trade_id)
            .execute(&pool)
            .await
            .unwrap();

        let resp = client(&pool).delete("/income/1").await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let text = resp.text().to_string();
        assert!(text.contains("undo the reinvestment"), "body: {text}");
        assert!(
            db_get(&pool, 1).await.unwrap().is_some(),
            "reinvested row must remain"
        );
    }

    #[tokio::test]
    async fn api_delete_missing_returns_404() {
        let pool = test_pool().await;
        let resp = client(&pool).delete("/income/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_decimal_precision_round_trip() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let body = serde_json::json!({
            "listing_id": 1,
            "date_paid": "2024-03-15",
            "franked_amount": "70.123456789",
            "unfranked_amount": "29.876543211",
            "franking_credits": "30.052631578"
        });
        let resp = client(&pool).put("/income/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        let resp = client(&pool).get("/income/1").await;
        let inc: Income = resp.json();
        assert_eq!(
            inc.franked_amount,
            "70.123456789".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            inc.unfranked_amount,
            "29.876543211".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            inc.franking_credits,
            "30.052631578".parse::<Decimal>().unwrap()
        );
    }

    // Per-share cross-check tests

    async fn put_income(
        pool: &SqlitePool,
        id: i64,
        body: serde_json::Value,
    ) -> (StatusCode, String) {
        let resp = client(pool).put(format!("/income/{id}"), &body).await;
        let status = resp.status;
        (status, resp.text().to_string())
    }

    /// PLS 2023 final dividend payment advice: 14 cents per share × 19,695
    /// shares = $2,757.30, 100% franked, franking credit $1,181.70.
    #[tokio::test]
    async fn api_per_share_figures_reconcile_fully_franked_dividend() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2023-09-27",
                "franked_amount": "2757.30",
                "franking_credits": "1181.70",
                "amount_per_security": "0.14",
                "securities_held": "19695"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount_per_security, Some("0.14".parse().unwrap()));
        assert_eq!(got.securities_held, Some(Decimal::from(19695)));
    }

    /// VDHG 2020-10 distribution advice: $0.89891492 per security × 866
    /// securities = $778.4603… — the statement's gross is the cent-rounded
    /// $778.46, so the check must round the product before comparing.
    #[tokio::test]
    async fn api_per_share_product_is_cent_rounded_before_comparison() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2020-10-16",
                "unfranked_amount": "778.46",
                "trust_income": true,
                "amount_per_security": "0.89891492",
                "securities_held": "866"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn api_per_share_mismatch_returns_422_with_detail_and_persists_nothing() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2023-09-27",
                "franked_amount": "2757.30",
                // Typo'd per-share rate: 0.15 × 19,695 = 2,954.25 ≠ 2,757.30.
                "amount_per_security": "0.15",
                "securities_held": "19695"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        // The rejection carries the computed product so the typo is findable.
        assert!(
            detail.contains("2954.25"),
            "detail should cite the product: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_per_share_field_supplied_alone_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for body in [
            serde_json::json!({
                "listing_id": 1, "date_paid": "2024-03-15",
                "unfranked_amount": "100", "amount_per_security": "0.14"
            }),
            serde_json::json!({
                "listing_id": 1, "date_paid": "2024-03-15",
                "unfranked_amount": "100", "securities_held": "19695"
            }),
        ] {
            let (status, detail) = put_income(&pool, 1, body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(detail.contains("together"), "detail: {detail}");
        }
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// The gross the product must match includes foreign source income but
    /// not franking credits or TFN withholding (notional / deducted-from).
    #[tokio::test]
    async fn api_per_share_gross_includes_foreign_income_not_credits_or_withholding() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        // 1.00 × 100 = 100 = franked 60 + unfranked 30 + foreign 10; the
        // franking credits and TFN withholding must not disturb the check.
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franked_amount": "60",
                "unfranked_amount": "30",
                "foreign_source_income": "10",
                "franking_credits": "25.71",
                "tfn_withholding_tax": "47",
                "amount_per_security": "1.00",
                "securities_held": "100"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// Both omitted = no check (existing clients unchanged), and the columns
    /// stay NULL.
    #[tokio::test]
    async fn api_omitted_per_share_pair_skips_the_check() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franked_amount": "70"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.amount_per_security, None);
        assert_eq!(got.securities_held, None);
    }

    #[tokio::test]
    async fn api_per_share_decimal_precision_round_trips() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2020-10-16",
                "unfranked_amount": "778.46",
                "amount_per_security": "0.89891492",
                "securities_held": "866"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let resp = client(&pool).get("/income/1").await;
        let inc: Income = resp.json();
        assert_eq!(
            inc.amount_per_security,
            Some("0.89891492".parse::<Decimal>().unwrap())
        );
        assert_eq!(inc.securities_held, Some(Decimal::from(866)));
    }

    /// Every money column rejects a negative value with 422 naming the field,
    /// and nothing is persisted (2026-07-12 review: negatives were accepted on
    /// every money column, silently reducing the year's totals).
    #[tokio::test]
    async fn api_negative_amount_on_any_money_column_returns_422() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        for field in [
            "franked_amount",
            "unfranked_amount",
            "foreign_source_income",
            "foreign_tax_paid",
            "tfn_withholding_tax",
            "franking_credits",
            "lic_capital_gain_amount",
            "conduit_foreign_income",
            "amount_per_security",
            "securities_held",
        ] {
            let mut body = serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "unfranked_amount": "100",
                // The per-share pair must come together; keep the partner
                // present so only the negativity rule is under test.
                "amount_per_security": "1",
                "securities_held": "100"
            });
            body[field] = "-1".into();
            let (status, detail) = put_income(&pool, 1, body).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "negative {field} must be rejected"
            );
            assert!(
                detail.contains(field) && detail.contains("cannot be negative"),
                "negative {field}: detail must name the field, got: {detail}"
            );
            assert!(
                db_get(&pool, 1).await.unwrap().is_none(),
                "negative {field}: nothing persisted"
            );
        }

        // Zero components stay accepted (a fully unfranked dividend has a
        // zero franked_amount — the defaults).
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "unfranked_amount": "100"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    /// A franking credit is attached to the franked part of a distribution, so
    /// a credit with nothing behind it is not a dividend a company could pay
    /// (SCENARIOS G-25) — the same rule a buy-back's terms already carry. It
    /// was accepted, and reported a refundable offset against no income at all.
    #[tokio::test]
    async fn db_franking_credit_without_a_dividend_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let row = test_support::income(1, 1, ymd(2024, 3, 15))
            .with(|i: &mut Income| i.franking_credits = Decimal::from(300))
            .build();
        let err = db_upsert(&pool, &row).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::FrankingCreditWithoutDividend(c) if c == Decimal::from(300)),
            "{err:?}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// The credit a company may attach is capped at the franked amount × 30/70
    /// (`domain::franking_credit`, from `docs/ato/allocating-franking-credits.md`
    /// — and where a statement shows more, only the maximum is claimable
    /// anyway). Above it the figure is a data-entry error, and it inflates a
    /// *refundable* offset, so the write is refused rather than reported.
    #[tokio::test]
    async fn db_franking_credit_above_the_company_maximum_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let row = |franked: &str, credits: &str| {
            let (franked, credits) = (franked.to_string(), credits.to_string());
            test_support::income(1, 1, ymd(2024, 3, 15))
                .with(move |i: &mut Income| {
                    i.franked_amount = franked.parse().unwrap();
                    i.franking_credits = credits.parse().unwrap();
                })
                .build()
        };

        // The transposed-column error: $700 franked against $7,000 of credits.
        let err = db_upsert(&pool, &row("700", "7000")).await.unwrap_err();
        assert!(
            matches!(
                err,
                UpsertError::FrankingCreditAboveMaximum { ceiling, .. }
                    if ceiling == "301.50".parse().unwrap()
            ),
            "{err:?}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // A fully franked 30% dividend sits exactly on the maximum (the ATO's
        // own Example 2 figures), and a base-rate entity's 25% dividend well
        // under it — the ceiling is a maximum over every corporate rate.
        db_upsert(&pool, &row("700", "300")).await.unwrap();
        db_upsert(&pool, &row("750", "250")).await.unwrap();
    }

    /// The ceiling is a *company's*. A trust's "franked distributions from
    /// trusts" component can be reduced by the trust's own deductions while
    /// the member still claims the full franking credit (AMMA guidance notes,
    /// Part B item 13Q), so a trust row above the ratio — credits with no
    /// franked component at all included — is left alone.
    #[tokio::test]
    async fn db_a_trust_rows_franking_credits_are_not_capped() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let trust = test_support::income(1, 1, ymd(2024, 3, 15))
            .with(|i: &mut Income| {
                i.trust_income = true;
                i.franked_amount = Decimal::from(100);
                i.franking_credits = Decimal::from(900);
            })
            .build();
        db_upsert(&pool, &trust).await.unwrap();
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().franking_credits,
            Decimal::from(900)
        );
    }

    /// The 422 body names the ceiling and what to check, so a transposed pair
    /// is findable from the message alone.
    #[tokio::test]
    async fn api_franking_credit_above_the_maximum_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franked_amount": "700",
                "franking_credits": "7000"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("301.50") && detail.contains("30/70") && detail.contains("transposed"),
            "detail must name the ceiling and what to check, got: {detail}"
        );

        // …and the no-dividend case says where the credit belongs instead.
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "franking_credits": "300"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("no franked dividend behind them") && detail.contains("trust income"),
            "got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// Conduit foreign income is a memo *within* `unfranked_amount`, so it can
    /// never exceed it (SCENARIOS G-03). The rejection is what stops the
    /// silent understatement the convention would otherwise allow: a user who
    /// keys the statement's CFI line as an amount of its own — leaving the
    /// unfranked amount at zero or short — is told to enter the full unfranked
    /// figure with the CFI portion inside it, rather than having the
    /// difference quietly vanish from the year's assessable income.
    #[tokio::test]
    async fn db_conduit_foreign_income_above_the_unfranked_amount_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let row = |unfranked: i64, cfi: i64| {
            test_support::income(1, 1, ymd(2024, 3, 15))
                .with(move |i: &mut Income| {
                    i.unfranked_amount = Decimal::from(unfranked);
                    i.conduit_foreign_income = Decimal::from(cfi);
                })
                .build()
        };

        // The data-entry error the convention invites: the CFI line keyed alone.
        let err = db_upsert(&pool, &row(0, 40)).await.unwrap_err();
        assert!(
            matches!(
                err,
                UpsertError::ConduitExceedsUnfranked { cfi, unfranked }
                    if cfi == Decimal::from(40) && unfranked == Decimal::ZERO
            ),
            "{err:?}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());

        // Partly keyed the same way (60 unfranked entered beside a 100 CFI).
        assert!(matches!(
            db_upsert(&pool, &row(60, 100)).await.unwrap_err(),
            UpsertError::ConduitExceedsUnfranked { .. }
        ));

        // A proper subset, and a wholly-CFI unfranked dividend (the boundary),
        // are both ordinary rows.
        db_upsert(&pool, &row(100, 40)).await.unwrap();
        assert_eq!(
            db_get(&pool, 1)
                .await
                .unwrap()
                .unwrap()
                .conduit_foreign_income,
            Decimal::from(40)
        );
        db_upsert(&pool, &row(100, 100)).await.unwrap();
    }

    /// The 422 body says which way round the two figures go — the user has to
    /// know to move the CFI amount *into* the unfranked amount, not just that
    /// the write failed.
    #[tokio::test]
    async fn api_conduit_foreign_income_above_unfranked_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "unfranked_amount": "0",
                "conduit_foreign_income": "40"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains("conduit foreign income")
                && detail.contains("unfranked")
                && detail.contains("within"),
            "detail must state the memo convention, got: {detail}"
        );
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    // Entitlement date (trust present-entitlement timing,
    // docs/ato/trust-income-timing.md): trust rows only.

    #[tokio::test]
    async fn db_entitlement_date_round_trips_on_trust_row() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.entitlement_date = Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.entitlement_date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap())
        );
    }

    #[tokio::test]
    async fn db_entitlement_date_on_non_trust_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut div = dividend_income();
        div.entitlement_date = Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        let err = db_upsert(&pool, &div).await.unwrap_err();
        assert!(matches!(err, UpsertError::EntitlementDateOnNonTrust));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_entitlement_date_on_dividend_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2026-07-15",
                "franked_amount": "70",
                "entitlement_date": "2026-06-30"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("trust distributions"), "detail: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    // Tax-deferred amount (CGT event E4 cross-check,
    // docs/ato/cgt-non-assessable-payments.md): trust rows only, ≥ 0.

    #[tokio::test]
    async fn db_tax_deferred_amount_round_trips_on_trust_row() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.tax_deferred_amount = Some("120.505".parse().unwrap());
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.tax_deferred_amount, Some("120.505".parse().unwrap()));
    }

    #[tokio::test]
    async fn db_tax_deferred_amount_on_non_trust_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut div = dividend_income();
        div.tax_deferred_amount = Some("50".parse().unwrap());
        let err = db_upsert(&pool, &div).await.unwrap_err();
        assert!(matches!(err, UpsertError::TaxDeferredOnNonTrust));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_negative_tax_deferred_amount_rejected() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.tax_deferred_amount = Some("-1".parse().unwrap());
        let err = db_upsert(&pool, &dist).await.unwrap_err();
        assert!(matches!(err, UpsertError::TaxDeferredNegative));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_tax_deferred_amount_on_dividend_returns_422_with_detail() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2025-03-15",
                "franked_amount": "70",
                "tax_deferred_amount": "50"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("trust distributions"), "detail: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// Omitted = NULL — the statement didn't report one, nothing to check.
    #[tokio::test]
    async fn api_omitted_tax_deferred_amount_stays_null() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2025-03-15",
                "unfranked_amount": "100",
                "trust_income": true
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.tax_deferred_amount, None);
    }

    // AMIT cash-only rows (REQUIREMENTS 2026-06-12): an AMIT listing's income
    // row funds the DRP chain but the AMMA statement is the only assessable
    // record — notional tax components are rejected at write time so they
    // can't be stored and silently never reported.

    async fn insert_amit_listing(pool: &SqlitePool) {
        test_support::listing(1)
            .ticker("VDHG")
            .amit(true)
            .insert(pool)
            .await;
    }

    /// SCENARIOS F-23: a fund that converted to an AMIT for FY2025. The
    /// pre-conversion years were ordinary trust distributions — franking
    /// credits, LIC deductions and tax-deferred amounts and all — and stay
    /// recordable after the flag goes on; only the AMIT years are cash-only.
    #[tokio::test]
    async fn db_amit_checks_apply_only_from_the_conversion_year() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .amit_from(ymd(2024, 7, 1)) // first AMIT year: FY2025
            .insert(&pool)
            .await;

        // FY2024, before the conversion: an ordinary trust distribution with
        // notional components and a tax-deferred amount.
        let mut before = test_support::income(1, 1, ymd(2024, 2, 15)).build();
        before.trust_income = true;
        before.unfranked_amount = Decimal::from(300);
        before.franking_credits = Decimal::from(30);
        before.tax_deferred_amount = Some(Decimal::from(20));
        db_upsert(&pool, &before).await.unwrap();

        // FY2025, the first AMIT year: the same row shape is refused.
        let mut after = before.clone();
        after.id = 2;
        after.date_paid = ymd(2025, 2, 15);
        let err = db_upsert(&pool, &after).await.unwrap_err();
        assert!(
            matches!(err, UpsertError::AmitNotionalComponent("franking_credits")),
            "{err:?}"
        );
        // …and its cash-only form is accepted.
        after.franking_credits = Decimal::ZERO;
        after.tax_deferred_amount = None;
        db_upsert(&pool, &after).await.unwrap();
    }

    /// The conversion year boundary is the financial year, not the date: a
    /// distribution paid in the AMIT year's first days is already an AMIT
    /// row, and a June payment of the year before is not.
    #[tokio::test]
    async fn db_the_conversion_boundary_is_the_financial_year() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .amit_from(ymd(2024, 7, 1))
            .insert(&pool)
            .await;
        let row = |id: i64, date: NaiveDate| {
            test_support::income(id, 1, date)
                .with(|i| {
                    i.trust_income = true;
                    i.unfranked_amount = Decimal::from(100);
                    i.franking_credits = Decimal::from(10);
                })
                .build()
        };
        // 30 June 2024 — the last day of FY2024, still an ordinary trust.
        db_upsert(&pool, &row(1, ymd(2024, 6, 30))).await.unwrap();
        // 1 July 2024 — the first day of FY2025, the first AMIT year.
        let err = db_upsert(&pool, &row(2, ymd(2024, 7, 1)))
            .await
            .unwrap_err();
        assert!(
            matches!(err, UpsertError::AmitNotionalComponent(_)),
            "{err:?}"
        );
    }

    /// The cash side stays fully recordable: gross components, source
    /// withholding (both reduce DRP-reinvestable cash), ex date, and the
    /// per-share cross-check pair.
    #[tokio::test]
    async fn db_amit_cash_only_row_accepted() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.franking_credits = Decimal::ZERO;
        dist.foreign_source_income = Decimal::from(10);
        dist.foreign_tax_paid = Decimal::from(2);
        dist.tfn_withholding_tax = Decimal::from(1);
        db_upsert(&pool, &dist).await.unwrap();
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.foreign_tax_paid, Decimal::from(2));
        assert_eq!(got.tfn_withholding_tax, Decimal::from(1));
    }

    #[tokio::test]
    async fn db_amit_non_trust_row_rejected() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let mut div = dividend_income();
        div.franking_credits = Decimal::ZERO;
        let err = db_upsert(&pool, &div).await.unwrap_err();
        assert!(matches!(err, UpsertError::AmitNonTrust));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_amit_notional_components_rejected() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let base = || {
            let mut d = dividend_income();
            d.trust_income = true;
            d.franking_credits = Decimal::ZERO;
            d
        };
        let mut with_credits = base();
        with_credits.franking_credits = Decimal::from(30);
        let mut with_lic = base();
        with_lic.lic_capital_gain_amount = Decimal::from(5);
        let mut with_cfi = base();
        with_cfi.conduit_foreign_income = Decimal::from(3);
        let cases = [
            ("franking_credits", with_credits),
            ("lic_capital_gain_amount", with_lic),
            ("conduit_foreign_income", with_cfi),
        ];
        for (field, dist) in cases {
            let err = db_upsert(&pool, &dist).await.unwrap_err();
            assert!(
                matches!(err, UpsertError::AmitNotionalComponent(f) if f == field),
                "expected AmitNotionalComponent({field}), got {err:?}"
            );
        }
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_amit_tax_deferred_amount_rejected() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let mut dist = dividend_income();
        dist.trust_income = true;
        dist.franking_credits = Decimal::ZERO;
        dist.tax_deferred_amount = Some("50".parse().unwrap());
        let err = db_upsert(&pool, &dist).await.unwrap_err();
        assert!(matches!(err, UpsertError::AmitTaxDeferred));
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn api_amit_franking_credits_return_422_with_detail() {
        let pool = test_pool().await;
        insert_amit_listing(&pool).await;
        let (status, detail) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2024-03-15",
                "unfranked_amount": "100",
                "franking_credits": "30",
                "trust_income": true
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(detail.contains("AMMA"), "detail: {detail}");
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    /// Non-AMIT listings are untouched by the AMIT validation: a dividend with
    /// credits and a trust row with a tax-deferred amount both still pass.
    #[tokio::test]
    async fn db_non_amit_rows_unaffected_by_amit_validation() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        db_upsert(&pool, &dividend_income()).await.unwrap();
        let mut trust = dividend_income();
        trust.id = 2;
        trust.trust_income = true;
        trust.tax_deferred_amount = Some("40".parse().unwrap());
        db_upsert(&pool, &trust).await.unwrap();
        assert!(db_get(&pool, 2).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn api_trust_entitlement_date_round_trips() {
        let pool = test_pool().await;
        insert_test_listing(&pool).await;
        let (status, _) = put_income(
            &pool,
            1,
            serde_json::json!({
                "listing_id": 1,
                "date_paid": "2026-07-15",
                "unfranked_amount": "100",
                "trust_income": true,
                "entitlement_date": "2026-06-30"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let resp = client(&pool).get("/income/1").await;
        let inc: Income = resp.json();
        assert_eq!(
            inc.entitlement_date,
            Some(NaiveDate::from_ymd_opt(2026, 6, 30).unwrap())
        );
        assert!(inc.trust_income);
    }
}
