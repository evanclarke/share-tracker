//! The three operations that consume **every** open parcel of a listing as a
//! matter of law rather than by choice — the scrip-for-scrip exchange
//! (`POST /corporate_actions/:id/exchange`), the demerger
//! (`POST /corporate_actions/:id/demerge`), and the worthless-shares recognise
//! (`POST /corporate_actions/:id/recognise`) — and the rule every
//! parcel-creating write owes them.
//!
//! Each of the three takes the holding as it stood at its own date and closes
//! all of it: a takeover leaves no original units behind, a demerger
//! re-bases every parcel's cost base across the head and the spun-off entity,
//! and a company written off is written off in whole. Each already refuses to
//! run while the listing has a trade dated on or after its date, so at the
//! moment it runs the picture is complete.
//!
//! What was not guarded is the other direction: a **parcel entered afterwards
//! but dated on or before** one of them. The operation cannot reach back to
//! consume it, so the units stay open forever — a holding of a security that
//! no longer exists, a head parcel that keeps 100% of a cost base the demerger
//! should have split, or an open position in a company already written off
//! (SCENARIOS V-d). [`db_back_dated_parcel`] is the write-time refusal for
//! that, and `reports::rollover_consistency`'s *unconsumed parcel* problem
//! reports any state that predates it — the same refuse-and-report pair the
//! AMIT-adjustment / rollover guards already form.
//!
//! # Which date is compared
//!
//! The parcel's own `trades.date`, never its `deemed_acquisition_date`. The
//! trade date is when the units entered the holding, so it alone answers "was
//! this parcel on foot when the operation ran?" — a rollover replacement
//! parcel (and an inherited one) legitimately carries a *deemed* acquisition
//! date decades earlier, purely to run the CGT discount clock and pick the
//! AUD translation month, and comparing that would refuse every one of them.
//!
//! A **transfer** and a **buy-back participation** are deliberately not among
//! the three: each moves a quantity the taxpayer chose, so a parcel left
//! behind is a legitimate outcome, not a hole. A transfer's transfer-in Buy is
//! still *subject* to the rule, on the date rule above — it takes the
//! transfer's own date, and units landing behind an executed operation are as
//! stranded there as any other.

use chrono::NaiveDate;
use sqlx::Row;

/// Which of the three whole-holding operations a group is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ScripForScrip,
    Demerger,
    WorthlessShares,
}

impl Kind {
    /// How the operation names itself in a sentence.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::ScripForScrip => "scrip-for-scrip exchange",
            Kind::Demerger => "demerger",
            Kind::WorthlessShares => "worthless-shares recognise",
        }
    }

    /// What a parcel the operation could not consume means for the reports —
    /// the consequence clause both the `422` and the report sentence end on.
    pub fn stranded_consequence(self) -> &'static str {
        match self {
            Kind::ScripForScrip => {
                "the units stay open as a holding of a security the exchange replaced, and no \
                 replacement units were issued for them"
            }
            Kind::Demerger => {
                "the units keep the whole of their cost base instead of the head company's share \
                 of it, and no demerged units were issued for them"
            }
            Kind::WorthlessShares => {
                "the units stay open as a holding of a company already written off, and their \
                 capital loss is never recognised"
            }
        }
    }
}

/// One executed whole-holding operation, as read from its closing Sell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executed {
    pub kind: Kind,
    /// The `corporate_actions.id` the group hangs off.
    pub action_id: i64,
    /// The operation date — the closing Sell's date.
    pub date: NaiveDate,
    /// The closing Sell, so the group is openable without a search.
    pub sell_trade_id: i64,
}

impl Executed {
    /// How the operation is named in a rejection: "*what* on *date*", the same
    /// shape `corporate_action::WriteError::BackDatedOverRollover` uses.
    fn named(&self) -> String {
        format!(
            "{} of corporate action #{} on {}",
            self.kind.noun(),
            self.action_id,
            self.date
        )
    }
}

/// The `trades` provenance column each of the three operations stamps on its
/// closing Sell, paired with the kind it identifies: **the** single place the
/// set of whole-holding operations is spelled out, so the write-time guard
/// ([`db_back_dated_parcel`]) and `reports::rollover_consistency`'s
/// unconsumed-parcel check can never disagree about what counts as one. A
/// fourth such operation is added here and both follow.
pub const CLOSING_SELL_COLUMNS: [(&str, Kind); 3] = [
    ("scrip_action_id", Kind::ScripForScrip),
    ("demerger_action_id", Kind::Demerger),
    ("worthless_action_id", Kind::WorthlessShares),
];

/// The `SELECT` fragment naming [`CLOSING_SELL_COLUMNS`] off a `trades` row
/// aliased `alias`, so [`kind_of`] can read them back by name.
pub fn closing_sell_columns(alias: &str) -> String {
    CLOSING_SELL_COLUMNS
        .iter()
        .map(|(column, _)| format!("{alias}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The `WHERE` fragment that is true exactly for a whole-holding operation's
/// closing Sell, off a `trades` row aliased `alias`.
pub fn closing_sell_predicate(alias: &str) -> String {
    CLOSING_SELL_COLUMNS
        .iter()
        .map(|(column, _)| format!("{alias}.{column} IS NOT NULL"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Which whole-holding operation a row selected with [`closing_sell_columns`]
/// belongs to, with the action id it hangs off — `None` when it is none of
/// them (an ordinary Sell, or a transfer's transfer-out).
pub fn kind_of(row: &sqlx::sqlite::SqliteRow) -> Result<Option<(Kind, i64)>, sqlx::Error> {
    for (column, kind) in CLOSING_SELL_COLUMNS {
        if let Some(action_id) = row.try_get::<Option<i64>, _>(column)? {
            return Ok(Some((kind, action_id)));
        }
    }
    Ok(None)
}

/// Classify a closing-Sell row read with [`closing_sell_columns`]. A row the
/// query's own predicate admitted always classifies; a `None` would mean the
/// two fragments had drifted apart, which is a bug rather than a data state.
fn executed_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Executed, sqlx::Error> {
    let (kind, action_id) = kind_of(row)?.ok_or_else(|| {
        sqlx::Error::Decode("a closing Sell carries none of the whole-holding columns".into())
    })?;
    Ok(Executed {
        kind,
        action_id,
        date: row.try_get("date")?,
        sell_trade_id: row.try_get("sell_trade_id")?,
    })
}

/// Every whole-holding operation of `listing_id` that has already run and is
/// dated **on or after** `date` — the operations a parcel dated `date` would
/// land behind. Newest first, so the rejection names the most recent one
/// first. Empty means the parcel is clear.
///
/// Read on the caller's own connection, so the check and the write it guards
/// see one state.
pub async fn db_executed_from(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    date: NaiveDate,
) -> Result<Vec<Executed>, sqlx::Error> {
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT s.id AS sell_trade_id, s.date, {} \
         FROM trades s \
         WHERE s.listing_id = ?1 AND s.trade_type = 'Sell' AND s.date >= ?2 AND ({}) \
         ORDER BY s.date DESC, s.id DESC",
        closing_sell_columns("s"),
        closing_sell_predicate("s"),
    )))
    .bind(listing_id)
    .bind(date)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter().map(executed_from_row).collect()
}

/// A parcel-creating write refused because it is dated on or before one or
/// more executed whole-holding operations of its listing.
///
/// Every parcel-creating write path carries this as one variant of its own
/// error enum (`trade`, `inheritance`, `ess_vest`, `rights_exercise`,
/// `drp_reinvestment`, and the three `domain::rollover` operations' own
/// replacement parcels), and each maps it to the one `422` body
/// [`Self::message`] builds — the wording lives here so eight refusals of the
/// same fact cannot drift apart.
#[derive(thiserror::Error, Debug)]
#[error("the parcel is dated on or before {} whole-holding operation(s) of its listing that have already run", .operations.len())]
pub struct BackDatedParcel {
    /// The operations the parcel would land behind, newest first.
    pub operations: Vec<Executed>,
}

impl BackDatedParcel {
    /// The user-facing `422` body: names every operation and its date, the
    /// consequence of leaving the parcel where it is, and the recovery its
    /// sibling refusal already gives — delete the operation, enter the parcel,
    /// run it again.
    pub fn message(&self) -> String {
        let named = self
            .operations
            .iter()
            .map(Executed::named)
            .collect::<Vec<_>>()
            .join(", ");
        // Every operation strands the parcel; the consequence clause of the
        // most recent one is the one shown, since that is the operation the
        // recovery starts from.
        let consequence = self
            .operations
            .first()
            .map(|op| op.kind.stranded_consequence())
            .unwrap_or_default();
        format!(
            "this parcel is dated on or before an operation on its listing that has already \
             consumed the whole holding ({named}) — that operation took every parcel open at its \
             date, so one entered behind it now can never be consumed: {consequence}. Delete that \
             operation, enter this parcel, then run it again, so it carries these units too"
        )
    }
}

/// The write-time guard itself: `Some(_)` when a parcel written at
/// `(listing_id, date)` would land behind an executed whole-holding operation
/// it was not already behind, `None` when it is clear.
///
/// Called by every parcel-creating write path inside its own write
/// transaction. A new one owes this call — the report is the safety net for
/// state that predates the guard, not a substitute for it.
///
/// `stored` is the parcel's **existing** `(listing_id, date)` when the write is
/// an edit of a row that is already there, and `None` for a fresh parcel. It is
/// what keeps the guard from refusing the very corrections the
/// rollover-consistency report asks for: a source parcel the operation *did*
/// consume sits behind it by definition, and editing its price, brokerage or
/// quantity is exactly the documented state that report surfaces — refusing it
/// would leave that state unfixable while fixing nothing. So an edit is refused
/// only when it *newly* puts the units behind an operation: a fresh parcel, a
/// parcel moved onto another listing, or one whose date is moved back past an
/// operation it was previously clear of.
pub async fn db_back_dated_parcel(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    date: NaiveDate,
    stored: Option<(i64, NaiveDate)>,
) -> Result<Option<BackDatedParcel>, sqlx::Error> {
    let operations = db_executed_from(conn, listing_id, date).await?;
    if operations.is_empty() {
        return Ok(None);
    }
    if let Some((stored_listing_id, stored_date)) = stored
        && !db_executed_from(conn, stored_listing_id, stored_date)
            .await?
            .is_empty()
    {
        return Ok(None);
    }
    Ok(Some(BackDatedParcel { operations }))
}
