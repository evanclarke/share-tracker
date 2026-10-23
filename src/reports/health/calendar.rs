//! The distribution calendar: announced distributions with no income entry,
//! and entries whose amount disagrees with the announcement.

use crate::entities::closing_price::HeldTimeline;
use crate::infra::currency::CurrencyCode;
use crate::infra::decimal::Money;
use crate::infra::id::{HoldingAccountId, ListingId};
use chrono::{DateTime, Duration, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::iter;

/// How far **before** an ex-date an income row may be dated and still be that
/// distribution's, for [`MissingDividendEntry`]'s matching.
///
/// A fund's `entitlement_date` is the distribution period's end and sits a day
/// or two *before* the ex-date (period ending 30 June, ex 1 July), so the
/// window has to reach backwards at all — but only just.
///
/// It reaches backwards **for the entitlement anchors alone**. A payment
/// cannot precede its own ex-date, so `date_paid` is held to
/// `0..=DIVIDEND_MATCH_AFTER_DAYS` instead — see
/// [`CalendarIncomeRow::distance_from`].
pub(super) const DIVIDEND_MATCH_BEFORE_DAYS: i64 = 15;

/// How far **after** an ex-date an income row may be dated and still be that
/// distribution's.
///
/// The registry pays ~15 days after the ex-date for a fund and up to about six
/// weeks for a company, and `date_paid` is the anchor most rows have. Together
/// with [`DIVIDEND_MATCH_BEFORE_DAYS`] the window is 61 days, comfortably
/// inside the ~88-day minimum spacing of a quarterly payer — so two of a
/// quarterly payer's events can never compete for one income row on spacing
/// alone.
///
/// A **monthly** payer is well inside it, though, and that is what
/// [`CalendarIncomeRow::distance_from`]'s two ordering rules are for: at ~15
/// days from ex-date to payment, the previous month's payment sits about as
/// far before an ex-date as this month's sits after it. The window cannot be
/// narrowed to separate them without losing the six-week company payment it
/// was widened for.
pub(super) const DIVIDEND_MATCH_AFTER_DAYS: i64 = 45;

/// The relative band [`DividendAmountMismatch`] treats as agreement, as a
/// percentage of the expected gross.
///
/// The per-unit figure carries the provider's ~7 significant digits and the
/// units are exact, so two facts that agree agree to fractions of a cent. The
/// band is not for arithmetic slack — it absorbs the small legitimate
/// divergences (a registry's own rounding convention, a component recorded
/// slightly differently) while leaving the error this alert exists for, a
/// mistyped figure, far outside it.
pub(super) const DIVIDEND_AMOUNT_TOLERANCE_PERCENT: i64 = 2;

/// The absolute floor under [`DIVIDEND_AMOUNT_TOLERANCE_PERCENT`], in the
/// distribution's own currency: a difference smaller than this is never
/// reported however large it is in percentage terms.
///
/// Without it a distribution of a fraction of a cent per unit — HNDQ paid
/// 0.018741 — would alert on a few cents of registry rounding.
pub(super) const DIVIDEND_AMOUNT_TOLERANCE_FLOOR: i64 = 1;

/// A distribution the provider knows about that no income row records
/// (see [`db_distribution_calendar`]).
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MissingDividendEntry {
    pub listing_id: ListingId,
    pub ticker: String,
    pub holding_account_id: HoldingAccountId,
    pub holding_account: String,
    /// The ex-date, as stored by the distribution calendar.
    pub ex_date: NaiveDate,
    /// Units held at the close of the **last cum-dividend day** (the day
    /// before the ex-date), which is what entitles a holder — expressed in the
    /// unit basis the per-unit figure came in.
    pub units_held: Decimal,
    pub amount_per_unit: Decimal,
    /// `amount_per_unit × units_held`, in `currency` — the gross the missing
    /// income row would carry.
    pub expected_amount: Decimal,
    pub currency: CurrencyCode,
    /// The date the distribution is expected to have been **paid** by: the
    /// ex-date plus the longest ex-date-to-payment gap this listing's entered
    /// distributions show (see [`expected_payment_lag_days`]). The calendar
    /// stores no payment date, so this is the listing's own record standing in
    /// for one.
    pub expected_payment_by: NaiveDate,
    /// True while `today` is on or before [`Self::expected_payment_by`]: the
    /// registry has not paid yet, so there is no payment advice to enter the
    /// row from and nothing is wrong. The entry is still reported — a
    /// question is never silently retired — but as a "due by" note rather
    /// than a problem.
    pub awaiting_payment: bool,
}

/// How many days after its ex-date a listing's distribution is expected to be
/// paid by: the **longest** ex-date-to-`date_paid` gap among the distributions
/// of that listing already matched to an income row, or
/// [`DIVIDEND_MATCH_AFTER_DAYS`] when none has been — the far edge of the
/// window a payment is matched in at all, so a listing with no history is
/// never judged late before its payment could have been recognised.
///
/// The longest rather than the typical gap, because the cost of the two errors
/// is not symmetric: calling a distribution late a few days early is a false
/// alarm on the banner, which is what teaches a reader to ignore it. Each gap
/// is clamped to the matching window, so one mistyped `date_paid` cannot push
/// every later alert out by a year.
pub(super) fn expected_payment_lag_days(
    lags: &HashMap<ListingId, i64>,
    listing_id: ListingId,
) -> i64 {
    lags.get(&listing_id)
        .copied()
        .unwrap_or(DIVIDEND_MATCH_AFTER_DAYS)
}

/// A recorded distribution whose gross does not match what the provider's
/// per-unit figure implies (see [`db_distribution_calendar`]).
#[derive(utoipa::ToSchema, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DividendAmountMismatch {
    /// The `income` row the alert is about.
    pub income_id: i64,
    pub listing_id: ListingId,
    pub ticker: String,
    pub holding_account_id: HoldingAccountId,
    pub holding_account: String,
    pub ex_date: NaiveDate,
    pub date_paid: NaiveDate,
    pub units_held: Decimal,
    pub amount_per_unit: Decimal,
    pub expected_amount: Decimal,
    /// The row's own gross cash — `franked + unfranked + foreign source`, the
    /// same total the per-share statement check reconciles against. **Gross
    /// only, never the components**: a component split is a matter for the
    /// registry statement, and no external feed is evidence about it.
    pub recorded_amount: Decimal,
    /// `recorded_amount - expected_amount`; negative when less was recorded
    /// than the calendar implies.
    ///
    /// Named `amount_difference` rather than `difference` because the display
    /// kinds are keyed on the column *name* across the whole app, and
    /// `difference` is already the AMIT generation's units figure — one name
    /// cannot be both money and a quantity.
    pub amount_difference: Decimal,
    pub currency: CurrencyCode,
}

/// A stored calendar event with its listing's ticker.
#[derive(sqlx::FromRow)]
pub(super) struct CalendarEventRow {
    listing_id: ListingId,
    ticker: String,
    ex_date: NaiveDate,
    #[sqlx(try_from = "Money")]
    amount_per_unit: Decimal,
    currency: CurrencyCode,
    fetched_at: String,
}

/// A candidate income row for the calendar match: the dates it can be anchored
/// on and the gross it records.
#[derive(sqlx::FromRow)]
pub(super) struct CalendarIncomeRow {
    id: i64,
    listing_id: ListingId,
    holding_account_id: HoldingAccountId,
    date_paid: NaiveDate,
    ex_date: Option<NaiveDate>,
    entitlement_date: Option<NaiveDate>,
    #[sqlx(try_from = "Money")]
    franked_amount: Decimal,
    #[sqlx(try_from = "Money")]
    unfranked_amount: Decimal,
    #[sqlx(try_from = "Money")]
    foreign_source_income: Decimal,
    /// ISO 4217 currency the three component amounts are recorded in. Nothing
    /// ties it to the listing's quote currency — `income.currency` defaults to
    /// `'AUD'` and the entity's write path never consults the listing — so it
    /// has to be read and compared, not assumed. See the amount check in
    /// [`db_distribution_calendar`].
    currency: CurrencyCode,
}

/// One DRP reinvestment's place in the registry's own running cash surplus:
/// the payment it was made on, what it brought forward, and what it carried
/// on (see [`DrpChain`]).
#[derive(sqlx::FromRow)]
pub(super) struct DrpReinvestmentRow {
    listing_id: ListingId,
    holding_account_id: HoldingAccountId,
    /// The reinvestment trade's date — the distribution's **payment** date,
    /// which is a couple of weeks after its own ex-date.
    date: NaiveDate,
    #[sqlx(try_from = "Money")]
    residual_brought_forward: Decimal,
    #[sqlx(try_from = "Money")]
    residual_carried_forward: Decimal,
}

/// The registry's running DRP cash surplus for one (listing, holding account),
/// by payment date — and the question worth asking of it: **did the registry
/// distribute anything in this gap?**
///
/// A DRP advice states the surplus brought forward from the last distribution
/// and the surplus carried to the next, so consecutive advices chain. That
/// chain is a *recorded fact from the registry itself*, and it is far better
/// evidence about whether a distribution happened than an absent income row
/// is: an income row can be missing because it was never entered, but a
/// surplus cannot arrive intact across a distribution that consumed it.
///
/// This is what the alert's one false positive on the real portfolio turned
/// out to be. The provider's stream carried a VDHG event on 2 November 2020;
/// the September advice carried $50.4362 forward and the December advice
/// brought exactly $50.4362 back, so nothing was distributed in between and
/// there was never a row to enter.
pub(super) struct DrpChain {
    /// Ascending by date.
    payments: HashMap<(ListingId, HoldingAccountId), Vec<DrpReinvestmentRow>>,
}

impl DrpChain {
    /// Whether the registry's own surplus arrives **intact** across `ex_date`
    /// — evidence that it distributed nothing there.
    ///
    /// True only when the reinvestments either side of the date exist, hand on
    /// a **non-zero** surplus unchanged, and the later one is too late to be
    /// the reinvestment of the distribution in question. The non-zero
    /// guard is what makes the equality mean something: an account whose
    /// residual handling is `PayOut` carries nothing by design, so its chain
    /// is `0 → 0` across every gap and would otherwise "prove" that no
    /// distribution ever happened anywhere.
    ///
    /// The direction of the risk is stated rather than hidden. Suppressing on
    /// this evidence trades a permanent false positive — which teaches a
    /// reader to ignore the banner, the failure this project guards against
    /// everywhere else — for a false negative that needs the registry to have
    /// distributed while leaving an enrolled holder's DRP surplus untouched.
    fn closes_across(
        &self,
        listing_id: ListingId,
        account_id: HoldingAccountId,
        ex_date: NaiveDate,
    ) -> bool {
        let Some(payments) = self.payments.get(&(listing_id, account_id)) else {
            return false;
        };
        let Some(before) = payments.iter().rev().find(|p| p.date < ex_date) else {
            return false;
        };
        let Some(after) = payments.iter().find(|p| p.date > ex_date) else {
            return false;
        };
        // **The next payment must be too late to be this distribution's own.**
        // A reinvestment lands on its distribution's *payment* date, a couple
        // of weeks after its ex-date — so if `after` is inside the window that
        // decides whether a payment belongs to this ex-date, it may be the
        // reinvestment of the very distribution in question, and its brought-
        // forward is then simply the previous carried-forward *before* this
        // distribution was added to it. Those two always agree, so without
        // this guard the chain would appear to close across every DRP
        // distribution and the alert would be suppressed exactly when it was
        // right — a missing income row for a reinvested distribution, which is
        // the likeliest missing row there is.
        if (after.date - ex_date).num_days() <= DIVIDEND_MATCH_AFTER_DAYS {
            return false;
        }
        before.residual_carried_forward > Decimal::ZERO
            && before.residual_carried_forward == after.residual_brought_forward
    }
}

impl CalendarIncomeRow {
    /// The same gross the entity's own `gross_cash_income` sums — franking
    /// credits are notional and withholdings come *out of* this, so neither
    /// belongs in it.
    fn gross(&self) -> Decimal {
        self.franked_amount + self.unfranked_amount + self.foreign_source_income
    }

    /// How far this row sits from `ex_date`, as the sort key
    /// `(days from the ex-date, dated before it)` — or `None` when no date it
    /// carries falls inside the match window. Lower is a better match, and the
    /// second element is the tie-break: **at equal distance the anchor on or
    /// after the ex-date wins**, because a distribution is always paid after
    /// it goes ex, never before.
    ///
    /// **A payment cannot precede its own ex-date**, so `date_paid` anchors
    /// only from the ex-date forwards, while the two entitlement anchors keep
    /// the full backwards reach [`DIVIDEND_MATCH_BEFORE_DAYS`] exists for. And
    /// where two anchors are equally far away, the one on or after the ex-date
    /// wins, for the same reason.
    ///
    /// Both rules are about a **monthly** payer, whose spacing fits inside the
    /// window ([`DIVIDEND_MATCH_AFTER_DAYS`]). A fund paying ~15 days after
    /// going ex puts the previous month's payment about 14 days *before* an
    /// ex-date and its own about 15 days *after* it — so on absolute distance
    /// the previous month's row was the better match and simply won. The
    /// symptom is one alert saying the wrong thing rather than none: the event
    /// matched, compared its gross against the wrong month's, and reported a
    /// mismatch that was really a mis-assignment. It needs that earlier row to
    /// still be unclaimed, which the provider's own coverage gaps supply —
    /// Yahoo serves 8 of HNDQ's 12 periods (see
    /// `entities::distribution_event`), so an income row whose event the
    /// calendar never knew about is an ordinary thing to meet.
    ///
    /// **Every date the row carries is an anchor, its own `ex_date` included —
    /// none of them is a filter.** An earlier version made `ex_date` decisive
    /// when present (match it exactly or not at all), on the reasoning that it
    /// is the same fact the calendar stores. Run against the real portfolio
    /// that reported 34 distributions missing which were all in fact recorded:
    /// registries label the date inconsistently, and the live database's rows
    /// carry what looks like the **record** date — VDHG's period ending
    /// 31 March 2021 is recorded `ex_date` 2021-04-06 against a true ex-date of
    /// 1 April, five days out across the Easter weekend. Treating a
    /// differently-labelled date as a mismatch excluded the row from matching
    /// altogether, so the event read as missing *and* the row went unclaimed —
    /// a false positive on both halves at once.
    ///
    /// So the nearest anchor wins and an exact `ex_date` simply scores zero,
    /// which no other anchor can beat. 13 of the 47 income rows in the live
    /// database carry no `ex_date` at all, which is why the window over
    /// `entitlement_date` / `date_paid` has to carry the matching regardless.
    fn distance_from(&self, ex_date: NaiveDate) -> Option<(i64, bool)> {
        [self.ex_date, self.entitlement_date]
            .into_iter()
            .flatten()
            .map(|anchor| (anchor, -DIVIDEND_MATCH_BEFORE_DAYS))
            .chain(iter::once((self.date_paid, 0)))
            .filter_map(|(anchor, earliest)| {
                let days = (anchor - ex_date).num_days();
                (earliest..=DIVIDEND_MATCH_AFTER_DAYS)
                    .contains(&days)
                    .then_some((days.abs(), days < 0))
            })
            .min()
    }
}

/// The distribution calendar's two advisory alerts, in one walk: a known
/// distribution the books never recorded, and a recorded one whose gross does
/// not match what the calendar implies.
///
/// **Advisory, and structurally so.** Nothing here can change a tax figure —
/// `reports::tax_report`'s completeness gate stays on recorded facts alone,
/// by decision (REQUIREMENTS, "Deliberately out of scope"). Reading "the
/// provider knows of no ex-date" as "there was no distribution" was checked
/// against an issuer before this was built (REQUIREMENTS, "Coverage settled"):
/// the four HNDQ periods Yahoo omits are the four Betashares' own table prints
/// a bare `-` against.
///
/// Three reads and an in-memory match, rather than a query per event: the
/// calendar, the dividend income rows, and the account names, over one
/// [`HeldTimeline`] the caller's transaction already affords.
///
/// # What "held" means here
///
/// Entitlement runs to the holders at the close of the **day before** the
/// ex-date — that is what an ex-date *is*, and on a T+2 market a purchase made
/// that day is still on the register at the record date. So the alert asks the
/// timeline about `ex_date - 1`, never about the ex-date itself: a Buy dated on
/// the ex-date bought the security without this distribution attached, and
/// counting it would invent an entitlement.
///
/// # What "per unit" means here
///
/// The stored per-unit amount is in the unit basis of its own `fetched_at`,
/// because that is the basis the provider quotes in (see
/// `entities::distribution_event::DistributionEvent::amount_per_unit`). The
/// units it multiplies are therefore requested in that same basis. A total is
/// basis-independent — the two scale inversely — so the product is the right
/// number of dollars whichever basis the pair happens to share, and a split
/// recorded between the ex-date and the fetch cannot skew it.
pub(super) async fn db_distribution_calendar(
    conn: &mut sqlx::SqliteConnection,
    today: NaiveDate,
) -> Result<(Vec<MissingDividendEntry>, Vec<DividendAmountMismatch>), sqlx::Error> {
    let events: Vec<CalendarEventRow> = sqlx::query_as(
        "SELECT de.listing_id AS listing_id, l.ticker AS ticker, de.ex_date AS ex_date, \
                de.amount_per_unit AS amount_per_unit, de.currency AS currency, \
                de.fetched_at AS fetched_at \
         FROM distribution_events de JOIN listings l ON l.id = de.listing_id \
         ORDER BY de.ex_date DESC, de.listing_id",
    )
    .fetch_all(&mut *conn)
    .await?;
    if events.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    // Only `Dividend` rows: an employment-income row is not a distribution of
    // the holding at all, and matching one to an ex-date would be a category
    // error rather than a near miss.
    let income: Vec<CalendarIncomeRow> = sqlx::query_as(
        "SELECT id, listing_id, holding_account_id, date_paid, ex_date, entitlement_date, \
                franked_amount, unfranked_amount, foreign_source_income, currency \
         FROM income WHERE income_type = 'Dividend'",
    )
    .fetch_all(&mut *conn)
    .await?;
    let accounts: HashMap<HoldingAccountId, String> =
        sqlx::query_as::<_, (HoldingAccountId, String)>("SELECT id, name FROM holding_accounts")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    let timeline = HeldTimeline::load_on(&mut *conn).await?;
    // The registry's own running DRP surplus — the strongest recorded evidence
    // there is about whether a distribution happened at all (see [`DrpChain`]).
    // Keyed on the trade's own `trade_type`, not on `income.reinvestment_trade_id`:
    // a DRP trade *is* a reinvestment by identity, and the residual columns are
    // its own. Reading the provenance link instead would make the evidence
    // depend on a column only the reinvest operation ever writes.
    let drp_rows: Vec<DrpReinvestmentRow> = sqlx::query_as(
        "SELECT listing_id, holding_account_id, date, \
                residual_brought_forward, residual_carried_forward \
         FROM trades WHERE trade_type = 'DRP' \
         ORDER BY listing_id, holding_account_id, date",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut chain = DrpChain {
        payments: HashMap::new(),
    };
    for row in drp_rows {
        chain
            .payments
            .entry((row.listing_id, row.holding_account_id))
            .or_default()
            .push(row);
    }

    // Income rows by (listing, account), so the match is run one registered
    // holder at a time — the same listing in two accounts pays two
    // distributions and is entered as two rows.
    let mut by_holder: HashMap<(ListingId, HoldingAccountId), Vec<&CalendarIncomeRow>> =
        HashMap::new();
    for row in &income {
        by_holder
            .entry((row.listing_id, row.holding_account_id))
            .or_default()
            .push(row);
    }

    let (mut missing, mut mismatches) = (Vec::new(), Vec::new());
    // A row is claimed by at most one event: without this a half-yearly payer
    // whose two events fall inside one row's window would report the second as
    // missing *and* match it, saying opposite things about the same fact.
    let mut claimed: HashSet<i64> = HashSet::new();
    // The longest ex-date-to-payment gap per listing among the matched
    // distributions — what each missing entry's due date is read from once the
    // walk has seen every match (see [`expected_payment_lag_days`]).
    let mut payment_lags: HashMap<ListingId, i64> = HashMap::new();
    for event in &events {
        let basis_at = DateTime::parse_from_rfc3339(&event.fetched_at)
            .map_err(|e| {
                sqlx::Error::Decode(
                    format!(
                        "distribution event for listing {} on {} has an unreadable fetched_at \
                         {:?}: {e}",
                        event.listing_id, event.ex_date, event.fetched_at
                    )
                    .into(),
                )
            })?
            .date_naive();
        let cum_date = event.ex_date - Duration::days(1);
        for (account_id, units) in
            timeline.units_by_account_on(event.listing_id, cum_date, basis_at)
        {
            let expected = event.amount_per_unit * units;
            let candidates = by_holder
                .get(&(event.listing_id, account_id))
                .map_or(&[][..], |v| v);
            let matched = candidates
                .iter()
                .filter(|row| !claimed.contains(&row.id))
                .filter_map(|row| row.distance_from(event.ex_date).map(|d| (d, *row)))
                .min_by_key(|(distance, row)| (*distance, row.id));

            let account = accounts
                .get(&account_id)
                .cloned()
                .unwrap_or_else(|| account_id.to_string());
            let Some((_, row)) = matched else {
                // Before reporting a distribution as unrecorded, ask the
                // registry's own books whether it happened: a DRP surplus that
                // arrives intact across the ex-date says nothing was
                // distributed there, which is a recorded fact and beats an
                // external feed's say-so.
                if chain.closes_across(event.listing_id, account_id, event.ex_date) {
                    continue;
                }
                missing.push(MissingDividendEntry {
                    listing_id: event.listing_id,
                    ticker: event.ticker.clone(),
                    holding_account_id: account_id,
                    holding_account: account,
                    ex_date: event.ex_date,
                    units_held: units,
                    amount_per_unit: event.amount_per_unit,
                    expected_amount: expected,
                    currency: event.currency.clone(),
                    // Filled in after the walk, once every match is known.
                    expected_payment_by: event.ex_date,
                    awaiting_payment: false,
                });
                continue;
            };
            claimed.insert(row.id);
            let lag = (row.date_paid - event.ex_date)
                .num_days()
                .clamp(0, DIVIDEND_MATCH_AFTER_DAYS);
            payment_lags
                .entry(event.listing_id)
                .and_modify(|longest| *longest = (*longest).max(lag))
                .or_insert(lag);
            // The row **is** this distribution's record — so it is claimed,
            // and the event is not reported missing — but its gross can only
            // be compared against the calendar's when the two are in the same
            // currency. `expected` is `amount_per_unit × units` in the
            // listing's quote currency (what the calendar stores, enforced at
            // write time), while `recorded` is in whatever the income row
            // says; a USD-quoted holding whose distribution was entered in AUD
            // would otherwise subtract one from the other and alert forever on
            // the exchange rate. Neither side is AUD-converted to rescue the
            // comparison: the event carries no `fx_rate`, and converting it
            // would put a tax-strict figure on a monthly reference rate for an
            // advisory alert.
            if !row.currency.eq_ignore_ascii_case(&event.currency) {
                continue;
            }
            let recorded = row.gross();
            let difference = recorded - expected;
            let tolerance = (expected.abs() * Decimal::from(DIVIDEND_AMOUNT_TOLERANCE_PERCENT)
                / Decimal::ONE_HUNDRED)
                .max(Decimal::from(DIVIDEND_AMOUNT_TOLERANCE_FLOOR));
            if difference.abs() > tolerance {
                mismatches.push(DividendAmountMismatch {
                    income_id: row.id,
                    listing_id: event.listing_id,
                    ticker: event.ticker.clone(),
                    holding_account_id: account_id,
                    holding_account: account,
                    ex_date: event.ex_date,
                    date_paid: row.date_paid,
                    units_held: units,
                    amount_per_unit: event.amount_per_unit,
                    expected_amount: expected,
                    recorded_amount: recorded,
                    amount_difference: difference,
                    currency: event.currency.clone(),
                });
            }
        }
    }
    for entry in &mut missing {
        entry.expected_payment_by = entry.ex_date
            + Duration::days(expected_payment_lag_days(&payment_lags, entry.listing_id));
        entry.awaiting_payment = today <= entry.expected_payment_by;
    }
    Ok((missing, mismatches))
}
