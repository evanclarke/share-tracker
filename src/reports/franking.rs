//! Franking-credit entitlement: the at-risk holding-period rule.
//!
//! To claim the franking credits attached to a dividend, the shares must be
//! held at risk for at least 45 days (90 for preference shares), not counting
//! the acquisition or disposal day, within the qualification period ending 45
//! (or 90) days after the shares go ex-dividend. Where parcels were acquired
//! at different times the ATO mandates **last-in first-out** identification of
//! which shares were sold — regardless of the specific parcel allocation
//! chosen for CGT. See `docs/ato/you-and-your-shares-dividends.md` (Examples 6–7).
//!
//! The tax summary loads every listing's walk inputs once via
//! [`HoldingWalks::load`] (inside its own read transaction, so the walks see
//! the same snapshot as the rest of the report) and runs
//! [`HoldingWalks::test`] per dividend to work out how many entitled units
//! fail the test, denying the proportional share of the attached credits —
//! unless the small-shareholder exemption applies (total attached franking
//! credits in the income year below A$5,000).

use crate::domain::tax_year::tax_year_for;
use crate::entities::corporate_action::{self, SplitEvent};
use crate::entities::income::Income;
use crate::entities::trade::TradeType;
use crate::infra::decimal::parse_dec;
use crate::infra::fx::{FxOverride, FxRates};
use chrono::{Datelike, Duration, NaiveDate};
use rust_decimal::Decimal;
use sqlx::{Row, SqliteConnection};
use std::collections::HashMap;

/// Small-shareholder exemption threshold (A$): with total franking credits in
/// an income year below this, the holding-period rule doesn't apply at all.
pub fn small_shareholder_threshold_aud() -> Decimal {
    Decimal::from(5000)
}

/// Outcome of the holding-period test for one dividend on one listing.
#[derive(Debug, PartialEq)]
pub struct HoldingPeriodTest {
    /// Units entitled to the dividend: held when the shares went ex-dividend.
    pub entitled_units: Decimal,
    /// Entitled units disposed of (under LIFO identification) without reaching
    /// the required at-risk days — their share of the credits is not claimable.
    pub disqualified_units: Decimal,
}

impl HoldingPeriodTest {
    /// The non-claimable portion of `credits`:
    /// credits × disqualified / entitled (multiply first to preserve precision).
    pub fn denied(&self, credits: Decimal) -> Decimal {
        if self.entitled_units.is_zero() || self.disqualified_units.is_zero() {
            return Decimal::ZERO;
        }
        credits * self.disqualified_units / self.entitled_units
    }
}

/// One trade in a listing's walk history, its quantity in its own day's unit
/// basis (re-based per dividend at walk time).
#[derive(Clone, Copy)]
struct Event {
    sell: bool,
    date: NaiveDate,
    qty: Decimal,
}

/// One listing's walk inputs: the preference flag (90 vs 45 required at-risk
/// days), its trade history in (date, id) order, and its split events.
struct ListingWalk {
    preference: bool,
    events: Vec<Event>,
    splits: Vec<SplitEvent>,
}

/// Every listing's holding-period-walk inputs, loaded once by
/// [`HoldingWalks::load`] so a report testing many dividends batches the
/// per-listing lookups (preference, trades, splits) instead of re-querying
/// per dividend — and, because it loads on the caller's connection, reads
/// them from the same snapshot as the rest of the report's inputs.
pub struct HoldingWalks {
    listings: HashMap<i64, ListingWalk>,
}

impl HoldingWalks {
    /// Load every listing's walk inputs in three batched queries. Callers run
    /// this inside their read transaction, so a trade committed after the
    /// snapshot can never change a denial computed from the snapshot's facts.
    pub async fn load(conn: &mut SqliteConnection) -> Result<Self, sqlx::Error> {
        let mut listings = sqlx::query("SELECT id, preference FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .iter()
            .map(|row| {
                Ok((
                    row.try_get("id")?,
                    ListingWalk {
                        preference: row.try_get("preference")?,
                        events: Vec::new(),
                        splits: Vec::new(),
                    },
                ))
            })
            .collect::<Result<HashMap<i64, ListingWalk>, sqlx::Error>>()?;

        // A demerger never disposes of the head shares — the closing Sell and
        // head replacement Buys (demerger-group trades on the action's own head
        // listing) are a modelling artifact, so they are excluded here: the
        // original acquisition parcels stay in the stack with their at-risk days
        // running, and a dividend going ex shortly after the demerger is not
        // spuriously disqualified. The demerged-entity Buys (group trades on the
        // *other* listing) are included — they are the only record of those
        // holdings. A holding-account transfer likewise never changes beneficial
        // ownership, so its transfer-out Sell / transfer-in Buys are excluded the
        // same way: the original parcels keep their at-risk clock running.
        let rows = sqlx::query(
            "SELECT t.listing_id, t.trade_type, t.date, t.quantity FROM trades t \
             LEFT JOIN corporate_actions ca ON ca.id = t.demerger_action_id \
             WHERE (t.demerger_action_id IS NULL OR ca.listing_id <> t.listing_id) \
               AND t.transfer_id IS NULL \
             ORDER BY t.date, t.id",
        )
        .fetch_all(&mut *conn)
        .await?;
        for row in &rows {
            let listing_id: i64 = row.try_get("listing_id")?;
            let trade_type: TradeType = row.try_get("trade_type")?;
            let event = Event {
                sell: trade_type == TradeType::Sell,
                date: row.try_get("date")?,
                qty: parse_dec("quantity", row.try_get("quantity")?)?,
            };
            if let Some(l) = listings.get_mut(&listing_id) {
                l.events.push(event);
            }
        }

        for (listing_id, splits) in corporate_action::db_share_split_events(&mut *conn).await? {
            if let Some(l) = listings.get_mut(&listing_id) {
                l.splits = splits;
            }
        }

        Ok(Self { listings })
    }

    /// At-risk days the listing's shares must accrue to keep the credits:
    /// 90 for preference shares, 45 otherwise.
    pub fn required_days(&self, listing_id: i64) -> i64 {
        match self.listings.get(&listing_id) {
            Some(l) if l.preference => 90,
            _ => 45,
        }
    }

    /// Run the holding-period test for a dividend on `listing_id` whose shares
    /// went ex-dividend on `ex_date` (the caller resolves it via
    /// `Income::ex_or_pay_date`, which falls back to the payment date when no
    /// ex-date was recorded).
    ///
    /// Walks the listing's trade history (trade dates; LIFO stack of acquisition
    /// parcels). Units held when the shares go ex-dividend are the entitled units;
    /// an entitled unit sold within the qualification period with fewer than the
    /// required at-risk days (both the acquisition and disposal day excluded) is
    /// disqualified. Anything still held at the end of the qualification period
    /// has necessarily accrued the required days, so later sales are irrelevant.
    pub fn test(&self, listing_id: i64, ex_date: NaiveDate) -> HoldingPeriodTest {
        self.test_with_sale(listing_id, ex_date, None)
    }

    /// [`Self::test`] with an optional contemplated Sell of `(date, units)`
    /// injected into the walk — the franking at-risk report's what-if mode,
    /// answering "which credits would *this* sale disqualify?" without
    /// writing anything. The hypothetical sale takes its place in date order
    /// after any recorded trades of the same day (as the newest trade id
    /// would), with its units in its own date's basis; a sale dated after the
    /// qualification window is ignored — it can no longer affect entitlement.
    pub fn test_with_sale(
        &self,
        listing_id: i64,
        ex_date: NaiveDate,
        contemplated_sale: Option<(NaiveDate, Decimal)>,
    ) -> HoldingPeriodTest {
        // A listing with no loaded walk holds nothing: nothing entitled,
        // nothing disqualified (income rows FK to listings, so this is only
        // reachable with a made-up what-if listing id).
        let Some(listing) = self.listings.get(&listing_id) else {
            return HoldingPeriodTest {
                entitled_units: Decimal::ZERO,
                disqualified_units: Decimal::ZERO,
            };
        };
        let required_days = self.required_days(listing_id);
        // Qualification period ends `required_days` after the ex-dividend day;
        // trades beyond it cannot affect entitlement.
        let window_end = ex_date + Duration::days(required_days);

        let in_window = listing.events.partition_point(|e| e.date <= window_end);
        let mut events = listing.events[..in_window].to_vec();
        if let Some((date, units)) = contemplated_sale
            && date <= window_end
        {
            // After same-day recorded trades, where the newest id would sort.
            let pos = events.partition_point(|e| e.date <= date);
            events.insert(
                pos,
                Event {
                    sell: true,
                    date,
                    qty: units,
                },
            );
        }

        struct Parcel {
            acquired: NaiveDate,
            qty: Decimal,
            entitled: bool,
        }
        let mut stack: Vec<Parcel> = Vec::new();
        let mut snapshotted = false;
        let mut entitled_units = Decimal::ZERO;
        let mut disqualified_units = Decimal::ZERO;

        for event in &events {
            let date = event.date;
            // Each trade's quantity is in its own day's unit basis; a share
            // split/consolidation (TD 2000/10) between trades would make them
            // incomparable, so every quantity is normalised to the basis at the
            // end of the qualification window. The entitled/disqualified ratio
            // (all the caller uses) is basis-invariant.
            let qty = corporate_action::split_adjusted_quantity(
                event.qty,
                &listing.splits,
                date,
                Some(window_end),
            );

            // Everything still held immediately before the first trade on or after
            // the ex-date is entitled to the dividend (acquiring on or after the
            // ex-date no longer carries the entitlement).
            if !snapshotted && date >= ex_date {
                for p in &mut stack {
                    p.entitled = true;
                    entitled_units += p.qty;
                }
                snapshotted = true;
            }

            if event.sell {
                // LIFO: a sale disposes of the most recently acquired shares first.
                let mut remaining = qty;
                while remaining > Decimal::ZERO {
                    let Some(top) = stack.last_mut() else { break };
                    let take = remaining.min(top.qty);
                    if top.entitled {
                        // At-risk days exclude both the acquisition and disposal day.
                        let at_risk = (date - top.acquired).num_days() - 1;
                        if at_risk < required_days {
                            disqualified_units += take;
                        }
                    }
                    top.qty -= take;
                    remaining -= take;
                    if top.qty.is_zero() {
                        stack.pop();
                    }
                }
            } else {
                // Buy or DRP: a new acquisition parcel (entitled only if it is
                // still held when the snapshot above fires).
                stack.push(Parcel {
                    acquired: date,
                    qty,
                    entitled: false,
                });
            }
        }
        if !snapshotted {
            // No trade on or after the ex-date: everything held is entitled (and
            // qualified — nothing was sold within the qualification period).
            entitled_units = stack.iter().map(|p| p.qty).sum();
        }

        HoldingPeriodTest {
            entitled_units,
            disqualified_units,
        }
    }
}

/// One franked dividend (an `income` row with attached credits) as the
/// holding-period rule sees it. Loaded by [`db_franked_dividends`], which the
/// tax summary (denying the credits) and the franking at-risk report
/// (explaining the denial) share — so the two can never disagree about which
/// dividends are candidates or what year/amount they count for.
pub struct FrankedDividend {
    /// The `income` row id.
    pub income_id: i64,
    pub tax_year: i32,
    pub listing_id: i64,
    pub date_paid: NaiveDate,
    /// Ex-dividend date; falls back to the payment date when not recorded
    /// (a dividend is never paid before its shares go ex-dividend).
    pub ex_date: NaiveDate,
    /// Attached franking credits, converted to AUD at the assessment month.
    pub credits_aud: Decimal,
}

/// Load every franked dividend (income rows with attached credits, assessed
/// per the tax summary's year-attribution rules) and each financial year's
/// total attached credits in AUD for the small-shareholder exemption test.
/// AMMA franking credits count toward the year's attached total but are never
/// themselves candidates: the holding-period rule needs a per-distribution
/// ex-date, which an annual AMMA statement doesn't carry.
pub async fn db_franked_dividends(
    conn: &mut SqliteConnection,
    fx: &FxRates,
) -> Result<(Vec<FrankedDividend>, HashMap<i32, Decimal>), sqlx::Error> {
    let mut dividends = Vec::new();
    let mut attached_by_year: HashMap<i32, Decimal> = HashMap::new();

    // AMIT listings' cash rows carry no attributable credits (write-time
    // validated; the fund's credits arrive via its AMMA statement below), so
    // they are excluded here just as the tax summary excludes them.
    let income_rows: Vec<Income> = sqlx::query_as(
        "SELECT i.* FROM income i JOIN listings l ON l.id = i.listing_id \
         WHERE NOT l.amit",
    )
    .fetch_all(&mut *conn)
    .await?;
    for income in &income_rows {
        // Assessed per `Income::assessment_date` — present entitlement for a
        // trust row, payment otherwise — the same attribution the tax summary
        // applies to every component.
        let assessed = income.assessment_date();
        let credits_aud = fx.to_aud(
            income.franking_credits,
            &income.currency,
            assessed,
            FxOverride::None,
        )?;
        if credits_aud <= Decimal::ZERO {
            continue;
        }
        let tax_year = tax_year_for(assessed);
        dividends.push(FrankedDividend {
            income_id: income.id,
            tax_year,
            listing_id: income.listing_id,
            date_paid: income.date_paid,
            ex_date: income.ex_or_pay_date(),
            credits_aud,
        });
        *attached_by_year.entry(tax_year).or_default() += credits_aud;
    }

    let amma_rows =
        sqlx::query("SELECT tax_year_end_date, franking_credits, currency FROM amma_statements")
            .fetch_all(&mut *conn)
            .await?;
    for row in &amma_rows {
        let tax_year_end_date: NaiveDate = row.try_get("tax_year_end_date")?;
        let currency: String = row.try_get("currency")?;
        let credits = parse_dec("franking_credits", row.try_get("franking_credits")?)?;
        let credits_aud = fx.to_aud(credits, &currency, tax_year_end_date, FxOverride::None)?;
        *attached_by_year
            .entry(tax_year_end_date.year())
            .or_default() += credits_aud;
    }

    Ok((dividends, attached_by_year))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{listing, trade};
    use crate::test_support::{self, test_pool};
    use sqlx::SqlitePool;

    /// Load the walk inputs once, as the reports do — tests then run one or
    /// more in-memory walks against the same load.
    async fn walks(pool: &SqlitePool) -> HoldingWalks {
        let mut conn = pool.acquire().await.unwrap();
        HoldingWalks::load(&mut conn).await.unwrap()
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, preference: bool) {
        test_support::listing(id)
            .ticker(&format!("TST{id}"))
            .security_type(listing::SecurityType::Share)
            .preference(preference)
            .insert(pool)
            .await;
    }

    async fn insert_trade(
        pool: &SqlitePool,
        id: i64,
        listing_id: i64,
        trade_type: trade::TradeType,
        date: NaiveDate,
        qty: i64,
    ) {
        test_support::trade(id, listing_id, trade_type)
            .date(date)
            .qty(Decimal::from(qty))
            .price(Decimal::ONE)
            .insert(pool)
            .await;
    }

    fn d(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn db_parcel_held_through_qualification_period_qualifies() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // Acquired the day before ex-date and never sold: entitled and qualified.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2025-03-13"), 100).await;
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(100));
        assert_eq!(t.disqualified_units, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_sale_under_45_at_risk_days_disqualifies() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // Matthew (Example 6) shape: acquired 1 Mar, sold 10 Apr — 39 at-risk
        // days (both end days excluded), under 45.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2025-03-01"), 1000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, d("2025-04-10"), 1000).await;
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(1000));
        assert_eq!(t.disqualified_units, Decimal::from(1000));
        assert_eq!(t.denied(Decimal::from(5600)), Decimal::from(5600));
    }

    #[tokio::test]
    async fn db_at_risk_days_exclude_acquisition_and_disposal_days() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // Bought 1 Mar. Sold 46 days later (16 Apr): exactly 45 at-risk days → qualifies.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2025-03-01"), 100).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, d("2025-04-16"), 100).await;
        let t = walks(&pool).await.test(1, d("2025-03-02"));
        assert_eq!(t.disqualified_units, Decimal::ZERO);

        // Same shape sold one day earlier (44 at-risk days) → disqualified.
        insert_listing(&pool, 2, false).await;
        insert_trade(&pool, 3, 2, trade::TradeType::Buy, d("2025-03-01"), 100).await;
        insert_trade(&pool, 4, 2, trade::TradeType::Sell, d("2025-04-15"), 100).await;
        let t = walks(&pool).await.test(2, d("2025-03-02"));
        assert_eq!(t.disqualified_units, Decimal::from(100));
    }

    /// A share split between the buy and the ex-date (TD 2000/10) re-bases the
    /// pre-split parcel so post-split sales compare in the same units; the
    /// original acquisition date keeps counting at-risk days.
    #[tokio::test]
    async fn db_split_between_buy_and_ex_date_compares_in_one_basis() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // 100 bought long before the ex-date; a 2-for-1 split, then 200
        // post-split units sold 20 days after the ex-date. The whole holding
        // is entitled (200 post-split) and fully qualified — the at-risk clock
        // runs from the 2024 acquisition, not the split.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2024-03-14"), 100).await;
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: d("2025-01-10"),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        insert_trade(&pool, 2, 1, trade::TradeType::Sell, d("2025-04-03"), 200).await;
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(200));
        // Held since 2024 → well over 45 at-risk days: nothing disqualified.
        assert_eq!(t.disqualified_units, Decimal::ZERO);

        // The same sale against a parcel bought 10 days before the ex-date
        // (post-split basis) is disqualified — proving the basis lines up.
        insert_listing(&pool, 2, false).await;
        insert_trade(&pool, 3, 2, trade::TradeType::Buy, d("2025-03-04"), 100).await;
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 2,
                listing_id: 2,
                date: d("2025-03-10"),
                kind: crate::entities::corporate_action::ActionKind::ShareSplit {
                    split_new_units: Decimal::from(2),
                    split_old_units: Decimal::ONE,
                },
            },
        )
        .await
        .unwrap();
        insert_trade(&pool, 4, 2, trade::TradeType::Sell, d("2025-04-03"), 200).await;
        let t = walks(&pool).await.test(2, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(200));
        assert_eq!(t.disqualified_units, Decimal::from(200));
    }

    #[tokio::test]
    async fn db_lifo_identifies_most_recent_parcel_as_sold() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // Jessica (Example 7) shape: 10,000 held 12 months, 4,000 bought 10
        // days before ex-date, 4,000 sold 20 days after. LIFO deems the recent
        // 4,000 sold (29 at-risk days < 45) — the long-held 10,000 qualify.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2024-03-14"), 10000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Buy, d("2025-03-04"), 4000).await;
        insert_trade(&pool, 3, 1, trade::TradeType::Sell, d("2025-04-03"), 4000).await;
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(14000));
        assert_eq!(t.disqualified_units, Decimal::from(4000));
        // Denied proportionally, multiply-before-divide: 7000 × 4000 / 14000 = 2000 exact.
        assert_eq!(t.denied(Decimal::from(7000)), Decimal::from(2000));
    }

    /// One [`HoldingWalks::load`] answers every dividend of a listing: two
    /// ex-dates run against the same loaded inputs, each with its own
    /// entitlement snapshot and qualification window.
    #[tokio::test]
    async fn db_one_load_answers_multiple_dividends_on_one_listing() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // Long-held 10,000; 4,000 bought 10 days before the March ex-date and
        // sold 20 days after it (29 at-risk days).
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2024-03-14"), 10000).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Buy, d("2025-03-04"), 4000).await;
        insert_trade(&pool, 3, 1, trade::TradeType::Sell, d("2025-04-03"), 4000).await;

        let w = walks(&pool).await;
        // August 2024 ex-date: only the long-held parcel is entitled, and the
        // 2025 sale is far outside its qualification window — fully qualified.
        let aug = w.test(1, d("2024-08-14"));
        assert_eq!(aug.entitled_units, Decimal::from(10000));
        assert_eq!(aug.disqualified_units, Decimal::ZERO);
        // March 2025 ex-date (Jessica's shape) from the same load: the recent
        // 4,000 are deemed sold under LIFO and disqualified.
        let mar = w.test(1, d("2025-03-14"));
        assert_eq!(mar.entitled_units, Decimal::from(14000));
        assert_eq!(mar.disqualified_units, Decimal::from(4000));
    }

    #[tokio::test]
    async fn db_preference_shares_require_90_at_risk_days() {
        let pool = test_pool().await;
        // Same trades on an ordinary and a preference listing: 69 at-risk days
        // passes the 45-day test but fails the 90-day preference test.
        for (listing_id, preference) in [(1, false), (2, true)] {
            insert_listing(&pool, listing_id, preference).await;
            let base = (listing_id - 1) * 10;
            insert_trade(
                &pool,
                base + 1,
                listing_id,
                trade::TradeType::Buy,
                d("2025-03-04"),
                100,
            )
            .await;
            insert_trade(
                &pool,
                base + 2,
                listing_id,
                trade::TradeType::Sell,
                d("2025-05-13"),
                100,
            )
            .await;
        }
        let ordinary = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(ordinary.disqualified_units, Decimal::ZERO);
        let preference = walks(&pool).await.test(2, d("2025-03-14"));
        assert_eq!(preference.disqualified_units, Decimal::from(100));
    }

    #[tokio::test]
    async fn db_pre_ex_date_sales_reduce_entitled_units() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // The 50 bought and re-sold before the ex-date never carry the
        // entitlement; the long-held 100 remain entitled and qualified.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2024-01-10"), 100).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Buy, d("2025-03-09"), 50).await;
        insert_trade(&pool, 3, 1, trade::TradeType::Sell, d("2025-03-12"), 50).await;
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(100));
        assert_eq!(t.disqualified_units, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_post_ex_date_buys_absorb_sales_under_lifo_first() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // The sale matches the (unentitled) parcel bought after the ex-date
        // under LIFO, so the entitled long-held parcel is untouched.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2024-01-10"), 100).await;
        insert_trade(&pool, 2, 1, trade::TradeType::Buy, d("2025-03-19"), 50).await;
        insert_trade(&pool, 3, 1, trade::TradeType::Sell, d("2025-03-24"), 50).await;
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(100));
        assert_eq!(t.disqualified_units, Decimal::ZERO);
    }

    #[tokio::test]
    async fn db_drp_acquisitions_count_as_parcels() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        // A DRP parcel acquired shortly before the ex-date and sold (LIFO)
        // 30 days later is disqualified like any other recent acquisition.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2024-01-10"), 100).await;
        insert_trade(&pool, 2, 1, trade::TradeType::DRP, d("2025-03-04"), 10).await;
        insert_trade(&pool, 3, 1, trade::TradeType::Sell, d("2025-04-03"), 10).await;
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(110));
        assert_eq!(t.disqualified_units, Decimal::from(10));
    }

    /// A demerger never disposes of the head shares: the demerge's closing
    /// Sell and head replacement Buys are excluded from the walk, so a
    /// dividend going ex shortly before the demerger is not spuriously
    /// disqualified — the original parcel's at-risk days keep running. The
    /// demerged-entity Buys are included: they are the only record of those
    /// holdings.
    #[tokio::test]
    async fn db_demerger_artifact_trades_keep_at_risk_days_running() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, false).await;
        insert_listing(&pool, 2, false).await;
        // Bought 1 Mar, ex-dividend 14 Mar, demerger 1 Apr — inside the
        // qualification window with only 30 at-risk days at the demerge.
        insert_trade(&pool, 1, 1, trade::TradeType::Buy, d("2025-03-01"), 1000).await;
        crate::entities::corporate_action::db_upsert(
            &pool,
            &crate::entities::corporate_action::CorporateAction {
                id: 10,
                listing_id: 1,
                date: d("2025-04-01"),
                kind: crate::entities::corporate_action::ActionKind::Demerger {
                    demerger_listing_id: 2,
                    demerger_new_units: Decimal::ONE,
                    demerger_held_units: Decimal::from(5),
                    demerger_cost_base_pct: "5.063".parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();
        crate::entities::demerger::db_demerge(&pool, 10)
            .await
            .unwrap();

        // The head shares were never disposed of: nothing is disqualified.
        let t = walks(&pool).await.test(1, d("2025-03-14"));
        assert_eq!(t.entitled_units, Decimal::from(1000));
        assert_eq!(t.disqualified_units, Decimal::ZERO);

        // The demerged listing's walk sees the demerged-entity parcel.
        let t = walks(&pool).await.test(2, d("2025-05-10"));
        assert_eq!(t.entitled_units, Decimal::from(200));
        assert_eq!(t.disqualified_units, Decimal::ZERO);
    }

    #[test]
    fn denied_is_zero_without_entitled_or_disqualified_units() {
        // No recorded holdings at the ex-date: the rule can't be evaluated, so
        // nothing is denied (and there is no division by zero).
        let t = HoldingPeriodTest {
            entitled_units: Decimal::ZERO,
            disqualified_units: Decimal::ZERO,
        };
        assert_eq!(t.denied(Decimal::from(100)), Decimal::ZERO);
        let t = HoldingPeriodTest {
            entitled_units: Decimal::from(100),
            disqualified_units: Decimal::ZERO,
        };
        assert_eq!(t.denied(Decimal::from(100)), Decimal::ZERO);
    }
}
