//! The `duplicate_*` family: one fact entered twice, on each side of the data
//! model that a re-submitted form or re-imported statement can double.

use crate::domain::tax_year::tax_year_for;
use crate::entities::ess_statement::{self, EssStatement};
use crate::entities::income::Income;
use crate::entities::inheritance::{self, Inheritance};
use crate::entities::interest_income::InterestIncome;
use crate::entities::investment_expense::{ExpenseType, InvestmentExpense};
use crate::entities::listing;
use crate::infra::currency::CurrencyCode;
use crate::infra::id::HoldingAccountId;
use crate::infra::id::ListingId;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqliteRow;
use std::cmp::Ordering;
use std::collections::HashMap;

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
    pub listing_id: ListingId,
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
    pub listing_id: ListingId,
    pub ticker: String,
    pub action_type: String,
    pub date: NaiveDate,
    /// How many actions share this (listing, type, date) — always ≥ 2.
    pub action_count: i64,
    /// The ids sharing it, ascending, so the surplus row can be found and
    /// deleted without a search.
    pub action_ids: Vec<i64>,
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
    pub listing_id: ListingId,
    pub ticker: String,
    /// The financial year the statements attribute, identified by the
    /// calendar year of their shared 30 June end (`domain::tax_year`).
    pub tax_year: i32,
    pub holding_account_id: HoldingAccountId,
    /// How many statements share this (listing, year, account) — always ≥ 2.
    pub statement_count: i64,
    /// The ids sharing it, ascending, so the superseded row can be found and
    /// deleted without a search.
    pub statement_ids: Vec<i64>,
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
    pub listing_id: ListingId,
    pub ticker: String,
    pub holding_account_id: HoldingAccountId,
    pub date_paid: NaiveDate,
    /// ISO 4217 currency the shared amounts are stated in (part of the key —
    /// two rows stating the same figures in different currencies are not the
    /// same payment).
    pub currency: CurrencyCode,
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
    pub currency: CurrencyCode,
    /// The gross interest each duplicated row declares, in `currency`.
    pub amount: Decimal,
    /// The free-text source the rows share (part of the key); `None` when none
    /// of them recorded one.
    pub source: Option<String>,
    /// The holding account the rows share (part of the key); `None` for
    /// interest from outside the portfolio's accounts.
    pub holding_account_id: Option<HoldingAccountId>,
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
    pub currency: CurrencyCode,
    /// The deductible amount each duplicated row claims, in `currency`.
    pub amount: Decimal,
    /// The free-text description the rows share (part of the key).
    pub description: Option<String>,
    /// The listing the rows are attributed to (part of the key); `None` for a
    /// portfolio-wide expense.
    pub listing_id: Option<ListingId>,
    /// That listing's ticker, so the warning names the holding rather than only
    /// its id; `None` for a portfolio-wide expense.
    pub ticker: Option<String>,
    /// The holding account the rows are attributed to (part of the key);
    /// `None` for a portfolio-wide expense.
    pub holding_account_id: Option<HoldingAccountId>,
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
    pub listing_id: ListingId,
    pub ticker: String,
    pub holding_account_id: HoldingAccountId,
    pub taxing_point_date: NaiveDate,
    /// ISO 4217 currency the shared figures are stated in (part of the key).
    pub currency: CurrencyCode,
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
    pub listing_id: ListingId,
    pub ticker: String,
    pub holding_account_id: HoldingAccountId,
    pub date_of_death: NaiveDate,
    /// ISO 4217 currency the shared figures are stated in (part of the key).
    pub currency: CurrencyCode,
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

/// One `duplicate_*` check: the rows a double entry repeats, what makes two of
/// them the same entry, and how a group of them is reported. Every check in the
/// family is this one shape run through [`find_duplicates`], so the grouping,
/// the ≥ 2 filter, the ascending ids and the newest-first order are written
/// once rather than once per table.
///
/// The grouping runs in Rust rather than SQL so that a key holding money
/// figures compares them as `Decimal`s — `"10.0"` and `"10.00"` are the same
/// dollars written by two different clients — where SQL would compare the
/// stored TEXT. [`candidates_sql`](Self::candidates_sql) narrows what is read
/// to the rows sharing a cheap part of the key with another row, so what
/// reaches memory is the handful of near-collisions a portfolio has, not the
/// table.
trait DuplicateCheck: for<'r> sqlx::FromRow<'r, SqliteRow> + Send + Unpin + Sized {
    /// The entry reported per group.
    type Report;

    /// The candidate rows, **in id order** (so each group's ids come out
    /// ascending): every row sharing the cheap part of the key with another.
    fn candidates_sql() -> String;

    /// Whether two candidates are one entry entered twice — the whole key.
    fn same_entry(&self, other: &Self) -> bool;

    /// The ids a group of rows is reported by.
    fn id(&self) -> i64;

    /// The entry for one group of ≥ 2 rows, ids ascending. `tickers` names a
    /// listing by id; the FK guarantees a set listing exists, so a miss is
    /// only a row read between the two queries, and the ids are named anyway.
    fn report(group: &[Self], ids: Vec<i64>, tickers: &HashMap<ListingId, String>) -> Self::Report;

    /// The list's order: newest first, then the tie-breaks that make it total.
    fn newest_first(a: &Self::Report, b: &Self::Report) -> Ordering;
}

/// Runs one [`DuplicateCheck`] on the caller's transaction.
async fn find_duplicates<R: DuplicateCheck>(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<R::Report>, sqlx::Error> {
    let rows: Vec<R> = sqlx::query_as(sqlx::AssertSqlSafe(R::candidates_sql()))
        .fetch_all(&mut *conn)
        .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let tickers = listing::db_tickers(&mut *conn).await?;

    // Grouped by scanning: the rows are already narrowed to near-collisions,
    // so the quadratic worst case is a handful of comparisons.
    let mut groups: Vec<Vec<R>> = Vec::new();
    for row in rows {
        match groups.iter_mut().find(|group| group[0].same_entry(&row)) {
            Some(group) => group.push(row),
            None => groups.push(vec![row]),
        }
    }
    let mut duplicates: Vec<R::Report> = groups
        .into_iter()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let ids = group.iter().map(R::id).collect();
            R::report(&group, ids, &tickers)
        })
        .collect();
    duplicates.sort_by(R::newest_first);
    Ok(duplicates)
}

/// Every list of the `duplicate_*` family, one per [`DuplicateCheck`].
pub(super) struct Duplicates {
    pub trades: Vec<DuplicateTrade>,
    pub actions: Vec<DuplicateAction>,
    pub amma_statements: Vec<DuplicateAmmaStatement>,
    pub income: Vec<DuplicateIncome>,
    pub interest: Vec<DuplicateInterest>,
    pub expenses: Vec<DuplicateExpense>,
    pub ess_statements: Vec<DuplicateEssStatement>,
    pub inheritances: Vec<DuplicateInheritance>,
}

/// Runs every check of the family on the caller's transaction.
pub(super) async fn db_duplicates(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Duplicates, sqlx::Error> {
    Ok(Duplicates {
        trades: find_duplicates::<ContractNoteTrade>(conn).await?,
        actions: find_duplicates::<ActionEntry>(conn).await?,
        amma_statements: find_duplicates::<AmmaEntry>(conn).await?,
        income: find_duplicates::<Income>(conn).await?,
        interest: find_duplicates::<InterestIncome>(conn).await?,
        expenses: find_duplicates::<InvestmentExpense>(conn).await?,
        ess_statements: find_duplicates::<EssStatement>(conn).await?,
        inheritances: find_duplicates::<Inheritance>(conn).await?,
    })
}

fn ticker(tickers: &HashMap<ListingId, String>, listing_id: ListingId) -> String {
    tickers.get(&listing_id).cloned().unwrap_or_default()
}

/// A trade carrying a contract note reference — the candidate row behind
/// [`DuplicateTrade`].
///
/// The key is a TEXT reference rather than a decimal, so SQLite's own string
/// comparison pre-groups it exactly. `TRIM` normalises the padding a pasted
/// reference picks up, and the `<> ''` guard drops a reference that is blank
/// or only whitespace — no reference at all — as it drops a NULL one, which is
/// what every derived trade records.
#[derive(sqlx::FromRow)]
struct ContractNoteTrade {
    id: i64,
    listing_id: ListingId,
    /// Trimmed.
    contract_note_ref: String,
    date: NaiveDate,
}

impl DuplicateCheck for ContractNoteTrade {
    type Report = DuplicateTrade;

    fn candidates_sql() -> String {
        "SELECT id, listing_id, TRIM(contract_note_ref) AS contract_note_ref, date \
         FROM trades \
         WHERE (listing_id, TRIM(contract_note_ref)) IN \
               (SELECT listing_id, TRIM(contract_note_ref) FROM trades \
                WHERE TRIM(contract_note_ref) <> '' \
                GROUP BY listing_id, TRIM(contract_note_ref) HAVING COUNT(*) > 1) \
         ORDER BY id"
            .to_owned()
    }

    fn same_entry(&self, other: &Self) -> bool {
        self.listing_id == other.listing_id && self.contract_note_ref == other.contract_note_ref
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn report(
        group: &[Self],
        ids: Vec<i64>,
        tickers: &HashMap<ListingId, String>,
    ) -> DuplicateTrade {
        let first = &group[0];
        DuplicateTrade {
            listing_id: first.listing_id,
            ticker: ticker(tickers, first.listing_id),
            contract_note_ref: first.contract_note_ref.clone(),
            date: group
                .iter()
                .map(|t| t.date)
                .max()
                .expect("a group is never empty"),
            trade_count: group.len() as i64,
            trade_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateTrade, b: &DuplicateTrade) -> Ordering {
        b.date
            .cmp(&a.date)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.contract_note_ref.cmp(&b.contract_note_ref))
    }
}

/// A corporate action — the candidate row behind [`DuplicateAction`], keyed
/// on (listing, type, date).
#[derive(sqlx::FromRow)]
struct ActionEntry {
    id: i64,
    listing_id: ListingId,
    action_type: String,
    date: NaiveDate,
}

impl DuplicateCheck for ActionEntry {
    type Report = DuplicateAction;

    fn candidates_sql() -> String {
        "SELECT id, listing_id, action_type, date FROM corporate_actions \
         WHERE (listing_id, action_type, date) IN \
               (SELECT listing_id, action_type, date FROM corporate_actions \
                GROUP BY listing_id, action_type, date HAVING COUNT(*) > 1) \
         ORDER BY id"
            .to_owned()
    }

    fn same_entry(&self, other: &Self) -> bool {
        self.listing_id == other.listing_id
            && self.action_type == other.action_type
            && self.date == other.date
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn report(
        group: &[Self],
        ids: Vec<i64>,
        tickers: &HashMap<ListingId, String>,
    ) -> DuplicateAction {
        let first = &group[0];
        DuplicateAction {
            listing_id: first.listing_id,
            ticker: ticker(tickers, first.listing_id),
            action_type: first.action_type.clone(),
            date: first.date,
            action_count: group.len() as i64,
            action_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateAction, b: &DuplicateAction) -> Ordering {
        b.date
            .cmp(&a.date)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.action_type.cmp(&b.action_type))
    }
}

/// An AMMA statement — the candidate row behind [`DuplicateAmmaStatement`],
/// keyed on (listing, financial year, holding account). The year is keyed on
/// `tax_year_end_date` itself rather than a derived year, which is the same
/// thing — the column is a 30 June date, enforced at write time
/// (`entities::amma::UpsertError::NotFinancialYearEnd`).
#[derive(sqlx::FromRow)]
struct AmmaEntry {
    id: i64,
    listing_id: ListingId,
    tax_year_end_date: NaiveDate,
    holding_account_id: HoldingAccountId,
}

impl DuplicateCheck for AmmaEntry {
    type Report = DuplicateAmmaStatement;

    fn candidates_sql() -> String {
        "SELECT id, listing_id, tax_year_end_date, holding_account_id FROM amma_statements \
         WHERE (listing_id, tax_year_end_date, holding_account_id) IN \
               (SELECT listing_id, tax_year_end_date, holding_account_id FROM amma_statements \
                GROUP BY listing_id, tax_year_end_date, holding_account_id \
                HAVING COUNT(*) > 1) \
         ORDER BY id"
            .to_owned()
    }

    fn same_entry(&self, other: &Self) -> bool {
        self.listing_id == other.listing_id
            && self.tax_year_end_date == other.tax_year_end_date
            && self.holding_account_id == other.holding_account_id
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn report(
        group: &[Self],
        ids: Vec<i64>,
        tickers: &HashMap<ListingId, String>,
    ) -> DuplicateAmmaStatement {
        let first = &group[0];
        DuplicateAmmaStatement {
            listing_id: first.listing_id,
            ticker: ticker(tickers, first.listing_id),
            tax_year: tax_year_for(first.tax_year_end_date),
            holding_account_id: first.holding_account_id,
            statement_count: group.len() as i64,
            statement_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateAmmaStatement, b: &DuplicateAmmaStatement) -> Ordering {
        b.tax_year
            .cmp(&a.tax_year)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
    }
}

/// Income rows declaring one payment twice — see [`DuplicateIncome`]. Only
/// rows already sharing a (listing, account, date) with another are read.
impl DuplicateCheck for Income {
    type Report = DuplicateIncome;

    fn candidates_sql() -> String {
        "SELECT * FROM income i \
         WHERE EXISTS (SELECT 1 FROM income o \
                       WHERE o.listing_id = i.listing_id \
                         AND o.holding_account_id = i.holding_account_id \
                         AND o.date_paid = i.date_paid AND o.id <> i.id) \
         ORDER BY i.id"
            .to_owned()
    }

    /// The same declared payment: same listing, holding account and payment
    /// date, and identical money figures stated in one currency. Every money
    /// column of the row is compared, including the informational ones: two
    /// rows agreeing on the assessable figures but disagreeing on, say,
    /// `tax_deferred_amount` were entered from different statements. The
    /// row's **kind** is part of it too — a dividend and a dividend equivalent
    /// of the same amount on one day are two different payments, and the whole
    /// point of the kind is that they are not the same thing (SCENARIOS J-10).
    fn same_entry(&self, b: &Self) -> bool {
        let a = self;
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

    fn id(&self) -> i64 {
        self.id
    }

    fn report(
        group: &[Self],
        ids: Vec<i64>,
        tickers: &HashMap<ListingId, String>,
    ) -> DuplicateIncome {
        let first = &group[0];
        DuplicateIncome {
            listing_id: first.listing_id,
            ticker: ticker(tickers, first.listing_id),
            holding_account_id: first.holding_account_id,
            date_paid: first.date_paid,
            currency: first.currency.clone(),
            gross_amount: first.gross_cash_income(),
            income_count: group.len() as i64,
            income_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateIncome, b: &DuplicateIncome) -> Ordering {
        b.date_paid
            .cmp(&a.date_paid)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.income_ids.cmp(&b.income_ids))
    }
}

/// Interest rows declaring one credit twice — see [`DuplicateInterest`]. The
/// pre-narrowing is on the date alone, since the rest of the key is nullable
/// (`source`, `holding_account_id`) and would need null-safe comparison in
/// SQL to no benefit: same-day interest rows are a handful even in a busy
/// year.
impl DuplicateCheck for InterestIncome {
    type Report = DuplicateInterest;

    fn candidates_sql() -> String {
        "SELECT * FROM interest_income i \
         WHERE EXISTS (SELECT 1 FROM interest_income o \
                       WHERE o.date_paid = i.date_paid AND o.id <> i.id) \
         ORDER BY i.id"
            .to_owned()
    }

    /// The same credit entered twice: every stored field except the id, with
    /// `source` / `holding_account_id` as the payer identity that interest has
    /// instead of a listing.
    fn same_entry(&self, b: &Self) -> bool {
        let a = self;
        a.date_paid == b.date_paid
            && a.currency == b.currency
            && a.amount == b.amount
            && a.tfn_withholding_tax == b.tfn_withholding_tax
            && a.foreign_source == b.foreign_source
            && a.foreign_tax_paid == b.foreign_tax_paid
            && a.source == b.source
            && a.holding_account_id == b.holding_account_id
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn report(group: &[Self], ids: Vec<i64>, _: &HashMap<ListingId, String>) -> DuplicateInterest {
        let first = &group[0];
        DuplicateInterest {
            date_paid: first.date_paid,
            currency: first.currency.clone(),
            amount: first.amount,
            source: first.source.clone(),
            holding_account_id: first.holding_account_id,
            interest_count: group.len() as i64,
            interest_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateInterest, b: &DuplicateInterest) -> Ordering {
        b.date_paid
            .cmp(&a.date_paid)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.interest_ids.cmp(&b.interest_ids))
    }
}

/// Investment-expense rows claiming one expense twice — see
/// [`DuplicateExpense`]. Pre-narrowed on the date alone, as interest is.
impl DuplicateCheck for InvestmentExpense {
    type Report = DuplicateExpense;

    fn candidates_sql() -> String {
        "SELECT * FROM investment_expenses e \
         WHERE EXISTS (SELECT 1 FROM investment_expenses o \
                       WHERE o.date_incurred = e.date_incurred AND o.id <> e.id) \
         ORDER BY e.id"
            .to_owned()
    }

    /// The same invoice entered twice: every stored field except the id,
    /// including the optional provenance pair — two rows agreeing on what is
    /// claimed but disagreeing on the gross they were apportioned from came
    /// off different invoices.
    fn same_entry(&self, b: &Self) -> bool {
        let a = self;
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

    fn id(&self) -> i64 {
        self.id
    }

    fn report(
        group: &[Self],
        ids: Vec<i64>,
        tickers: &HashMap<ListingId, String>,
    ) -> DuplicateExpense {
        let first = &group[0];
        DuplicateExpense {
            date_incurred: first.date_incurred,
            expense_type: first.expense_type,
            currency: first.currency.clone(),
            amount: first.amount,
            description: first.description.clone(),
            listing_id: first.listing_id,
            ticker: first.listing_id.and_then(|id| tickers.get(&id).cloned()),
            holding_account_id: first.holding_account_id,
            expense_count: group.len() as i64,
            expense_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateExpense, b: &DuplicateExpense) -> Ordering {
        b.date_incurred
            .cmp(&a.date_incurred)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.expense_ids.cmp(&b.expense_ids))
    }
}

/// ESS statements declaring one vest twice — see [`DuplicateEssStatement`].
/// Only rows already sharing a (listing, account, taxing point) with another
/// are read. Selects the entity's own `COLUMNS` rather than `*`, since
/// `vest_trade_id` is a derived back-link the row mapping requires.
impl DuplicateCheck for EssStatement {
    type Report = DuplicateEssStatement;

    fn candidates_sql() -> String {
        format!(
            "SELECT {} FROM ess_statements \
             WHERE EXISTS (SELECT 1 FROM ess_statements o \
                           WHERE o.listing_id = ess_statements.listing_id \
                             AND o.holding_account_id = ess_statements.holding_account_id \
                             AND o.taxing_point_date = ess_statements.taxing_point_date \
                             AND o.id <> ess_statements.id) \
             ORDER BY ess_statements.id",
            ess_statement::COLUMNS
        )
    }

    /// One vest entered twice: every stored field except the id, the
    /// statement-AUD overrides included — two rows agreeing on the foreign
    /// labels but disagreeing on the AUD the employer stated for them came off
    /// different statements. `vest_trade_id` is derived, not stored, and is
    /// deliberately not part of the key: whether the surplus statement has
    /// been vested yet says nothing about whether it is a duplicate.
    fn same_entry(&self, b: &Self) -> bool {
        let a = self;
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

    fn id(&self) -> i64 {
        self.id
    }

    fn report(
        group: &[Self],
        ids: Vec<i64>,
        tickers: &HashMap<ListingId, String>,
    ) -> DuplicateEssStatement {
        let first = &group[0];
        DuplicateEssStatement {
            listing_id: first.listing_id,
            ticker: ticker(tickers, first.listing_id),
            holding_account_id: first.holding_account_id,
            taxing_point_date: first.taxing_point_date,
            currency: first.currency.clone(),
            quantity: first.quantity,
            discount_total: ess_statement::discount_labels(first),
            statement_count: group.len() as i64,
            statement_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateEssStatement, b: &DuplicateEssStatement) -> Ordering {
        b.taxing_point_date
            .cmp(&a.taxing_point_date)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.statement_ids.cmp(&b.statement_ids))
    }
}

/// Inheritances recording one parcel twice — see [`DuplicateInheritance`].
/// Only rows already sharing a (listing, account, date of death) with another
/// are read, so a portfolio of unrelated inheritances never reaches memory.
impl DuplicateCheck for Inheritance {
    type Report = DuplicateInheritance;

    fn candidates_sql() -> String {
        format!(
            "SELECT {} FROM inheritances \
             WHERE EXISTS (SELECT 1 FROM inheritances o \
                           WHERE o.listing_id = inheritances.listing_id \
                             AND o.holding_account_id = inheritances.holding_account_id \
                             AND o.date_of_death = inheritances.date_of_death \
                             AND o.id <> inheritances.id) \
             ORDER BY inheritances.id",
            inheritance::COLUMNS
        )
    }

    /// One parcel entered twice: every stored field except the id. The
    /// cost-base *rule* is part of it too: the same units and the same figure
    /// under `DeceasedCostBase` and `MarketValueAtDeath` are two different
    /// claims about the same holding, which is a contradiction worth showing
    /// rather than a duplicate to collapse.
    fn same_entry(&self, b: &Self) -> bool {
        let a = self;
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

    fn id(&self) -> i64 {
        self.id
    }

    fn report(
        group: &[Self],
        ids: Vec<i64>,
        tickers: &HashMap<ListingId, String>,
    ) -> DuplicateInheritance {
        let first = &group[0];
        DuplicateInheritance {
            listing_id: first.listing_id,
            ticker: ticker(tickers, first.listing_id),
            holding_account_id: first.holding_account_id,
            date_of_death: first.date_of_death,
            currency: first.currency.clone(),
            quantity: first.quantity,
            // What the parcel Buy carries: the first element plus the LPR
            // expenditure, the sum `inheritance::db_upsert` writes.
            cost_base_total: first.cost_base + first.lpr_expenditure,
            inheritance_count: group.len() as i64,
            inheritance_ids: ids,
        }
    }

    fn newest_first(a: &DuplicateInheritance, b: &DuplicateInheritance) -> Ordering {
        b.date_of_death
            .cmp(&a.date_of_death)
            .then_with(|| a.ticker.cmp(&b.ticker))
            .then_with(|| a.holding_account_id.cmp(&b.holding_account_id))
            .then_with(|| a.inheritance_ids.cmp(&b.inheritance_ids))
    }
}
