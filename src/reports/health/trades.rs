//! Checks over individual trade and listing rows: a listing quoting in another
//! currency than its exchange, trades on non-trading days, ESS 30-day-rule
//! disposals, and nil-proceeds disposals.

use crate::domain::rollover;
use crate::domain::tax_year::tax_year_for;
use crate::entities::closing_price::{self, NonTradingReason};
use crate::entities::ess_statement::{self, EssStatement};
use crate::entities::trade;
use crate::infra::currency::CurrencyCode;
use crate::infra::decimal::Money;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// The ESS 30-day rule's window: a disposal **within 30 days after** the
/// deferred taxing point moves the taxing point to the disposal date
/// (`docs/ato/ess-30-day-rule.md`, QC 23058 Example 11). Unlike the wash-sale
/// window this is statutory (ITAA 1997 s 83A-115(3)), not a review convention,
/// so it is a constant rather than a request parameter.
pub const ESS_THIRTY_DAY_WINDOW: i64 = 30;

/// A listing whose `currency` is not the currency its exchange quotes in
/// (SCENARIOS R-01) — an AUD listing sitting on a USD market, or the reverse.
///
/// The listing is unpriceable from that moment on: every fetch resolves the
/// exchange's symbol, and either the provider serves no such series (an
/// errored row) or the candle's currency is not the listing's, which
/// `entities::closing_price`'s cross-check refuses (also an errored row). And
/// it cannot be corrected in place, because `listings.currency` denominates
/// every stored closing price and is frozen once the listing has any history
/// (`entities::listing::UpsertError::CurrencyChangeWithHistory`) — the
/// documented remedy is a new listing in the right currency plus a
/// [transfer](crate::entities::transfer) of the parcels to it.
///
/// `POST /listings/:id/renames` refuses to *create* the state
/// (`entities::listing_rename::RenameError::ExchangeCurrencyMismatch`), but a
/// plain `PUT /listings/:id` can still enter a listing in it directly — no
/// write path compares the two columns — so it is reported here wherever it
/// came from. Deliberately a **warning, not a constraint** on `PUT`: a real
/// exchange can quote more than one currency (LSE quotes GBp, USD and EUR)
/// while `exchanges.currency` is a single column, so the pairing is a strong
/// smell rather than a certainty.
///
/// Exchange-less listings — `security_type = 'Crypto'`, NULL `exchange_mic` —
/// have no exchange to disagree with and are excluded (the SQL joins
/// `exchanges`, so they never appear).
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExchangeCurrencyMismatch {
    pub listing_id: i64,
    pub ticker: String,
    /// The listing's own `currency` — what every stored closing price for it
    /// is denominated in.
    pub currency: CurrencyCode,
    pub exchange_mic: String,
    /// The currency that exchange quotes in (`exchanges.currency`).
    pub exchange_currency: CurrencyCode,
}

/// A trade dated on a day its own exchange did not trade — a weekend, or a
/// seeded public holiday on the calendar that was in force **on that date**
/// (SCENARIOS S-08). Exchange-less (Crypto) listings trade every day and never
/// appear.
///
/// The trade date is the CGT event date: it sets the 12-month discount clock,
/// the financial year the gain falls in, and the day the T+n settlement count
/// starts from. A day the market was shut is a data-entry error by
/// construction — which is why `PUT /trades/:id` and `PUT /sells/:id` now
/// refuse one (`trade::UpsertError::NonTradingDay`,
/// `sell::SellError::NonTradingDay`) over the very same calendar helper
/// (`closing_price::non_trading_day`).
///
/// This list exists for the rows those two routes never see: an ESS vest, an
/// inherited parcel, a DRP reinvestment, a rights exercise and every
/// parcel-substituting operation write their trade rows directly, and are
/// exempt from the refusal on purpose — an ESS taxing point, a date of death
/// and a corporate action's effective date are all dated by facts that do not
/// have to fall on a market day. Non-blocking, and nothing is rewritten:
/// `source` says which path wrote the row, so it is clear whether to correct
/// the trade or the fact behind it. Rows entered before the refusal existed
/// surface here too.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct NonTradingDayTrade {
    pub trade_id: i64,
    /// `Buy`, `Sell` or `DRP`.
    pub trade_type: String,
    pub listing_id: i64,
    pub ticker: String,
    pub date: NaiveDate,
    /// The MIC whose calendar was in force on `date` — not necessarily the
    /// listing's exchange today, if it has since changed exchange.
    pub exchange_mic: String,
    pub reason: NonTradingReason,
    /// The write path the row came from, e.g. `an ESS vest`, `a demerger`,
    /// `entered directly` — plain English, for the banner sentence.
    pub source: String,
}

/// The joined row behind [`NonTradingDayTrade`], before its date is put to the
/// listing's calendar. Mapped by column name via `FromRow`.
#[derive(sqlx::FromRow)]
pub(super) struct NonTradingDayCandidate {
    trade_id: i64,
    trade_type: String,
    listing_id: i64,
    ticker: String,
    date: NaiveDate,
    source: String,
}

/// A disposal of ESS-vested shares **within 30 days after** the statement's
/// taxing point, where the ESS 30-day rule moves the taxing point to the
/// disposal date (SCENARIOS J-04, `docs/ato/ess-30-day-rule.md`, QC 23058
/// Example 11 — ITAA 1997 s 83A-115(3)).
///
/// What the rule does, and why silence here is costly: the discount is
/// re-measured at what the disposal actually realised, and the CGT cost base
/// resets to that same figure on that same date — so **there is no separate
/// capital gain**, and the discount can move into the **next financial year**.
/// A user who enters the employer's *original* statement and then the sale gets
/// both figures wrong at once, in two different years, from an entry the system
/// accepts without comment: the original discount assessed in the original
/// year, plus a capital gain that does not exist.
///
/// Advisory only, the same call as `reports::wash_sales`: nothing is rejected
/// and no figure is rewritten. The correction is an **amended employer
/// statement** — the employer must issue one within 30 days of becoming aware
/// of the disposal — and the system cannot know whether one was issued, nor
/// perform a re-measurement the ATO puts on the employer. Enter the amended
/// statement over the original (taxing point = the disposal date, market value
/// = what the disposal realised) and the figures follow.
///
/// A disposal **on** the taxing point is never flagged: the rule's effect is a
/// no-op there — the taxing point is already the disposal date — so the
/// corrected entry (the amended statement, vested and sold the same day, as in
/// `ato_examples::ess_30_day_rule_example_11_wyatt_amended_statement`) must not
/// nag.
///
/// # What counts as a disposal (SCENARIOS N-08)
///
/// The rule turns on a *disposal*, so the Sells that are not one are not
/// candidates — `docs/ato/ess-takeovers-and-restructures.md` (ITAA 1997
/// s 83A-130) is the source for the two rollover cases:
///
/// - A **holding-account transfer**'s closing Sell (`trades.transfer_id`) is
///   excluded in SQL. Nothing is disposed of — the same beneficial owner holds
///   the same interests before and after, which is why no CGT event arises
///   either — and it is not a takeover or restructure, so s 83A-130 does not
///   even come into it. This is the RSU-plan-to-broker move `entities::transfer`
///   exists for, so leaving it in flagged the *ordinary* use of the feature and
///   invited an amended return over a change of custody.
/// - A **scrip-for-scrip exchange** or **demerger** closing Sell stays, carrying
///   [`EssDisposalKind::TakeoverOrRestructure`]: s 83A-130(2) treats matching
///   replacement interests as a continuation, so the taxing point normally does
///   *not* move — but that rests on facts this system does not record ((4)'s
///   ordinary-shares test, (9)'s continuing-employment and two 10% tests), and
///   (5) makes a partial-rollover's cash component a disposal to that extent.
///   Advisory is the honest answer there, not silence.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct EssThirtyDaySale {
    /// The Sell whose allocation draws on the vest parcel.
    pub sale_trade_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    pub sale_date: NaiveDate,
    /// Units of the vest parcel this sale allocated — the ESS interests
    /// disposed of, which need not be the whole vest.
    pub units_sold: Decimal,
    pub ess_statement_id: i64,
    /// The statement's vest Buy, i.e. the parcel allocated from.
    pub vest_trade_id: i64,
    pub taxing_point_date: NaiveDate,
    /// `sale_date − taxing_point_date` in days: always 1..=30
    /// ([`ESS_THIRTY_DAY_WINDOW`]).
    pub days_after: i64,
    /// ISO 4217 currency `statement_discount` is stated in (the statement's).
    pub currency: CurrencyCode,
    /// The discount the statement currently declares (D + E + F + the pre-2009
    /// cessation label, in `currency`) — the figure the rule re-measures.
    pub statement_discount: Decimal,
    /// The financial year the statement's taxing point falls in, which is where
    /// that discount is assessed today.
    pub statement_tax_year: i32,
    /// The financial year the **disposal** falls in — where the rule moves the
    /// discount to. Equal to `statement_tax_year` when the window does not
    /// cross a 30 June, and that is the *common* case: the years differing is
    /// the Example 11 shape, where the correction also moves a return.
    pub disposal_tax_year: i32,
    /// Whether the Sell is an ordinary disposal or a rollover operation's
    /// closing Sell, which changes what the row is asking the user to check.
    pub disposal_kind: EssDisposalKind,
}

/// What kind of Sell reached the 30-day-rule alert — see
/// [`EssThirtyDaySale`]'s "What counts as a disposal".
#[derive(utoipa::ToSchema, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EssDisposalKind {
    /// An ordinary disposal: the rule applies, and the correction is an amended
    /// employer statement.
    Sale,
    /// A scrip-for-scrip exchange or demerger closing Sell. ITAA 1997
    /// s 83A-130(2) treats matching replacement interests as a **continuation**
    /// of the old ESS interests, so the deferred taxing point normally does not
    /// move — subject to (4)/(9), which this system cannot test, and to (5),
    /// which does treat a partial-rollover's cash component as a disposal.
    TakeoverOrRestructure,
}

/// The joined row behind [`EssThirtyDaySale`]: the statement's own figures are
/// attached from a second read, so the discount is summed by
/// `ess_statement::discount_labels` rather than re-added here.
#[derive(sqlx::FromRow)]
pub(super) struct EssThirtyDaySaleRow {
    sale_trade_id: i64,
    listing_id: i64,
    ticker: String,
    sale_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    units_sold: Decimal,
    /// The parcel the allocation consumed — a vest parcel, or a replacement one
    /// down its rollover chain; the statement and vest parcel are looked up
    /// from it.
    parcel_id: i64,
    /// Set when the Sell is a scrip-for-scrip exchange or demerger closing
    /// Sell (the transfer case never reaches here — it is filtered in SQL).
    rollover_action_id: Option<i64>,
}

/// A disposal recorded at **nil proceeds** — a Sell at a zero price, or a
/// disposal of *paid-for* rights at nothing per right (SCENARIOS AA-03).
///
/// Why it is worth naming. Under the **market-value substitution rule**
/// (`docs/ato/capital-proceeds-market-value-substitution.md`, QC 66021; ITAA
/// 1997 s 116-30) a taxpayer who receives nothing for a CGT asset is taken to
/// have received its **market value** at the time of the event — which is
/// exactly why `docs/API.md`'s *Gifts / off-market related-party transfers*
/// limitation prescribes entering a gift out as a Sell at market-value
/// proceeds. The failure mode that convention exists to prevent is entering
/// what was actually *received*: nothing. Nothing refuses it, every figure
/// downstream is individually valid, and the result is a capital loss the size
/// of the whole cost base — netted against the year's gains and carried
/// forward under 18V for as long as it takes to absorb.
///
/// **Advisory, and it blocks nothing** — like `duplicate_trades` and
/// `non_trading_day_trades` beside it. A nil-proceeds disposal is not wrong by
/// construction: a crypto burn, an abandonment, or paid-for rights left to
/// lapse each realise a real loss at genuinely nil proceeds, and no stored fact
/// says whether a Sell was a gift. So this names the shape and leaves the
/// judgement where it belongs.
///
/// # What is *not* listed
///
/// - **Every operation-written closing Sell.** A worthless-shares recognise, a
///   scrip-for-scrip exchange, a demerger, a holding-account transfer (and its
///   crypto network-fee disposal) and a buy-back participation all construct
///   their Sells mechanically, at nil or derived proceeds; none is a user
///   entry, and the worthless-shares one is nil proceeds *by definition*. They
///   are excluded by `trade::operation_written_sql` — the one place that
///   answers "did an operation write this row?", so a future operation's
///   column is covered without editing this check.
/// - **A free right that lapses.** `rights_cost` defaults to `0` for rights
///   issued free, so nil proceeds against a nil cost base is the "nil/nil
///   non-event" `docs/API.md` describes: no gain, no loss, nothing fabricated
///   — and it is what *every* ordinary lapse looks like. Only a right that was
///   **paid for** is listed, where nil proceeds turn `rights_cost` into a
///   capital loss.
///
/// # Why the *price*, not the computed proceeds
///
/// Net proceeds are `average_price × quantity − brokerage − GST`, but the test
/// here is `average_price == 0`: nothing was received **per unit**, which is
/// the shape the market-value substitution rule is about. A non-zero price
/// whose brokerage happens to cancel it to the cent is a different and much
/// stranger animal — arithmetic, not a nil-consideration disposal — and
/// flagging it under this rule's reason would be saying something untrue about
/// it.
#[derive(utoipa::ToSchema, Debug, Serialize, Deserialize)]
pub struct NilProceedsDisposal {
    /// Which record the alert is about, and therefore what `record_id` and
    /// `quantity` mean.
    pub kind: NilProceedsKind,
    /// The `trades` id (`Sell`) or the `rights_sales` id (`RightsSale`).
    pub record_id: i64,
    pub listing_id: i64,
    pub ticker: String,
    /// The disposal date — the CGT event date the market value would be read
    /// at.
    pub date: NaiveDate,
    /// Units disposed of: the Sell's `quantity`, or the rights sale's `units`.
    pub quantity: Decimal,
    /// ISO 4217 currency of the record (the trade's, or the rights issue's).
    pub currency: CurrencyCode,
    /// `RightsSale` only: the `rights_cost` the nil proceeds realise as a
    /// capital loss, in `currency`. Always positive when set — a nil cost is
    /// the free-right non-event and is not listed. `None` for a Sell, whose
    /// loss is the parcel's adjusted cost base and is not computed here: that
    /// needs the whole `domain::cost_base` pipeline, and the figure actually
    /// wanted — market value on `date` — is not a stored fact at all.
    pub rights_cost: Option<Decimal>,
}

/// Which kind of record a [`NilProceedsDisposal`] names.
#[derive(utoipa::ToSchema, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NilProceedsKind {
    /// An ordinary Sell trade entered by hand, at a zero `average_price`.
    Sell,
    /// A disposal of rights that were **paid for** (`rights_cost > 0`) at a
    /// zero `proceeds_per_right` — a sale, a lapse, or a nil retail premium.
    RightsSale,
}

/// A candidate Sell for [`NilProceedsDisposal`]: the price is compared as a
/// `Decimal` in Rust rather than in SQL, since the money columns are exact
/// TEXT decimals ("0", "0.00" and "0.0000" are one value and three strings)
/// and casting them to SQLite REAL to compare would be the one thing the
/// decimal rule forbids.
#[derive(sqlx::FromRow)]
pub(super) struct NilProceedsSellRow {
    record_id: i64,
    listing_id: i64,
    ticker: String,
    date: NaiveDate,
    #[sqlx(try_from = "Money")]
    average_price: Decimal,
    #[sqlx(try_from = "Money")]
    quantity: Decimal,
    currency: CurrencyCode,
}

/// A candidate rights disposal for [`NilProceedsDisposal`], compared in Rust
/// for the same reason as [`NilProceedsSellRow`].
#[derive(sqlx::FromRow)]
pub(super) struct NilProceedsRightsSaleRow {
    record_id: i64,
    listing_id: i64,
    ticker: String,
    date: NaiveDate,
    #[sqlx(try_from = "Money")]
    units: Decimal,
    currency: CurrencyCode,
    #[sqlx(try_from = "Money")]
    proceeds_per_right: Decimal,
    #[sqlx(try_from = "Money")]
    rights_cost: Decimal,
}

/// Disposals of ESS-vested shares inside the 30-day window — see
/// [`EssThirtyDaySale`].
///
/// Read on the caller's transaction, starting from the **vest parcels** rather
/// than from the allocations: each ESS statement's own vest Buy, plus every
/// parcel a rollover has since carried its units into
/// (`domain::rollover::replacement_descendants`). Following the chain is what
/// makes the alert work on the ordinary RSU path (SCENARIOS N-08) — vest into
/// the employer's plan account, move the shares to a personal broker account,
/// sell there — where the parcel actually sold carries no `ess_statement_id` of
/// its own, so joining `trades.ess_statement_id` to the allocation saw nothing
/// at all. Only the ESS parcels' own allocations are read, so an unrelated
/// disposal never reaches memory; the window itself is applied in Rust, since
/// the candidate set is now the small one.
///
/// The pairing is per **allocation × statement**: a Sell drawing on two vest
/// parcels inside their windows is two rows, since each statement would be
/// amended separately. Where one rollover moved several parcels at once, its
/// replacements descend from all of that group's sources (the data records no
/// finer link — see `replacement_descendants`), so such a sale can report
/// against more than one statement; the alert is advisory, and each row names
/// the vest parcel and statement it is about.
///
/// The statements are read separately rather than joined, so the discount is
/// summed by `ess_statement::discount_labels` — the tax summary's own definition
/// of the discount — instead of being re-added over TEXT columns here.
pub(super) async fn db_ess_30_day_rule(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<EssThirtyDaySale>, sqlx::Error> {
    // Every vest parcel, and the statement behind it.
    let vests: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, ess_statement_id FROM trades \
         WHERE ess_statement_id IS NOT NULL AND trade_type IN ('Buy', 'DRP') ORDER BY id",
    )
    .fetch_all(&mut *conn)
    .await?;
    if vests.is_empty() {
        return Ok(Vec::new());
    }
    // Which (vest parcel, statement) each parcel now holding ESS units answers
    // for — the vest parcel itself, and every replacement down the chain.
    let mut ess_parcels: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    for (vest_trade_id, ess_statement_id) in vests {
        ess_parcels
            .entry(vest_trade_id)
            .or_default()
            .push((vest_trade_id, ess_statement_id));
        for descendant in rollover::replacement_descendants(&mut *conn, vest_trade_id).await? {
            ess_parcels
                .entry(descendant)
                .or_default()
                .push((vest_trade_id, ess_statement_id));
        }
    }

    // Their allocations, with the disposing Sell. A transfer-out Sell is not a
    // disposal at all and is excluded here (see `EssThirtyDaySale`); a
    // scrip-exchange/demerger closing Sell is kept and labelled.
    let placeholders = std::iter::repeat_n("?", ess_parcels.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut query = sqlx::query_as::<_, EssThirtyDaySaleRow>(sqlx::AssertSqlSafe(format!(
        "SELECT s.id AS sale_trade_id, s.listing_id AS listing_id, l.ticker AS ticker, \
                s.date AS sale_date, pa.quantity_allocated AS units_sold, \
                pa.purchase_trade_id AS parcel_id, \
                COALESCE(s.scrip_action_id, s.demerger_action_id) AS rollover_action_id \
         FROM parcel_allocations pa \
         JOIN trades s ON s.id = pa.sale_trade_id \
         JOIN listings l ON l.id = s.listing_id \
         WHERE pa.purchase_trade_id IN ({placeholders}) AND s.transfer_id IS NULL \
         ORDER BY s.date DESC, s.id"
    )));
    let mut parcel_ids: Vec<i64> = ess_parcels.keys().copied().collect();
    parcel_ids.sort_unstable();
    for id in &parcel_ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(&mut *conn).await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    // The whole (small) statement table, the same pre-loading call as the
    // ticker maps above: every candidate row needs its statement's figures.
    let statements: HashMap<i64, EssStatement> =
        sqlx::query_as::<_, EssStatement>(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM ess_statements",
            ess_statement::COLUMNS
        )))
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();

    let mut alerts = Vec::new();
    for row in rows {
        // Every (vest parcel, statement) this parcel answers for; the map was
        // built from the same read, so the entry is always there.
        for &(vest_trade_id, ess_statement_id) in
            ess_parcels.get(&row.parcel_id).into_iter().flatten()
        {
            // A row read between the two queries is the only way to miss the
            // statement, and the alert would be unable to name what it is about.
            let statement = statements.get(&ess_statement_id).ok_or_else(|| {
                sqlx::Error::Decode(
                    format!("ESS statement {ess_statement_id} disappeared mid-read").into(),
                )
            })?;
            let days_after = (row.sale_date - statement.taxing_point_date).num_days();
            if !(1..=ESS_THIRTY_DAY_WINDOW).contains(&days_after) {
                continue;
            }
            alerts.push(EssThirtyDaySale {
                sale_trade_id: row.sale_trade_id,
                listing_id: row.listing_id,
                ticker: row.ticker.clone(),
                sale_date: row.sale_date,
                units_sold: row.units_sold,
                ess_statement_id,
                vest_trade_id,
                taxing_point_date: statement.taxing_point_date,
                days_after,
                currency: statement.currency.clone(),
                statement_discount: ess_statement::discount_labels(statement),
                statement_tax_year: tax_year_for(statement.taxing_point_date),
                disposal_tax_year: tax_year_for(row.sale_date),
                disposal_kind: match row.rollover_action_id {
                    Some(_) => EssDisposalKind::TakeoverOrRestructure,
                    None => EssDisposalKind::Sale,
                },
            });
        }
    }
    // Newest sale first, then the sale's own id, then the statement — the order
    // the SQL used to produce, now that the window is applied in Rust.
    alerts.sort_by(|a, b| {
        b.sale_date
            .cmp(&a.sale_date)
            .then_with(|| a.sale_trade_id.cmp(&b.sale_trade_id))
            .then_with(|| a.ess_statement_id.cmp(&b.ess_statement_id))
    });
    Ok(alerts)
}

/// Listings whose `currency` is not their exchange's — see
/// [`ExchangeCurrencyMismatch`].
///
/// One join on the caller's transaction, like the `duplicate_*` family. The
/// join to `exchanges` is what excludes exchange-less (Crypto) listings: they
/// have no exchange, so there is nothing for their currency to disagree with.
pub(super) async fn db_exchange_currency_mismatches(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<ExchangeCurrencyMismatch>, sqlx::Error> {
    sqlx::query_as::<_, ExchangeCurrencyMismatch>(
        "SELECT l.id AS listing_id, l.ticker AS ticker, l.currency AS currency, \
                e.mic AS exchange_mic, e.currency AS exchange_currency \
         FROM listings l JOIN exchanges e ON e.mic = l.exchange_mic \
         WHERE l.currency <> e.currency \
         ORDER BY l.ticker, l.id",
    )
    .fetch_all(&mut *conn)
    .await
}

/// Every trade whose date is not a trading day on its listing's own calendar
/// (see [`NonTradingDayTrade`]).
///
/// Reads every trade — not only the ones the write-time refusal covers — and
/// puts each date to `closing_price::non_trading_day`, the same helper the
/// refusal and the closing-price write path use, so there is exactly one
/// definition of "the market was shut". The calendar is resolved **as at the
/// trade date**, so a listing that has changed exchange is judged on the
/// market it actually traded on then.
pub(super) async fn db_non_trading_day_trades(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<NonTradingDayTrade>, sqlx::Error> {
    let candidates: Vec<NonTradingDayCandidate> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.id AS trade_id, t.trade_type AS trade_type, t.listing_id AS listing_id, \
                l.ticker AS ticker, t.date AS date, {} AS source \
         FROM trades t JOIN listings l ON l.id = t.listing_id \
         ORDER BY t.date DESC, t.id DESC",
        trade::source_case_sql("t"),
    )))
    .fetch_all(&mut *conn)
    .await?;

    // Every calendar first, one load per listing that has trades — not one per
    // trade: the `Market` behind it is four queries, and a listing with six
    // years of history has hundreds of rows.
    let listing_ids: HashSet<i64> = candidates.iter().map(|c| c.listing_id).collect();
    let mut markets: HashMap<i64, closing_price::Market> = HashMap::new();
    for listing_id in listing_ids {
        if let Some(market) = closing_price::load_market_on(&mut *conn, listing_id).await? {
            markets.insert(listing_id, market);
        }
    }

    let mut alerts = Vec::new();
    for c in candidates {
        let Some(market) = markets.get(&c.listing_id) else {
            continue;
        };
        let Some(shut) = closing_price::non_trading_day(market, c.date) else {
            continue;
        };
        alerts.push(NonTradingDayTrade {
            trade_id: c.trade_id,
            trade_type: c.trade_type,
            listing_id: c.listing_id,
            ticker: c.ticker,
            date: c.date,
            exchange_mic: shut.exchange_mic,
            reason: shut.reason,
            source: c.source,
        });
    }
    Ok(alerts)
}

/// Every disposal recorded at nil proceeds (see [`NilProceedsDisposal`]).
///
/// Two reads, one list. The Sell half excludes the mechanically constructed
/// closing Sells through `trade::operation_written_sql`, the single place that
/// answers what wrote a trade row; the rights half keeps only rights that were
/// paid for, since nil proceeds on a free right is the nil/nil non-event every
/// ordinary lapse looks like. Both compare the money column in Rust — see
/// [`NilProceedsSellRow`].
pub(super) async fn db_nil_proceeds_disposals(
    conn: &mut sqlx::SqliteConnection,
) -> Result<Vec<NilProceedsDisposal>, sqlx::Error> {
    let sells: Vec<NilProceedsSellRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.id AS record_id, t.listing_id AS listing_id, l.ticker AS ticker, \
                t.date AS date, t.average_price AS average_price, t.quantity AS quantity, \
                t.currency AS currency \
         FROM trades t JOIN listings l ON l.id = t.listing_id \
         WHERE t.trade_type = 'Sell' AND NOT {}",
        trade::operation_written_sql("t"),
    )))
    .fetch_all(&mut *conn)
    .await?;

    let mut alerts: Vec<NilProceedsDisposal> = sells
        .into_iter()
        .filter(|s| s.average_price.is_zero())
        .map(|s| NilProceedsDisposal {
            kind: NilProceedsKind::Sell,
            record_id: s.record_id,
            listing_id: s.listing_id,
            ticker: s.ticker,
            date: s.date,
            quantity: s.quantity,
            currency: s.currency,
            rights_cost: None,
        })
        .collect();

    // The rights issue's own `currency` is on the corporate action — the
    // rights_sales row deliberately has no currency column of its own.
    let rights_sales: Vec<NilProceedsRightsSaleRow> = sqlx::query_as(
        "SELECT rs.id AS record_id, ca.listing_id AS listing_id, l.ticker AS ticker, \
                rs.date AS date, rs.units AS units, ca.currency AS currency, \
                rs.proceeds_per_right AS proceeds_per_right, rs.rights_cost AS rights_cost \
         FROM rights_sales rs \
         JOIN corporate_actions ca ON ca.id = rs.rights_action_id \
         JOIN listings l ON l.id = ca.listing_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    alerts.extend(
        rights_sales
            .into_iter()
            .filter(|r| r.proceeds_per_right.is_zero() && r.rights_cost > Decimal::ZERO)
            .map(|r| NilProceedsDisposal {
                kind: NilProceedsKind::RightsSale,
                record_id: r.record_id,
                listing_id: r.listing_id,
                ticker: r.ticker,
                date: r.date,
                quantity: r.units,
                currency: r.currency,
                rights_cost: Some(r.rights_cost),
            }),
    );

    // Newest first, like every other dated list here; the two kinds share one
    // id space only by accident, so the id is a tie-break for stability.
    alerts.sort_by(|a, b| b.date.cmp(&a.date).then(b.record_id.cmp(&a.record_id)));
    Ok(alerts)
}
