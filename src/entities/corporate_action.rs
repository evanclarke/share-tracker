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
//! [`split_ratio`] / [`split_adjusted_quantity`] / [`as_acquired_quantity`].
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
    RocEvent, SplitEvent, as_acquired_quantity, db_return_of_capital_events, db_share_split_events,
    db_splits_for_listing, per_unit_reduction, sold_in_acquired_units, split_adjusted_quantity,
    split_ratio,
};
pub use db::db_get_tx;
pub use http::router;
pub use model::{ActionKind, CorporateAction, WorthlessEvent};

/// Referenced by name only from other modules' tests (production code calls
/// these through the HTTP routes in `http.rs`, not this re-export), so the
/// re-export is test-gated to keep the non-test build warning-free — same
/// reasoning as `trade.rs`'s `UpsertError` re-export.
#[cfg(test)]
pub use db::{WriteError, db_delete, db_get, db_list, db_upsert};

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
    use crate::entities::listing;
    use crate::test_support::{self, test_pool};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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
            },
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
            },
        }
    }

    fn split_event(date: NaiveDate, new: &str, old: &str) -> SplitEvent {
        SplitEvent {
            date,
            new_units: new.parse().unwrap(),
            old_units: old.parse().unwrap(),
        }
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
            },
            RocEvent {
                date: d(2024, 6, 1),
                amount_per_unit: "0.20".parse().unwrap(),
                currency: "AUD".into(),
            },
            RocEvent {
                date: d(2025, 1, 1),
                amount_per_unit: "0.40".parse().unwrap(),
                currency: "AUD".into(),
            },
        ];
        // Acquired between the first and second events: the first doesn't apply.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 3, 1), None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Acquired on the event date: held on the payment date, so it applies.
        let pu = per_unit_reduction(&events, &[], "AUD", d(2024, 6, 1), None).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
    }

    #[test]
    fn per_unit_reduction_bounds_at_sale_date() {
        let events = vec![
            RocEvent {
                date: d(2024, 6, 1),
                amount_per_unit: "0.20".parse().unwrap(),
                currency: "AUD".into(),
            },
            RocEvent {
                date: d(2025, 1, 1),
                amount_per_unit: "0.40".parse().unwrap(),
                currency: "AUD".into(),
            },
        ];
        // Sold between the events: only the payment received while held applies.
        let pu =
            per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), Some(d(2024, 9, 1))).unwrap();
        assert_eq!(pu, "0.20".parse::<Decimal>().unwrap());
        // Sold on the payment date: still held at the payment, so it applies.
        let pu =
            per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), Some(d(2025, 1, 1))).unwrap();
        assert_eq!(pu, "0.60".parse::<Decimal>().unwrap());
        // Sold before any payment: unaffected.
        let pu =
            per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), Some(d(2024, 5, 1))).unwrap();
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
            },
            // After a 2-for-1 split: each as-acquired unit receives it twice.
            RocEvent {
                date: d(2024, 9, 1),
                amount_per_unit: "0.20".parse().unwrap(),
                currency: "AUD".into(),
            },
        ];
        let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
        let pu = per_unit_reduction(&events, &splits, "AUD", d(2024, 1, 1), None).unwrap();
        // 0.30 + 0.20 × 2 = 0.70 per as-acquired unit.
        assert_eq!(pu, "0.70".parse::<Decimal>().unwrap());

        // A parcel acquired after the split holds post-split units already:
        // the later payment applies unscaled.
        let pu = per_unit_reduction(&events, &splits, "AUD", d(2024, 7, 1), None).unwrap();
        assert_eq!(pu, "0.20".parse::<Decimal>().unwrap());
    }

    #[test]
    fn per_unit_reduction_rejects_currency_mismatch() {
        let events = vec![RocEvent {
            date: d(2024, 6, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "USD".into(),
        }];
        // Never net amounts across currencies: fail loudly, don't skip or zero.
        assert!(per_unit_reduction(&events, &[], "AUD", d(2024, 1, 1), None).is_err());
        // An out-of-range event in another currency is not an error — it doesn't
        // participate in the calculation at all.
        assert!(per_unit_reduction(&events, &[], "AUD", d(2024, 7, 1), None).is_ok());
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
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/corporate_actions/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/corporate_actions/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let got: CorporateAction = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(got.kind, roc(1, 1, d(2024, 11, 30), "0.50").kind);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/corporate_actions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let items: Vec<CorporateAction> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(items.len(), 1);

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/corporate_actions/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(db_get(&pool, 1).await.unwrap().is_none());
    }

    async fn api_put_expecting(pool: &SqlitePool, body: serde_json::Value, expected: StatusCode) {
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/corporate_actions/1")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), expected);
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
            }
        );
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
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "-4", "exercise_price": "1.80",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "0",
                "currency": "AUD",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "amount_per_unit": "0.50",
            }),
            serde_json::json!({
                "action_type": "RightsIssue", "listing_id": 1, "date": "2024-11-30",
                "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
                "currency": "AUD", "split_new_units": "2", "split_old_units": "1",
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
                "currency": "AUD", "buyback_market_value": "10.20",
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

    #[tokio::test]
    async fn api_get_and_delete_missing_return_404() {
        let pool = test_pool().await;
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri("/corporate_actions/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/corporate_actions/999")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
