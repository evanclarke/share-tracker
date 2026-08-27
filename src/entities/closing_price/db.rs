//! Persistence: the `db_*` reads and writes over `closing_prices`, the
//! price-basis re-derivation that keeps stored figures in the unit basis in
//! force on their date, and the `unpriced_before` clear.

use super::fetcher::clean_price;
use super::model::ClosingPrice;
use crate::infra::db::write_tx;
use crate::infra::decimal::{Money, OptMoney};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::{QueryBuilder, SqlitePool};
use std::collections::HashSet;

/// Executor-generic so it can run on a caller's own connection (snapshot
/// generation reads every price **inside** the transaction that stores the
/// result) as well as on the pool.
pub async fn db_get_one<'e, E>(
    executor: E,
    listing_id: i64,
    price_date: NaiveDate,
) -> Result<Option<ClosingPrice>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as(
        "SELECT id, listing_id, price_date, price, price_as_observed, source, fetched_at, \
                fetched_symbol, status, error, origin, sourced_from, reason \
         FROM closing_prices WHERE listing_id = ? AND price_date = ?",
    )
    .bind(listing_id)
    .bind(price_date)
    .fetch_optional(executor)
    .await
}

/// The listing's latest **ok** stored price at or before `on_or_before` and
/// not earlier than `not_before`, as `(price_date, price)`.
///
/// The carry-forward source for a listing the provider has stopped quoting
/// (`listings.unpriced_from`, SCENARIOS Q-02): `reports::valuation` reads it
/// when the valuation day itself has no ok price. It returns the *date* too,
/// so the caller can tell a genuinely contemporaneous price from a carried
/// one. A manual price entered during the unpriced run wins over an older
/// fetched one simply by being later.
///
/// `not_before` is the listing's `unpriced_before` (migration 0037), when it
/// has one: a row dated before the provider's series begins is not a price
/// for this security by the listing's own record, so it cannot be the figure
/// carried forward either. `None` means no floor.
/// Executor-generic for the same reason [`db_get_one`] is.
pub async fn db_latest_ok_price_on_or_before<'e, E>(
    executor: E,
    listing_id: i64,
    on_or_before: NaiveDate,
    not_before: Option<NaiveDate>,
) -> Result<Option<(NaiveDate, Decimal)>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row: Option<(NaiveDate, Money)> = sqlx::query_as(
        "SELECT price_date, price FROM closing_prices \
         WHERE listing_id = ?1 AND status = 'ok' AND price_date <= ?2 \
           AND (?3 IS NULL OR price_date >= ?3) \
         ORDER BY price_date DESC LIMIT 1",
    )
    .bind(listing_id)
    .bind(on_or_before)
    .bind(not_before)
    .fetch_optional(executor)
    .await?;
    Ok(row.map(|(date, Money(price))| (date, price)))
}

/// Stored prices, newest first, optionally filtered by listing and date range.
pub async fn db_list(
    pool: &SqlitePool,
    listing_id: Option<i64>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Result<Vec<ClosingPrice>, sqlx::Error> {
    let mut qb = QueryBuilder::new(
        "SELECT id, listing_id, price_date, price, price_as_observed, source, fetched_at, \
                fetched_symbol, status, error, origin, sourced_from, reason \
         FROM closing_prices WHERE 1=1",
    );
    if let Some(id) = listing_id {
        qb.push(" AND listing_id = ").push_bind(id);
    }
    if let Some(from) = from {
        qb.push(" AND price_date >= ").push_bind(from);
    }
    if let Some(to) = to {
        qb.push(" AND price_date <= ").push_bind(to);
    }
    qb.push(" ORDER BY price_date DESC, listing_id");
    qb.build_query_as().fetch_all(pool).await
}

/// The dates in `from..=to` already stored with status ok for the listing
/// (so collection/backfill never re-fetches a good price).
pub(super) async fn db_ok_dates(
    pool: &SqlitePool,
    listing_id: i64,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<HashSet<NaiveDate>, sqlx::Error> {
    let dates: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT price_date FROM closing_prices \
         WHERE listing_id = ? AND status = 'ok' AND price_date BETWEEN ? AND ?",
    )
    .bind(listing_id)
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(dates.into_iter().collect())
}

/// Upsert one row: a re-fetch replaces whatever is stored for the
/// (listing, date) — in particular, a success replaces an errored row — and a
/// manual entry replaces whatever was stored before it. Every column moves
/// together, so a row can never keep the origin of one write and the
/// provenance of another.
///
/// `row.id` is ignored (see [`UNASSIGNED_ID`]): the natural key is the conflict
/// target, so the database assigns a new surrogate id on an insert and keeps
/// the stored one when this updates — which is what lets the row's audit trail
/// span every version of it. A replacing write is an UPDATE, so the superseded
/// row (a manual price's own `sourced_from`/`reason` included) is recorded in
/// `row_history` by the 0021 trigger rather than lost.
pub(crate) async fn db_store(pool: &SqlitePool, row: &ClosingPrice) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO closing_prices \
             (listing_id, price_date, price, price_as_observed, source, fetched_at, \
              fetched_symbol, status, error, origin, sourced_from, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(listing_id, price_date) DO UPDATE SET \
             price = excluded.price, \
             price_as_observed = excluded.price_as_observed, \
             source = excluded.source, \
             fetched_at = excluded.fetched_at, \
             fetched_symbol = excluded.fetched_symbol, \
             status = excluded.status, \
             error = excluded.error, \
             origin = excluded.origin, \
             sourced_from = excluded.sourced_from, \
             reason = excluded.reason",
    )
    .bind(row.listing_id)
    .bind(row.price_date)
    .bind(OptMoney(row.price))
    .bind(OptMoney(row.price_as_observed))
    .bind(&row.source)
    .bind(&row.fetched_at)
    .bind(&row.fetched_symbol)
    .bind(row.status)
    .bind(&row.error)
    .bind(row.origin)
    .bind(&row.sourced_from)
    .bind(&row.reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// A provider figure restated into its own trading day's unit basis, rounded
/// back to the provider's precision.
///
/// The arithmetic is `corporate_action::contemporaneous_price` — the shared
/// re-basing math, never re-derived here — and [`clean_price`] then holds the
/// result to 7 significant digits: the observation only ever carried that many
/// (Yahoo serves float32), so a ratio that does not divide out exactly must
/// not be written down as if it recovered more.
pub(super) fn contemporaneous(
    as_observed: Decimal,
    events: &[crate::entities::corporate_action::PriceBasisEvent],
    price_date: NaiveDate,
    observed: NaiveDate,
) -> Decimal {
    clean_price(crate::entities::corporate_action::contemporaneous_price(
        as_observed,
        events,
        price_date,
        observed,
    ))
}

/// One stored provider figure, as the demerger factor's denominator: the
/// figure exactly as observed, and the UTC date it was observed on.
struct ObservedFigure {
    as_observed: Decimal,
    observed: NaiveDate,
}

/// The stored provider figure for one (listing, day), or `None` when there is
/// none to read. Manual rows are excluded: a hand-entered price is
/// contemporaneous by declaration, so it is not a *restated* figure and
/// dividing by it would only ever answer 1.
async fn db_observed_figure(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
    price_date: NaiveDate,
) -> Result<Option<ObservedFigure>, sqlx::Error> {
    let row: Option<(Money, String)> = sqlx::query_as(
        "SELECT price_as_observed, fetched_at FROM closing_prices \
         WHERE listing_id = ? AND price_date = ? AND status = 'ok' AND origin = 'fetched'",
    )
    .bind(listing_id)
    .bind(price_date)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|(price, fetched_at)| {
        Ok(ObservedFigure {
            as_observed: price.0,
            observed: observation_date(&fetched_at)?,
        })
    })
    .transpose()
}

/// The UTC date a row's `fetched_at` records — the date that fixes which unit
/// and price basis the figure arrived in (module docs).
fn observation_date(fetched_at: &str) -> Result<NaiveDate, sqlx::Error> {
    Ok(DateTime::parse_from_rfc3339(fetched_at)
        .map_err(|e| {
            sqlx::Error::Decode(
                format!("closing_prices.fetched_at {fetched_at:?} is not RFC 3339: {e}").into(),
            )
        })?
        .with_timezone(&Utc)
        .date_naive())
}

/// Every event that restated the provider's **price** series for one listing:
/// its `ShareSplit`/`BonusIssue` ratios, plus a derived factor for each
/// `Demerger` that carries a stated pre-demerger close (module docs, and
/// `corporate_action::adjustments` for why this is a different set from the
/// quantity one).
///
/// A split states its own factor. A demerger does not — the provider's
/// spin-off factor is set by the two entities' market values, which no term of
/// the action gives — so it is **derived**, here, from the two facts that
/// bracket it: what the operator states the security actually closed at on the
/// last pre-demerger trading day, over what the provider says about that same
/// day. The provider's side is read now rather than stored, so
///
/// - the close can be stated before any pre-demerger history exists (the
///   factor simply resolves to nothing until a figure is there to divide), and
/// - re-fetching that day re-derives the factor instead of leaving a stored
///   quotient stale.
///
/// The denominator is not the raw stored figure but what the walk **without
/// this demerger** would already make of it: a split dated between the close
/// date and the observation has restated that figure too, and the factor must
/// not absorb it a second time. That is also why the statements are resolved
/// latest-first — a *later* demerger restated the same figure and has to be
/// divided out first, while an earlier one is outside the half-open window
/// `(close_date, observed]` and cannot matter.
///
/// A demerger whose reference day was observed **before** it contributes
/// nothing: that figure is already contemporaneous, so there is no factor to
/// recover from it (and the rows it would apply to are exactly the ones the
/// half-open window already leaves alone).
pub async fn db_price_basis_events(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<Vec<crate::entities::corporate_action::PriceBasisEvent>, sqlx::Error> {
    use crate::entities::corporate_action::{self, PriceBasisEvent};

    let splits = corporate_action::db_splits_for_listing(&mut *conn, listing_id).await?;
    let mut events: Vec<PriceBasisEvent> = splits.iter().map(PriceBasisEvent::from).collect();

    let statements = corporate_action::db_demerger_price_statements(&mut *conn, listing_id).await?;
    for statement in statements.iter().rev() {
        let Some(reference) =
            db_observed_figure(&mut *conn, listing_id, statement.close_date).await?
        else {
            continue; // nothing of that day is stored yet
        };
        if reference.observed < statement.date {
            continue; // observed before the demerger: already contemporaneous
        }
        let partly = corporate_action::contemporaneous_price(
            reference.as_observed,
            &events,
            statement.close_date,
            reference.observed,
        );
        if partly <= Decimal::ZERO {
            continue; // no factor is recoverable from a non-positive figure
        }
        events.push(PriceBasisEvent {
            date: statement.date,
            recover_new: statement.close_price,
            recover_old: partly,
        });
    }
    Ok(events)
}

/// One stored ok, provider-fetched row, as the re-basing pass reads it.
#[derive(sqlx::FromRow)]
struct ObservedRow {
    id: i64,
    price_date: NaiveDate,
    fetched_at: String,
    #[sqlx(try_from = "Money")]
    price: Decimal,
    #[sqlx(try_from = "Money")]
    price_as_observed: Decimal,
}

/// Re-derive every stored provider price for one listing from the figure as
/// observed, over the listing's re-basing actions as they now stand. Returns
/// how many rows changed.
///
/// This is the other half of the basis invariant (module docs): normalising on
/// the way in fixes a price fetched *after* an event is recorded, and this
/// fixes one fetched before it. Because each price is recomputed from
/// `price_as_observed` rather than adjusted in place, the pass is idempotent
/// and order-free — it is equally the answer to a split or a demerger's stated
/// close being recorded, a ratio, close or date being edited, an action being
/// re-typed into another kind, and one being deleted.
/// `corporate_action::db_upsert`/`db_delete` run it on their own transaction so
/// the prices and the action can never be committed out of step, and the
/// `price-rebase` job runs it over every listing as the one-off repair of a
/// database that predates this rule.
///
/// The event set is [`db_price_basis_events`]', not the quantity re-basing one
/// — a demerger belongs to it and must never reach `split_ratio`.
///
/// An **empty** event set is not an early exit: it is the state a listing is
/// left in when its last re-basing action is deleted, or a demerger's stated
/// close removed, and the prices then have to come back to the figures as
/// observed. So the walk runs either way — over no events it re-derives each
/// price as `clean_price(price_as_observed)`, which is what a fetch with
/// nothing to restate would have stored, and writes nothing where that is
/// already the stored figure.
///
/// Manual rows are excluded: a hand-entered price is contemporaneous by
/// declaration and is never rewritten (module docs).
pub async fn db_rebase_listing_prices(
    conn: &mut sqlx::SqliteConnection,
    listing_id: i64,
) -> Result<usize, sqlx::Error> {
    let events = db_price_basis_events(&mut *conn, listing_id).await?;
    let rows: Vec<ObservedRow> = sqlx::query_as(
        "SELECT id, price_date, fetched_at, price, price_as_observed FROM closing_prices \
         WHERE listing_id = ? AND status = 'ok' AND origin = 'fetched' ORDER BY price_date",
    )
    .bind(listing_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut changed = 0;
    for row in rows {
        let observed = observation_date(&row.fetched_at)?;
        let wanted = contemporaneous(row.price_as_observed, &events, row.price_date, observed);
        if wanted == row.price {
            continue;
        }
        sqlx::query("UPDATE closing_prices SET price = ? WHERE id = ?")
            .bind(Money(wanted))
            .bind(row.id)
            .execute(&mut *conn)
            .await?;
        changed += 1;
    }
    Ok(changed)
}

/// Re-base every listing that has a price re-basing action recorded against
/// it — a `ShareSplit`/`BonusIssue`, or a `Demerger` carrying a stated
/// pre-demerger close — as the `price-rebase` maintenance job, and the one-off
/// repair for a database whose prices were stored before the basis rule
/// existed (migrations 0034 and 0036). One transaction, so the whole repair
/// lands or none of it does; idempotent, so running it again is a no-op.
///
/// Only listings with such an action can have a price to correct, so those are
/// the only ones read. This stays the single repair path: a demerger's stated
/// close was folded into the same job rather than given one of its own.
pub async fn run_rebase(pool: &SqlitePool) -> Result<(), String> {
    let mut tx = write_tx(pool).await.map_err(|e| e.to_string())?;
    let listing_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT listing_id FROM corporate_actions \
         WHERE action_type IN ('ShareSplit', 'BonusIssue') \
            OR (action_type = 'Demerger' AND demerger_close_date IS NOT NULL) \
         ORDER BY listing_id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;
    let mut changed = 0;
    for listing_id in &listing_ids {
        changed += db_rebase_listing_prices(&mut tx, *listing_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    tracing::info!(
        listings = listing_ids.len(),
        rebased = changed,
        "closing-price re-base complete"
    );
    Ok(())
}

/// Delete one stored row, reporting whether one was there. Callers must have
/// established that the row is one of the two kinds no snapshot was ever
/// valued at (the handler rejects any other):
///
/// * an **errored** row — `reports::valuation` blocks the date outright;
/// * an ok row dated **before the listing's `unpriced_before`** — the marker
///   supersedes the stored rows for that span, so valuation excludes the
///   holding from those dates instead of pricing it, and even the
///   `unpriced_from` carry-forward is floored at the marker
///   ([`db_latest_ok_price_on_or_before`]).
///
/// Either way removing the row cannot invalidate a stored snapshot figure:
/// no stored figure was computed from it. That is what lets `closing_prices`
/// keep its single `..._stale_snapshots_update` trigger (0001_schema.sql)
/// with no DELETE counterpart, unlike the fact tables. Setting or moving the
/// marker is itself what stales the affected snapshots (0037's
/// `listings_stale_snapshots_update` stales the prefix before the later of
/// the old and new dates), so a span whose rows are then cleared has already
/// been regenerated without them, and clearing or moving the marker back
/// later stales the prefix again — regeneration then reports the dates
/// blocked for want of a price, which is the truth once the rows are gone.
pub async fn db_delete(
    pool: &SqlitePool,
    listing_id: i64,
    price_date: NaiveDate,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM closing_prices WHERE listing_id = ? AND price_date = ?")
        .bind(listing_id)
        .bind(price_date)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// What [`db_clear_unpriced_before`] found to do.
#[derive(Debug, PartialEq, Eq)]
pub enum ClearOutcome {
    /// No such listing.
    NoListing,
    /// The listing declares no `unpriced_before`, so it has no superseded
    /// span and nothing here may be cleared in bulk.
    NoMarker,
    /// The span was cleared (possibly of nothing — the operation is
    /// idempotent).
    Cleared {
        unpriced_before: NaiveDate,
        deleted: u64,
    },
}

/// Clear every stored row a listing's `unpriced_before` marker supersedes —
/// the whole span before it, ok rows included — in one transaction.
///
/// The bulk form of the single-date delete, and deliberately the *only* bulk
/// form: the span it clears is not a caller-supplied date range but the
/// listing's own declaration, read from the `listings` row by the DELETE
/// itself, so this can never become a general bulk-delete of price history.
/// A listing with no marker deletes nothing ([`ClearOutcome::NoMarker`]).
/// Re-running it is a no-op that reports `deleted: 0`.
///
/// Why an ok row may go: see [`db_delete`] — inside the span no stored figure
/// is read by valuation, so none of it is a valuation to lose. Nothing is
/// destroyed either way: `closing_prices` is audited, and the per-row `AFTER
/// DELETE` trigger fires once per row of a multi-row DELETE, so every
/// cleared figure and its `sourced_from`/`reason` land in `row_history`.
///
/// It cannot break `unpriced_from`'s write-time pairing (a stored ok price
/// must exist *before* that marker to be carried forward), because that check
/// only ever looks at rows on or after `unpriced_before` — exactly the ones
/// this leaves alone.
pub async fn db_clear_unpriced_before(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<ClearOutcome, sqlx::Error> {
    let mut tx = write_tx(pool).await?;
    let found: Option<(Option<NaiveDate>,)> =
        sqlx::query_as("SELECT unpriced_before FROM listings WHERE id = ?")
            .bind(listing_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((marker,)) = found else {
        return Ok(ClearOutcome::NoListing);
    };
    let Some(unpriced_before) = marker else {
        return Ok(ClearOutcome::NoMarker);
    };
    // The bound is the subquery, not the value read above: the rows deleted
    // are exactly the ones the listing's own row calls superseded at the
    // moment the statement runs.
    let deleted = sqlx::query(
        "DELETE FROM closing_prices \
         WHERE listing_id = ?1 \
           AND price_date < (SELECT unpriced_before FROM listings WHERE id = ?1)",
    )
    .bind(listing_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(ClearOutcome::Cleared {
        unpriced_before,
        deleted,
    })
}
