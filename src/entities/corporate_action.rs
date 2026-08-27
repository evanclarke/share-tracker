//! Corporate actions recorded against a listing.
//!
//! The action types modelled so far:
//!
//! **ReturnOfCapital** — a non-assessable payment from a company (a
//! shareholder-approved return of share capital, CGT event G1; see
//! `docs/ato/cgt-non-assessable-payments.md`). The per-unit payment reduces the
//! cost base of every parcel of the listing held on the payment date (units
//! sold before the payment were not held for it, so they are unaffected).
//! Where cumulative payments exceed a parcel's per-unit cost base, the cost
//! base floors at nil and the excess is an immediate capital gain in the
//! payment's income year — G1 can never produce a capital loss — computed by
//! the net-capital-gain report (`g1_gains`). Distinct from the AMIT
//! tax-deferred regime (CGT event E10, `amit_adjustment`), which applies to
//! trust units, not company shares.
//!
//! **ShareSplit** — a share split or consolidation (TD 2000/10; see
//! `docs/ato/share-splits-and-consolidations.md`): on the conversion date every
//! `split_old_units` units become `split_new_units` units (a 2-for-1 split is
//! new=2/old=1; a 1-for-10 consolidation is new=1/old=10). No CGT event
//! happens: the converted parcel keeps its total cost base and its original
//! acquisition date — only the unit count (and so the per-unit cost base)
//! changes. Trade rows keep the quantities as originally transacted; reports
//! and write-time allocation checks re-base quantities between unit bases via
//! [`split_adjusted_quantity`] / [`as_acquired_quantity`].
//! A trade dated on the conversion date is already in post-split units.
//!
//! **BonusIssue** — a non-assessable bonus share issue (the general
//! post-1 July 1998 case; see `docs/ato/bonus-shares.md`): on the issue date
//! every `bonus_held_units` units held receive `bonus_units` additional
//! units. The ATO apportions the parcel's cost base over original + bonus
//! shares and the bonus shares take the original acquisition date — exactly
//! the quantity re-base `(held + bonus) / held` with total cost base and
//! acquisition date preserved, so a BonusIssue folds into the split-event
//! stream as its equivalent split (new = held + bonus, old = held) and every
//! report and write-time check inherits the treatment. A trade dated on the
//! issue date is ex-bonus. (Bonus shares chosen *in lieu of a dividend* are
//! assessed as a dividend — entered as a DRP trade, not as this action.)
//!
//! **RightsIssue** — rights to acquire new shares, issued free to existing
//! holders (the dominant retail case; see `docs/ato/rights-issues.md`): on the
//! record `date` every `rights_held_units` units held entitle the holder to
//! acquire `rights_units` new units at `exercise_price` per unit in
//! `currency` (a trade dated on the record date is ex-rights). Recording the
//! action changes nothing by itself — free rights are non-assessable
//! non-exempt income on issue. Exercising it (`POST
//! /corporate_actions/:id/exercise`, `entities::rights_exercise`) creates a
//! new Buy parcel dated the exercise date — no CGT event, the 12-month
//! discount clock runs from exercise — whose cost base is the amount paid to
//! exercise plus any amount paid to acquire the rights. Cumulative exercised
//! units are capped at the entitlement, so an action referenced by exercise
//! trades cannot be edited or deleted (delete the exercise trades first).
//! Selling or letting the rights themselves lapse is not modelled.
//!
//! **BuyBack** — an off-market share buy-back (see `docs/ato/share-buy-backs.md`):
//! the company offers to buy back shares directly from holders. The action
//! records the offer terms: on/after the buy-back `date`, each unit bought
//! back is paid `buyback_price` in `currency`, of which `buyback_dividend` is
//! an assessable franked dividend carrying `buyback_franking_credit` (both 0
//! for a listed-company buy-back announced after 7:30 pm AEDT 25 Oct 2022 —
//! no dividend component), and `buyback_market_value` is the per-unit market
//! value had the buy-back not been proposed (capital proceeds can't be less
//! than it; `None` when the price is at or above market value). Recording the
//! action changes nothing by itself; participating (`POST
//! /corporate_actions/:id/participate`, `entities::buyback_participation`)
//! atomically creates the Sell trade — per-unit price = capital proceeds per
//! unit = `max(price, market value) − dividend` — with its parcel
//! allocations, plus the dividend-component income row when there is one.
//! An action referenced by participation trades is frozen against edits.
//!
//! **ScripForScrip** — a takeover or merger completed as a scrip exchange
//! with scrip-for-scrip rollover (Subdiv 124-M; see
//! `docs/ato/takeovers-and-scrip-for-scrip.md`): on the exchange `date` every
//! `scrip_old_units` units of the original (target) listing become
//! `scrip_new_units` units of `scrip_listing_id` (the replacement listing),
//! plus — the partial-rollover case (Example 27) — an optional
//! `scrip_cash_per_unit` cash per old unit (with `scrip_market_value`, the
//! replacement unit's market value just after issue, and
//! `scrip_cash_currency`; the three are all present or all absent).
//! Recording the action changes nothing by itself; exchanging (`POST
//! /corporate_actions/:id/exchange`, `entities::scrip_exchange`) atomically
//! creates a closing Sell on the original listing consuming every open
//! parcel — at zero proceeds and excluded from the realised-gains and
//! net-capital-gain reports when all-scrip (the rollover disregards the
//! capital gain); at the cash per-unit price when there is a cash component,
//! whose gain over the cash-apportioned cost base those reports assess now —
//! plus one replacement Buy per consumed parcel carrying the parcel's
//! remaining reduced cost base (the scrip-apportioned share when there is
//! cash) and (as `trades.deemed_acquisition_date`) its acquisition date, the
//! rollover's combined-holding-period rule for the 12-month CGT discount.
//! An action referenced by exchange trades is frozen against edits.
//!
//! **Demerger** — an eligible demerger with the Div 125 rollover chosen (see
//! `docs/ato/demergers.md`): on the demerger `date` every `demerger_held_units`
//! units held in the head entity (the action's own `listing_id`) receive
//! `demerger_new_units` units of `demerger_listing_id` (the demerged
//! entity's listing), and `demerger_cost_base_pct` percent of each parcel's
//! cost base is apportioned to the new interests (the head-entity-advised
//! percentage; the head parcels keep the rest). Recording the action changes
//! nothing by itself; demerging (`POST /corporate_actions/:id/demerge`,
//! `entities::demerger`) atomically closes every open head parcel with a
//! zero-proceeds Sell — excluded from the realised-gains and net-capital-gain
//! reports, because the rollover disregards any gain — and recreates it as a
//! head replacement Buy plus a demerged-entity Buy splitting the parcel's
//! remaining reduced cost base by the percentage, both carrying the parcel's
//! acquisition date as `trades.deemed_acquisition_date` (the head dates are
//! unchanged by law; the new interests' 12-month discount clock runs from the
//! original acquisition). An action referenced by demerge trades is frozen
//! against edits.
//!
//! **WorthlessShares** — a capital loss on a failed company without an
//! ordinary sale (CGT events G3 and C2; see `docs/ato/worthless-shares.md`):
//! the action records, against the failed listing, which event the user is
//! invoking (`worthless_event`: `G3Declaration` — a liquidator's/administrator's
//! written declaration of no-likely-distribution, s 104-145/TD 2000/52; or
//! `C2Cancellation` — deregistration cancelling the shares, s 104-25/TD
//! 2000/7), the `date` being the declaration or cancellation date. Recording
//! the action changes nothing by itself; recognising (`POST
//! /corporate_actions/:id/recognise`, `entities::worthless`) atomically closes
//! every open parcel held at the event date through a provenance-marked Sell at
//! **nil proceeds**, each parcel producing a capital loss equal to its
//! remaining reduced cost base. Unlike the scrip-for-scrip and demerger closing
//! Sells, this Sell is **not** excluded from the realised-gains and
//! net-capital-gain reports — its nil proceeds against the cost base
//! *recognise* the loss (never income, never discounted). An action referenced
//! by a recognise Sell is frozen against edits.
//!
//! `ActionKind` is the extension point for future corporate actions, each
//! widening the enum and its CHECK.

mod adjustments;
mod db;
mod http;
mod model;

pub use adjustments::{
    PriceBasisEvent, RocEvent, RolloverOrigin, SplitEvent, as_acquired_quantity,
    checked_as_acquired_quantity, contemporaneous_price, db_demerger_price_statements,
    db_payment_currency_conflict, db_return_of_capital_events, db_share_split_events,
    db_splits_for_listing, per_unit_reduction, sold_in_acquired_units, split_adjusted_quantity,
};
pub use db::{db_get_tx, rebased_quantity_beyond_range};
pub use http::router;
pub use model::{
    ActionKind, CorporateAction, NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS, WorthlessEvent,
};

/// Referenced by name only from other modules' tests (production code calls
/// these through the HTTP routes in `http.rs`, not this re-export), so the
/// re-export is test-gated to keep the non-test build warning-free — same
/// reasoning as `trade.rs`'s `UpsertError` re-export.
#[cfg(test)]
pub use db::{DeleteError, WriteError, db_delete, db_get, db_list, db_upsert};

/// The raw conversion ratio is now used only *inside* `adjustments` — every
/// caller outside it asks a higher-level question instead
/// ([`split_adjusted_quantity`] / [`as_acquired_quantity`] for a quantity,
/// [`RocEvent::per_unit_for`] for a payment) — so the re-export is test-gated
/// for its own unit tests, keeping the non-test build warning-free.
#[cfg(test)]
pub use adjustments::split_ratio;

#[cfg(test)]
use axum::http::StatusCode;
#[cfg(test)]
use chrono::NaiveDate;
#[cfg(test)]
use rust_decimal::Decimal;
#[cfg(test)]
use sqlx::SqlitePool;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::sell::{AllocationInput, SellBody, SellError};
    use crate::entities::{listing, sell};
    use crate::test_support::{self, ApiClient, test_pool};

    /// Client over this module's own routes.
    fn client(pool: &SqlitePool) -> ApiClient {
        ApiClient::over(router().with_state(pool.clone()))
    }

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .security_type(listing::SecurityType::Share)
            .insert(pool)
            .await;
    }

    fn roc(id: i64, listing_id: i64, date: NaiveDate, amount: &str) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::ReturnOfCapital {
                amount_per_unit: amount.parse().unwrap(),
                currency: "AUD".to_string(),
                record_date: None,
            },
        }
    }

    /// [`roc`] carrying the record date that fixed entitlement to it.
    fn roc_with_record(
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        amount: &str,
        record: NaiveDate,
    ) -> CorporateAction {
        CorporateAction {
            kind: ActionKind::ReturnOfCapital {
                amount_per_unit: amount.parse().unwrap(),
                currency: "AUD".to_string(),
                record_date: Some(record),
            },
            ..roc(id, listing_id, date, amount)
        }
    }

    fn split(id: i64, listing_id: i64, date: NaiveDate, new: &str, old: &str) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::ShareSplit {
                split_new_units: new.parse().unwrap(),
                split_old_units: old.parse().unwrap(),
            },
        }
    }

    fn bonus(
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        units: &str,
        held: &str,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::BonusIssue {
                bonus_units: units.parse().unwrap(),
                bonus_held_units: held.parse().unwrap(),
            },
        }
    }

    fn rights(
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        units: &str,
        held: &str,
        price: &str,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::RightsIssue {
                rights_units: units.parse().unwrap(),
                rights_held_units: held.parse().unwrap(),
                exercise_price: price.parse().unwrap(),
                currency: "AUD".to_string(),
                renounceable: true,
            },
        }
    }

    fn buyback(
        id: i64,
        listing_id: i64,
        date: NaiveDate,
        price: &str,
        dividend: &str,
        credit: &str,
        market_value: Option<&str>,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::BuyBack {
                buyback_price: price.parse().unwrap(),
                buyback_dividend: dividend.parse().unwrap(),
                buyback_franking_credit: credit.parse().unwrap(),
                buyback_market_value: market_value.map(|v| v.parse().unwrap()),
                currency: "AUD".to_string(),
            },
        }
    }

    fn scrip(
        id: i64,
        listing_id: i64,
        scrip_listing_id: i64,
        date: NaiveDate,
        new: &str,
        old: &str,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::ScripForScrip {
                scrip_listing_id,
                scrip_new_units: new.parse().unwrap(),
                scrip_old_units: old.parse().unwrap(),
                scrip_cash_per_unit: None,
                scrip_market_value: None,
                scrip_cash_currency: None,
            },
        }
    }

    fn demerger(
        id: i64,
        listing_id: i64,
        demerger_listing_id: i64,
        date: NaiveDate,
        new: &str,
        held: &str,
        pct: &str,
    ) -> CorporateAction {
        CorporateAction {
            id,
            listing_id,
            date,
            kind: ActionKind::Demerger {
                demerger_listing_id,
                demerger_new_units: new.parse().unwrap(),
                demerger_held_units: held.parse().unwrap(),
                demerger_cost_base_pct: pct.parse().unwrap(),
                demerger_close_date: None,
                demerger_close_price: None,
                demerger_close_sourced_from: None,
                demerger_close_reason: None,
            },
        }
    }

    /// A `Demerger` carrying the stated pre-demerger close (and its
    /// provenance) that the price re-base derives its factor from.
    fn demerger_with_close(
        id: i64,
        listing_id: i64,
        demerger_listing_id: i64,
        date: NaiveDate,
        close_date: NaiveDate,
        close: &str,
    ) -> CorporateAction {
        let mut action = demerger(id, listing_id, demerger_listing_id, date, "1", "1", "36");
        if let ActionKind::Demerger {
            demerger_close_date,
            demerger_close_price,
            demerger_close_sourced_from,
            demerger_close_reason,
            ..
        } = &mut action.kind
        {
            *demerger_close_date = Some(close_date);
            *demerger_close_price = Some(close.parse().unwrap());
            *demerger_close_sourced_from = Some("nyse.com daily close".to_string());
            *demerger_close_reason =
                Some("the provider adjusts the pre-demerger series".to_string());
        }
        action
    }

    fn split_event(date: NaiveDate, new: &str, old: &str) -> SplitEvent {
        SplitEvent::share_split(date, new.parse().unwrap(), old.parse().unwrap())
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // DB-level tests

    #[tokio::test]
    async fn db_insert_and_retrieve_preserves_precision() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        db_upsert(&pool, &roc(1, 1, d(2024, 11, 30), "0.505"))
            .await
            .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(got.date, d(2024, 11, 30));
        assert_eq!(
            got.kind,
            ActionKind::ReturnOfCapital {
                amount_per_unit: "0.505".parse().unwrap(),
                currency: "AUD".to_string(),
                record_date: None,
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_share_split_preserves_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // An uneven ratio (e.g. a 7-for-2 split) must round-trip exactly.
        db_upsert(&pool, &split(1, 1, d(2024, 11, 30), "7", "2"))
            .await
            .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::ShareSplit {
                split_new_units: Decimal::from(7),
                split_old_units: Decimal::from(2),
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_bonus_issue_preserves_ratio() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        // An uneven ratio (e.g. 3 bonus shares per 7 held) must round-trip exactly.
        db_upsert(&pool, &bonus(1, 1, d(2024, 11, 30), "3", "7"))
            .await
            .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BonusIssue {
                bonus_units: Decimal::from(3),
                bonus_held_units: Decimal::from(7),
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_rights_issue_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        // An uneven ratio and a sub-cent price must round-trip exactly.
        db_upsert(&pool, &rights(1, 1, d(2024, 11, 30), "3", "7", "1.805"))
            .await
            .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::RightsIssue {
                rights_units: Decimal::from(3),
                rights_held_units: Decimal::from(7),
                exercise_price: "1.805".parse().unwrap(),
                currency: "AUD".to_string(),
                renounceable: true,
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_buy_back_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        // Sub-cent per-unit components must round-trip exactly.
        db_upsert(
            &pool,
            &buyback(
                1,
                1,
                d(2024, 11, 30),
                "9.60",
                "1.405",
                "0.605",
                Some("10.20"),
            ),
        )
        .await
        .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BuyBack {
                buyback_price: "9.60".parse().unwrap(),
                buyback_dividend: "1.405".parse().unwrap(),
                buyback_franking_credit: "0.605".parse().unwrap(),
                buyback_market_value: Some("10.20".parse().unwrap()),
                currency: "AUD".to_string(),
            }
        );

        // The market value is optional: absent round-trips as None.
        db_upsert(
            &pool,
            &buyback(2, 1, d(2024, 12, 31), "5.00", "0", "0", None),
        )
        .await
        .unwrap();
        let got = db_get(&pool, 2).await.unwrap().unwrap();
        assert!(matches!(
            got.kind,
            ActionKind::BuyBack {
                buyback_market_value: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_scrip_for_scrip_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // An uneven exchange ratio (e.g. 3 new shares per 7 old) must
        // round-trip exactly.
        db_upsert(&pool, &scrip(1, 1, 2, d(2024, 11, 30), "3", "7"))
            .await
            .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(
            got.kind,
            ActionKind::ScripForScrip {
                scrip_listing_id: 2,
                scrip_new_units: Decimal::from(3),
                scrip_old_units: Decimal::from(7),
                scrip_cash_per_unit: None,
                scrip_market_value: None,
                scrip_cash_currency: None,
            }
        );
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_demerger_preserves_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // An uneven ratio and a sub-unit percentage (BHP Steel's 5.063%) must
        // round-trip exactly.
        db_upsert(
            &pool,
            &demerger(1, 1, 2, d(2024, 11, 30), "1", "5", "5.063"),
        )
        .await
        .unwrap();

        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(got.listing_id, 1);
        assert_eq!(
            got.kind,
            ActionKind::Demerger {
                demerger_listing_id: 2,
                demerger_new_units: Decimal::ONE,
                demerger_held_units: Decimal::from(5),
                demerger_cost_base_pct: "5.063".parse().unwrap(),
                demerger_close_date: None,
                demerger_close_price: None,
                demerger_close_sourced_from: None,
                demerger_close_reason: None,
            }
        );
    }

    /// A Demerger never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel (the demerge
    /// operation does the apportionment).
    #[tokio::test]
    async fn db_demerger_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        db_upsert(
            &pool,
            &demerger(1, 1, 2, d(2024, 11, 30), "1", "5", "5.063"),
        )
        .await
        .unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    /// **The structural point.** A demerger's stated close belongs to the
    /// *price* re-basing set only. Recording one must move no quantity, no
    /// cost base and no allocation capacity — the three things `split_ratio`
    /// decides, and which `domain::cost_base`, `domain::open_parcels`, the
    /// AMIT re-basing and the Sell/trade write-time checks all read. If the
    /// factor ever leaked into the quantity set this test would fail, which is
    /// the whole reason the two event sets are separate types.
    #[tokio::test]
    async fn db_a_demergers_stated_close_moves_no_quantity_cost_base_or_capacity() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC").await;
        insert_listing(&pool, 2, "LAR").await;
        test_support::buy(10, 1)
            .date(d(2021, 3, 25))
            .qty(Decimal::from(1049))
            .price(Decimal::from(20))
            .insert(&pool)
            .await;
        // A sale that consumes part of the parcel: its allocation capacity is
        // computed from the split stream at read time, so a demerger sneaking
        // into that stream would change what still fits.
        test_support::sell(11, 1)
            .date(d(2023, 5, 1))
            .qty(Decimal::from(49))
            .price(Decimal::from(30))
            .insert(&pool)
            .await;
        sqlx::query(
            "INSERT INTO parcel_allocations (sale_trade_id, purchase_trade_id, quantity_allocated) \
             VALUES (11, 10, '49')",
        )
        .execute(&pool)
        .await
        .unwrap();

        async fn open_position(pool: &SqlitePool) -> (Decimal, Decimal) {
            let mut conn = pool.acquire().await.unwrap();
            let parcels = crate::domain::open_parcels::load(&mut conn, None)
                .await
                .unwrap();
            assert_eq!(parcels.len(), 1);
            (parcels[0].remaining_as_of, parcels[0].cost_base.adjusted)
        }

        let before = open_position(&pool).await;
        assert_eq!(before.0, Decimal::from(1000));

        db_upsert(
            &pool,
            &demerger_with_close(1, 1, 2, d(2023, 10, 3), d(2023, 10, 2), "24.90"),
        )
        .await
        .unwrap();

        assert_eq!(
            open_position(&pool).await,
            before,
            "a demerger's stated close is a price fact — it must not restate a \
             single unit or a single dollar of cost base"
        );
        assert!(
            db_splits_for_listing(&pool, 1).await.unwrap().is_empty(),
            "the quantity re-basing set never sees a demerger"
        );
        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert_eq!(
            split_adjusted_quantity(Decimal::from(1000), &[], d(2021, 3, 25), None),
            Decimal::from(1000),
            "and the conversion the capacity checks run is still the identity"
        );

        // The remaining 1000 units still fit a sale of exactly 1000 — the
        // allocation-capacity invariant a corporate-action write re-checks.
        let statements = db_demerger_price_statements(&pool, 1).await.unwrap();
        assert_eq!(statements.len(), 1, "…while the price side does see it");
        assert_eq!(statements[0].close_date, d(2023, 10, 2));
        assert_eq!(statements[0].close_price, "24.90".parse().unwrap());
    }

    /// The statement loader reads only demergers that actually carry a close,
    /// on the listing asked for, in date order.
    #[tokio::test]
    async fn db_demerger_price_statements_skips_a_demerger_with_no_stated_close() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        db_upsert(
            &pool,
            &demerger(1, 1, 2, d(2024, 11, 30), "1", "5", "5.063"),
        )
        .await
        .unwrap();
        assert!(
            db_demerger_price_statements(&pool, 1)
                .await
                .unwrap()
                .is_empty()
        );

        db_upsert(
            &pool,
            &demerger_with_close(2, 1, 2, d(2025, 6, 2), d(2025, 5, 30), "12.5"),
        )
        .await
        .unwrap();
        let statements = db_demerger_price_statements(&pool, 1).await.unwrap();
        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0].date, d(2025, 6, 2));
        assert!(
            db_demerger_price_statements(&pool, 2)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// The CHECK rejects a demerger of a listing into itself even on a raw
    /// SQL write — the body validation is the first line of defence.
    #[tokio::test]
    async fn db_check_rejects_self_demerger() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        let result = sqlx::query(
            "INSERT INTO corporate_actions \
             (id, action_type, listing_id, date, demerger_listing_id, demerger_new_units, \
              demerger_held_units, demerger_cost_base_pct) \
             VALUES (1, 'Demerger', 1, '2024-11-30', 1, '1', '5', '5.063')",
        )
        .execute(&pool)
        .await;
        assert!(
            result.is_err(),
            "demerger_listing_id == listing_id should violate the CHECK"
        );
    }

    /// A ScripForScrip never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel (the exchange
    /// operation does the substitution).
    #[tokio::test]
    async fn db_scrip_for_scrip_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        db_upsert(&pool, &scrip(1, 1, 2, d(2024, 11, 30), "2", "1"))
            .await
            .unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    /// The CHECK rejects an exchange of a listing into itself even on a raw
    /// SQL write — the body validation is the first line of defence.
    #[tokio::test]
    async fn db_check_rejects_self_exchange() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        let result = sqlx::query(
            "INSERT INTO corporate_actions \
             (id, action_type, listing_id, date, scrip_listing_id, scrip_new_units, scrip_old_units) \
             VALUES (1, 'ScripForScrip', 1, '2024-11-30', 1, '2', '1')",
        )
        .execute(&pool)
        .await;
        assert!(
            result.is_err(),
            "scrip_listing_id == listing_id should violate the CHECK"
        );
    }

    /// A BuyBack never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel.
    #[tokio::test]
    async fn db_buy_back_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        db_upsert(
            &pool,
            &buyback(1, 1, d(2024, 11, 30), "9.60", "1.40", "0.60", Some("10.20")),
        )
        .await
        .unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    /// A RightsIssue never appears in the split-event or return-of-capital
    /// streams — recording one changes no existing parcel.
    #[tokio::test]
    async fn db_rights_issue_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        db_upsert(&pool, &rights(1, 1, d(2024, 11, 30), "1", "4", "1.80"))
            .await
            .unwrap();

        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_splits_for_listing(&pool, 1).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    /// The record date round-trips, reaches the reports through the event
    /// stream they read, and clears again — the payment-date fallback is a
    /// state a correction can return to, not just the absence of a first write.
    #[tokio::test]
    async fn db_return_of_capital_record_date_round_trips() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        let action = roc_with_record(1, 1, d(2025, 3, 1), "0.50", d(2025, 2, 10));
        db_upsert(&pool, &action).await.unwrap();

        assert_eq!(db_get(&pool, 1).await.unwrap().unwrap().kind, action.kind);
        let events = db_return_of_capital_events(&pool).await.unwrap();
        assert_eq!(events[&1][0].record_date, Some(d(2025, 2, 10)));

        db_upsert(&pool, &roc(1, 1, d(2025, 3, 1), "0.50"))
            .await
            .unwrap();
        let events = db_return_of_capital_events(&pool).await.unwrap();
        assert_eq!(events[&1][0].record_date, None);
    }

    /// The record date fixes entitlement to *this* payment, so it can never
    /// follow it, and no other action type has one (the CHECK of 0023 — the
    /// last line of defence behind the body validation, reached only by a raw
    /// SQL write).
    #[tokio::test]
    async fn db_check_rejects_an_impossible_record_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        db_upsert(&pool, &roc(1, 1, d(2025, 3, 1), "0.50"))
            .await
            .unwrap();
        db_upsert(&pool, &split(2, 1, d(2025, 3, 1), "2", "1"))
            .await
            .unwrap();

        for (id, record_date) in [(1, "2025-03-02"), (2, "2025-02-10")] {
            let result = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE corporate_actions SET record_date = '{record_date}' WHERE id = {id}"
            )))
            .execute(&pool)
            .await;
            assert!(
                result.is_err(),
                "record_date {record_date} on action {id} should violate the CHECK"
            );
        }
        // …and the record date the payment does allow still writes.
        sqlx::query("UPDATE corporate_actions SET record_date = '2025-03-01' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn db_upsert_updates_existing() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        db_upsert(&pool, &roc(1, 1, d(2024, 11, 30), "0.50"))
            .await
            .unwrap();
        db_upsert(&pool, &roc(1, 1, d(2024, 12, 31), "0.75"))
            .await
            .unwrap();

        let all = db_list(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].date, d(2024, 12, 31));
        assert_eq!(all[0].kind, roc(1, 1, d(2024, 12, 31), "0.75").kind);
    }

    #[tokio::test]
    async fn db_listing_fk_enforced() {
        let pool = test_pool().await;
        let err = db_upsert(&pool, &roc(1, 999, d(2024, 11, 30), "0.50")).await;
        assert!(err.is_err(), "unknown listing FK should be rejected");
    }

    /// Mixed payloads are unrepresentable in [`ActionKind`], so a raw SQL
    /// write is the only path that could produce one — the per-type table
    /// CHECKs are the last line of defence and must still reject it.
    #[tokio::test]
    async fn db_check_rejects_mixed_payloads() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_listing(&pool, 2, "NEW").await;
        // (action_type, the stray columns the CHECK must reject for it)
        for (action_type, stray_cols) in [
            // A ShareSplit carrying a payment, a bonus ratio, or rights terms…
            ("ShareSplit", "amount_per_unit = '0.50', currency = 'AUD'"),
            ("ShareSplit", "bonus_units = '1', bonus_held_units = '10'"),
            (
                "ShareSplit",
                "rights_units = '1', rights_held_units = '4', exercise_price = '1.80'",
            ),
            // …a ReturnOfCapital carrying a split ratio…
            (
                "ReturnOfCapital",
                "split_new_units = '2', split_old_units = '1'",
            ),
            // …a BonusIssue carrying a split ratio…
            ("BonusIssue", "split_new_units = '2', split_old_units = '1'"),
            // …a RightsIssue carrying a payment or a split ratio…
            ("RightsIssue", "amount_per_unit = '0.50'"),
            (
                "RightsIssue",
                "split_new_units = '2', split_old_units = '1'",
            ),
            // …a BuyBack carrying a payment, a split ratio, or rights terms…
            ("BuyBack", "amount_per_unit = '0.50'"),
            ("BuyBack", "split_new_units = '2', split_old_units = '1'"),
            (
                "BuyBack",
                "rights_units = '1', rights_held_units = '4', exercise_price = '1.80'",
            ),
            // …the other types carrying buy-back terms…
            (
                "ShareSplit",
                "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'",
            ),
            (
                "ReturnOfCapital",
                "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'",
            ),
            ("RightsIssue", "buyback_market_value = '10.20'"),
            // …a ScripForScrip carrying a payment, a split ratio, or buy-back
            // terms…
            (
                "ScripForScrip",
                "amount_per_unit = '0.50', currency = 'AUD'",
            ),
            (
                "ScripForScrip",
                "split_new_units = '2', split_old_units = '1'",
            ),
            (
                "ScripForScrip",
                "buyback_price = '9.60', buyback_dividend = '0', buyback_franking_credit = '0'",
            ),
            // …and the other types carrying scrip terms…
            (
                "ShareSplit",
                "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'",
            ),
            (
                "BuyBack",
                "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'",
            ),
            // …a ScripForScrip carrying a partial cash set (all-or-none) and
            // another type carrying a full cash set…
            ("ScripForScrip", "scrip_cash_per_unit = '10'"),
            (
                "ShareSplit",
                "scrip_cash_per_unit = '10', scrip_market_value = '20', \
                 scrip_cash_currency = 'AUD'",
            ),
            // …a Demerger carrying a payment, a split ratio, or scrip terms…
            ("Demerger", "amount_per_unit = '0.50', currency = 'AUD'"),
            ("Demerger", "split_new_units = '2', split_old_units = '1'"),
            (
                "Demerger",
                "scrip_listing_id = 2, scrip_new_units = '2', scrip_old_units = '1'",
            ),
            // …and the other types carrying demerger terms.
            (
                "ShareSplit",
                "demerger_listing_id = 2, demerger_new_units = '1', demerger_held_units = '5', demerger_cost_base_pct = '5.063'",
            ),
            (
                "ScripForScrip",
                "demerger_listing_id = 2, demerger_new_units = '1', demerger_held_units = '5', demerger_cost_base_pct = '5.063'",
            ),
        ] {
            let (base_cols, base_vals) = match action_type {
                "ShareSplit" => ("split_new_units, split_old_units", "'2', '1'"),
                "ReturnOfCapital" => ("amount_per_unit, currency", "'0.50', 'AUD'"),
                "RightsIssue" => (
                    "rights_units, rights_held_units, exercise_price, currency",
                    "'1', '4', '1.80', 'AUD'",
                ),
                "BuyBack" => (
                    "buyback_price, buyback_dividend, buyback_franking_credit, currency",
                    "'9.60', '1.40', '0.60', 'AUD'",
                ),
                "ScripForScrip" => (
                    "scrip_listing_id, scrip_new_units, scrip_old_units",
                    "2, '2', '1'",
                ),
                "Demerger" => (
                    "demerger_listing_id, demerger_new_units, demerger_held_units, \
                     demerger_cost_base_pct",
                    "2, '1', '5', '5.063'",
                ),
                _ => ("bonus_units, bonus_held_units", "'1', '10'"),
            };
            // Insert a valid row, then try to smuggle the stray columns in.
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO corporate_actions (id, action_type, listing_id, date, {base_cols}) \
                 VALUES (1, '{action_type}', 1, '2024-11-30', {base_vals})"
            )))
            .execute(&pool)
            .await
            .unwrap();
            let result = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE corporate_actions SET {stray_cols} WHERE id = 1"
            )))
            .execute(&pool)
            .await;
            assert!(
                result.is_err(),
                "{action_type} + {stray_cols} should violate the CHECK"
            );
            sqlx::query("DELETE FROM corporate_actions")
                .execute(&pool)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn db_get_missing_returns_none() {
        let pool = test_pool().await;
        assert!(db_get(&pool, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_events_grouped_by_listing_sorted_by_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_listing(&pool, 2, "XYZ").await;
        db_upsert(&pool, &roc(1, 1, d(2025, 3, 1), "0.30"))
            .await
            .unwrap();
        db_upsert(&pool, &roc(2, 1, d(2024, 11, 30), "0.50"))
            .await
            .unwrap();
        db_upsert(&pool, &roc(3, 2, d(2024, 6, 1), "1.00"))
            .await
            .unwrap();

        let events = db_return_of_capital_events(&pool).await.unwrap();
        assert_eq!(events.len(), 2);
        let l1: Vec<NaiveDate> = events[&1].iter().map(|e| e.date).collect();
        assert_eq!(l1, vec![d(2024, 11, 30), d(2025, 3, 1)]);
        assert_eq!(events[&2].len(), 1);
    }

    #[tokio::test]
    async fn db_split_events_grouped_by_listing_sorted_by_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_listing(&pool, 2, "XYZ").await;
        db_upsert(&pool, &split(1, 1, d(2025, 3, 1), "2", "1"))
            .await
            .unwrap();
        db_upsert(&pool, &split(2, 1, d(2024, 11, 30), "1", "10"))
            .await
            .unwrap();
        db_upsert(&pool, &split(3, 2, d(2024, 6, 1), "3", "1"))
            .await
            .unwrap();
        // A ReturnOfCapital on the same listing must not appear as a split.
        db_upsert(&pool, &roc(4, 1, d(2024, 6, 1), "0.50"))
            .await
            .unwrap();

        let events = db_share_split_events(&pool).await.unwrap();
        assert_eq!(events.len(), 2);
        let l1: Vec<NaiveDate> = events[&1].iter().map(|e| e.date).collect();
        assert_eq!(l1, vec![d(2024, 11, 30), d(2025, 3, 1)]);
        assert_eq!(events[&2].len(), 1);

        let for_listing = db_splits_for_listing(&pool, 1).await.unwrap();
        assert_eq!(for_listing.len(), 2);
        assert_eq!(for_listing[0].date, d(2024, 11, 30));
    }

    /// A BonusIssue is folded into the split-event stream as its equivalent
    /// split: every `bonus_held_units` units become `bonus_held_units +
    /// bonus_units` units (a 1-for-10 bonus issue re-bases 11-for-10).
    #[tokio::test]
    async fn db_split_events_include_bonus_issues_as_equivalent_splits() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        db_upsert(&pool, &bonus(1, 1, d(2025, 3, 1), "1", "10"))
            .await
            .unwrap();
        // A real split on the same listing interleaves in date order…
        db_upsert(&pool, &split(2, 1, d(2024, 11, 30), "2", "1"))
            .await
            .unwrap();
        // …and a ReturnOfCapital never appears as a re-basing event.
        db_upsert(&pool, &roc(3, 1, d(2024, 6, 1), "0.50"))
            .await
            .unwrap();

        let events = db_share_split_events(&pool).await.unwrap();
        let l1 = &events[&1];
        assert_eq!(l1.len(), 2);
        assert_eq!(l1[0].date, d(2024, 11, 30));
        assert_eq!(
            (l1[0].new_units, l1[0].old_units),
            (Decimal::from(2), Decimal::ONE)
        );
        assert_eq!(l1[1].date, d(2025, 3, 1));
        assert_eq!(
            (l1[1].new_units, l1[1].old_units),
            (Decimal::from(11), Decimal::from(10))
        );

        let for_listing = db_splits_for_listing(&pool, 1).await.unwrap();
        assert_eq!(for_listing.len(), 2);
        assert_eq!(for_listing[1].new_units, Decimal::from(11));
        assert_eq!(for_listing[1].old_units, Decimal::from(10));
    }

    /// SCENARIOS Z-e: the loaders carry each action's **announced terms**
    /// alongside the derived rebase factor, so a human-readable surface can
    /// name the event the way the company announced it. The factor is what it
    /// always was — a bonus issue still re-bases 11-for-10 — and only the
    /// label reads from the terms.
    #[tokio::test]
    async fn db_split_events_carry_each_actions_announced_terms() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        db_upsert(&pool, &split(1, 1, d(2024, 11, 30), "2", "1"))
            .await
            .unwrap();
        db_upsert(&pool, &bonus(2, 1, d(2025, 3, 1), "1", "10"))
            .await
            .unwrap();
        // new < old: a consolidation, announced in the same new-for-old terms.
        db_upsert(&pool, &split(3, 1, d(2025, 6, 1), "1", "2"))
            .await
            .unwrap();
        // Terms typed with a scale were still announced as "2-for-1".
        db_upsert(&pool, &split(4, 1, d(2025, 9, 1), "2.00", "1.00"))
            .await
            .unwrap();

        for events in [
            db_splits_for_listing(&pool, 1).await.unwrap(),
            db_share_split_events(&pool).await.unwrap()[&1].clone(),
        ] {
            let labels: Vec<String> = events.iter().map(|e| e.terms.label()).collect();
            assert_eq!(
                labels,
                vec![
                    "2-for-1 split",
                    "1-for-10 bonus issue",
                    "1-for-2 consolidation",
                    "2-for-1 split",
                ]
            );
            // The re-basing arithmetic is untouched by the terms travelling
            // with the event: the bonus issue is still its equivalent split.
            let factors: Vec<(Decimal, Decimal)> =
                events.iter().map(|e| (e.new_units, e.old_units)).collect();
            assert_eq!(
                factors,
                vec![
                    (Decimal::from(2), Decimal::ONE),
                    (Decimal::from(11), Decimal::from(10)),
                    (Decimal::ONE, Decimal::from(2)),
                    ("2.00".parse().unwrap(), "1.00".parse().unwrap()),
                ]
            );
            // 100 units → 200 → 220 → 110 → 220, over the same walk as before.
            assert_eq!(
                split_adjusted_quantity(Decimal::from(100), &events, d(2024, 1, 1), None),
                Decimal::from(220)
            );
        }
    }

    #[test]
    fn split_ratio_covers_half_open_interval() {
        let splits = vec![
            split_event(d(2024, 6, 1), "2", "1"),
            split_event(d(2025, 1, 1), "1", "10"),
        ];
        // Acquired before both: both apply → 2/10.
        assert_eq!(
            split_ratio(&splits, d(2024, 1, 1), None),
            (Decimal::from(2), Decimal::from(10))
        );
        // Acquired on the first conversion date: already post-split — only the
        // second applies.
        assert_eq!(
            split_ratio(&splits, d(2024, 6, 1), None),
            (Decimal::ONE, Decimal::from(10))
        );
        // Re-basing to a date on the second conversion: it applies (inclusive end).
        assert_eq!(
            split_ratio(&splits, d(2024, 1, 1), Some(d(2025, 1, 1))),
            (Decimal::from(2), Decimal::from(10))
        );
        // Re-basing to a date before the second conversion: only the first.
        assert_eq!(
            split_ratio(&splits, d(2024, 1, 1), Some(d(2024, 12, 31))),
            (Decimal::from(2), Decimal::ONE)
        );
    }

    /// A price scales the opposite way to a quantity: what a re-basing event
    /// multiplies the unit count by, it divides the per-unit price by. Only
    /// events between the price date and the observation restate the figure,
    /// so a price observed before one is already in its own day's basis.
    #[test]
    fn contemporaneous_price_undoes_the_events_between_the_day_and_the_observation() {
        let splits: Vec<PriceBasisEvent> = [split_event(d(2024, 6, 10), "10", "1")]
            .iter()
            .map(PriceBasisEvent::from)
            .collect();
        let observed = |price: &str, on: NaiveDate| {
            contemporaneous_price(price.parse().unwrap(), &splits, d(2024, 6, 7), on)
        };
        // Observed after the split: the provider's series is restated, so the
        // 7 June close comes back ten times the figure served.
        assert_eq!(
            observed("120.888", d(2024, 6, 20)),
            "1208.880".parse().unwrap()
        );
        // Observed before it: nothing to undo.
        assert_eq!(
            observed("1208.88", d(2024, 6, 8)),
            "1208.88".parse().unwrap()
        );
        // Observed on the event date itself — already restated (the interval
        // is closed at that end, as `split_ratio` documents).
        assert_eq!(
            observed("120.888", d(2024, 6, 10)),
            "1208.880".parse().unwrap()
        );
        // An event on the price date has already restated that day's close.
        assert_eq!(
            contemporaneous_price(
                "120.888".parse().unwrap(),
                &splits,
                d(2024, 6, 10),
                d(2024, 7, 1)
            ),
            "120.888".parse().unwrap()
        );
        // A consolidation runs the other way.
        let consol: Vec<PriceBasisEvent> = [split_event(d(2024, 6, 10), "1", "10")]
            .iter()
            .map(PriceBasisEvent::from)
            .collect();
        assert_eq!(
            contemporaneous_price(
                "12088.8".parse().unwrap(),
                &consol,
                d(2024, 6, 7),
                d(2024, 7, 1)
            ),
            "1208.88".parse().unwrap()
        );
    }

    /// The structural separation: the price re-basing event set is a strict
    /// superset of the quantity one. A demerger's factor restates the price
    /// series and composes with a split in the same walk, while the quantity
    /// walk — which `domain::cost_base`, `domain::open_parcels`, the AMIT
    /// re-basing and the allocation-capacity checks all read — never sees it.
    #[test]
    fn the_price_event_set_is_a_superset_of_the_quantity_event_set() {
        let split = split_event(d(2024, 6, 10), "2", "1");
        // The demerger's derived factor: the security actually closed at 24.90
        // on the last pre-demerger day, and the provider's figure for it — once
        // the later split has been divided out — is 10.13.
        let demerger = PriceBasisEvent {
            date: d(2024, 3, 1),
            recover_new: "24.90".parse().unwrap(),
            recover_old: "10.13".parse().unwrap(),
        };
        let price_events = vec![PriceBasisEvent::from(&split), demerger];

        // A pre-demerger day observed after both events recovers through both:
        // twice for the split, then 24.90/10.13 for the demerger.
        let recovered = contemporaneous_price(
            "5.065".parse().unwrap(),
            &price_events,
            d(2024, 2, 20),
            d(2026, 7, 26),
        );
        assert_eq!(recovered, "24.90".parse::<Decimal>().unwrap());

        // The very same demerger contributes nothing to a quantity: it is not
        // a `SplitEvent` and has no way to become one.
        let quantity_events = vec![split];
        assert_eq!(
            split_ratio(&quantity_events, d(2024, 2, 20), Some(d(2026, 7, 26))),
            (Decimal::from(2), Decimal::ONE),
            "only the split converts unit bases — the demerger issues units of \
             another listing and moves no count here"
        );
    }

    #[test]
    fn split_adjusted_and_as_acquired_quantities_are_inverse() {
        let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
        // 100 as-acquired units are 200 post-split units…
        assert_eq!(
            split_adjusted_quantity(Decimal::from(100), &splits, d(2024, 1, 1), None),
            Decimal::from(200)
        );
        // …and 80 post-split units sold come from 40 as-acquired units.
        assert_eq!(
            as_acquired_quantity(Decimal::from(80), &splits, d(2024, 1, 1), d(2024, 9, 1)),
            Decimal::from(40)
        );
        // A consolidation shrinks: 1-for-10 turns 100 into 10.
        let consol = vec![split_event(d(2024, 6, 1), "1", "10")];
        assert_eq!(
            split_adjusted_quantity(Decimal::from(100), &consol, d(2024, 1, 1), None),
            Decimal::from(10)
        );
    }

    #[test]
    fn per_unit_reduction_sums_events_from_acquisition() {
        let events = vec![
            RocEvent {
                date: d(2024, 1, 1),
                amount_per_unit: "0.10".parse().unwrap(),
                currency: "AUD".into(),
                record_date: None,
            },
            RocEvent {
                date: d(2024, 6, 1),
                amount_per_unit: "0.20".parse().unwrap(),
                currency: "AUD".into(),
                record_date: None,
            },
            RocEvent {
                date: d(2025, 1, 1),
                amount_per_unit: "0.40".parse().unwrap(),
                currency: "AUD".into(),
                record_date: None,
            },
        ];
        // Acquired between the first and second events: the first doesn't apply.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 3, 1), None, None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Acquired on the event date: held on the payment date, so it applies.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 6, 1), None, None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
    }

    /// A rollover replacement parcel's entitlement is decided by when its
    /// units joined *this listing's* register, not by the parcel's own trade
    /// date — which is the operation date, and so always on or after a record
    /// date the operation fell after.
    ///
    /// A **transfer** keeps the units registered throughout (they move between
    /// the taxpayer's own accounts), so a payment they were entitled to at its
    /// record date still reduces their cost base in the replacement parcel. A
    /// **scrip exchange** does not: those units are of a listing the taxpayer
    /// was not on the register of when the record date passed.
    #[test]
    fn per_unit_reduction_dates_a_replacement_parcels_entitlement_from_the_register() {
        let events = vec![RocEvent {
            date: d(2023, 11, 1),
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "AUD".into(),
            record_date: Some(d(2023, 9, 25)),
        }];
        // Units bought 10 Jan, rolled over 3 Oct — inside the window.
        let operation = d(2023, 10, 3);
        let acquired = d(2023, 1, 10);

        let transfer = RolloverOrigin {
            on: operation,
            registered_from: acquired,
        };
        assert_eq!(
            per_unit_reduction(&events, &[], "AUD", operation, Some(transfer), None).unwrap(),
            "0.50".parse::<Decimal>().unwrap()
        );

        // A scrip exchange's replacement: registered only from the operation.
        let scrip = RolloverOrigin {
            on: operation,
            registered_from: operation,
        };
        assert_eq!(
            per_unit_reduction(&events, &[], "AUD", operation, Some(scrip), None).unwrap(),
            Decimal::ZERO
        );

        // Either way, a payment on or before the operation date is already in
        // the carried cost base and must not reduce it twice (SCENARIOS N-06).
        let already_carried = vec![RocEvent {
            date: operation,
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "AUD".into(),
            record_date: Some(d(2023, 9, 25)),
        }];
        assert_eq!(
            per_unit_reduction(
                &already_carried,
                &[],
                "AUD",
                operation,
                Some(transfer),
                None
            )
            .unwrap(),
            Decimal::ZERO
        );
    }

    #[test]
    fn per_unit_reduction_bounds_at_sale_date() {
        let events = vec![
            RocEvent {
                date: d(2024, 6, 1),
                amount_per_unit: "0.20".parse().unwrap(),
                currency: "AUD".into(),
                record_date: None,
            },
            RocEvent {
                date: d(2025, 1, 1),
                amount_per_unit: "0.40".parse().unwrap(),
                currency: "AUD".into(),
                record_date: None,
            },
        ];
        // Sold between the events: only the payment received while held applies.
        let pu = per_unit_reduction(
            &events,
            &[],
            "AUD",
            d(2024, 1, 1),
            None,
            Some(d(2024, 9, 1)),
        )
        .unwrap();
        assert_eq!(pu, "0.20".parse::<Decimal>().unwrap());
        // Sold on the payment date: still held at the payment, so it applies.
        let pu = per_unit_reduction(
            &events,
            &[],
            "AUD",
            d(2024, 1, 1),
            None,
            Some(d(2025, 1, 1)),
        )
        .unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Sold before any payment: unaffected.
        let pu = per_unit_reduction(
            &events,
            &[],
            "AUD",
            d(2024, 1, 1),
            None,
            Some(d(2024, 5, 1)),
        )
        .unwrap();
        assert_eq!(pu, Decimal::ZERO);
    }

    /// A payment after a split is per *post-split* unit: each as-acquired unit
    /// became `new/old` units, so the per-as-acquired-unit reduction scales by
    /// the split ratio.
    #[test]
    fn per_unit_reduction_scales_payments_across_a_split() {
        let events = vec![
            // Before the split: per as-acquired unit as-is.
            RocEvent {
                date: d(2024, 3, 1),
                amount_per_unit: "0.30".parse().unwrap(),
                currency: "AUD".into(),
                record_date: None,
            },
            // After a 2-for-1 split: each as-acquired unit receives it twice.
            RocEvent {
                date: d(2024, 9, 1),
                amount_per_unit: "0.20".parse().unwrap(),
                currency: "AUD".into(),
                record_date: None,
            },
        ];
        let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
        let pu = per_unit_reduction(&events, &splits, "AUD", d(2024, 1, 1), None, None).unwrap();
        // 0.30 + 0.20 × 2 = 0.70 per as-acquired unit.
        assert_eq!(pu, "0.70".parse::<Decimal>().unwrap());

        // A parcel acquired after the split holds post-split units already:
        // the later payment applies unscaled.
        let pu = per_unit_reduction(&events, &splits, "AUD", d(2024, 7, 1), None, None).unwrap();
        assert_eq!(pu, "0.20".parse::<Decimal>().unwrap());
    }

    #[test]
    fn per_unit_reduction_rejects_currency_mismatch() {
        let events = vec![RocEvent {
            date: d(2024, 6, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "USD".into(),
            record_date: None,
        }];
        // Never net amounts across currencies: fail loudly, don't skip or zero.
        assert!(per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), None, None).is_err());
        // An out-of-range event in another currency is not an error — it doesn't
        // participate in the calculation at all.
        assert!(per_unit_reduction(&events, &[], "AUD", d(2024, 7, 1), None, None).is_ok());
    }

    /// The window, the currency guard and the split re-basing above are all
    /// `RocEvent::per_unit_for`'s, reached through `per_unit_reduction`. What
    /// only the method itself exposes is the `Option`: a payment that doesn't
    /// reach the units *declines* rather than reducing them by nil — which is
    /// what lets `domain::cost_base::adjustment_detail` leave the payment out
    /// of the itemised breakdown entirely instead of printing a zero row.
    #[test]
    fn per_unit_for_declines_a_payment_outside_the_holding_period() {
        let payment = RocEvent {
            date: d(2024, 6, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        };
        let pu = |acquired, up_to| {
            payment
                .per_unit_for(&[], "AUD", acquired, None, up_to)
                .expect("same currency")
        };
        assert_eq!(pu(d(2024, 1, 1), None), Some("0.20".parse().unwrap()));
        assert_eq!(pu(d(2024, 7, 1), None), None, "acquired after the payment");
        assert_eq!(
            pu(d(2024, 1, 1), Some(d(2024, 5, 1))),
            None,
            "sold before the payment"
        );
    }

    /// Entitlement to a return of capital is fixed at its **record date**, weeks
    /// before the money arrives (SCENARIOS B-09): a parcel bought inside that
    /// window receives nothing, so nothing reduces it. Without a record date the
    /// payment date stands in — the behaviour of every action recorded before
    /// the field existed.
    #[test]
    fn per_unit_for_tests_entitlement_at_the_record_date() {
        let payment = |record_date| RocEvent {
            date: d(2025, 3, 1),
            amount_per_unit: "0.50".parse().unwrap(),
            currency: "AUD".into(),
            record_date,
        };
        let record = d(2025, 2, 10);
        let pu = |event: RocEvent, acquired, up_to| {
            event
                .per_unit_for(&[], "AUD", acquired, None, up_to)
                .expect("same currency")
        };
        let paid = Some("0.50".parse::<Decimal>().unwrap());

        // Held before the record date: entitled.
        assert_eq!(pu(payment(Some(record)), d(2025, 2, 9), None), paid);
        // Bought on the record date, or anywhere in the window up to the
        // payment: ex-entitlement, so untouched.
        assert_eq!(pu(payment(Some(record)), record, None), None);
        assert_eq!(pu(payment(Some(record)), d(2025, 2, 15), None), None);
        // No record date recorded: the payment date decides, as before.
        assert_eq!(pu(payment(None), d(2025, 2, 15), None), paid);
        // The other half of the test is unchanged by a record date: a parcel
        // entitled at the record date but sold before the payment is still not
        // reduced (G1 adjusts the shares owned at the time of the payment).
        assert_eq!(
            pu(payment(Some(record)), d(2025, 2, 9), Some(d(2025, 2, 20))),
            None,
            "sold between the record date and the payment"
        );
    }

    // API-level tests

    #[tokio::test]
    async fn api_put_get_list_delete_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        let body = serde_json::json!({
            "action_type": "ReturnOfCapital",
            "listing_id": 1,
            "date": "2024-11-30",
            "amount_per_unit": "0.50",
            "currency": "AUD",
        });
        let resp = client(&pool).put("/corporate_actions/1", &body).await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);

        let resp = client(&pool).get("/corporate_actions/1").await;
        assert_eq!(resp.status, StatusCode::OK);
        let got: CorporateAction = resp.json();
        assert_eq!(got.kind, roc(1, 1, d(2024, 11, 30), "0.50").kind);

        let resp = client(&pool).get("/corporate_actions").await;
        assert_eq!(resp.status, StatusCode::OK);
        let items: Vec<CorporateAction> = resp.json();
        assert_eq!(items.len(), 1);

        let resp = client(&pool).delete("/corporate_actions/1").await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    async fn api_put_expecting(pool: &SqlitePool, body: serde_json::Value, expected: StatusCode) {
        let resp = client(pool).put("/corporate_actions/1", &body).await;
        assert_eq!(resp.status, expected);
    }

    #[tokio::test]
    async fn api_share_split_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ShareSplit",
                "listing_id": 1,
                "date": "2024-11-30",
                "split_new_units": "2",
                "split_old_units": "1",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::ShareSplit {
                split_new_units: Decimal::from(2),
                split_old_units: Decimal::ONE,
            }
        );
    }

    #[tokio::test]
    async fn api_bonus_issue_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "BonusIssue",
                "listing_id": 1,
                "date": "2024-11-30",
                "bonus_units": "1",
                "bonus_held_units": "10",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BonusIssue {
                bonus_units: Decimal::ONE,
                bonus_held_units: Decimal::from(10),
            }
        );
    }

    #[tokio::test]
    async fn api_rights_issue_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "RightsIssue",
                "listing_id": 1,
                "date": "2024-11-30",
                "rights_units": "1",
                "rights_held_units": "4",
                "exercise_price": "1.80",
                "currency": "AUD",
                "renounceable": true,
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::RightsIssue {
                rights_units: Decimal::ONE,
                rights_held_units: Decimal::from(4),
                exercise_price: "1.80".parse().unwrap(),
                currency: "AUD".to_string(),
                renounceable: true,
            }
        );
    }

    /// SCENARIOS AA-b. Whether the offer was renounceable is a term of the
    /// action like any other: it round-trips through the PUT, it reads back on
    /// the GET, and `false` — the offer whose retail premium is an unfranked
    /// dividend rather than capital proceeds — is a state the row can hold.
    #[tokio::test]
    async fn api_non_renounceable_rights_issue_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "RightsIssue",
                "listing_id": 1,
                "date": "2024-11-30",
                "rights_units": "1",
                "rights_held_units": "4",
                "exercise_price": "1.80",
                "currency": "AUD",
                "renounceable": false,
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::RightsIssue {
                rights_units: Decimal::ONE,
                rights_held_units: Decimal::from(4),
                exercise_price: "1.80".parse().unwrap(),
                currency: "AUD".to_string(),
                renounceable: false,
            }
        );
        // And it is on the JSON the UI reads back, not only in the row.
        let resp = client(&pool).get("/corporate_actions/1").await;
        let json: serde_json::Value = resp.json();
        assert_eq!(json["renounceable"], serde_json::json!(false));
    }

    /// The flag is **required** on a rights issue and forbidden on every other
    /// type. Required because the whole finding was that the fact was never
    /// asked for: a quiet default would leave the same assumption in place for
    /// every new entry, and the offer document always states it.
    #[tokio::test]
    async fn api_a_rights_issue_without_the_renounceable_flag_is_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
        for stray in [
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1", "renounceable": true,
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "10", "renounceable": false,
            }),
            serde_json::json!({
                "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
                "amount_per_unit": "0.50", "currency": "AUD", "renounceable": true,
            }),
        ] {
            api_put_expecting(&pool, stray, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM corporate_actions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "nothing was written");
    }

    #[tokio::test]
    async fn api_invalid_rights_issue_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RTS").await;
        // Missing terms, a missing currency, non-positive ratio/price, a
        // stray payment amount, a stray split ratio — and the ratio-only
        // types carrying rights fields or a currency.
        for body in [
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "0", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "renounceable": true,
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "-4", "exercise_price": "1.80",
                "currency": "AUD", "renounceable": true,
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "0",
                "currency": "AUD", "renounceable": true,
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "renounceable": true, "amount_per_unit": "0.50",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "renounceable": true, "split_new_units": "2",
                "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "10", "currency": "AUD",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_buy_back_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "BuyBack",
                "listing_id": 1,
                "date": "2024-11-30",
                "buyback_price": "9.60",
                "buyback_dividend": "1.40",
                "buyback_franking_credit": "0.60",
                "buyback_market_value": "10.20",
                "currency": "AUD",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BuyBack {
                buyback_price: "9.60".parse().unwrap(),
                buyback_dividend: "1.40".parse().unwrap(),
                buyback_franking_credit: "0.60".parse().unwrap(),
                buyback_market_value: Some("10.20".parse().unwrap()),
                currency: "AUD".to_string(),
            }
        );

        // The no-dividend (listed post-Oct-2022) shape: dividend/credit
        // default to 0 and the market value may be omitted.
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "BuyBack",
                "listing_id": 1,
                "date": "2024-12-31",
                "buyback_price": "5.00",
                "currency": "AUD",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::BuyBack {
                buyback_price: "5.00".parse().unwrap(),
                buyback_dividend: Decimal::ZERO,
                buyback_franking_credit: Decimal::ZERO,
                buyback_market_value: None,
                currency: "AUD".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn api_invalid_buy_back_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BBK").await;
        // Missing/non-positive price, a missing currency, a dividend
        // exceeding the price (it is a component of it), a negative
        // dividend, a credit without a dividend to attach to, a negative
        // credit, a non-positive market value, stray cross-type fields —
        // and the other types carrying buy-back fields.
        for body in [
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "0", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_dividend": "9.61", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_dividend": "-1.40", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_franking_credit": "0.60", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_dividend": "1.40",
                "buyback_franking_credit": "-0.60", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "buyback_market_value": "0", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "currency": "AUD",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "currency": "AUD", "amount_per_unit": "0.50",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1", "buyback_price": "9.60",
            }),
            serde_json::json!({
                "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
                "amount_per_unit": "0.50", "currency": "AUD", "buyback_dividend": "1.40",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "renounceable": true, "buyback_market_value": "10.20",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_invalid_bonus_issue_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        // Missing ratio, non-positive ratio, a stray payment field, a stray
        // split ratio — and a ShareSplit carrying a bonus ratio.
        for body in [
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "0", "bonus_held_units": "10",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "-10",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "10",
                "amount_per_unit": "0.50", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "BonusIssue", "listing_id": 1, "date": "2024-11-30",
                "bonus_units": "1", "bonus_held_units": "10",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "bonus_units": "1", "bonus_held_units": "10",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_scrip_for_scrip_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ScripForScrip",
                "listing_id": 1,
                "date": "2024-11-30",
                "scrip_listing_id": 2,
                "scrip_new_units": "2",
                "scrip_old_units": "1",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::ScripForScrip {
                scrip_listing_id: 2,
                scrip_new_units: Decimal::from(2),
                scrip_old_units: Decimal::ONE,
                scrip_cash_per_unit: None,
                scrip_market_value: None,
                scrip_cash_currency: None,
            }
        );
    }

    /// The optional cash component (partial rollover, Example 27) round-trips
    /// with sub-cent precision.
    #[tokio::test]
    async fn api_scrip_for_scrip_cash_component_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "1", "scrip_old_units": "1",
                "scrip_cash_per_unit": "10.005", "scrip_market_value": "20.105",
                "scrip_cash_currency": "AUD",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::ScripForScrip {
                scrip_listing_id: 2,
                scrip_new_units: Decimal::ONE,
                scrip_old_units: Decimal::ONE,
                scrip_cash_per_unit: Some("10.005".parse().unwrap()),
                scrip_market_value: Some("20.105".parse().unwrap()),
                scrip_cash_currency: Some("AUD".to_string()),
            }
        );
    }

    /// The cash component is all-or-none (cash, market value, currency) and
    /// both amounts must be positive; cash fields on another action type are
    /// stray.
    #[tokio::test]
    async fn api_invalid_scrip_cash_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        let scrip_base = serde_json::json!({
            "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            "scrip_listing_id": 2, "scrip_new_units": "1", "scrip_old_units": "1",
        });
        let with = |extra: serde_json::Value| {
            let mut body = scrip_base.clone();
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            body
        };
        for body in [
            // Partial sets.
            with(serde_json::json!({ "scrip_cash_per_unit": "10" })),
            with(serde_json::json!({ "scrip_cash_per_unit": "10", "scrip_market_value": "20" })),
            with(serde_json::json!({ "scrip_market_value": "20", "scrip_cash_currency": "AUD" })),
            with(serde_json::json!({ "scrip_cash_currency": "AUD" })),
            // Non-positive amounts.
            with(serde_json::json!({
                "scrip_cash_per_unit": "0", "scrip_market_value": "20",
                "scrip_cash_currency": "AUD",
            })),
            with(serde_json::json!({
                "scrip_cash_per_unit": "10", "scrip_market_value": "-20",
                "scrip_cash_currency": "AUD",
            })),
            // An unknown cash currency (FK).
            with(serde_json::json!({
                "scrip_cash_per_unit": "10", "scrip_market_value": "20",
                "scrip_cash_currency": "ZZZ",
            })),
            // The shared `currency` column stays forbidden for ScripForScrip.
            with(serde_json::json!({
                "scrip_cash_per_unit": "10", "scrip_market_value": "20",
                "scrip_cash_currency": "AUD", "currency": "AUD",
            })),
            // Cash fields are stray on another action type.
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "scrip_cash_per_unit": "10", "scrip_market_value": "20",
                "scrip_cash_currency": "AUD",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_invalid_scrip_for_scrip_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEW").await;
        // Missing terms, a non-positive ratio, the same listing on both
        // sides, an unknown replacement listing, a stray currency, stray
        // cross-type fields — and the other types carrying scrip fields.
        for body in [
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "0", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "-1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 1, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 999, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "amount_per_unit": "0.50",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-30",
                "buyback_price": "9.60", "currency": "AUD", "scrip_listing_id": 2,
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_demerger_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "Demerger",
                "listing_id": 1,
                "date": "2024-11-30",
                "demerger_listing_id": 2,
                "demerger_new_units": "1",
                "demerger_held_units": "5",
                "demerger_cost_base_pct": "5.063",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::Demerger {
                demerger_listing_id: 2,
                demerger_new_units: Decimal::ONE,
                demerger_held_units: Decimal::from(5),
                demerger_cost_base_pct: "5.063".parse().unwrap(),
                demerger_close_date: None,
                demerger_close_price: None,
                demerger_close_sourced_from: None,
                demerger_close_reason: None,
            }
        );
    }

    #[tokio::test]
    async fn api_invalid_demerger_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "HEAD").await;
        insert_listing(&pool, 2, "DEM").await;
        // Missing terms, a non-positive ratio, a percentage at/outside the
        // (0, 100) bounds, the same listing on both sides, an unknown
        // demerged listing, a stray currency, stray cross-type fields — and
        // the other types carrying demerger fields.
        for body in [
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "0",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "-5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "0",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "100",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "-5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 1, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 999, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
                "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "Demerger", "listing_id": 1, "date": "2024-11-30",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "5.063",
            }),
            serde_json::json!({
                "action_type": "ScripForScrip", "listing_id": 1, "date": "2024-11-30",
                "scrip_listing_id": 2, "scrip_new_units": "2", "scrip_old_units": "1",
                "demerger_cost_base_pct": "5.063",
            }),
            // A stated close on a type that has no price series to restate.
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "demerger_close_date": "2024-11-29", "demerger_close_price": "24.90",
                "demerger_close_sourced_from": "nyse.com", "demerger_close_reason": "spin-off",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    /// The stated pre-demerger close round-trips with its provenance, and the
    /// write-time refusals around it: it is all-or-none, the day must be
    /// strictly before the demerger, the close positive, and neither
    /// provenance field blank — the same shape `PUT /closing_prices/…` applies
    /// to a hand-entered price, for the same reason.
    #[tokio::test]
    async fn api_demerger_stated_close_round_trips_and_rejects_a_partial_one() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "LAC").await;
        insert_listing(&pool, 2, "LAR").await;
        let complete = serde_json::json!({
            "action_type": "Demerger", "listing_id": 1, "date": "2023-10-03",
            "demerger_listing_id": 2, "demerger_new_units": "1",
            "demerger_held_units": "1", "demerger_cost_base_pct": "36",
            "demerger_close_date": "2023-10-02", "demerger_close_price": "24.90",
            "demerger_close_sourced_from": "  nyse.com daily close  ",
            "demerger_close_reason": "the provider adjusts the pre-demerger series",
        });
        api_put_expecting(&pool, complete.clone(), StatusCode::NO_CONTENT).await;
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().kind,
            ActionKind::Demerger {
                demerger_listing_id: 2,
                demerger_new_units: Decimal::ONE,
                demerger_held_units: Decimal::ONE,
                demerger_cost_base_pct: Decimal::from(36),
                demerger_close_date: Some(d(2023, 10, 2)),
                demerger_close_price: Some("24.90".parse().unwrap()),
                demerger_close_sourced_from: Some("nyse.com daily close".to_string()),
                demerger_close_reason: Some(
                    "the provider adjusts the pre-demerger series".to_string()
                ),
            },
            "the provenance is stored trimmed, beside the two sides of the factor"
        );

        let without = |field: &str| {
            let mut body = complete.clone();
            body.as_object_mut().unwrap().remove(field);
            body
        };
        let with = |field: &str, value: serde_json::Value| {
            let mut body = complete.clone();
            body[field] = value;
            body
        };
        for body in [
            // Each of the four missing on its own: a partial statement leaves
            // a factor with one side, or a figure with no recorded source.
            without("demerger_close_date"),
            without("demerger_close_price"),
            without("demerger_close_sourced_from"),
            without("demerger_close_reason"),
            // The close of the demerger date itself, or a later day, is
            // already in the post-demerger basis.
            with("demerger_close_date", serde_json::json!("2023-10-03")),
            with("demerger_close_date", serde_json::json!("2023-10-04")),
            // A non-positive close would make the factor zero or negative.
            with("demerger_close_price", serde_json::json!("0")),
            with("demerger_close_price", serde_json::json!("-24.90")),
            // Provenance that records nothing.
            with("demerger_close_sourced_from", serde_json::json!("   ")),
            with("demerger_close_reason", serde_json::json!("")),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn db_insert_and_retrieve_worthless_shares_preserves_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        for (id, event) in [
            (1, WorthlessEvent::G3Declaration),
            (2, WorthlessEvent::C2Cancellation),
        ] {
            db_upsert(
                &pool,
                &CorporateAction {
                    id,
                    listing_id: 1,
                    date: d(2025, 3, 31),
                    kind: ActionKind::WorthlessShares {
                        worthless_event: event,
                    },
                },
            )
            .await
            .unwrap();
            let got = db_get(&pool, id).await.unwrap().unwrap();
            assert_eq!(
                got.kind,
                ActionKind::WorthlessShares {
                    worthless_event: event
                }
            );
        }
    }

    /// A WorthlessShares action never appears in the split-event or
    /// return-of-capital streams — recording one changes no existing parcel
    /// (the recognise operation closes the holding).
    #[tokio::test]
    async fn db_worthless_shares_is_not_a_split_or_payment_event() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: d(2025, 3, 31),
                kind: ActionKind::WorthlessShares {
                    worthless_event: WorthlessEvent::G3Declaration,
                },
            },
        )
        .await
        .unwrap();
        assert!(db_share_split_events(&pool).await.unwrap().is_empty());
        assert!(db_return_of_capital_events(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_worthless_shares_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "WorthlessShares",
                "listing_id": 1,
                "date": "2025-03-31",
                "worthless_event": "G3Declaration",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        let got = db_get(&pool, 1).await.unwrap().unwrap();
        assert_eq!(
            got.kind,
            ActionKind::WorthlessShares {
                worthless_event: WorthlessEvent::G3Declaration
            }
        );
    }

    #[tokio::test]
    async fn api_invalid_worthless_shares_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "DEAD").await;
        // Missing the event discriminator, an unknown event value, a stray
        // currency, stray cross-type payload fields — and another type
        // carrying a worthless_event.
        for body in [
            serde_json::json!({
                "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
            }),
            serde_json::json!({
                "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
                "worthless_event": "Nope",
            }),
            serde_json::json!({
                "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
                "worthless_event": "G3Declaration", "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "WorthlessShares", "listing_id": 1, "date": "2025-03-31",
                "worthless_event": "G3Declaration", "split_new_units": "2", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2025-03-31",
                "split_new_units": "2", "split_old_units": "1",
                "worthless_event": "G3Declaration",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_non_positive_amount_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        for amount in ["0", "-0.50"] {
            api_put_expecting(
                &pool,
                serde_json::json!({
                    "action_type": "ReturnOfCapital",
                    "listing_id": 1,
                    "date": "2024-11-30",
                    "amount_per_unit": amount,
                    "currency": "AUD",
                }),
                StatusCode::UNPROCESSABLE_ENTITY,
            )
            .await;
        }
    }

    /// A return of capital may carry the record date that fixed entitlement to
    /// it; it round-trips on the flat wire shape like any other field.
    #[tokio::test]
    async fn api_return_of_capital_record_date_round_trip() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2025-03-01",
                "amount_per_unit": "0.50",
                "currency": "AUD",
                "record_date": "2025-02-10",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;

        let resp = client(&pool).get("/corporate_actions/1").await;
        let got: CorporateAction = resp.json();
        assert_eq!(
            got.kind,
            roc_with_record(1, 1, d(2025, 3, 1), "0.50", d(2025, 2, 10)).kind
        );
        let raw: serde_json::Value = client(&pool).get("/corporate_actions/1").await.json();
        assert_eq!(raw["record_date"], "2025-02-10");
    }

    /// A record date after the payment it entitles the holder to is a
    /// contradiction, and no other action type has one at all — both rejected
    /// rather than silently dropped.
    #[tokio::test]
    async fn api_invalid_record_dates_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        for body in [
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2025-03-01",
                "amount_per_unit": "0.50",
                "currency": "AUD",
                "record_date": "2025-03-02",
            }),
            serde_json::json!({
                "action_type": "ShareSplit",
                "listing_id": 1,
                "date": "2025-03-01",
                "split_new_units": "2",
                "split_old_units": "1",
                "record_date": "2025-02-10",
            }),
            serde_json::json!({
                "action_type": "RightsIssue",
                "listing_id": 1,
                "date": "2025-03-01",
                "rights_units": "1",
                "rights_held_units": "4",
                "exercise_price": "1.80",
                "currency": "AUD",
                "renounceable": true,
                "record_date": "2025-02-10",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
        // The payment date itself is a legal record date (a same-day fixing).
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2025-03-01",
                "amount_per_unit": "0.50",
                "currency": "AUD",
                "record_date": "2025-03-01",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
    }

    #[tokio::test]
    async fn api_invalid_share_split_payloads_return_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        // Missing ratio, non-positive ratio, and a stray payment field.
        for body in [
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "0", "split_old_units": "1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "-1",
            }),
            serde_json::json!({
                "action_type": "ShareSplit", "listing_id": 1, "date": "2024-11-30",
                "split_new_units": "2", "split_old_units": "1",
                "amount_per_unit": "0.50", "currency": "AUD",
            }),
            // …and a ReturnOfCapital carrying a split ratio.
            serde_json::json!({
                "action_type": "ReturnOfCapital", "listing_id": 1, "date": "2024-11-30",
                "amount_per_unit": "0.50", "currency": "AUD",
                "split_new_units": "2", "split_old_units": "1",
            }),
        ] {
            api_put_expecting(&pool, body, StatusCode::UNPROCESSABLE_ENTITY).await;
        }
    }

    #[tokio::test]
    async fn api_unknown_listing_returns_422() {
        let pool = test_pool().await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 999,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_unknown_currency_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "ZZZ",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    #[tokio::test]
    async fn api_unknown_action_type_rejected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        // Serde rejects an unrecognised enum variant before it reaches the DB.
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "Merger",
                "listing_id": 1,
                "date": "2024-11-30",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }

    // Currency agreement between a payment and the parcels it reduces
    //
    // A return of capital reduces each parcel's cost base in the *parcel's*
    // own currency, and amounts are never netted across currencies, so
    // `RocEvent::per_unit_for` refuses to compute the mismatched pair. Nothing
    // checked it at write time, so the pair was accepted and every cost-base
    // report of the listing then answered 500 with an empty body (SCENARIOS
    // E-07, E-39).

    /// E-07: the typo — an AUD holding, a payment keyed as USD.
    #[tokio::test]
    async fn api_payment_in_another_currency_than_its_parcels_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AAA").await;
        // 100 units @ $10, in the listing's own AUD.
        test_support::buy(1, 1)
            .date(d(2023, 1, 10))
            .insert(&pool)
            .await;

        let payment = |currency: &str| {
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-05-01",
                "amount_per_unit": "0.50",
                "currency": currency,
            })
        };
        let resp = client(&pool)
            .put("/corporate_actions/1", &payment("USD"))
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(
            detail.contains("recorded in USD") && detail.contains("held in AUD"),
            "detail must name both currencies: {detail}"
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_none(),
            "nothing persisted"
        );
        // The reports the accepted pair used to kill still answer, because the
        // state that killed them can no longer be written.
        let full = ApiClient::full(&pool);
        assert_eq!(
            full.get("/portfolio/open-parcels").await.status,
            StatusCode::OK
        );
        // The same payment in the parcels' own currency is accepted.
        api_put_expecting(&pool, payment("AUD"), StatusCode::NO_CONTENT).await;
    }

    /// Re-denominate a parcel the way a **rollover** does: a scrip-for-scrip
    /// or demerger replacement Buy carries its consumed parcel's currency and
    /// FX rate onto the new listing, so the carried cost base is unchanged by
    /// the substitution (`domain::rollover::insert_replacement_buy`, which
    /// writes the row itself). That is the one writer that can leave a parcel
    /// in a currency other than its listing's — `trade::db_upsert` refuses the
    /// pair (SCENARIOS M-08) — and it is exactly the state this check exists
    /// for, so the tests reproduce it directly rather than driving a whole
    /// exchange.
    async fn carried_over_from_a_rollover(pool: &SqlitePool, trade_id: i64, currency: &str) {
        sqlx::query("UPDATE trades SET currency = ?, brokerage_currency = ? WHERE id = ?")
            .bind(currency)
            .bind(currency)
            .bind(trade_id)
            .execute(pool)
            .await
            .unwrap();
    }

    /// E-39: the same trap without a typo — a scrip-for-scrip replacement
    /// parcel keeps the *original's* currency (docs/API.md), so a USD-listed
    /// security can hold AUD parcels, and recording that listing's return of
    /// capital in its own listed currency — the obvious entry — is what breaks
    /// the reports. Refused with the same 422 naming both sides.
    #[tokio::test]
    async fn api_payment_in_the_listed_currency_of_a_carried_over_parcel_returns_422() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("USL")
            .name("USL")
            .currency("USD")
            .security_type(listing::SecurityType::Share)
            .insert(&pool)
            .await;
        // The replacement parcel a rollover leaves behind: AUD cost base
        // carried onto a USD-listed security.
        test_support::buy(1, 1)
            .date(d(2023, 1, 10))
            .insert(&pool)
            .await;
        carried_over_from_a_rollover(&pool, 1, "AUD").await;

        let resp = client(&pool)
            .put(
                "/corporate_actions/1",
                &serde_json::json!({
                    "action_type": "ReturnOfCapital",
                    "listing_id": 1,
                    "date": "2024-05-01",
                    "amount_per_unit": "0.50",
                    "currency": "USD",
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(
            detail.contains("recorded in USD") && detail.contains("held in AUD"),
            "detail must name both currencies: {detail}"
        );
    }

    /// The check is scoped to the parcels the payment actually *reaches* — the
    /// same entitlement test `RocEvent::per_unit_for` applies — so a parcel in
    /// another currency that the payment never reduces is no obstacle.
    #[tokio::test]
    async fn api_payment_currency_check_covers_only_the_parcels_it_reaches() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AAA").await;
        test_support::buy(1, 1)
            .date(d(2023, 1, 10))
            .insert(&pool)
            .await;
        test_support::buy(2, 1)
            .date(d(2025, 1, 6))
            .insert(&pool)
            .await;
        carried_over_from_a_rollover(&pool, 2, "USD").await;

        // Dated before the USD parcel was acquired: it was never entitled.
        api_put_expecting(
            &pool,
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2024-06-01",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            }),
            StatusCode::NO_CONTENT,
        )
        .await;
        // With a record date it is entitlement at *that* date that decides: a
        // parcel acquired on the record date is ex-entitlement …
        let payment = |record: &str| {
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": "2025-06-01",
                "amount_per_unit": "0.50",
                "currency": "AUD",
                "record_date": record,
            })
        };
        let resp = client(&pool)
            .put("/corporate_actions/2", &payment("2025-01-06"))
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
        // … while one acquired the day before it is not, and the same edit is
        // refused — the check runs over the state the write would leave.
        let resp = client(&pool)
            .put("/corporate_actions/2", &payment("2025-01-07"))
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let stored = db_get(&pool, 2).await.unwrap().unwrap();
        assert_eq!(
            stored.kind,
            roc_with_record(2, 1, d(2025, 6, 1), "0.50", d(2025, 1, 6)).kind,
            "the refused edit left the stored terms untouched"
        );
    }

    // A return of capital is refused on an AMIT
    //
    // An AMIT reduces its unit holders' cost base through its AMMA
    // statement's per-unit `cost_base_adjustment` (CGT event E10); the E4
    // mechanism a return of capital models is for non-AMIT trusts. Nothing
    // relates the two, so the same money entered both ways used to reduce the
    // parcel twice with no cross-check catching it (SCENARIOS E-04).

    /// E-04: the double entry — the AMMA statement's 50c/unit generated onto
    /// the parcel, then the same 50c entered again as a payment. The write is
    /// refused, naming where the reduction belongs, and the parcel keeps the
    /// one reduction it should have.
    #[tokio::test]
    async fn api_return_of_capital_on_an_amit_listing_returns_422() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .name("VDHG")
            .amit(true)
            .insert(&pool)
            .await;
        test_support::buy(1, 1)
            .date(d(2023, 1, 10))
            .qty("100".parse().unwrap())
            .price("10".parse().unwrap())
            .insert(&pool)
            .await;
        test_support::amma(1, 1)
            .cost_base_adjustment("0.50".parse().unwrap())
            .with(|a| a.tax_year_end_date = d(2024, 6, 30))
            .insert(&pool)
            .await;
        test_support::amit_adjustment(&pool, 1, 1, 1, "100".parse().unwrap()).await;

        let resp = client(&pool)
            .put(
                "/corporate_actions/1",
                &serde_json::json!({
                    "action_type": "ReturnOfCapital",
                    "listing_id": 1,
                    "date": "2024-05-01",
                    "amount_per_unit": "0.50",
                    "currency": "AUD",
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(
            detail.contains("cost_base_adjustment") && detail.contains("E10"),
            "the 422 must name where the reduction belongs: {detail}"
        );
        assert!(
            db_get(&pool, 1).await.unwrap().is_none(),
            "nothing persisted"
        );

        // The parcel still carries the AMMA statement's reduction alone:
        // 1000 − 50, not the 900 the accepted double entry produced.
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        assert_eq!(parcels.len(), 1);
        assert_eq!(
            parcels[0].amit_cost_base_reduction,
            "50".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            parcels[0].return_of_capital_reduction,
            "0".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            parcels[0].remaining_cost_base,
            "950".parse::<Decimal>().unwrap()
        );
    }

    /// SCENARIOS F-23: on a fund that *converted* to an AMIT, the refusal
    /// follows the payment's own year. The pre-conversion years' tax-deferred
    /// amounts were ordinary E4 reductions and stay both enterable and
    /// editable after the flag goes on — the E4 cross-check asks for them, so
    /// refusing them left a year that could not be completed at all — while a
    /// payment dated in an AMIT year is refused as usual.
    #[tokio::test]
    async fn api_return_of_capital_on_a_converted_fund_follows_the_payments_year() {
        let pool = test_pool().await;
        test_support::listing(1)
            .ticker("VDHG")
            .name("VDHG")
            .amit_from(d(2024, 7, 1)) // first AMIT year: FY2025
            .insert(&pool)
            .await;

        let payment = |date: &str, amount: &str| {
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": 1,
                "date": date,
                "amount_per_unit": amount,
                "currency": "AUD",
            })
        };
        let c = client(&pool);
        // FY2024, before the conversion: accepted…
        assert_eq!(
            c.put("/corporate_actions/1", &payment("2024-05-01", "0.50"))
                .await
                .status,
            StatusCode::NO_CONTENT
        );
        // …and still editable afterwards (correcting the amount years later).
        assert_eq!(
            c.put("/corporate_actions/1", &payment("2024-05-01", "0.55"))
                .await
                .status,
            StatusCode::NO_CONTENT
        );

        // FY2025, the first AMIT year: refused, pointing at the AMMA.
        let resp = c
            .put("/corporate_actions/2", &payment("2025-05-01", "0.50"))
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            resp.text().contains("cost_base_adjustment"),
            "{}",
            resp.text()
        );
        assert!(db_get(&pool, 2).await.unwrap().is_none());
    }

    /// The refusal is keyed on the listing's `amit` flag, so the E4 path it
    /// exists for is untouched: the same payment on an ordinary trust is
    /// accepted, and *moving* an accepted one onto an AMIT is refused (the
    /// check runs over the state the write would leave, like its neighbours).
    #[tokio::test]
    async fn api_return_of_capital_is_refused_only_where_the_listing_is_an_amit() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "TRU").await;
        test_support::listing(2)
            .ticker("VDHG")
            .name("VDHG")
            .amit(true)
            .insert(&pool)
            .await;

        let payment = |listing_id: i64| {
            serde_json::json!({
                "action_type": "ReturnOfCapital",
                "listing_id": listing_id,
                "date": "2024-05-01",
                "amount_per_unit": "0.50",
                "currency": "AUD",
            })
        };
        let resp = client(&pool).put("/corporate_actions/1", &payment(1)).await;
        assert_eq!(
            resp.status,
            StatusCode::NO_CONTENT,
            "a non-AMIT trust's E4 reduction is exactly what this action is for"
        );

        let resp = client(&pool).put("/corporate_actions/1", &payment(2)).await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap().listing_id,
            1,
            "the refused move left the stored row on its own listing"
        );

        // Every other action type is unaffected — only the payment carries a
        // cost-base reduction the AMMA statement already carries.
        let resp = client(&pool)
            .put(
                "/corporate_actions/2",
                &serde_json::json!({
                    "action_type": "ShareSplit",
                    "listing_id": 2,
                    "date": "2024-05-01",
                    "split_new_units": "2",
                    "split_old_units": "1",
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    // Delete-time guard: the three types that create no trades
    //
    // `ShareSplit`, `BonusIssue`, and `ReturnOfCapital` re-base or reduce
    // parcels at read time, so — unlike the five types frozen by their trade
    // group's foreign key — nothing stops a delete from restating figures the
    // reports already produced (SCENARIOS A-06, A-20, A-21).

    /// The parcel the guarded actions below act on: 100 units @ $10 on
    /// 2023-01-10.
    async fn insert_parcel(pool: &SqlitePool) {
        test_support::buy(1, 1)
            .date(d(2023, 1, 10))
            .insert(pool)
            .await;
    }

    /// A Sell of `qty` units, entered on the unit basis in force at `date`,
    /// allocated wholly against parcel 1 — the write path validates the
    /// allocation against the parcel's re-based capacity.
    async fn insert_sell(pool: &SqlitePool, date: NaiveDate, qty: &str) -> Result<(), SellError> {
        sell::db_upsert_sell(
            pool,
            2,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date,
                settlement_date: Some(date),
                listing_id: 1,
                average_price: "6".parse().unwrap(),
                quantity: qty.parse().unwrap(),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: qty.parse().unwrap(),
                }],
            },
        )
        .await
    }

    /// SCENARIOS E-09: a consolidation whose ratio does not divide the
    /// holding evenly, followed by a sale of the whole of it. The reported
    /// remaining quantity is the exact re-based figure (no rounding — company
    /// rounding and cash-in-lieu are not modelled), and selling exactly that
    /// is accepted and consumes the parcel to nothing: the re-basing and its
    /// inverse agree, so no dust is left behind that could never be sold.
    #[tokio::test]
    async fn db_a_consolidation_that_does_not_divide_still_sells_out_exactly() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "CON").await;
        insert_parcel(&pool).await; // 100 units @ $10 on 2023-01-10
        db_upsert(
            &pool,
            &CorporateAction {
                id: 1,
                listing_id: 1,
                date: d(2024, 3, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: "3".parse().unwrap(),
                    split_old_units: "7".parse().unwrap(),
                },
            },
        )
        .await
        .unwrap();

        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        let remaining = parcels[0].remaining_quantity;
        assert_eq!(
            remaining,
            "42.857142857142857142857142857".parse::<Decimal>().unwrap()
        );

        insert_sell(&pool, d(2024, 4, 2), &remaining.to_string())
            .await
            .unwrap();
        assert!(
            crate::reports::open_parcels::db_open_parcels(&pool)
                .await
                .unwrap()
                .is_empty()
        );
        // The whole $1,000 cost base is in the disposal, undiminished.
        let gains = crate::reports::realised_gains::db_realised_gains(&pool)
            .await
            .unwrap();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].cost_base.round_dp(6), "1000".parse().unwrap());
    }

    /// SCENARIOS E-02: a per-unit payment carried to six decimal places (the
    /// scale a registry states a small return of capital at). It survives the
    /// round trip and the reduction is exact — `Decimal` all the way, never a
    /// float and never rounded to cents on the way in.
    #[tokio::test]
    async fn db_a_six_decimal_place_payment_reduces_exactly() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SIX").await;
        test_support::buy(1, 1)
            .date(d(2023, 1, 10))
            .qty("3333".parse().unwrap())
            .price(Decimal::ONE)
            .insert(&pool)
            .await;
        db_upsert(&pool, &roc(1, 1, d(2024, 3, 1), "0.123456"))
            .await
            .unwrap();

        assert_eq!(
            db_get(&pool, 1).await.unwrap().unwrap(),
            roc(1, 1, d(2024, 3, 1), "0.123456")
        );
        let parcels = crate::reports::open_parcels::db_open_parcels(&pool)
            .await
            .unwrap();
        // 3,333 × 0.123456 = 411.478848, to the last digit.
        assert_eq!(
            parcels[0].return_of_capital_reduction,
            "411.478848".parse::<Decimal>().unwrap()
        );
        assert_eq!(
            parcels[0].remaining_cost_base,
            "2921.521152".parse::<Decimal>().unwrap()
        );
    }

    /// A-20: the split is what makes the 200-unit Sell fit the 100-unit
    /// parcel, so deleting it would leave allocations the Sell's own write
    /// path refuses. The delete is refused while that trade stands, and
    /// allowed again once it is gone.
    #[tokio::test]
    async fn db_deleting_a_split_is_refused_while_a_later_trade_is_on_the_post_split_basis() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "2", "1"))
            .await
            .unwrap();
        insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

        let err = db_delete(&pool, 10).await.unwrap_err();
        assert!(matches!(err, DeleteError::RebasedTrades), "{err:?}");
        assert!(db_get(&pool, 10).await.unwrap().is_some(), "still there");

        // With the Sell gone the split deletes freely — and the identical Sell
        // is then refused, which is exactly the state the guard keeps the
        // delete from leaving behind.
        assert!(matches!(
            sell::db_delete_sell(&pool, 2).await.unwrap(),
            sell::DeleteOutcome::Deleted
        ));
        assert!(db_delete(&pool, 10).await.unwrap());
        assert!(matches!(
            insert_sell(&pool, d(2023, 6, 1), "200").await,
            Err(SellError::PurchaseQuantityExceeded)
        ));
    }

    /// The same guard for a `BonusIssue` (it folds into the split-event stream
    /// as its equivalent re-base, and deleting it restates quantities the same
    /// way), and a trade dated *before* the action does not block the delete.
    #[tokio::test]
    async fn db_deleting_a_bonus_issue_is_refused_the_same_way() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BON").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &bonus(10, 1, d(2023, 3, 1), "1", "1"))
            .await
            .unwrap();

        // The parcel predates the issue: nothing is recorded on the post-issue
        // basis yet, so the delete stands.
        assert!(db_delete(&pool, 10).await.unwrap());

        db_upsert(&pool, &bonus(10, 1, d(2023, 3, 1), "1", "1"))
            .await
            .unwrap();
        insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();
        let err = db_delete(&pool, 10).await.unwrap_err();
        assert!(matches!(err, DeleteError::RebasedTrades), "{err:?}");
    }

    /// A-21: deleting a return of capital breaks no quantity invariant, but it
    /// silently restores the cost base it reduced — and drops any CGT event G1
    /// excess gain already reported for the payment's year. Refused while a
    /// parcel it reduced exists; a payment before every acquisition (which
    /// reduces nothing) still deletes.
    #[tokio::test]
    async fn db_deleting_a_return_of_capital_is_refused_while_it_reduced_a_parcel() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RAP").await;
        insert_parcel(&pool).await;

        db_upsert(&pool, &roc(10, 1, d(2023, 6, 1), "0.50"))
            .await
            .unwrap();
        let err = db_delete(&pool, 10).await.unwrap_err();
        assert!(matches!(err, DeleteError::ReducedParcels), "{err:?}");

        // Dated before the parcel was acquired, it reduces nothing.
        db_upsert(&pool, &roc(11, 1, d(2022, 12, 1), "0.50"))
            .await
            .unwrap();
        assert!(db_delete(&pool, 11).await.unwrap());

        // Nor does a payment the parcel was never entitled to: its record date
        // (not the payment date) is what decides which parcels it reached, so
        // the guard bounds the acquisitions it looks for by the same date the
        // cost-base pipeline does.
        db_upsert(
            &pool,
            &roc_with_record(12, 1, d(2023, 6, 1), "0.50", d(2023, 1, 5)),
        )
        .await
        .unwrap();
        assert!(db_delete(&pool, 12).await.unwrap());

        // One day later the parcel *is* entitled, and the guard holds again.
        db_upsert(
            &pool,
            &roc_with_record(13, 1, d(2023, 6, 1), "0.50", d(2023, 1, 11)),
        )
        .await
        .unwrap();
        let err = db_delete(&pool, 13).await.unwrap_err();
        assert!(matches!(err, DeleteError::ReducedParcels), "{err:?}");
    }

    /// The five action types whose trades carry a `*_action_id` back-reference
    /// keep the foreign key as their guard: with no such trade recorded, the
    /// delete is not blocked by the new check.
    #[tokio::test]
    async fn db_deleting_an_unapplied_trade_creating_action_is_unaffected() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "RIT").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &rights(10, 1, d(2023, 6, 1), "1", "4", "2.00"))
            .await
            .unwrap();
        assert!(db_delete(&pool, 10).await.unwrap());
    }

    #[tokio::test]
    async fn api_deleting_a_depended_on_action_returns_422_naming_the_dependency() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "2", "1"))
            .await
            .unwrap();
        insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

        let resp = client(&pool).delete("/corporate_actions/10").await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("trade dated on or after"), "{body}");

        db_upsert(&pool, &roc(11, 1, d(2023, 6, 1), "0.50"))
            .await
            .unwrap();
        let resp = client(&pool).delete("/corporate_actions/11").await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("reduced the cost base"), "{body}");
    }

    // Write-time state check: an edit stays possible, but not into an
    // over-consumed parcel
    //
    // `PUT` is deliberately not frozen the way `DELETE` is — a mis-keyed
    // ratio, date, or amount has to stay correctable — so the state the write
    // leaves behind is checked instead: allocations must still fit the parcels
    // they draw on, the same invariant the delete guard upholds.

    /// The stored terms of the split entered by [`split`], for asserting a
    /// refused write changed nothing.
    async fn stored_split_terms(pool: &SqlitePool, id: i64) -> (NaiveDate, Decimal, Decimal) {
        let stored = db_get(pool, id).await.unwrap().unwrap();
        let ActionKind::ShareSplit {
            split_new_units,
            split_old_units,
        } = stored.kind
        else {
            panic!("not a share split");
        };
        (stored.date, split_new_units, split_old_units)
    }

    /// A-20 reached one verb over: the 2:1 split is what makes the 200-unit
    /// Sell fit the 100-unit parcel, so re-terming it 1:1 — or moving it past
    /// the Sell — would leave the allocations the Sell's own write path
    /// refuses. Both are rejected, and the stored terms are untouched.
    #[tokio::test]
    async fn db_editing_a_split_into_an_over_consumed_parcel_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "2", "1"))
            .await
            .unwrap();
        insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

        let err = db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "1", "1"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, WriteError::AllocationsExceedParcel),
            "{err:?}"
        );

        let err = db_upsert(&pool, &split(10, 1, d(2023, 7, 1), "2", "1"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, WriteError::AllocationsExceedParcel),
            "{err:?}"
        );

        assert_eq!(
            stored_split_terms(&pool, 10).await,
            (d(2023, 3, 1), Decimal::from(2), Decimal::ONE),
            "a refused write must not have been persisted"
        );
    }

    /// The correction path the guard is deliberately not allowed to close: a
    /// re-term that keeps every allocation covered still lands — a wider ratio
    /// (3:1 leaves the parcel worth 300 post-split units), and a date move that
    /// stays before the Sell.
    #[tokio::test]
    async fn db_editing_a_split_that_breaks_nothing_still_lands() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "2", "1"))
            .await
            .unwrap();
        insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

        db_upsert(&pool, &split(10, 1, d(2023, 2, 1), "3", "1"))
            .await
            .unwrap();
        assert_eq!(
            stored_split_terms(&pool, 10).await,
            (d(2023, 2, 1), Decimal::from(3), Decimal::ONE)
        );

        // A return of capital moves cost base, not quantities, so correcting
        // its per-unit amount over the same holding is unaffected by the check.
        db_upsert(&pool, &roc(11, 1, d(2023, 6, 1), "0.50"))
            .await
            .unwrap();
        db_upsert(&pool, &roc(11, 1, d(2023, 6, 1), "0.75"))
            .await
            .unwrap();
    }

    /// The check is over the written state, not the fields that changed, so it
    /// equally covers a *new* consolidation recorded over sales already
    /// allocated in the pre-consolidation basis — the same over-consumption
    /// reached by inserting rather than editing.
    #[tokio::test]
    async fn db_recording_a_consolidation_over_existing_sales_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "CON").await;
        insert_parcel(&pool).await;
        insert_sell(&pool, d(2023, 6, 1), "100").await.unwrap();

        // 1-for-2: the 100 units sold are 200 as-acquired units of a 100-unit
        // parcel.
        let err = db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "1", "2"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, WriteError::AllocationsExceedParcel),
            "{err:?}"
        );
        assert!(db_get(&pool, 10).await.unwrap().is_none(), "not persisted");

        // Dated after the Sell it re-bases nothing already recorded, so it
        // stands.
        db_upsert(&pool, &split(10, 1, d(2023, 8, 1), "1", "2"))
            .await
            .unwrap();
    }

    /// Moving a re-basing action to another listing has to leave *both*
    /// listings legal: the one it lands on, and the one whose split stream it
    /// is removed from.
    #[tokio::test]
    async fn db_moving_a_split_off_a_listing_that_needs_it_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_listing(&pool, 2, "OTH").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "2", "1"))
            .await
            .unwrap();
        insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

        let err = db_upsert(&pool, &split(10, 2, d(2023, 3, 1), "2", "1"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, WriteError::AllocationsExceedParcel),
            "{err:?}"
        );
        assert_eq!(
            db_get(&pool, 10).await.unwrap().unwrap().listing_id,
            1,
            "a refused write must not have been persisted"
        );
    }

    #[tokio::test]
    async fn api_editing_a_split_into_an_over_consumed_parcel_returns_422() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        insert_parcel(&pool).await;
        db_upsert(&pool, &split(10, 1, d(2023, 3, 1), "2", "1"))
            .await
            .unwrap();
        insert_sell(&pool, d(2023, 6, 1), "200").await.unwrap();

        let resp = client(&pool)
            .put(
                "/corporate_actions/10",
                &serde_json::json!({
                    "action_type": "ShareSplit",
                    "listing_id": 1,
                    "date": "2023-03-01",
                    "split_new_units": "1",
                    "split_old_units": "1",
                }),
            )
            .await;
        let (status, body) = resp.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            body.contains("more units than the parcel it draws on holds"),
            "{body}"
        );

        // The same edit against a listing whose allocations still fit lands.
        let resp = client(&pool)
            .put(
                "/corporate_actions/10",
                &serde_json::json!({
                    "action_type": "ShareSplit",
                    "listing_id": 1,
                    "date": "2023-03-01",
                    "split_new_units": "4",
                    "split_old_units": "1",
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::NO_CONTENT);
    }

    /// SCENARIOS N-06, N-07: a rollover stores the cost base and quantity its
    /// replacement parcels carry, so an event back-dated over one restates only
    /// the parcels it consumed. Entering the *same* return of capital before the
    /// transfer reported a $400 cost base; entering it afterwards reported $500
    /// — a $100 understated gain from nothing but the order of entry — and a
    /// back-dated split left the source parcel open again beside the untouched
    /// replacement, reporting 200 units and $750 for a $500 holding of 200. Both
    /// are refused now, naming the operation and the delete-enter-redo recovery.
    #[tokio::test]
    async fn events_back_dated_over_a_rollover_are_refused_naming_it() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "AAA").await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Broker".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1)
            .date(d(2023, 1, 10))
            .qty(Decimal::from(100))
            .price(Decimal::from(5))
            .insert(&pool)
            .await;
        crate::entities::transfer::db_transfer(
            &pool,
            1,
            &crate::entities::transfer::TransferBody {
                listing_id: 1,
                date: d(2023, 8, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: Decimal::from(100),
                }],
                fee_allocations: Vec::new(),
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();

        // A payment before the move, a payment on the day of it, and a
        // back-dated split — each restates parcels the transfer consumed.
        for action in [
            roc(10, 1, d(2023, 5, 1), "1"),
            roc(11, 1, d(2023, 8, 1), "1"),
            split(12, 1, d(2023, 5, 1), "2", "1"),
            bonus(13, 1, d(2023, 5, 1), "1", "10"),
        ] {
            let err = db_upsert(&pool, &action).await.expect_err("refused");
            assert!(
                matches!(err, WriteError::BackDatedOverRollover { .. }),
                "{action:?} → {err:?}"
            );
        }

        // The 422 the web UI shows names the operation to redo and the recovery.
        let resp = client(&pool)
            .put(
                "/corporate_actions/10",
                &serde_json::json!({
                    "action_type": "ReturnOfCapital",
                    "listing_id": 1,
                    "date": "2023-05-01",
                    "amount_per_unit": "1",
                    "currency": "AUD",
                }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
        let detail = resp.text().to_string();
        assert!(
            detail.contains("holding-account transfer #1 on 2023-08-01"),
            "detail: {detail}"
        );
        assert!(detail.contains("Delete that operation"), "detail: {detail}");

        // After the move is fine — the replacement parcel receives it — and so
        // is a rights issue, which creates its own trades rather than restating
        // anything.
        db_upsert(&pool, &roc(14, 1, d(2023, 8, 2), "1"))
            .await
            .unwrap();

        // And nothing of another listing is affected.
        insert_listing(&pool, 2, "BBB").await;
        db_upsert(&pool, &roc(15, 2, d(2023, 5, 1), "1"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn api_get_and_delete_missing_return_404() {
        let pool = test_pool().await;
        let resp = client(&pool).get("/corporate_actions/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);

        let resp = client(&pool).delete("/corporate_actions/999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    // -----------------------------------------------------------------------
    // "A replacement quantity no `Decimal` can hold" — the re-basing actions
    // -----------------------------------------------------------------------

    /// A `ShareSplit` or `BonusIssue` materialises nothing: the re-base is
    /// applied at *read* time, so terms that overflow were accepted `204` and
    /// then killed every open-holdings report of the whole portfolio — a
    /// logged `500` with an empty body — until someone worked out which action
    /// did it, with several of the reports that would have found it among the
    /// ones that were down. Refused at the write instead, `422` naming the
    /// ratio and the holding, and nothing is persisted.
    #[tokio::test]
    async fn api_a_split_that_rebases_a_parcel_beyond_the_decimal_range_is_refused() {
        for (action, body) in [
            // 1000-for-1, and its bonus-issue equivalent (999 new units for
            // every 1 held is the same 1000-for-1 re-base).
            (
                split(10, 1, d(2024, 7, 1), "1000", "1"),
                serde_json::json!({"action_type": "ShareSplit", "listing_id": 1,
                                   "date": "2024-07-01", "split_new_units": "1000",
                                   "split_old_units": "1"}),
            ),
            (
                bonus(10, 1, d(2024, 7, 1), "999", "1"),
                serde_json::json!({"action_type": "BonusIssue", "listing_id": 1,
                                   "date": "2024-07-01", "bonus_units": "999",
                                   "bonus_held_units": "1"}),
            ),
        ] {
            let pool = test_pool().await;
            insert_listing(&pool, 1, "BIG").await;
            // Nil-priced, so the parcel's own cost base is representable
            // (W-e's bound accepts it) and only the re-base is at the ceiling.
            test_support::buy(1, 1)
                .date(d(2020, 10, 1))
                .settlement(d(2020, 10, 1))
                .qty("1000000000000000000000000000".parse().unwrap())
                .price(Decimal::ZERO)
                .insert(&pool)
                .await;

            let err = db_upsert(&pool, &action).await.unwrap_err();
            assert!(
                matches!(err, WriteError::UnrepresentableRebasedQuantity(_)),
                "expected the unrepresentable-quantity refusal, got: {err:?}"
            );

            let response = client(&pool).put("/corporate_actions/10", &body).await;
            let (status, detail) = response.status_and_body();
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
            assert!(
                detail.contains(
                    "quantity 1000000000000000000000000000 × new units 1000 / old units 1"
                ),
                "the ratio and the holding are not named: {detail}"
            );
            assert!(
                detail.contains(&Decimal::MAX.to_string()),
                "the limit is not named: {detail}"
            );

            // Nothing persisted, and the reports the row would have killed
            // still read.
            let stored: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM corporate_actions WHERE id = 10)")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(!stored);
            let client = ApiClient::full(&pool);
            let rows: Vec<serde_json::Value> = client.get_json("/portfolio/open-parcels").await;
            assert_eq!(rows.len(), 1);
            let overview = client
                .post("/portfolio/overview", &serde_json::json!({}))
                .await;
            assert_eq!(overview.status, StatusCode::OK);
        }
    }

    /// The other direction of the same re-base, and the reason it has to be
    /// checked *before* `allocations_fit_parcels`: a **consolidation**
    /// recorded over an existing sale multiplies that sale's allocated units
    /// back up into the parcel's as-acquired basis, and the over-consumption
    /// check computes exactly that figure — so it overflowed inside the check
    /// that would otherwise have refused the write, answering a logged `500`
    /// instead of its own `422`.
    #[tokio::test]
    async fn api_a_consolidation_that_rebases_an_allocation_beyond_the_range_is_refused() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        test_support::buy(1, 1)
            .date(d(2020, 10, 1))
            .settlement(d(2020, 10, 1))
            .qty("1000000000000000000000000000".parse().unwrap())
            .price(Decimal::ZERO)
            .insert(&pool)
            .await;
        // The whole holding sold at nil, allocated 1:1 while no split exists.
        sell::db_upsert_sell(
            &pool,
            2,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2024, 6, 3),
                settlement_date: Some(d(2024, 6, 5)),
                listing_id: 1,
                average_price: Decimal::ZERO,
                quantity: "1000000000000000000000000000".parse().unwrap(),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: "1000000000000000000000000000".parse().unwrap(),
                }],
            },
        )
        .await
        .unwrap();

        // A 1-for-1000 consolidation dated between the parcel and the sale.
        let action = split(10, 1, d(2021, 1, 1), "1", "1000");
        let err = db_upsert(&pool, &action).await.unwrap_err();
        assert!(
            matches!(err, WriteError::UnrepresentableRebasedQuantity(_)),
            "expected the unrepresentable-quantity refusal, got: {err:?}"
        );
        let response = client(&pool)
            .put(
                "/corporate_actions/10",
                &serde_json::json!({"action_type": "ShareSplit", "listing_id": 1,
                                    "date": "2021-01-01", "split_new_units": "1",
                                    "split_old_units": "1000"}),
            )
            .await;
        let (status, detail) = response.status_and_body();
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            detail.contains(
                "quantity_allocated 1000000000000000000000000000 × old units 1000 / new units 1"
            ),
            "the ratio and the allocation are not named: {detail}"
        );
    }

    /// A database may already hold such an action — this rule postdates them,
    /// and the mirror rule that stops one arriving behind an in-range ratio
    /// postdates it again (`trade::UpsertError::UnrepresentableRebasedQuantity`
    /// and its seven siblings). The refusal is judged on the terms being
    /// *written*, never on the stored ones, so the edit that brings the ratio
    /// back inside the range still lands, and the row is still deletable.
    #[tokio::test]
    async fn api_an_already_unrepresentable_action_can_still_be_edited_back_into_range() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        test_support::buy(1, 1)
            .date(d(2020, 10, 1))
            .settlement(d(2020, 10, 1))
            .qty("1000000000000000000000000000".parse().unwrap())
            .price(Decimal::ZERO)
            .insert(&pool)
            .await;
        // Straight into SQLite, behind the guard: only a pre-guard database
        // can hold this, which is precisely what the test is about.
        sqlx::query(
            "INSERT INTO corporate_actions              (id, action_type, listing_id, date, split_new_units, split_old_units)              VALUES (10, 'ShareSplit', 1, '2024-07-01', '1000', '1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // The state the guard exists to prevent: the open-holdings reads are
        // a logged 500 while it stands.
        assert_eq!(
            ApiClient::full(&pool)
                .get("/portfolio/open-parcels")
                .await
                .status,
            StatusCode::INTERNAL_SERVER_ERROR
        );

        // The correction: the same action, re-termed 2-for-1, lands.
        let corrected = split(10, 1, d(2024, 7, 1), "2", "1");
        db_upsert(&pool, &corrected).await.unwrap();
        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert_eq!(
            rows[0]["remaining_quantity"],
            "2000000000000000000000000000"
        );

        // And it is still deletable.
        assert_eq!(
            client(&pool).delete("/corporate_actions/10").await.status,
            StatusCode::NO_CONTENT
        );
    }

    /// SCENARIOS W. A consolidation re-basing a very large holding:
    /// `split_adjusted_quantity` multiplies 1e27 units by the ratio's
    /// numerator (1000) before dividing by its denominator (1e6), and 1e30 is
    /// past `Decimal`'s ~7.9228e28 ceiling even though the answer — 1e24
    /// post-consolidation units — is representable. Driven through the
    /// open-parcels report, the shortest read that re-bases a parcel.
    #[tokio::test]
    async fn api_open_parcels_past_the_old_split_rebasing_ceiling_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        // Nil-priced, so the parcel's own cost base is not what is at the
        // limit (that bound is W-e's); only the unit re-basing is.
        test_support::buy(1, 1)
            .date(d(2024, 1, 15))
            .settlement(d(2024, 1, 15))
            .qty("1000000000000000000000000000".parse().unwrap())
            .price(Decimal::ZERO)
            .insert(&pool)
            .await;
        db_upsert(&pool, &split(10, 1, d(2024, 6, 1), "1000", "1000000"))
            .await
            .unwrap();

        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["remaining_quantity"], "1000000000000000000000000",
            "{rows:?}"
        );
    }

    /// SCENARIOS W. The inverse re-basing, `as_acquired_quantity`, on the Sell
    /// path: 1e24 post-consolidation units allocated back against the parcel
    /// multiply by the denominator (1e6) first — 1e30 — before dividing by the
    /// numerator, though the as-acquired figure (1e27) is exactly the parcel's
    /// own quantity. A **write**, so the panic aborted the Sell.
    #[tokio::test]
    async fn api_sell_past_the_old_as_acquired_rebasing_ceiling_allocates() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        test_support::buy(1, 1)
            .date(d(2024, 1, 15))
            .settlement(d(2024, 1, 15))
            .qty("1000000000000000000000000000".parse().unwrap())
            .price(Decimal::ZERO)
            .insert(&pool)
            .await;
        db_upsert(&pool, &split(10, 1, d(2024, 6, 1), "1000", "1000000"))
            .await
            .unwrap();

        // The whole holding, in post-consolidation units, sold at nil.
        sell::db_upsert_sell(
            &pool,
            2,
            &SellBody {
                brokerage_includes_gst: false,
                statement_total: None,
                holding_account_id: 1,
                date: d(2024, 9, 2),
                settlement_date: Some(d(2024, 9, 2)),
                listing_id: 1,
                average_price: Decimal::ZERO,
                quantity: "1000000000000000000000000".parse().unwrap(),
                currency: "AUD".to_string(),
                brokerage: Decimal::ZERO,
                gst_on_brokerage: Decimal::ZERO,
                brokerage_currency: "AUD".to_string(),
                fx_rate: Decimal::ONE,
                spot_fx_rate: None,
                contract_note_ref: None,
                allocations: vec![AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: "1000000000000000000000000".parse().unwrap(),
                }],
            },
        )
        .await
        .unwrap();

        // The parcel is fully consumed, which is what says the allocation was
        // converted back to the parcel's own 1e27 as-acquired units.
        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert!(rows.is_empty(), "{rows:?}");
    }

    /// SCENARIOS W. A return-of-capital payment re-based across a split it was
    /// quoted after: `RocEvent::per_unit_for` multiplies the per-unit amount
    /// (1e24) by the split's numerator (1e6) before dividing by its
    /// denominator, and 1e30 is past `Decimal`'s ~7.9228e28 ceiling even
    /// though the per-as-acquired-unit figure, 1e27, is representable.
    #[tokio::test]
    async fn api_open_parcels_past_the_old_payment_rebasing_ceiling_reports() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "BIG").await;
        // One nil-priced unit, so only the payment's re-basing is at the
        // ceiling — the parcel's own cost base is nil.
        test_support::buy(1, 1)
            .date(d(2024, 1, 15))
            .settlement(d(2024, 1, 15))
            .qty(Decimal::ONE)
            .price(Decimal::ZERO)
            .insert(&pool)
            .await;
        // A 1,000,000-for-1,000 split, i.e. 1000 new units for each old one.
        db_upsert(&pool, &split(10, 1, d(2024, 6, 3), "1000000", "1000"))
            .await
            .unwrap();
        // …and a payment quoted per *post*-split unit.
        db_upsert(
            &pool,
            &roc(11, 1, d(2024, 7, 1), "1000000000000000000000000"),
        )
        .await
        .unwrap();

        let rows: Vec<serde_json::Value> = ApiClient::full(&pool)
            .get_json("/portfolio/open-parcels")
            .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["return_of_capital_reduction"], "1000000000000000000000000000",
            "{rows:?}"
        );
    }

    /// SCENARIOS W. The price dual of the same re-basing: a figure observed
    /// after a 1000-for-1 split, restated into its own trading day's basis,
    /// multiplies by the numerator (1e6) first. A unit test rather than an
    /// API one because the only production caller is the provider-price
    /// re-basing pass, which would need a stubbed fetch and a stored
    /// `price_as_observed` row to reach one multiplication.
    #[test]
    fn contemporaneous_price_past_the_old_ceiling_still_recovers_the_day() {
        let splits = [split_event(d(2024, 6, 3), "1000000", "1000")];
        let events: Vec<PriceBasisEvent> = splits.iter().map(PriceBasisEvent::from).collect();
        // 1e24 × 1e6 is 1e30 — past the ceiling — while the recovered
        // pre-split price, 1e27, is representable.
        assert_eq!(
            contemporaneous_price(
                "1000000000000000000000000".parse().unwrap(),
                &events,
                d(2024, 5, 1),
                d(2024, 7, 1),
            ),
            "1000000000000000000000000000".parse::<Decimal>().unwrap()
        );
    }
}
