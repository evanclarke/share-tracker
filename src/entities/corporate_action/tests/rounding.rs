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
/// counts do not move (the action has no record date, so its register is
/// every trade dated before it).
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

/// [`split_json`] with a record date: the register the rounding reads.
fn split_on_record(rule: &str, new: &str, old: &str, record: &str) -> Value {
    let mut body = split_json(rule, new, old);
    body["record_date"] = json!(record);
    body
}

/// A Buy traded before the action's date (so stated in the pre-split basis)
/// whose settlement falls after the record date was on no register when the
/// holding was rounded: it is converted on its own, as the settlement
/// obligation it is. Two 50-unit parcels, 1-for-3, rounded down: with no
/// record date (the timetable's register) the holding is 100 → 33, as 17 +
/// 16; with the second parcel settling after the record date, the registered
/// holding is 50 → 16 and the late parcel 50 → 16 alone. The register and the
/// re-base agree either way — every parcel traded before the split is
/// converted, by its stored count.
#[tokio::test]
async fn a_buy_settling_after_the_record_date_is_converted_on_its_own() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50"]).await;
    test_support::buy(2, 1)
        .qty(dec("50"))
        .date(d(2024, 5, 31))
        .settlement(d(2024, 6, 5))
        .insert(&pool)
        .await;
    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "17".into()), (2, "50".into(), "16".into())]
    );

    app.put_ok(
        "/corporate_actions/7",
        &split_on_record("Down", "1", "3", "2024-06-04"),
    )
    .await;
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "16".into()), (2, "50".into(), "16".into())]
    );
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("16")), (2, dec("16"))]
    );
    let action: Value = app.get_json("/corporate_actions/7").await;
    assert_eq!(action["record_date"], "2024-06-04");

    // Settling it on the record date puts it back on the register, and the
    // write recomputes the counts in its own transaction: 100 → 33 again,
    // and — the distribution being stable — the step goes to the later
    // acquisition rather than taking parcel 1's count back.
    let mut tx = write_tx(&pool).await.unwrap();
    sqlx::query("UPDATE trades SET settlement_date = '2024-06-04' WHERE id = 2")
        .execute(&mut *tx)
        .await
        .unwrap();
    db_settle(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "16".into()), (2, "50".into(), "17".into())]
    );
}

/// A Sell traded before the action's date but settling after the record date
/// still had its units on the seller's register: they are rounded with the
/// holding, and the sale delivers its own converted count out of it. Parcels
/// of 50 and 50, 2 sold from the first; 1-for-3 rounded down. By trade date
/// the holding is 98 → 32; on the register it is 100 → 33, of which the
/// 2-unit obligation delivers none (2/3 rounded down), so the parcels share
/// 33 — 16 and 17, each within a unit of its pro-rata share.
#[tokio::test]
async fn a_sell_settling_after_the_record_date_is_rounded_with_the_holding() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    let mut sale = sell_json("2024-05-31", &[(1, "2")]);
    sale["settlement_date"] = json!("2024-06-05");
    let created: Value = app
        .post("/sells", &sale)
        .await
        .expect_status(StatusCode::CREATED)
        .json();
    let sell_id = created["id"].as_i64().unwrap();

    app.put_ok("/corporate_actions/7", &split_json("Down", "1", "3"))
        .await;
    assert_eq!(
        counts(&pool).await,
        [(1, "48".into(), "16".into()), (2, "50".into(), "16".into())]
    );
    app.put_ok(
        "/corporate_actions/7",
        &split_on_record("Down", "1", "3", "2024-06-04"),
    )
    .await;
    assert_eq!(
        counts(&pool).await,
        [(1, "48".into(), "16".into()), (2, "50".into(), "17".into())]
    );
    // The open parcels re-base by those counts: 33 units, all sellable.
    assert_eq!(
        remaining(&pool, d(2024, 6, 11)).await,
        [(1, dec("16")), (2, dec("17"))]
    );
    app.post("/sells", &sell_json("2024-06-11", &[(1, "16"), (2, "17")]))
        .await
        .expect_status(StatusCode::CREATED);
    assert!(remaining(&pool, d(2024, 6, 11)).await.is_empty());

    // Settled by the record date instead, the sale left the register before
    // it was rounded — 98 → 32 — and the later sale of 33 no longer fits.
    let mut tx = write_tx(&pool).await.unwrap();
    sqlx::query("UPDATE trades SET settlement_date = '2024-06-04' WHERE id = ?")
        .bind(sell_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let refused = db_settle(&mut tx).await.unwrap_err();
    assert!(refused.to_string().contains("sale"), "{refused}");
}

/// A trade dated on or after the action's date is stated in the post-action
/// basis however it settles, so a post-ex Buy settled on or before the record
/// date (same-day settlement, a record date after the ex-date) takes no part
/// in the rounding — and a record date before the action's date (a NYSE
/// forward split's) is read the same way as one after it.
#[tokio::test]
async fn a_post_action_trade_takes_no_part_whatever_its_settlement() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50", "50"]).await;
    test_support::buy(3, 1)
        .qty(dec("7"))
        .date(d(2024, 6, 3))
        .settlement(d(2024, 6, 3))
        .insert(&pool)
        .await;
    app.put_ok(
        "/corporate_actions/7",
        &split_on_record("Down", "1", "3", "2024-06-05"),
    )
    .await;
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "17".into()), (2, "50".into(), "16".into())]
    );
    assert_eq!(
        remaining(&pool, d(2024, 6, 3)).await,
        [(1, dec("17")), (2, dec("16")), (3, dec("7"))]
    );
    // A record date before the ex-date: the second parcel (settled
    // 2024-01-05) is registered by it, so nothing changes.
    app.put_ok(
        "/corporate_actions/7",
        &split_on_record("Down", "1", "3", "2024-05-30"),
    )
    .await;
    assert_eq!(
        counts(&pool).await,
        [(1, "50".into(), "17".into()), (2, "50".into(), "16".into())]
    );
}

/// A record date on any other type than a return of capital, share split or
/// bonus issue is refused; on a split it may fall either side of the date.
#[tokio::test]
async fn a_split_record_date_is_stored_either_side_of_its_date() {
    let pool = test_pool().await;
    let app = whole_unit_holding(&pool, &["50"]).await;
    for record in ["2024-05-29", "2024-06-04"] {
        app.put_ok(
            "/corporate_actions/7",
            &split_on_record("Exact", "2", "1", record),
        )
        .await;
        let stored: Option<String> =
            sqlx::query_scalar("SELECT rebase_record_date FROM corporate_actions WHERE id = 7")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored.as_deref(), Some(record));
    }
    let mut bonus = json!({
        "listing_id": 1, "date": "2024-06-03", "action_type": "BonusIssue",
        "bonus_units": "1", "bonus_held_units": "3", "fraction_rule": "Down",
        "record_date": "2024-06-04",
    });
    app.put_ok("/corporate_actions/8", &bonus).await;
    bonus["action_type"] = json!("WorthlessShares");
    app.put("/corporate_actions/9", &bonus)
        .await
        .expect_status(StatusCode::UNPROCESSABLE_ENTITY);
}

mod properties {
    use super::*;
    use crate::reports::unrealised_gains::tests::properties::{fact, realise_everything};
    use proptest::prelude::*;
    use proptest::test_runner::TestCaseError;
    use std::collections::HashMap;

    /// The trading days the straddling property dates its trades on: two
    /// weeks of 2024 with the split's ex-date (2024-06-03) at index 5.
    const DAYS: [(u32, u32); 10] = [
        (5, 27),
        (5, 28),
        (5, 29),
        (5, 30),
        (5, 31),
        (6, 3),
        (6, 4),
        (6, 5),
        (6, 6),
        (6, 7),
    ];
    const EX: usize = 5;

    fn day(i: usize) -> NaiveDate {
        d(2024, DAYS[i].0, DAYS[i].1)
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Rule {
        Down,
        Up,
        Nearest,
        CashInLieu,
    }

    impl Rule {
        fn name(self) -> &'static str {
            match self {
                Rule::Down => "Down",
                Rule::Up => "Up",
                Rule::Nearest => "Nearest",
                Rule::CashInLieu => "CashInLieu",
            }
        }

        /// `num / den` rounded to a whole unit by the rule — computed here
        /// independently of `domain::entitlement_rounding`.
        fn round(self, num: i64, den: i64) -> i64 {
            let floor = num.div_euclid(den);
            let rem = num.rem_euclid(den);
            match self {
                Rule::Down | Rule::CashInLieu => floor,
                Rule::Up => floor + i64::from(rem > 0),
                Rule::Nearest => floor + i64::from(2 * rem >= den),
            }
        }

        /// A settlement obligation's rounding: cash in lieu is the
        /// registered holder's, so the obligation rounds down.
        fn obligation(self) -> Self {
            if self == Rule::CashInLieu {
                Rule::Down
            } else {
                self
            }
        }
    }

    /// One generated history: Buys (trade-day index, quantity, settlement
    /// lag in calendar days) and Sells traded before the split (parcel,
    /// trade-day index, quantity, settlement lag).
    #[derive(Debug, Clone)]
    struct World {
        buys: Vec<(usize, i64, i64)>,
        sells: Vec<(usize, usize, i64, i64)>,
    }

    /// Every invariant the register-by-settlement rounding promises, checked
    /// against an independent model of the register: each straddling Buy is
    /// converted on its own by the rule; the registered parcels' counts sum
    /// to the register's rounded total less each straddling Sell's own
    /// rounded count, each within a unit of its pro-rata share; every count is
    /// whole and positive; and the re-base agrees — each parcel traded before
    /// the split holds exactly its stored count at the split, each one traded
    /// after it its own quantity.
    async fn check(
        pool: &SqlitePool,
        world: &World,
        settled: &[NaiveDate],
        sold_settled: &[NaiveDate],
        record: Option<NaiveDate>,
        (new, old): (i64, i64),
        rule: Rule,
    ) -> Result<(), TestCaseError> {
        let ex = day(EX);
        let registered = |s: NaiveDate| record.is_none_or(|r| s <= r);
        let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
            "SELECT trade_id, units_before, units_after, in_lieu_units \
             FROM corporate_action_parcel_counts WHERE corporate_action_id = 7",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        let stored: HashMap<i64, (Decimal, Decimal)> = rows
            .into_iter()
            .map(|(t, _, after, in_lieu)| (t, (dec(&after), dec(&in_lieu))))
            .collect();

        let mut register = 0i64;
        let mut pending: Vec<i64> = Vec::new();
        let mut held: Vec<(i64, i64)> = Vec::new(); // (trade id, units before)
        for (i, (buy_day, qty, _)) in world.buys.iter().enumerate() {
            let id = i64::try_from(i).unwrap() + 1;
            if *buy_day >= EX {
                prop_assert!(
                    !stored.contains_key(&id),
                    "post-ex parcel {id} was converted"
                );
                continue;
            }
            let sold: i64 = world.sells.iter().filter(|s| s.0 == i).map(|s| s.2).sum();
            let units = qty - sold;
            if !registered(settled[i]) {
                if units > 0 {
                    let (after, in_lieu) = stored[&id];
                    prop_assert_eq!(in_lieu, Decimal::ZERO);
                    prop_assert_eq!(
                        after,
                        Decimal::from(rule.obligation().round(units * new, old)),
                        "straddling Buy {} converted alone",
                        id
                    );
                }
                continue;
            }
            register += units;
            for (s, sale) in world.sells.iter().enumerate().filter(|(_, s)| s.0 == i) {
                if !registered(sold_settled[s]) {
                    register += sale.2;
                    pending.push(sale.2);
                }
            }
            if units > 0 {
                held.push((id, units));
            }
        }
        let rounded = rule.round(register * new, old);
        let target = if pending.is_empty() {
            rounded
        } else {
            rounded
                - pending
                    .iter()
                    .map(|p| rule.obligation().round(p * new, old))
                    .sum::<i64>()
        };
        let sum_units: i64 = held.iter().map(|h| h.1).sum();
        let mut on_grid_total = Decimal::ZERO;
        for (id, units) in &held {
            let (after, in_lieu) = stored[id];
            let on_grid = after - in_lieu;
            prop_assert!(on_grid > Decimal::ZERO || after > Decimal::ZERO);
            prop_assert_eq!(on_grid, on_grid.trunc(), "parcel {} off the grid", id);
            on_grid_total += on_grid;
            // Within a unit of its share: `units × new / old`, or its
            // pro-rata part of what the straddling Sells leave.
            let (num, den) = if pending.is_empty() {
                (units * new, old)
            } else {
                (units * target, sum_units)
            };
            let scaled = on_grid * Decimal::from(den) - Decimal::from(num);
            prop_assert!(
                scaled.abs() < Decimal::from(den),
                "parcel {id}: {on_grid} vs {num}/{den}"
            );
        }
        if !held.is_empty() {
            prop_assert_eq!(on_grid_total, Decimal::from(target));
        }

        // The re-base reads the same counts.
        let at_ex = open_parcels::load(&mut pool.acquire().await.unwrap(), Some(ex))
            .await
            .unwrap();
        for p in at_ex {
            let id = p.parcel.id.0;
            let expected = match stored.get(&id) {
                Some((after, _)) => *after,
                None => {
                    let i = usize::try_from(id - 1).unwrap();
                    prop_assert!(world.buys[i].0 >= EX, "parcel {id} has no count");
                    Decimal::from(world.buys[i].1)
                }
            };
            prop_assert_eq!(p.remaining_as_of.quantity(), expected);
        }
        Ok(())
    }

    fn world() -> impl Strategy<Value = World> {
        prop::collection::vec((0usize..=7, 5i64..=200, 0i64..=6), 1..=4).prop_flat_map(|buys| {
            let n = buys.len();
            let sells = prop::collection::vec((0..n, 0usize..EX, 1i64..=60, 0i64..=6), 0..=3);
            (Just(buys), sells).prop_map(|(buys, sells)| World { buys, sells })
        })
    }

    async fn straddling(
        world: World,
        record: Option<i64>,
        (new, old): (i64, i64),
        rule: Rule,
        edit: (usize, i64),
    ) -> Result<(), TestCaseError> {
        let pool = test_pool().await;
        let app = whole_unit_holding(&pool, &[]).await;
        let ex = day(EX);
        let record = record.map(|r| ex + chrono::Duration::days(r));
        let mut settled = Vec::new();
        for (i, (buy_day, qty, lag)) in world.buys.iter().enumerate() {
            let date = day(*buy_day);
            let settlement = date + chrono::Duration::days(*lag);
            test_support::buy(i64::try_from(i).unwrap() + 1, 1)
                .qty(Decimal::from(*qty))
                .date(date)
                .settlement(settlement)
                .insert(&pool)
                .await;
            settled.push(settlement);
        }
        // Each Sell traded before the split, after its parcel, for what is
        // left of it; one that would sell nothing is dropped.
        let mut left: Vec<i64> = world.buys.iter().map(|b| b.1).collect();
        let mut kept = World {
            buys: world.buys.clone(),
            sells: Vec::new(),
        };
        let mut sold_settled = Vec::new();
        for (parcel, sale_day, qty, lag) in world.sells {
            let buy_day = world.buys[parcel].0;
            let qty = qty.min(left[parcel]);
            if buy_day >= EX || qty == 0 {
                continue;
            }
            let sale_day = sale_day.max(buy_day);
            let date = day(sale_day);
            let settlement = date + chrono::Duration::days(lag);
            let mut body = sell_json(
                &date.to_string(),
                &[(i64::try_from(parcel).unwrap() + 1, &qty.to_string())],
            );
            body["settlement_date"] = json!(settlement.to_string());
            app.post("/sells", &body)
                .await
                .expect_status(StatusCode::CREATED);
            left[parcel] -= qty;
            kept.sells.push((parcel, sale_day, qty, lag));
            sold_settled.push(settlement);
        }

        let mut action = split_json(rule.name(), &new.to_string(), &old.to_string());
        if let Some(r) = record {
            action["record_date"] = json!(r.to_string());
        }
        let resp = app.put("/corporate_actions/7", &action).await;
        if resp.status == StatusCode::UNPROCESSABLE_ENTITY {
            // Only the documented refusal: a parcel rounded to nothing.
            prop_assert!(resp.text().contains("with no units"), "{}", resp.text());
            return Ok(());
        }
        prop_assert!(resp.status.is_success(), "{}", resp.text());
        check(
            &pool,
            &kept,
            &settled,
            &sold_settled,
            record,
            (new, old),
            rule,
        )
        .await?;

        // Move one parcel's settlement date — across the record date or not —
        // and the write recomputes the counts, which hold up the same way.
        let (which, lag) = edit;
        let which = which % kept.buys.len();
        let settlement = day(kept.buys[which].0) + chrono::Duration::days(lag);
        let mut tx = write_tx(&pool).await.unwrap();
        sqlx::query("UPDATE trades SET settlement_date = ? WHERE id = ?")
            .bind(settlement)
            .bind(i64::try_from(which).unwrap() + 1)
            .execute(&mut *tx)
            .await
            .unwrap();
        if let Err(refused) = db_settle(&mut tx).await {
            // A recount that empties a parcel (or no longer fits a sale —
            // there are none after the split here) is refused, storing
            // nothing.
            prop_assert!(refused.to_string().contains("no units"), "{refused}");
            return Ok(());
        }
        tx.commit().await.unwrap();
        settled[which] = settlement;
        check(
            &pool,
            &kept,
            &settled,
            &sold_settled,
            record,
            (new, old),
            rule,
        )
        .await
    }

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

        /// Trades straddling a rounded split — traded before its ex-date and
        /// settled after its record date, or the reverse — against an
        /// independent model of the register (see [`check`]), after the
        /// split is recorded and again after a parcel's settlement date
        /// moves. The record date falls up to three days either side of the
        /// ex-date, or is absent (the timetable's register).
        #[test]
        fn a_rounded_split_rounds_the_register_at_its_record_date(
            world in world(),
            record in prop::option::of(-3i64..=3),
            new in 1i64..=4,
            old in 1i64..=4,
            rule in prop_oneof![
                Just(Rule::Down),
                Just(Rule::Up),
                Just(Rule::Nearest),
                Just(Rule::CashInLieu),
            ],
            edit in (0usize..4, 0i64..=6),
        ) {
            test_support::block_on(straddling(world, record, (new, old), rule, edit))?;
        }
    }
}
