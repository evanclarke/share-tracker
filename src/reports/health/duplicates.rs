//! The `duplicate_*` family: one fact entered twice, on each side of the data
//! model that a re-submitted form or re-imported statement can double.

use crate::domain::tax_year::tax_year_for;
use crate::entities::ess_statement::{self, EssStatement};
use crate::entities::income::Income;
use crate::entities::inheritance::{self, Inheritance};
use crate::entities::interest_income::InterestIncome;
use crate::entities::investment_expense::{ExpenseType, InvestmentExpense};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// More than one trade on one listing carrying the **same contract note
/// reference**. A broker's contract note reference identifies one confirmation
/// document, so two trades of a listing quoting the same one are the same
/// confirmation entered twice — the duplication a bulk back-entry of history
/// produces most easily and pays for most dearly (SCENARIOS V-09): a doubled
/// Buy inflates both the holding and its cost base in every open-parcels,
/// valuation and unrealised-gains figure, and a doubled Sell inflates realised
/// gains while its allocations quietly consume a second parcel. Nothing else
/// sees it — the two rows are individually valid, and a reconciliation against
/// a registry statement is the only other thing that would ever catch it.
///
/// **The key is the reference, not the figures**, which is the whole reason
/// this list can exist at all. Keyed the way [`DuplicateIncome`] is — listing,
/// account, date, price, quantity — a trade check would flag the legitimate
/// repeats that trading actually produces (an order filled in equal clips
/// across one day, a regular fixed-dollar purchase) and the derived rows a
/// rollover or reinvestment writes. A repeated document id has no such
/// innocent reading, so this reports **no false positives** — at the price of
/// only seeing trades whose entry recorded the reference. It is a check on
/// what was typed in, not a check on the portfolio: a re-keyed trade that
/// left `contract_note_ref` empty is invisible here, and deliberately so.
///
/// **Scoped per listing.** One contract note can cover a multi-line order —
/// two securities bought under one confirmation — and those two trades share a
/// reference legitimately, so the listing is part of the key. Within one
/// listing the same reference twice has no such reading: a broker issues one
/// note per confirmation, and an order filled over two days is two notes with
/// two references. The holding account is deliberately **not** part of the
/// key: the same note re-keyed against the wrong account is exactly the double
/// entry this exists for, and it would escape a per-account key.
///
/// The reference is compared **trimmed and case-sensitively**: surrounding
/// whitespace is how the same reference pasted twice usually differs, while a
/// reference that is empty or only whitespace is no reference at all and never
/// groups (nor does a NULL one). Derived trades — the rollover, transfer,
/// buy-back, rights-exercise, ESS-vest and inheritance-linked trades, and the
/// DRPs a reinvestment writes — record no reference at any path, so they fall
/// out of this check entirely rather than being excluded by name.
///
/// **Reported, never rejected, and with no way to silence it**, exactly like
/// the rest of the `duplicate_*` family: it clears when the surplus trade goes
/// (`DELETE /trades/:id`, `DELETE /sells/:id`) or when the reference on it is
/// corrected to the one the second note actually carries.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateTrade {
    pub listing_id: i64,
    pub ticker: String,
    /// The shared reference, trimmed — the second half of the key, and what
    /// the note itself is found by in the broker's records.
    pub contract_note_ref: String,
    /// The latest `date` among the trades sharing it. The rows normally share
    /// one date (they are one confirmation); two dates under one reference is
    /// itself part of the evidence, and this is what the list is sorted by,
    /// newest first.
    pub date: NaiveDate,
    /// How many trades share this (listing, reference) — always ≥ 2.
    pub trade_count: i64,
    /// The ids sharing it, ascending, so both rows can be opened and the
    /// surplus one deleted without a search.
    pub trade_ids: Vec<i64>,
}

/// The grouped row behind [`DuplicateTrade`], shaped like
/// [`DuplicateActionRow`]: SQLite returns the ids as one `GROUP_CONCAT`
/// string, split into the public struct's `Vec<i64>` by
/// [`db_duplicate_trades`].
#[derive(sqlx::FromRow)]
pub(super) struct DuplicateTradeRow {
    listing_id: i64,
    ticker: String,
    contract_note_ref: String,
    date: NaiveDate,
    trade_count: i64,
    trade_ids: String,
}

/// More than one corporate action of the same type, on the same listing and
/// date. Two such rows are two independent events to every reader — the
/// cost-base pipeline sums both `ReturnOfCapital` reductions and multiplies
/// both `ShareSplit` ratios (SCENARIOS E-03, E-15) — so a re-submitted form or
/// a re-imported statement restates every cost base and quantity of the
/// listing with nothing to show for it.
///
/// Deliberately a **warning, not a constraint**: a genuine same-day pair
/// exists in principle (two tranches of one capital return), so the pair stays
/// enterable and this names it for the user to judge.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateAction {
    pub listing_id: i64,
    pub ticker: String,
    pub action_type: String,
    pub date: NaiveDate,
    /// How many actions share this (listing, type, date) — always ≥ 2.
    pub action_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be found and
    /// deleted without a search.
    pub action_ids: Vec<i64>,
}

/// The grouped row behind [`DuplicateAction`]: SQLite returns the ids as one
/// `GROUP_CONCAT` string, split into the public struct's `Vec<i64>` by
/// [`db_duplicate_actions`].
#[derive(sqlx::FromRow)]
pub(super) struct DuplicateActionRow {
    listing_id: i64,
    ticker: String,
    action_type: String,
    date: NaiveDate,
    action_count: i64,
    action_ids: String,
}

/// More than one AMMA statement for the same listing, financial year and
/// holding account. Every reader counts both: the tax summary's `amma_*`
/// lines and the net-capital-gain report's gain buckets each sum all
/// statements of the year, and — because the one-adjustment-per-parcel UNIQUE
/// index is per *statement* — each statement can also generate its own full
/// set of AMIT adjustments, reducing every parcel a second time
/// (SCENARIOS F-06). The usual cause is an amended statement entered as a new
/// row instead of over the original.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateAction`]: a genuine pair exists in principle (a registry change
/// mid-year, or a fund merger, leaves two part-year statements for one
/// account) and an amended statement is easier to check against the original
/// while both are enterable. The holding account is part of the key because
/// the statement is issued per holder account — a fund held in two accounts
/// legitimately has two statements for one year (SCENARIOS F-03).
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateAmmaStatement {
    pub listing_id: i64,
    pub ticker: String,
    /// The financial year the statements attribute, identified by the
    /// calendar year of their shared 30 June end (`domain::tax_year`).
    pub tax_year: i32,
    pub holding_account_id: i64,
    /// How many statements share this (listing, year, account) — always ≥ 2.
    pub statement_count: i64,
    /// The ids sharing it, ascending, so the superseded row can be found and
    /// deleted without a search.
    pub statement_ids: Vec<i64>,
}

/// The grouped row behind [`DuplicateAmmaStatement`], shaped like
/// [`DuplicateActionRow`]: SQLite returns the ids as one `GROUP_CONCAT`
/// string, split into the public struct's `Vec<i64>` by
/// [`db_duplicate_amma_statements`].
#[derive(sqlx::FromRow)]
pub(super) struct DuplicateAmmaStatementRow {
    listing_id: i64,
    ticker: String,
    tax_year_end_date: NaiveDate,
    holding_account_id: i64,
    statement_count: i64,
    statement_ids: String,
}

/// More than one income row for the same listing, holding account and payment
/// date, carrying **identical amounts**. Every reader counts both: the tax
/// summary's dividend lines, the franking credits behind them, the foreign
/// income and the FITO limit are each summed row by row, so a re-submitted
/// form or a re-imported statement declares the distribution twice
/// (SCENARIOS G-24).
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateAction`] and [`DuplicateAmmaStatement`]: two dividends from one
/// company on one day are legitimate in principle (an ordinary and a special
/// dividend), so the pair stays enterable.
///
/// The amounts are part of the key for exactly that reason. (listing, account,
/// date) alone would flag that legitimate pair, which differs in what it pays;
/// requiring every money figure to match as well — and the currency they are
/// stated in — leaves only what is almost certainly one distribution entered
/// twice. Non-money differences (an `ex_date` filled in on one row only, a
/// trust flag) are ignored: they are how a re-entry usually differs from the
/// original, not evidence of a second payment.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateIncome {
    pub listing_id: i64,
    pub ticker: String,
    pub holding_account_id: i64,
    pub date_paid: NaiveDate,
    /// ISO 4217 currency the shared amounts are stated in (part of the key —
    /// two rows stating the same figures in different currencies are not the
    /// same payment).
    pub currency: String,
    /// The gross cash the duplicated rows each declare
    /// (`Income::gross_cash_income`, in `currency`), so the warning names the
    /// distribution rather than only its date.
    pub gross_amount: Decimal,
    /// How many rows share this (listing, account, date, amounts) — always ≥ 2.
    pub income_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be found and
    /// deleted without a search.
    pub income_ids: Vec<i64>,
}

/// More than one interest-income row carrying identical figures on one date
/// from one source (SCENARIOS H-01). Interest has no listing to key on, so
/// `source` — the free-text payer, "ANZ savings account" — and the optional
/// holding account stand in for it: two $250 credits on one day from two
/// different banks are legitimate and stay unflagged, while the same credit
/// keyed twice doubles the year's `interest_income` line and any withholding
/// beside it.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateIncome`]: a payer really can credit the same amount twice in one
/// day (two term deposits of equal size maturing together), so the pair stays
/// enterable.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateInterest {
    pub date_paid: NaiveDate,
    /// ISO 4217 currency the shared amount is stated in (part of the key).
    pub currency: String,
    /// The gross interest each duplicated row declares, in `currency`.
    pub amount: Decimal,
    /// The free-text source the rows share (part of the key); `None` when none
    /// of them recorded one.
    pub source: Option<String>,
    /// The holding account the rows share (part of the key); `None` for
    /// interest from outside the portfolio's accounts.
    pub holding_account_id: Option<i64>,
    /// How many rows share the whole key — always ≥ 2.
    pub interest_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub interest_ids: Vec<i64>,
}

/// More than one investment-expense row carrying identical figures on one date
/// (SCENARIOS H-06). The deduction is claimed once per row, so a re-submitted
/// form lifts the year's `deductions_*` line and lowers
/// `net_assessable_investment_income` by the same amount again.
///
/// The key is everything that identifies the expense: the date, the type, the
/// money figures (including the `gross_amount` / `deductible_percentage`
/// provenance pair), the currency, the free-text description, and both optional
/// attributions. Two advice fees of $200 on one day against two different
/// listings — or with different descriptions — are legitimate and stay
/// unflagged; the same invoice keyed twice is not.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateIncome`].
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateExpense {
    pub date_incurred: NaiveDate,
    pub expense_type: ExpenseType,
    /// ISO 4217 currency the shared amount is stated in (part of the key).
    pub currency: String,
    /// The deductible amount each duplicated row claims, in `currency`.
    pub amount: Decimal,
    /// The free-text description the rows share (part of the key).
    pub description: Option<String>,
    /// The listing the rows are attributed to (part of the key); `None` for a
    /// portfolio-wide expense.
    pub listing_id: Option<i64>,
    /// That listing's ticker, so the warning names the holding rather than only
    /// its id; `None` for a portfolio-wide expense.
    pub ticker: Option<String>,
    /// The holding account the rows are attributed to (part of the key);
    /// `None` for a portfolio-wide expense.
    pub holding_account_id: Option<i64>,
    /// How many rows share the whole key — always ≥ 2.
    pub expense_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub expense_ids: Vec<i64>,
}

/// More than one ESS statement for the same listing, holding account and
/// taxing point, carrying **identical figures** (SCENARIOS J-11). Every reader
/// counts both: the tax summary's Item 12 discount labels (and the $1,000
/// taxed-upfront reduction computed over them) sum statement by statement, and
/// each statement vests its **own** parcel — so a $1,000 grant of 100 shares
/// entered twice reports $2,000 of discount and 200 shares held.
///
/// The 30-day rule makes this the expected accident rather than a hypothetical:
/// an employer issues an **amended** statement for one vest
/// (`docs/ato/ess-30-day-rule.md`), and a user who enters both instead of
/// editing the original has exactly this shape.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateIncome`]: two vests on one date from different grants are
/// ordinary, so the pair stays enterable. The figures are part of the key for
/// exactly that reason — two same-date statements differing in quantity, market
/// value or any discount label are two grants, not one entered twice.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateEssStatement {
    pub listing_id: i64,
    pub ticker: String,
    pub holding_account_id: i64,
    pub taxing_point_date: NaiveDate,
    /// ISO 4217 currency the shared figures are stated in (part of the key).
    pub currency: String,
    /// The shares each duplicated statement vests, so the warning names the
    /// grant rather than only its date.
    pub quantity: Decimal,
    /// The total Item 12 discount each duplicated statement declares (D + E +
    /// F + the pre-2009 cessation label, in `currency`) — the figure that is
    /// counted once per statement.
    pub discount_total: Decimal,
    /// How many statements share the whole key — always ≥ 2.
    pub statement_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub statement_ids: Vec<i64>,
}

/// Two or more inheritances of one listing, in one holding account, from one
/// date of death, carrying **identical figures** (SCENARIOS K-09). Each one
/// creates its own parcel Buy, so a 100-unit holding entered twice is 200
/// units held at twice the cost base — every open-parcels, valuation and
/// realised-gains figure for the listing doubles, and unlike the income-side
/// duplicates there is no year to bound the error: it persists until the
/// parcel is sold.
///
/// Deliberately a **warning, not a constraint**, the same call as
/// [`DuplicateEssStatement`]: two inheritances of one listing from one death
/// are ordinary — two beneficiaries' shares are not modelled, but two holding
/// accounts, two estates, or a part interest recorded in stages all are. The
/// figures are part of the key for exactly that reason: two rows differing in
/// quantity, cost base or LPR expenditure are two inheritances, not one
/// entered twice.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct DuplicateInheritance {
    pub listing_id: i64,
    pub ticker: String,
    pub holding_account_id: i64,
    pub date_of_death: NaiveDate,
    /// ISO 4217 currency the shared figures are stated in (part of the key).
    pub currency: String,
    /// The units each duplicated row inherits, so the warning names the parcel
    /// rather than only its date.
    pub quantity: Decimal,
    /// The whole cost base each duplicated row carries onto its parcel (first
    /// element + LPR expenditure, in `currency`) — the figure counted once per
    /// row.
    pub cost_base_total: Decimal,
    /// How many inheritances share the whole key — always ≥ 2.
    pub inheritance_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be opened and
    /// deleted without a search.
    pub inheritance_ids: Vec<i64>,
}

/// Trades of one listing sharing a contract note reference — see
/// [`DuplicateTrade`].
///
/// Grouped in SQL on the caller's transaction, like [`db_duplicate_actions`]:
/// the key is a TEXT reference rather than a decimal, so SQLite's own string
/// comparison is the right one (the reason the money-keyed lists have to group
/// in Rust does not arise). `TRIM` normalises the padding a pasted reference
/// picks up, and the `<> ''` guard drops a reference that is blank or only
/// whitespace — no reference at all — as the `IS NOT NULL` guard drops the
/// derived trades, which record none.
pub(super) async fn db_duplicate_trades(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateTrade>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DuplicateTradeRow>(
        "SELECT t.listing_id AS listing_id, l.ticker AS ticker, \
                TRIM(t.contract_note_ref) AS contract_note_ref, \
                MAX(t.date) AS date, \
                COUNT(*) AS trade_count, GROUP_CONCAT(t.id) AS trade_ids \
         FROM trades t JOIN listings l ON l.id = t.listing_id \
         WHERE t.contract_note_ref IS NOT NULL AND TRIM(t.contract_note_ref) <> '' \
         GROUP BY t.listing_id, TRIM(t.contract_note_ref) \
         HAVING COUNT(*) > 1 \
         ORDER BY MAX(t.date) DESC, l.ticker, TRIM(t.contract_note_ref)",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            // GROUP_CONCAT's order is unspecified, so sort rather than trust it.
            let mut trade_ids = row
                .trade_ids
                .split(',')
                .map(|id| {
                    id.parse::<i64>()
                        .map_err(|e| sqlx::Error::Decode(format!("trade id {id}: {e}").into()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            trade_ids.sort_unstable();
            Ok(DuplicateTrade {
                listing_id: row.listing_id,
                ticker: row.ticker,
                contract_note_ref: row.contract_note_ref,
                date: row.date,
                trade_count: row.trade_count,
                trade_ids,
            })
        })
        .collect()
}

pub(super) async fn db_duplicate_actions(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateAction>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DuplicateActionRow>(
        "SELECT ca.listing_id AS listing_id, l.ticker AS ticker, \
                ca.action_type AS action_type, ca.date AS date, \
                COUNT(*) AS action_count, GROUP_CONCAT(ca.id) AS action_ids \
         FROM corporate_actions ca JOIN listings l ON l.id = ca.listing_id \
         GROUP BY ca.listing_id, ca.action_type, ca.date \
         HAVING COUNT(*) > 1 \
         ORDER BY ca.date DESC, l.ticker, ca.action_type",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            // GROUP_CONCAT's order is unspecified, so sort rather than trust it.
            let mut action_ids = row
                .action_ids
                .split(',')
                .map(|id| {
                    id.parse::<i64>().map_err(|e| {
                        sqlx::Error::Decode(format!("corporate action id {id}: {e}").into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            action_ids.sort_unstable();
            Ok(DuplicateAction {
                listing_id: row.listing_id,
                ticker: row.ticker,
                action_type: row.action_type,
                date: row.date,
                action_count: row.action_count,
                action_ids,
            })
        })
        .collect()
}

/// AMMA statements sharing a (listing, financial year, holding account) — see
/// [`DuplicateAmmaStatement`].
///
/// Grouped in SQL on the caller's transaction, like [`db_duplicate_actions`]:
/// one small aggregate over `amma_statements`. The year is grouped on
/// `tax_year_end_date` itself rather than a derived year, which is the same
/// thing — the column is a 30 June date, enforced at write time
/// (`entities::amma::UpsertError::NotFinancialYearEnd`).
pub(super) async fn db_duplicate_amma_statements(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateAmmaStatement>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DuplicateAmmaStatementRow>(
        "SELECT a.listing_id AS listing_id, l.ticker AS ticker, \
                a.tax_year_end_date AS tax_year_end_date, \
                a.holding_account_id AS holding_account_id, \
                COUNT(*) AS statement_count, GROUP_CONCAT(a.id) AS statement_ids \
         FROM amma_statements a JOIN listings l ON l.id = a.listing_id \
         GROUP BY a.listing_id, a.tax_year_end_date, a.holding_account_id \
         HAVING COUNT(*) > 1 \
         ORDER BY a.tax_year_end_date DESC, l.ticker, a.holding_account_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            // GROUP_CONCAT's order is unspecified, so sort rather than trust it.
            let mut statement_ids = row
                .statement_ids
                .split(',')
                .map(|id| {
                    id.parse::<i64>().map_err(|e| {
                        sqlx::Error::Decode(format!("AMMA statement id {id}: {e}").into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            statement_ids.sort_unstable();
            Ok(DuplicateAmmaStatement {
                listing_id: row.listing_id,
                ticker: row.ticker,
                tax_year: tax_year_for(row.tax_year_end_date),
                holding_account_id: row.holding_account_id,
                statement_count: row.statement_count,
                statement_ids,
            })
        })
        .collect()
}

/// True when two income rows are the same declared payment: same listing,
/// holding account and payment date, and identical money figures stated in one
/// currency — the fingerprint behind [`DuplicateIncome`].
///
/// Compared as `Decimal`s rather than as the stored TEXT, so `"10.0"` and
/// `"10.00"` — the same dollars written by two different clients — are the
/// match they are, which is also why the grouping cannot be pushed into SQL.
/// Every money column of the row is compared, including the informational
/// ones: two rows agreeing on the assessable figures but disagreeing on, say,
/// `tax_deferred_amount` were entered from different statements. The row's
/// **kind** is part of it too — a dividend and a dividend equivalent of the
/// same amount on one day are two different payments, and the whole point of
/// the kind is that they are not the same thing (SCENARIOS J-10).
pub(super) fn same_income_entry(a: &Income, b: &Income) -> bool {
    a.income_type == b.income_type
        && a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
        && a.date_paid == b.date_paid
        && a.currency == b.currency
        && a.franked_amount == b.franked_amount
        && a.unfranked_amount == b.unfranked_amount
        && a.foreign_source_income == b.foreign_source_income
        && a.foreign_tax_paid == b.foreign_tax_paid
        && a.tfn_withholding_tax == b.tfn_withholding_tax
        && a.franking_credits == b.franking_credits
        && a.lic_capital_gain_amount == b.lic_capital_gain_amount
        && a.conduit_foreign_income == b.conduit_foreign_income
        && a.amount_per_security == b.amount_per_security
        && a.securities_held == b.securities_held
        && a.tax_deferred_amount == b.tax_deferred_amount
}

/// Income rows declaring one payment twice — see [`DuplicateIncome`].
///
/// Read on the caller's transaction, but grouped in Rust rather than in SQL:
/// the amounts are part of the key and they are TEXT decimals, which SQL would
/// compare as strings (see [`same_income_entry`]). Only rows that already
/// share a (listing, account, date) with another row are read, so what is
/// carried into memory is at most the handful of same-day pairs a portfolio
/// has, not the income table.
pub(super) async fn db_duplicate_income(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateIncome>, sqlx::Error> {
    let rows: Vec<Income> = sqlx::query_as(
        "SELECT * FROM income i \
         WHERE EXISTS (SELECT 1 FROM income o \
                       WHERE o.listing_id = i.listing_id \
                         AND o.holding_account_id = i.holding_account_id \
                         AND o.date_paid = i.date_paid AND o.id <> i.id) \
         ORDER BY i.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers = crate::entities::listing::db_tickers(&mut *conn).await?;

    // Grouped by scanning: the rows here are already narrowed to same-day
    // clusters, so the quadratic worst case is a handful of comparisons.
    // Read in id order, so each group's ids come out ascending.
    let mut groups: Vec<Vec<Income>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_income_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateIncome> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateIncome {
                listing_id: first.listing_id,
                ticker: tickers
                    .get(&first.listing_id)
                    .cloned()
                    // The FK guarantees the listing exists; a row read between
                    // the two queries is the only way here, and an empty
                    // ticker still names the ids to open.
                    .unwrap_or_default(),
                holding_account_id: first.holding_account_id,
                date_paid: first.date_paid,
                currency: first.currency.clone(),
                gross_amount: first.gross_cash_income(),
                income_count: group.len() as i64,
                income_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other two duplicate lists.
    duplicates.sort_by(|a, b| {
        b.date_paid
            .cmp(&a.date_paid)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.income_ids.cmp(&b.income_ids))
    });
    Ok(duplicates)
}

/// Whether two interest rows are the same credit entered twice — see
/// [`DuplicateInterest`]. Every stored field except the id is compared: the
/// money figures as `Decimal`s rather than as the stored TEXT (so `"250.0"`
/// and `"250.00"` match), and `source` / `holding_account_id` as the payer
/// identity that interest has instead of a listing.
pub(super) fn same_interest_entry(a: &InterestIncome, b: &InterestIncome) -> bool {
    a.date_paid == b.date_paid
        && a.currency == b.currency
        && a.amount == b.amount
        && a.tfn_withholding_tax == b.tfn_withholding_tax
        && a.foreign_source == b.foreign_source
        && a.foreign_tax_paid == b.foreign_tax_paid
        && a.source == b.source
        && a.holding_account_id == b.holding_account_id
}

/// Whether two expense rows are the same invoice entered twice — see
/// [`DuplicateExpense`]. Every stored field except the id is compared,
/// including the optional provenance pair: two rows agreeing on what is claimed
/// but disagreeing on the gross they were apportioned from came off different
/// invoices.
pub(super) fn same_expense_entry(a: &InvestmentExpense, b: &InvestmentExpense) -> bool {
    a.date_incurred == b.date_incurred
        && a.expense_type == b.expense_type
        && a.currency == b.currency
        && a.amount == b.amount
        && a.gross_amount == b.gross_amount
        && a.deductible_percentage == b.deductible_percentage
        && a.description == b.description
        && a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
}

/// Interest rows declaring one credit twice — see [`DuplicateInterest`].
///
/// Read on the caller's transaction and grouped in Rust for the same reason as
/// [`db_duplicate_income`]: the amounts are part of the key and they are TEXT
/// decimals, which SQL would compare as strings. Only rows sharing a date with
/// another row are read — the pre-narrowing is on the date alone, since the
/// rest of the key is nullable (`source`, `holding_account_id`) and would need
/// null-safe comparison in SQL to no benefit: same-day interest rows are a
/// handful even in a busy year.
pub(super) async fn db_duplicate_interest(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateInterest>, sqlx::Error> {
    let rows: Vec<InterestIncome> = sqlx::query_as(
        "SELECT * FROM interest_income i \
         WHERE EXISTS (SELECT 1 FROM interest_income o \
                       WHERE o.date_paid = i.date_paid AND o.id <> i.id) \
         ORDER BY i.id",
    )
    .fetch_all(&mut *conn)
    .await?;

    // Grouped by scanning, read in id order so each group's ids come out
    // ascending (the same shape as `db_duplicate_income`).
    let mut groups: Vec<Vec<InterestIncome>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_interest_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateInterest> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateInterest {
                date_paid: first.date_paid,
                currency: first.currency.clone(),
                amount: first.amount,
                source: first.source.clone(),
                holding_account_id: first.holding_account_id,
                interest_count: group.len() as i64,
                interest_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other duplicate lists.
    duplicates.sort_by(|a, b| {
        b.date_paid
            .cmp(&a.date_paid)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.interest_ids.cmp(&b.interest_ids))
    });
    Ok(duplicates)
}

/// Investment-expense rows claiming one expense twice — see
/// [`DuplicateExpense`]. Same shape as [`db_duplicate_interest`], plus the
/// ticker lookup for a listing-attributed row.
pub(super) async fn db_duplicate_expenses(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateExpense>, sqlx::Error> {
    let rows: Vec<InvestmentExpense> = sqlx::query_as(
        "SELECT * FROM investment_expenses e \
         WHERE EXISTS (SELECT 1 FROM investment_expenses o \
                       WHERE o.date_incurred = e.date_incurred AND o.id <> e.id) \
         ORDER BY e.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers = crate::entities::listing::db_tickers(&mut *conn).await?;

    let mut groups: Vec<Vec<InvestmentExpense>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_expense_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateExpense> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateExpense {
                date_incurred: first.date_incurred,
                expense_type: first.expense_type,
                currency: first.currency.clone(),
                amount: first.amount,
                description: first.description.clone(),
                listing_id: first.listing_id,
                // The FK guarantees a set listing exists; a row read between
                // the two queries is the only way to miss it, and the ids are
                // named either way.
                ticker: first.listing_id.and_then(|id| tickers.get(&id).cloned()),
                holding_account_id: first.holding_account_id,
                expense_count: group.len() as i64,
                expense_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    duplicates.sort_by(|a, b| {
        b.date_incurred
            .cmp(&a.date_incurred)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.expense_ids.cmp(&b.expense_ids))
    });
    Ok(duplicates)
}

/// Whether two ESS statements are one vest entered twice — see
/// [`DuplicateEssStatement`]. Every stored field except the id is compared: the
/// money figures as `Decimal`s rather than as the stored TEXT (so `"1000.0"`
/// and `"1000.00"` match), the statement-AUD overrides included — two rows
/// agreeing on the foreign labels but disagreeing on the AUD the employer
/// stated for them came off different statements. `vest_trade_id` is derived,
/// not stored, and is deliberately not part of the key: whether the surplus
/// statement has been vested yet says nothing about whether it is a duplicate.
pub(super) fn same_ess_entry(a: &EssStatement, b: &EssStatement) -> bool {
    a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
        && a.taxing_point_date == b.taxing_point_date
        && a.currency == b.currency
        && a.quantity == b.quantity
        && a.market_value_per_share == b.market_value_per_share
        && a.taxed_upfront_eligible == b.taxed_upfront_eligible
        && a.taxed_upfront_not_eligible == b.taxed_upfront_not_eligible
        && a.deferral_discount == b.deferral_discount
        && a.pre_2009_cessation_discount == b.pre_2009_cessation_discount
        && a.foreign_source_discount == b.foreign_source_discount
        && a.tfn_withholding == b.tfn_withholding
        && a.fx_rate == b.fx_rate
        && a.aud_taxed_upfront_eligible == b.aud_taxed_upfront_eligible
        && a.aud_taxed_upfront_not_eligible == b.aud_taxed_upfront_not_eligible
        && a.aud_deferral_discount == b.aud_deferral_discount
        && a.aud_pre_2009_cessation_discount == b.aud_pre_2009_cessation_discount
        && a.aud_foreign_source_discount == b.aud_foreign_source_discount
}

/// ESS statements declaring one vest twice — see [`DuplicateEssStatement`].
///
/// Read on the caller's transaction and grouped in Rust for the same reason as
/// [`db_duplicate_income`]: the figures are part of the key and they are TEXT
/// decimals, which SQL would compare as strings. Only rows already sharing a
/// (listing, account, taxing point) with another row are read, so what is
/// carried into memory is at most the handful of same-day vests a plan has.
/// Selects the entity's own `COLUMNS` rather than `*`, since `vest_trade_id` is
/// a derived back-link the row mapping requires.
pub(super) async fn db_duplicate_ess_statements(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateEssStatement>, sqlx::Error> {
    let rows: Vec<EssStatement> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM ess_statements \
         WHERE EXISTS (SELECT 1 FROM ess_statements o \
                       WHERE o.listing_id = ess_statements.listing_id \
                         AND o.holding_account_id = ess_statements.holding_account_id \
                         AND o.taxing_point_date = ess_statements.taxing_point_date \
                         AND o.id <> ess_statements.id) \
         ORDER BY ess_statements.id",
        ess_statement::COLUMNS
    )))
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers = crate::entities::listing::db_tickers(&mut *conn).await?;

    // Grouped by scanning, read in id order so each group's ids come out
    // ascending (the same shape as `db_duplicate_income`).
    let mut groups: Vec<Vec<EssStatement>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_ess_entry(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateEssStatement> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateEssStatement {
                listing_id: first.listing_id,
                ticker: tickers
                    .get(&first.listing_id)
                    .cloned()
                    // The FK guarantees the listing exists; a row read between
                    // the two queries is the only way here, and an empty
                    // ticker still names the ids to open.
                    .unwrap_or_default(),
                holding_account_id: first.holding_account_id,
                taxing_point_date: first.taxing_point_date,
                currency: first.currency.clone(),
                quantity: first.quantity,
                discount_total: ess_statement::discount_labels(first),
                statement_count: group.len() as i64,
                statement_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other duplicate lists.
    duplicates.sort_by(|a, b| {
        b.taxing_point_date
            .cmp(&a.taxing_point_date)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.statement_ids.cmp(&b.statement_ids))
    });
    Ok(duplicates)
}

/// Whether two inheritances are one parcel entered twice — see
/// [`DuplicateInheritance`]. Every stored field except the id is compared, the
/// money figures as `Decimal`s rather than as the stored TEXT (so `"3000.0"`
/// and `"3000.00"` match). The cost-base *rule* is part of it too: the same
/// units and the same figure under `DeceasedCostBase` and `MarketValueAtDeath`
/// are two different claims about the same holding, which is a contradiction
/// worth showing rather than a duplicate to collapse.
pub(super) fn same_inherited_parcel(a: &Inheritance, b: &Inheritance) -> bool {
    a.listing_id == b.listing_id
        && a.holding_account_id == b.holding_account_id
        && a.date_of_death == b.date_of_death
        && a.currency == b.currency
        && a.quantity == b.quantity
        && a.cost_base_rule == b.cost_base_rule
        && a.cost_base == b.cost_base
        && a.lpr_expenditure == b.lpr_expenditure
        && a.lpr_expenditure_date == b.lpr_expenditure_date
        && a.deceased_acquisition_date == b.deceased_acquisition_date
        && a.fx_rate == b.fx_rate
}

/// Inheritances recording one parcel twice — see [`DuplicateInheritance`].
///
/// Read on the caller's transaction and grouped in Rust for the same reason as
/// [`db_duplicate_ess_statements`]: the figures are part of the key and they
/// are TEXT decimals, which SQL would compare as strings. Only rows already
/// sharing a (listing, account, date of death) with another row are read, so a
/// portfolio of unrelated inheritances never reaches memory.
pub(super) async fn db_duplicate_inheritances(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<DuplicateInheritance>, sqlx::Error> {
    let rows: Vec<Inheritance> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM inheritances \
         WHERE EXISTS (SELECT 1 FROM inheritances o \
                       WHERE o.listing_id = inheritances.listing_id \
                         AND o.holding_account_id = inheritances.holding_account_id \
                         AND o.date_of_death = inheritances.date_of_death \
                         AND o.id <> inheritances.id) \
         ORDER BY inheritances.id",
        inheritance::COLUMNS
    )))
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers = crate::entities::listing::db_tickers(&mut *conn).await?;

    let mut groups: Vec<Vec<Inheritance>> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| same_inherited_parcel(&group[0], &row))
        {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<DuplicateInheritance> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let first = &group[0];
            DuplicateInheritance {
                listing_id: first.listing_id,
                ticker: tickers.get(&first.listing_id).cloned().unwrap_or_default(),
                holding_account_id: first.holding_account_id,
                date_of_death: first.date_of_death,
                currency: first.currency.clone(),
                quantity: first.quantity,
                // What the parcel Buy carries: the first element plus the LPR
                // expenditure, the sum `inheritance::db_upsert` writes.
                cost_base_total: first.cost_base + first.lpr_expenditure,
                inheritance_count: group.len() as i64,
                inheritance_ids: group.iter().map(|row| row.id).collect(),
            }
        })
        .collect();
    // Newest first, matching the other duplicate lists.
    duplicates.sort_by(|a, b| {
        b.date_of_death
            .cmp(&a.date_of_death)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.inheritance_ids.cmp(&b.inheritance_ids))
    });
    Ok(duplicates)
}
