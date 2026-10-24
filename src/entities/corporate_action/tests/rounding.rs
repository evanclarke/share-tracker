//! A non-`Exact` share split or bonus issue rounds each holding onto its
//! unit grid and stores each parcel's count (`rounding.rs`, migration 0055).

use super::*;
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

/// The trades a stored count was computed from are frozen behind it: a trade
/// or sale dated before the action is refused `422` naming the way out, one
/// on or after it is not, and a cost-only edit of an earlier parcel moves no
/// count so it is let through. The grid is frozen the same way.
#[tokio::test]
async fn the_facts_a_stored_count_depends_on_are_frozen() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;

    let buy = |date: &str, qty: &str| {
        json!({
            "trade_type": "Buy", "date": date, "listing_id": 1, "average_price": "10",
            "quantity": qty, "currency": "AUD", "brokerage": "0",
            "brokerage_currency": "AUD", "fx_rate": "1",
        })
    };
    let resp = app.post("/trades", &buy("2024-05-01", "10")).await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("fraction_rule to Exact"), "{body}");
    app.post("/trades", &buy("2024-06-03", "10"))
        .await
        .expect_status(StatusCode::CREATED);

    // A sale before the action, its delete, and a quantity edit: refused.
    app.post("/sells", &sell_json("2024-05-01", &[(1, "5")]))
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
    app.delete("/trades/1")
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
    let mut edited: Value = app.get_json("/trades/1").await;
    edited["quantity"] = json!("51");
    let edited = test_support::writable_body(&edited, &[]);
    app.put("/trades/1", &edited)
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
    // The cost of a parcel is no count: editing it lands.
    let mut priced: Value = app.get_json("/trades/1").await;
    priced["average_price"] = json!("11");
    let priced = test_support::writable_body(&priced, &[]);
    let resp = app.put("/trades/1", &priced).await;
    let (status, body) = resp.status_and_body();
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // The grid: neither the account's scale nor an override may move.
    app.put(
        "/holding_accounts/1",
        &json!({ "name": "Default", "unit_scale": 2 }),
    )
    .await
    .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
    app.post(
        "/holding_unit_scales",
        &json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": 2 }),
    )
    .await
    .expect_status(StatusCode::UNPROCESSABLE_ENTITY);

    // Setting the rule to Exact releases it; setting it back recomputes.
    app.put_ok("/corporate_actions/7", &split_json("Exact", "1", "3"))
        .await;
    app.post("/trades", &buy("2024-05-01", "10"))
        .await
        .expect_status(StatusCode::CREATED);
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    // 110 units → 36: 50/50/10 → 16.67/16.67/3.33 → 17, 16, 3.
    let after = remaining(&pool, d(2024, 6, 3)).await;
    let total: Decimal = after
        .iter()
        .filter(|(id, _)| *id != 3)
        .map(|(_, q)| *q)
        .sum();
    assert_eq!(total, dec("36"));
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
