//! A non-`Exact` share split or bonus issue rounds each holding onto its
//! unit grid and stores each parcel's count (`rounding.rs`, migration 0055),
//! recomputed by every write that moves a fact it rests on (migration 0057).

use super::*;
use crate::entities::trade;
use crate::infra::db::write_tx;
use crate::test_support::dec;
use serde_json::{Value, json};

/// Listing 1, account 1 held to whole units, and Buys of the given sizes
/// dated a day apart from 2024-01-02 (trade ids 1, 2, …).
async fn whole_unit_holding(pool: &SqlitePool, parcels: &[&str]) -> ApiClient {
    insert_listing(pool, 1, "RND").await;
    let app = ApiClient::full(pool);
    app.put_ok(
        "/holding_accounts/1",
        &json!({ "name": "Default", "unit_scale": 0 }),
    )
    .await;
    for (i, qty) in parcels.iter().enumerate() {
        let day = u32::try_from(i).unwrap() + 2;
        test_support::buy(i64::try_from(i).unwrap() + 1, 1)
            .qty(dec(qty))
            .date(d(2024, 1, day))
            .insert(pool)
            .await;
    }
    app
}

fn split_json(rule: &str, new: &str, old: &str) -> Value {
    json!({
        "listing_id": 1, "date": "2024-06-03", "action_type": "ShareSplit",
        "split_new_units": new, "split_old_units": old, "fraction_rule": rule,
    })
}

/// Each open parcel's remaining count as at `on`, by trade id.
async fn remaining(pool: &SqlitePool, on: NaiveDate) -> Vec<(i64, Decimal)> {
    let mut rows: Vec<(i64, Decimal)> =
        open_parcels::load(&mut pool.acquire().await.unwrap(), Some(on))
            .await
            .unwrap()
            .into_iter()
            .map(|p| (p.parcel.id.0, p.remaining_as_of.quantity()))
            .collect();
    rows.sort_unstable();
    rows
}

async fn stored(pool: &SqlitePool) -> Vec<(i64, i64, String, String, String)> {
    sqlx::query_as(
        "SELECT corporate_action_id, trade_id, units_before, units_after, in_lieu_units \
         FROM corporate_action_parcel_counts ORDER BY corporate_action_id, trade_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn sell_json(date: &str, allocations: &[(i64, &str)]) -> Value {
    let total: Decimal = allocations.iter().map(|(_, q)| dec(q)).sum();
    let allocations: Vec<Value> = allocations
        .iter()
        .map(|(id, q)| json!({ "purchase_trade_id": id, "quantity_allocated": q }))
        .collect();
    json!({
        "date": date, "listing_id": 1, "average_price": "15", "quantity": total.to_string(),
        "currency": "AUD", "brokerage": "0", "brokerage_currency": "AUD", "fx_rate": "1",
        "allocations": allocations,
    })
}

/// The section's own example: two 50-unit parcels after a 1-for-3
/// consolidation are 33 units, never 16.67 + 16.67 — 17 and 16, the earlier
/// parcel taking the tied step — and each keeps its whole cost base.
#[tokio::test]
async fn a_consolidation_rounds_the_holding_and_stores_each_parcels_count() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    let before = remaining(&pool, d(2024, 6, 1)).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;

    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("17")), (2, dec("16"))]
    );
    assert_eq!(before, [(1, dec("50")), (2, dec("50"))]);
    assert_eq!(
        stored(&pool).await,
        [
            (7, 1, "50".into(), "17".into(), "0".into()),
            (7, 2, "50".into(), "16".into(), "0".into()),
        ]
    );
    // No cost base moves: rounding is no CGT event (TD 2000/10).
    let parcels = open_parcels::load(&mut pool.acquire().await.unwrap(), Some(d(2024, 6, 3)))
        .await
        .unwrap();
    let old = open_parcels::load(&mut pool.acquire().await.unwrap(), Some(d(2024, 6, 1)))
        .await
        .unwrap();
    for (after, before) in parcels.iter().zip(&old) {
        assert_eq!(after.cost_base.adjusted, before.cost_base.adjusted);
    }
    // The activity ledger's running balance lands on the register's 33 too.
    let ledger: Value = app
        .get_json("/portfolio/activity?listing_id=1&price=1")
        .await;
    let units: Vec<&str> = ledger["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["units_after"].as_str().unwrap())
        .collect();
    assert_eq!(units, ["50", "100", "33"]);

    // The whole stored count sells, exactly — and no more.
    app.post("/sells", &sell_json("2024-06-11", &[(2, "17")]))
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
    app.post("/sells", &sell_json("2024-06-11", &[(1, "17"), (2, "16")]))
        .await
        .expect_status(StatusCode::CREATED);
    assert!(remaining(&pool, d(2024, 6, 11)).await.is_empty());
}

/// The rule decides the holding's total; the distribution decides who has
/// it. A bonus issue rounds the same way a split does.
#[tokio::test]
async fn up_nearest_and_a_bonus_issue_round_the_holding_total() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Up", "1", "3"))
        .await;
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("17")), (2, dec("17"))]
    );
    app.put_ok("/corporate_actions/7", &split_json("Nearest", "1", "3"))
        .await;
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("17")), (2, dec("16"))]
    );

    // 101 units, 1-for-3 bonus: 134.67 exact, 134 rounded down.
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["101"]).await;
    app.put_ok(
        "/corporate_actions/8",
        &json!({
            "listing_id": 1, "date": "2024-06-03", "action_type": "BonusIssue",
            "bonus_units": "1", "bonus_held_units": "3", "fraction_rule": "Down",
        }),
    )
    .await;
    assert_eq!(remaining(&pool, d(2024, 6, 3)).await, [(1, dec("134"))]);
}

/// A holding with no grid keeps the exact count and stores nothing; setting
/// the rule back to `Exact` removes the stored counts.
#[tokio::test]
async fn an_unconstrained_holding_and_an_exact_rule_store_nothing() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    assert_eq!(stored(&pool).await.len(), 2);
    app.put_ok("/corporate_actions/7", &split_json("Exact", "1", "3"))
        .await;
    assert!(stored(&pool).await.is_empty());
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await[0].1.round_dp(6),
        dec("16.666667")
    );

    let pool = test_pool().await;
    insert_listing(&pool, 1, "RND").await;
    test_support::buy(1, 1)
        .qty(dec("50"))
        .date(d(2024, 1, 2))
        .insert(&pool)
        .await;
    let app = ApiClient::full(&pool);
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    assert!(stored(&pool).await.is_empty());
}

/// A statement's "units received" replaces the holding's rounded total, and
/// deleting it reverts to the rule's own.
#[tokio::test]
async fn a_units_received_override_replaces_the_rounded_total() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "150"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    // 16.67 + 50 = 66.67, rounded down to 66: 16 + 50.
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("16")), (2, dec("50"))]
    );
    let created: Value = app
        .post(
            "/entitlement_overrides",
            &json!({ "corporate_action_id": 7, "holding_account_id": 1, "units_received": "67" }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("17")), (2, dec("50"))]
    );
    // Off the grid, or not positive: refused, nothing stored.
    for bad in ["66.5", "0"] {
        app.put(
            format!("/entitlement_overrides/{}", created["id"]),
            &json!({ "corporate_action_id": 7, "holding_account_id": 1, "units_received": bad }),
        )
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
    }
    app.delete(format!("/entitlement_overrides/{}", created["id"]))
        .await
        .expect_status(StatusCode::NO_CONTENT);
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("16")), (2, dec("50"))]
    );
    // An Exact action rounds nothing to override.
    app.put_ok("/corporate_actions/7", &split_json("Exact", "1", "3"))
        .await;
    let resp = app
        .post(
            "/entitlement_overrides",
            &json!({ "corporate_action_id": 7, "holding_account_id": 1, "units_received": "67" }),
        )
        .await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("not Exact"), "{body}");
}

fn buy_json(date: &str, qty: &str) -> Value {
    json!({
        "trade_type": "Buy", "date": date, "listing_id": 1, "average_price": "10",
        "quantity": qty, "currency": "AUD", "brokerage": "0",
        "brokerage_currency": "AUD", "fx_rate": "1",
    })
}

/// (parcel, units before, units after) of action 7's stored counts.
async fn counts(pool: &SqlitePool) -> Vec<(i64, String, String)> {
    stored(pool)
        .await
        .into_iter()
        .filter(|row| row.0 == 7)
        .map(|(_, trade, before, after, _)| (trade, before, after))
        .collect()
}

/// The TODO's own case. Parcel A (100 units) consolidated 1-for-3, rounded
/// up, is 34 on the register, and all 34 are sold. A Buy entered afterwards
/// but dated before the consolidation (B, 50) makes the holding 150 → 50: the
/// write recomputes the counts in its own transaction, and A keeps the 34 it
/// sold while B absorbs the difference, 16 — where a fresh distribution would
/// have handed out 33 / 17 and left A's sale over-allocated.
#[tokio::test]
async fn a_back_dated_parcel_absorbs_the_difference_and_a_sold_count_stays() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["100"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Up", "1", "3"))
        .await;
    app.post("/sells", &sell_json("2024-06-11", &[(1, "34")]))
        .await
        .expect_status(StatusCode::CREATED);

    let created: Value = app
        .post("/trades", &buy_json("2024-05-01", "50"))
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    let b = created["id"].as_i64().unwrap();
    assert_eq!(
        counts(&pool).await,
        [
            (1, "100".into(), "34".into()),
            (b, "50".into(), "16".into())
        ]
    );
    assert_eq!(remaining(&pool, d(2024, 6, 11)).await, [(b, dec("16"))]);
    // Deleting it again restores the holding exactly.
    app.delete(format!("/trades/{b}"))
        .await
        .expect_status(StatusCode::NO_CONTENT);
    assert_eq!(counts(&pool).await, [(1, "100".into(), "34".into())]);
}

/// A back-dated write that leaves a later sale no longer fitting the
/// recomputed counts is refused `422`, and nothing is stored. Here the
/// newcomer (one unit, a third of a unit after the consolidation) must have a
/// unit of its own, and the total has none to spare: the kept counts are
/// released, the holding is distributed afresh (16, 16, 1), and parcel 1's
/// sale of 17 no longer fits.
#[tokio::test]
async fn a_back_dated_write_a_later_sale_no_longer_fits_is_refused() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    app.post("/sells", &sell_json("2024-06-11", &[(1, "17")]))
        .await
        .expect_status(StatusCode::CREATED);
    let before = counts(&pool).await;
    let trades: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .unwrap();

    let resp = app.post("/trades", &buy_json("2024-05-01", "1")).await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body.contains("no longer") || body.contains("now holds"),
        "{body}"
    );
    assert_eq!(counts(&pool).await, before);
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, trades);
}

/// Every fact a stored count rests on recomputes it in the writing
/// transaction instead of being frozen: a sale dated before the action and
/// its undo, a quantity edit, a deletion, and the holding's grid — the
/// account's scale and a per-listing override.
#[tokio::test]
async fn every_fact_a_stored_count_rests_on_recomputes_it() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "17".into()), (2, "50".into(), "16".into())]
    );

    // A sale of parcel 1 before the action: 45 → 15 (on the grid exactly, so
    // its 17 is no count for it), parcel 2 keeping its 16 of the 31.
    let sold: Value = app
        .post("/sells", &sell_json("2024-05-01", &[(1, "5")]))
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert_eq!(
        counts(&pool).await,
        [(1, "45".into(), "15".into()), (2, "50".into(), "16".into())]
    );
    app.delete(format!("/sells/{}", sold["id"]))
        .await
        .expect_status(StatusCode::NO_CONTENT);
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "17".into()), (2, "50".into(), "16".into())]
    );

    // The cost of a parcel is no count: editing it moves nothing. Its
    // quantity is: 51 is a share of exactly 17, which it keeps.
    let mut edited: Value = app.get_json("/trades/1").await;
    edited["average_price"] = json!("11");
    edited["quantity"] = json!("51");
    let edited = test_support::writable_body(&edited, &[]);
    let resp = app.put("/trades/1", &edited).await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        counts(&pool).await,
        [(1, "51".into(), "17".into()), (2, "50".into(), "16".into())]
    );

    // The grid: two places, then whole units again by a per-listing
    // override, then back. A count made on another grid is none on this
    // one, so each change redistributes the holding afresh.
    app.put_ok(
        "/holding_accounts/1",
        &json!({ "name": "Default", "unit_scale": 2 }),
    )
    .await;
    assert_eq!(
        counts(&pool).await,
        [
            (1, "51".into(), "17".into()),
            (2, "50".into(), "16.66".into())
        ]
    );
    let scale: Value = app
        .post(
            "/holding_unit_scales",
            &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": 0 }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert_eq!(
        counts(&pool).await,
        [(1, "51".into(), "17".into()), (2, "50".into(), "16".into())]
    );
    app.delete(format!("/holding_unit_scales/{}", scale["id"]))
        .await
        .expect_status(StatusCode::NO_CONTENT);
    assert_eq!(
        counts(&pool).await,
        [
            (1, "51".into(), "17".into()),
            (2, "50".into(), "16.66".into())
        ]
    );

    // An unconstrained override: no grid, so the holding goes exact and
    // stores nothing; deleting it rounds afresh on the account's grid.
    let open: Value = app
        .post(
            "/holding_unit_scales",
            &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": null }),
        )
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    assert!(counts(&pool).await.is_empty());
    app.delete(format!("/holding_unit_scales/{}", open["id"]))
        .await
        .expect_status(StatusCode::NO_CONTENT);
    assert_eq!(
        counts(&pool).await,
        [
            (1, "51".into(), "17".into()),
            (2, "50".into(), "16.66".into())
        ]
    );

    // Deleting a parcel: 51 → 17 of a 17 total, nothing left to move.
    app.delete("/trades/2")
        .await
        .expect_status(StatusCode::NO_CONTENT);
    assert_eq!(counts(&pool).await, [(1, "51".into(), "17".into())]);
}

/// What makes recomputing structural: a write that marks a listing with a
/// rounded split or bonus issue cannot commit until the counts are settled.
/// A transaction that forgets `db_settle` fails at COMMIT on the deferred
/// foreign key 0057 hangs off `rounding_dirty` — and is rolled back whole —
/// while the same write settled commits; an autocommit statement is refused
/// the same way.
#[tokio::test]
async fn a_write_that_forgets_to_settle_cannot_commit() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    let insert = "INSERT INTO trades (id, trade_type, date, settlement_date, listing_id, \
                  average_price, quantity, currency, brokerage, brokerage_currency, fx_rate) \
                  VALUES (9, 'Buy', '2024-05-01', '2024-05-03', 1, '10', '30', 'AUD', '0', \
                  'AUD', '1')";

    let mut tx = write_tx(&pool).await.unwrap();
    sqlx::query(insert).execute(&mut *tx).await.unwrap();
    let err = tx.commit().await.unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM trades WHERE id = 9")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "the unsettled write was rolled back");
    let err = sqlx::query(insert).execute(&pool).await.unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"), "{err}");

    let mut tx = write_tx(&pool).await.unwrap();
    sqlx::query(insert).execute(&mut *tx).await.unwrap();
    db_settle(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    // 130 → 43: 17 and 16 kept, the newcomer's share exactly 10.
    assert_eq!(
        counts(&pool).await,
        [
            (1, "50".into(), "17".into()),
            (2, "50".into(), "16".into()),
            (9, "30".into(), "10".into())
        ]
    );
    let dirty: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rounding_dirty")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dirty, 0);
}

/// The scheduled `settlement-recompute` job rewrites computed settlement
/// dates — a column that marks a rounded listing (0057) — so it settles in its
/// own transaction too: it runs to completion over a rounded listing, and the
/// counts (decided by trade date) do not move.
#[tokio::test]
async fn the_settlement_recompute_job_settles_what_it_rewrites() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    let mut tx = write_tx(&pool).await.unwrap();
    sqlx::query(
        "UPDATE trades SET settlement_date = '2024-01-30', settlement_date_source = 'computed' \
         WHERE id = 1",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    db_settle(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    trade::run_recompute(&pool).await.unwrap();
    let settled: NaiveDate = sqlx::query_scalar("SELECT settlement_date FROM trades WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(settled, d(2024, 1, 30));
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "17".into()), (2, "50".into(), "16".into())]
    );
}

/// A rounded total too small to leave every parcel a unit is refused,
/// naming the parcel and the way out; nothing is stored.
#[tokio::test]
async fn a_rounding_that_empties_a_parcel_is_refused() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["3", "3"]).await;
    let resp = app
        .put("/corporate_actions/7", &split_json("Down", "1", "10"))
        .await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("parcel (trade #1)"), "{body}");
    assert!(body.contains("Exact"), "{body}");
    let actions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM corporate_actions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(actions, 0);
}

/// Rounding with a sale after the action already against the old count:
/// recomputing below it is refused rather than leaving the sale
/// over-allocated.
#[tokio::test]
async fn a_recount_below_a_later_sale_is_refused() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Up", "1", "3"))
        .await;
    app.post("/sells", &sell_json("2024-06-11", &[(1, "17"), (2, "17")]))
        .await
        .expect_status(StatusCode::CREATED);
    app.put("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
}

/// A second rounded action re-bases each parcel through the first one's
/// stored count, not its ratio.
#[tokio::test]
async fn a_later_rounded_action_starts_from_the_earlier_ones_counts() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    app.put_ok(
        "/corporate_actions/8",
        &json!({
            "listing_id": 1, "date": "2024-08-01", "action_type": "ShareSplit",
            "split_new_units": "3", "split_old_units": "2", "fraction_rule": "Down",
        }),
    )
    .await;
    // 17 → 25.5, 16 → 24: holding 49.5 → 49; 25 + 24.
    assert_eq!(
        remaining(&pool, d(2024, 8, 1)).await,
        [(1, dec("25")), (2, dec("24"))]
    );
    let rows = stored(&pool).await;
    assert_eq!(rows[2], (8, 1, "17".into(), "25".into(), "0".into()));
    // Deleting the earlier action recomputes the later one from the raw
    // parcels: 50 → 75 each, 150 in all.
    app.delete("/corporate_actions/7")
        .await
        .expect_status(StatusCode::NO_CONTENT);
    assert_eq!(
        remaining(&pool, d(2024, 8, 1)).await,
        [(1, dec("75")), (2, dec("75"))]
    );
}

mod properties {
    use crate::reports::unrealised_gains::tests::properties::{fact, realise_everything};
    use crate::test_support;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(test_support::db_proptest_config())]

        /// `reports::unrealised_gains`' sell-everything property — the same
        /// strategy, so a seed recorded against either generates the same
        /// history — with every split, bonus issue and exchange rounded onto
        /// whole-unit accounts. Each holding is then a sum of stored grid
        /// counts, so the units the open-parcels schedule prints add up to
        /// the holding row exactly. Under exact fractions they cannot always:
        /// each parcel's remainder after a ratio that does not divide is a
        /// 28-digit rounding of a non-terminating count, and a sum of those
        /// has no single right last digit — the CI seed recorded in
        /// `proptest-regressions` for this file (TODO.md, 2026-10-24: three
        /// Buys, two transfers, a 1-for-4 bonus issue and a 2-for-3 split)
        /// fails the `Exact` run and passes this one.
        #[test]
        fn a_rounded_unrealised_gain_is_what_selling_everything_that_day_realises(
            amit in any::<bool>(),
            facts in prop::collection::vec(fact(), 1..=12),
            gap_days in 1u64..=500,
            price_cents in (1i64..=9_000, 1i64..=9_000),
        ) {
            test_support::block_on(realise_everything(amit, facts, gap_days, price_cents, true))?;
        }
    }
}
