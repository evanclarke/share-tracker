//! The write operations against their own undo and preview: whatever history
//! of buys, sales, transfers, corporate-action operations, reinvestments and
//! renames came before, every operation's `?dry_run=true` preview stores
//! nothing and answers what the real run then does, and the route that
//! undoes it puts every table back exactly as it was.

use super::*;
use crate::domain::cgt_discount::discount_eligible;
use crate::entities::corporate_action::{
    self, ActionKind, CorporateAction, FractionRule, WorthlessEvent,
};
use crate::entities::drp_enrolment::{self, DrpEnrolment, ResidualHandling};
use crate::infra::id::HoldingAccountId;
use crate::reports::{net_capital_gain, open_parcels, realised_gains, rollover_consistency};
use crate::test_support::{ApiResponse, table_contents, table_differences};
use chrono::{Datelike, Days, NaiveDate, Weekday};
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::{BTreeMap, HashMap};

/// Tables an undo is meant to leave changed: the audit trail records the
/// operation and its undo, and `AUTOINCREMENT` counters never run back.
const CHANGED_BY_AN_UNDO: &[&str] = &["row_history", "sqlite_sequence"];

/// One operation, with the knobs the generator varies.
#[derive(Clone, Debug)]
enum Op {
    Buy {
        account: i64,
        units: i64,
        cents: i64,
    },
    Sell {
        account: i64,
        pct: i64,
        cents: i64,
    },
    Transfer {
        to_two: bool,
        pct: i64,
    },
    ReturnOfCapital {
        cents: i64,
    },
    Split {
        new: i64,
        old: i64,
        rule: FractionRule,
    },
    Bonus {
        units: i64,
        held: i64,
        rule: FractionRule,
    },
    Scrip {
        new: i64,
        old: i64,
        cash_cents: Option<i64>,
        rule: FractionRule,
    },
    Demerger {
        new: i64,
        held: i64,
        pct: i64,
        rule: FractionRule,
    },
    /// Pay the newest `CashInLieu` action's fraction to `account`.
    CashInLieu {
        account: i64,
        cents: i64,
    },
    /// A Buy entered now but dated before every corporate action — behind
    /// whatever has been rounded since.
    BackBuy {
        account: i64,
        units: i64,
        cents: i64,
    },
    /// A Sell dated before every corporate action, of `pct`% of each parcel
    /// `account` held then.
    BackSell {
        account: i64,
        pct: i64,
        cents: i64,
    },
    /// A unit-scale override for account 1's holding of listing 1 (`None`:
    /// unconstrained) — the grid a rounded split/bonus rounds it onto.
    Scale {
        scale: Option<u32>,
    },
    BuyBack {
        pct: i64,
        price_cents: i64,
        dividend_cents: i64,
    },
    Worthless {
        c2: bool,
    },
    Exercise {
        pct: i64,
    },
    SellRights {
        pct: i64,
    },
    Reinvest {
        cash_cents: i64,
        price_cents: i64,
        stated_units: bool,
    },
    Rename,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (1i64..=2, 1i64..=2_000, 1i64..=5_000)
            .prop_map(|(account, units, cents)| Op::Buy { account, units, cents }),
        3 => (1i64..=2, 1i64..=100, 1i64..=5_000)
            .prop_map(|(account, pct, cents)| Op::Sell { account, pct, cents }),
        2 => (any::<bool>(), 1i64..=100).prop_map(|(to_two, pct)| Op::Transfer { to_two, pct }),
        1 => (1i64..=300).prop_map(|cents| Op::ReturnOfCapital { cents }),
        1 => (1i64..=4, 1i64..=4)
            .prop_map(|(new, old)| Op::Split { new, old, rule: FractionRule::Exact }),
        1 => (1i64..=3, 1i64..=7)
            .prop_map(|(units, held)| Op::Bonus { units, held, rule: FractionRule::Exact }),
        1 => (1i64..=3, 1i64..=3, prop::option::of(1i64..=200))
            .prop_map(|(new, old, cash_cents)| Op::Scrip { new, old, cash_cents, rule: FractionRule::Exact }),
        1 => (1i64..=3, 1i64..=5, 1i64..=99)
            .prop_map(|(new, held, pct)| Op::Demerger { new, held, pct, rule: FractionRule::Exact }),
        1 => (1i64..=100, 100i64..=2_000, 0i64..=100)
            .prop_map(|(pct, price_cents, dividend_cents)| Op::BuyBack { pct, price_cents, dividend_cents }),
        1 => any::<bool>().prop_map(|c2| Op::Worthless { c2 }),
        1 => (1i64..=100).prop_map(|pct| Op::Exercise { pct }),
        1 => (1i64..=100).prop_map(|pct| Op::SellRights { pct }),
        2 => (1i64..=50_000, 50i64..=5_000, any::<bool>())
            .prop_map(|(cash_cents, price_cents, stated_units)| Op::Reinvest { cash_cents, price_cents, stated_units }),
        1 => Just(Op::Rename),
    ]
}

/// Every Wednesday from 2021 on: the operations' dates, one a week apart
/// so a corporate action and the operation applying it fit between steps.
fn wednesday(n: usize) -> NaiveDate {
    let first = NaiveDate::from_ymd_opt(2021, 1, 6).unwrap();
    assert_eq!(first.weekday(), Weekday::Wed);
    first + Days::new(14 * n as u64)
}

fn cents(c: i64) -> String {
    Decimal::new(c, 2).to_string()
}

/// `pct`% of `units`, to the whole unit (at least one when there is any);
/// all of them, fraction included, at 100%.
fn share(units: Decimal, pct: i64) -> Decimal {
    if pct == 100 {
        return units;
    }
    (units * Decimal::from(pct) / Decimal::from(100))
        .floor()
        .max(Decimal::ONE)
        .min(units.floor())
}

async fn held(pool: &SqlitePool, date: NaiveDate, account: Option<i64>) -> Vec<(i64, Decimal)> {
    open_parcels::db_open_parcels(pool, Some(date))
        .await
        .unwrap()
        .into_iter()
        .filter(|p| p.listing_id == 1 && account.is_none_or(|a| p.holding_account_id == a))
        .map(|p| (p.trade_id.0, p.remaining_quantity))
        .collect()
}

/// `pct`% of each parcel, as allocations under `key`.
fn allocations(parcels: &[(i64, Decimal)], pct: i64, key: &str) -> Vec<Value> {
    parcels
        .iter()
        .map(|(id, units)| (id, share(*units, pct)))
        .filter(|(_, units)| *units > Decimal::ZERO)
        .map(|(id, units)| json!({ "purchase_trade_id": id, key: units.to_string() }))
        .collect()
}

fn total(allocs: &[Value], key: &str) -> Decimal {
    allocs
        .iter()
        .map(|a| a[key].as_str().unwrap().parse::<Decimal>().unwrap())
        .sum()
}

async fn put_action(pool: &SqlitePool, id: i64, date: NaiveDate, kind: ActionKind) {
    corporate_action::db_upsert(
        pool,
        &CorporateAction {
            id,
            listing_id: ListingId(1),
            date,
            kind,
        },
    )
    .await
    .unwrap();
}

/// What a step runs: the write, whether it takes `?dry_run=true`, and how to
/// find the route that undoes it from the write's answer.
struct Step {
    method: &'static str,
    path: String,
    body: Option<Value>,
    previewable: bool,
    undo: Box<dyn Fn(&Value) -> String>,
    /// A corporate action that has already adjusted a parcel cannot be
    /// deleted (`docs/API.md`, *Deleting an action that is already depended
    /// on*); every operation's undo must succeed.
    undo_may_be_refused: bool,
}

async fn send(app: &ApiClient, method: &str, path: &str, body: Option<&Value>) -> ApiResponse {
    match (method, body) {
        ("PUT", Some(body)) => app.put(path, body).await,
        ("POST", Some(body)) => app.post(path, body).await,
        ("POST", None) => app.post_empty(path).await,
        _ => unreachable!("{method} {path}"),
    }
}

/// Build the step for `op` at index `n`, writing whatever it applies (a
/// corporate action, a distribution) first. `None` when nothing is held for
/// it to act on.
async fn prepare(pool: &SqlitePool, n: usize, op: &Op) -> Option<Step> {
    let action_date = wednesday(n);
    let on = action_date + Days::new(7);
    let action_id = 100 + n as i64;
    let to_sell = |id: &Value| format!("/sells/{}", id["sell"]["id"]);
    Some(match op.clone() {
        Op::Buy {
            account,
            units,
            cents: c,
        } => {
            // Below every id the operations assign (`MAX(id) + 1`).
            let id = 900 - n as i64;
            Step {
                method: "PUT",
                path: format!("/trades/{id}"),
                body: Some(json!({
                    "listing_id": 1, "holding_account_id": account, "date": on,
                    "trade_type": "Buy", "quantity": units.to_string(), "average_price": cents(c),
                    "currency": "AUD", "brokerage": "9.95", "brokerage_currency": "AUD", "fx_rate": "1",
                })),
                previewable: false,
                undo: Box::new(move |_| format!("/trades/{id}")),
                undo_may_be_refused: false,
            }
        }
        Op::Sell {
            account,
            pct,
            cents: c,
        } => {
            let allocs = allocations(
                &held(pool, on, Some(account)).await,
                pct,
                "quantity_allocated",
            );
            if allocs.is_empty() {
                return None;
            }
            Step {
                method: "POST",
                path: "/sells".into(),
                body: Some(json!({
                    "listing_id": 1, "holding_account_id": account, "date": on,
                    "quantity": total(&allocs, "quantity_allocated").to_string(),
                    "average_price": cents(c), "currency": "AUD", "brokerage": "9.95",
                    "brokerage_currency": "AUD", "fx_rate": "1", "allocations": allocs,
                })),
                previewable: true,
                undo: Box::new(|created| format!("/sells/{}", created["id"])),
                undo_may_be_refused: false,
            }
        }
        Op::Transfer { to_two, pct } => {
            let (from, to) = if to_two { (1, 2) } else { (2, 1) };
            let allocs = allocations(&held(pool, on, Some(from)).await, pct, "quantity_allocated");
            if allocs.is_empty() {
                return None;
            }
            Step {
                method: "POST",
                path: "/transfers".into(),
                body: Some(json!({
                    "listing_id": 1, "date": on, "from_account_id": from, "to_account_id": to,
                    "allocations": allocs, "fee_allocations": [],
                })),
                previewable: true,
                undo: Box::new(|created| format!("/transfers/{}", created["transfer"]["id"])),
                undo_may_be_refused: false,
            }
        }
        Op::ReturnOfCapital { cents: c } => Step {
            method: "PUT",
            path: format!("/corporate_actions/{action_id}"),
            body: Some(json!({
                "listing_id": 1, "date": on, "action_type": "ReturnOfCapital",
                "amount_per_unit": cents(c), "currency": "AUD",
            })),
            previewable: false,
            undo: Box::new(move |_| format!("/corporate_actions/{action_id}")),
            undo_may_be_refused: true,
        },
        Op::Split { new, old, rule } => Step {
            method: "PUT",
            path: format!("/corporate_actions/{action_id}"),
            body: Some(json!({
                "listing_id": 1, "date": on, "action_type": "ShareSplit", "fraction_rule": rule,
                "split_new_units": new.to_string(), "split_old_units": old.to_string(),
            })),
            previewable: false,
            undo: Box::new(move |_| format!("/corporate_actions/{action_id}")),
            undo_may_be_refused: true,
        },
        Op::Bonus { units, held, rule } => Step {
            method: "PUT",
            path: format!("/corporate_actions/{action_id}"),
            body: Some(json!({
                "listing_id": 1, "date": on, "action_type": "BonusIssue", "fraction_rule": rule,
                "bonus_units": units.to_string(), "bonus_held_units": held.to_string(),
            })),
            previewable: false,
            undo: Box::new(move |_| format!("/corporate_actions/{action_id}")),
            undo_may_be_refused: true,
        },
        Op::Scrip {
            new,
            old,
            cash_cents,
            rule,
        } => {
            put_action(
                pool,
                action_id,
                on,
                ActionKind::ScripForScrip {
                    fraction_rule: rule,
                    scrip_listing_id: ListingId(2),
                    scrip_new_units: new.into(),
                    scrip_old_units: old.into(),
                    scrip_cash_per_unit: cash_cents.map(|c| Decimal::new(c, 2)),
                    scrip_market_value: cash_cents.map(|_| Decimal::new(250, 2)),
                    scrip_cash_currency: cash_cents.map(|_| "AUD".into()),
                },
            )
            .await;
            Step {
                method: "POST",
                path: format!("/corporate_actions/{action_id}/exchange"),
                body: None,
                previewable: true,
                undo: Box::new(to_sell),
                undo_may_be_refused: false,
            }
        }
        Op::Demerger {
            new,
            held,
            pct,
            rule,
        } => {
            put_action(
                pool,
                action_id,
                on,
                ActionKind::Demerger {
                    fraction_rule: rule,
                    demerger_listing_id: ListingId(3),
                    demerger_new_units: new.into(),
                    demerger_held_units: held.into(),
                    demerger_cost_base_pct: pct.into(),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            )
            .await;
            Step {
                method: "POST",
                path: format!("/corporate_actions/{action_id}/demerge"),
                body: None,
                previewable: true,
                undo: Box::new(to_sell),
                undo_may_be_refused: false,
            }
        }
        Op::BuyBack {
            pct,
            price_cents,
            dividend_cents,
        } => {
            put_action(
                pool,
                action_id,
                action_date,
                ActionKind::BuyBack {
                    buyback_price: Decimal::new(price_cents, 2),
                    buyback_dividend: Decimal::new(dividend_cents.min(price_cents), 2),
                    buyback_franking_credit: Decimal::ZERO,
                    buyback_market_value: Some(Decimal::new(price_cents + 50, 2)),
                    currency: "AUD".into(),
                },
            )
            .await;
            let allocs = allocations(&held(pool, on, Some(1)).await, pct, "quantity_allocated");
            if allocs.is_empty() {
                return None;
            }
            Step {
                method: "POST",
                path: format!("/corporate_actions/{action_id}/participate"),
                body: Some(json!({
                    "date": on, "units": total(&allocs, "quantity_allocated").to_string(),
                    "allocations": allocs,
                })),
                previewable: true,
                undo: Box::new(|created| format!("/sells/{}", created["trade"]["id"])),
                undo_may_be_refused: false,
            }
        }
        Op::Worthless { c2 } => {
            put_action(
                pool,
                action_id,
                on,
                ActionKind::WorthlessShares {
                    worthless_event: if c2 {
                        WorthlessEvent::C2Cancellation
                    } else {
                        WorthlessEvent::G3Declaration
                    },
                },
            )
            .await;
            Step {
                method: "POST",
                path: format!("/corporate_actions/{action_id}/recognise"),
                body: None,
                previewable: true,
                undo: Box::new(to_sell),
                undo_may_be_refused: false,
            }
        }
        Op::Exercise { pct } | Op::SellRights { pct } => {
            put_action(
                pool,
                action_id,
                action_date,
                ActionKind::RightsIssue {
                    rights_units: Decimal::ONE,
                    rights_held_units: Decimal::from(4),
                    exercise_price: Decimal::new(180, 2),
                    currency: "AUD".into(),
                    renounceable: true,
                },
            )
            .await;
            let parcels = held(pool, action_date, Some(1)).await;
            let entitled: Vec<(i64, Decimal)> = parcels
                .iter()
                .map(|(id, units)| (*id, (units / Decimal::from(4)).floor()))
                .filter(|(_, units)| *units > Decimal::ZERO)
                .collect();
            let allocs = allocations(&entitled, pct, "units");
            if allocs.is_empty() {
                return None;
            }
            let units = total(&allocs, "units").to_string();
            if matches!(op, Op::Exercise { .. }) {
                Step {
                    method: "POST",
                    path: format!("/corporate_actions/{action_id}/exercise"),
                    body: Some(json!({ "date": on, "units": units })),
                    previewable: true,
                    undo: Box::new(|created| format!("/trades/{}", created["id"])),
                    undo_may_be_refused: false,
                }
            } else {
                Step {
                    method: "POST",
                    path: format!("/corporate_actions/{action_id}/sell_rights"),
                    body: Some(json!({
                        "date": on, "units": units, "proceeds_per_right": "0.20",
                        "allocations": allocs,
                    })),
                    previewable: true,
                    undo: Box::new(|created| format!("/rights_sales/{}", created["id"])),
                    undo_may_be_refused: false,
                }
            }
        }
        Op::Reinvest {
            cash_cents,
            price_cents,
            stated_units,
        } => {
            let income_id = 500 + n as i64;
            test_support::income(income_id, 1, on)
                .with(|i| {
                    i.unfranked_amount = Decimal::new(cash_cents, 2);
                    i.trust_income = true;
                })
                .insert(pool)
                .await;
            let price = Decimal::new(price_cents, 2);
            let mut body = json!({ "reinvestment_price": price.to_string() });
            if stated_units {
                let units = (Decimal::new(cash_cents, 2) / price).floor();
                body["units"] = json!(units.to_string());
            }
            Step {
                method: "POST",
                path: format!("/income/{income_id}/reinvest"),
                body: Some(body),
                previewable: true,
                undo: Box::new(move |_| format!("/income/{income_id}/reinvest")),
                undo_may_be_refused: false,
            }
        }
        Op::CashInLieu { account, cents: c } => {
            let action: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM corporate_actions WHERE fraction_rule = 'CashInLieu' \
                 ORDER BY date DESC, id DESC LIMIT 1",
            )
            .fetch_optional(pool)
            .await
            .unwrap();
            let action = action?;
            Step {
                method: "POST",
                path: format!("/corporate_actions/{action}/cash_in_lieu"),
                body: Some(json!({
                    "holding_account_id": account, "date": on, "proceeds": cents(c),
                })),
                previewable: true,
                undo: Box::new(|created| format!("/sells/{}", created["id"])),
                undo_may_be_refused: false,
            }
        }
        Op::BackBuy {
            account,
            units,
            cents: c,
        } => {
            // Below every id the operations and `Op::Buy` assign.
            let id = 800 - n as i64;
            Step {
                method: "PUT",
                path: format!("/trades/{id}"),
                body: Some(json!({
                    "listing_id": 1, "holding_account_id": account, "date": wednesday(0),
                    "trade_type": "Buy", "quantity": units.to_string(), "average_price": cents(c),
                    "currency": "AUD", "brokerage": "9.95", "brokerage_currency": "AUD", "fx_rate": "1",
                })),
                previewable: false,
                undo: Box::new(move |_| format!("/trades/{id}")),
                undo_may_be_refused: false,
            }
        }
        Op::BackSell {
            account,
            pct,
            cents: c,
        } => {
            let date = wednesday(0) + Days::new(1);
            let allocs = allocations(
                &held(pool, date, Some(account)).await,
                pct,
                "quantity_allocated",
            );
            if allocs.is_empty() {
                return None;
            }
            Step {
                method: "POST",
                path: "/sells".into(),
                body: Some(json!({
                    "listing_id": 1, "holding_account_id": account, "date": date,
                    "quantity": total(&allocs, "quantity_allocated").to_string(),
                    "average_price": cents(c), "currency": "AUD", "brokerage": "9.95",
                    "brokerage_currency": "AUD", "fx_rate": "1", "allocations": allocs,
                })),
                previewable: true,
                undo: Box::new(|created| format!("/sells/{}", created["id"])),
                undo_may_be_refused: false,
            }
        }
        Op::Scale { scale } => Step {
            method: "POST",
            path: "/holding_unit_scales".into(),
            body: Some(json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": scale })),
            previewable: false,
            undo: Box::new(|created| format!("/holding_unit_scales/{}", created["id"])),
            undo_may_be_refused: false,
        },
        Op::Rename => Step {
            method: "POST",
            path: "/listings/1/renames".into(),
            body: Some(json!({ "effective_date": on, "ticker": format!("R{n}") })),
            previewable: false,
            undo: Box::new(|created| format!("/listings/1/renames/{}", created["id"])),
            undo_may_be_refused: false,
        },
    })
}

/// The stored counts' table, compared by value rather than row: a recompute
/// deletes and re-inserts a row (a parcel sold out before the action and back)
/// under a new `id`, which is no change to what is stored.
const COUNTS_TABLE: &str = "corporate_action_parcel_counts";

/// Run one step: preview it, run it, and when `undo` undo it again,
/// checking the database at each point. `rounded`: the world rounds, so the
/// write — and its undo, itself a write — is also held to the stable
/// distribution ([`stable_counts_hold`]). An undo restores the stored counts
/// exactly when the write moved none of them; when it did, stability makes
/// them a property of the history (a count kept through the write is kept
/// through its undo too), so the undo restores every holding's parcels, total
/// and units converted ([`holdings_restored`]) but not necessarily which
/// parcel holds each grid step.
async fn run(
    pool: &SqlitePool,
    app: &ApiClient,
    step: Step,
    undo: bool,
    rounded: bool,
) -> Result<(), TestCaseError> {
    let what = format!("{} {} {:?}", step.method, step.path, step.body);
    let before = table_contents(pool).await;
    let counts_before = count_rows(pool).await;
    let preview = if step.previewable {
        let dry = send(
            app,
            step.method,
            &format!("{}?dry_run=true", step.path),
            step.body.as_ref(),
        )
        .await;
        let changed = table_differences(&before, &table_contents(pool).await, &[]);
        prop_assert!(changed.is_empty(), "{what}?dry_run=true stored {changed:?}");
        Some(dry)
    } else {
        None
    };
    let real = send(app, step.method, &step.path, step.body.as_ref()).await;
    if real.status == StatusCode::UNPROCESSABLE_ENTITY {
        if let Some(dry) = preview {
            prop_assert_eq!(
                dry.status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{} previewed {}",
                what,
                dry.text()
            );
            prop_assert_eq!(dry.text(), real.text(), "{}: the refusals differ", what);
        }
        let changed = table_differences(&before, &table_contents(pool).await, &[]);
        prop_assert!(changed.is_empty(), "refused {what} stored {changed:?}");
        return Ok(());
    }
    prop_assert!(
        real.status == StatusCode::CREATED || real.status == StatusCode::NO_CONTENT,
        "{what}: {} {}",
        real.status,
        real.text()
    );
    let created: Value = if real.body.is_empty() {
        Value::Null
    } else {
        real.json()
    };
    if let Some(dry) = preview {
        prop_assert_eq!(
            dry.status,
            StatusCode::OK,
            "{} previewed {}",
            what,
            dry.text()
        );
        prop_assert_eq!(
            dry.json::<Value>(),
            created.clone(),
            "{}: preview and write differ",
            what
        );
    }
    let counts_after = count_rows(pool).await;
    if rounded {
        stable_counts_hold(pool, &counts_before, &counts_after, &what).await?;
    }
    if undo {
        let path = (step.undo)(&created);
        let undone = app.delete(&path).await;
        if step.undo_may_be_refused && undone.status == StatusCode::UNPROCESSABLE_ENTITY {
            return Ok(());
        }
        prop_assert_eq!(
            undone.status,
            StatusCode::NO_CONTENT,
            "undo {} of {}: {}",
            path,
            what,
            undone.text()
        );
        let skip: Vec<&str> = CHANGED_BY_AN_UNDO
            .iter()
            .copied()
            .chain([COUNTS_TABLE])
            .collect();
        let changed = table_differences(&before, &table_contents(pool).await, &skip);
        prop_assert!(changed.is_empty(), "undo {path} of {what} left {changed:?}");
        let restored = count_rows(pool).await;
        if rounded {
            stable_counts_hold(pool, &counts_after, &restored, &format!("undo of {what}")).await?;
        }
        if counts_after == counts_before {
            prop_assert_eq!(
                &restored,
                &counts_before,
                "undo {} of {} moved stored counts the write had not",
                path,
                what
            );
        } else {
            holdings_restored(pool, &counts_before, &restored, &what).await?;
        }
    }
    Ok(())
}

/// Every stored count by (action, parcel): (units before, count on the grid,
/// cash-in-lieu fraction).
type CountRows = BTreeMap<(i64, i64), (Decimal, Decimal, Decimal)>;

async fn count_rows(pool: &SqlitePool) -> CountRows {
    let rows: Vec<(i64, i64, String, String, String)> = sqlx::query_as(
        "SELECT corporate_action_id, trade_id, units_before, units_after, in_lieu_units \
         FROM corporate_action_parcel_counts",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let d = |s: &str| s.parse::<Decimal>().unwrap();
    rows.into_iter()
        .map(|(action, trade, before, after, in_lieu)| {
            let in_lieu = d(&in_lieu);
            ((action, trade), (d(&before), d(&after) - in_lieu, in_lieu))
        })
        .collect()
}

/// What an undo restores of the stored counts however the write moved them:
/// the same parcels under every action, and per rounded holding the same
/// units converted and the same total on the grid — every figure the
/// registry's holding is, as against which of its parcels holds a step.
async fn holdings_restored(
    pool: &SqlitePool,
    before: &CountRows,
    restored: &CountRows,
    what: &str,
) -> Result<(), TestCaseError> {
    prop_assert_eq!(
        before.keys().collect::<Vec<_>>(),
        restored.keys().collect::<Vec<_>>(),
        "undo of {} changed which parcels have stored counts",
        what
    );
    let accounts: HashMap<i64, i64> = sqlx::query_as("SELECT id, holding_account_id FROM trades")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .collect();
    let totals = |rows: &CountRows| {
        let mut by_holding: BTreeMap<(i64, i64), (Decimal, Decimal, Decimal)> = BTreeMap::new();
        for ((action, trade), (units, count, in_lieu)) in rows {
            let entry = by_holding.entry((*action, accounts[trade])).or_default();
            entry.0 += *units;
            entry.1 += *count;
            entry.2 += *in_lieu;
        }
        by_holding
    };
    prop_assert_eq!(
        totals(before),
        totals(restored),
        "undo of {} did not restore a holding's units or total",
        what
    );
    Ok(())
}

/// The stable distribution, over one write: in every rounded holding of a
/// split or bonus issue, a parcel whose units before the action did not
/// change, and whose count is still a valid count for it (on the holding's
/// grid, its share's floor or one step over), keeps that count — unless
/// keeping every such count leaves the holding's other parcels unable to
/// absorb its total, each at its share's floor or one step over (and every
/// zero floor taking the step). That is the only thing that may force a
/// registry's count to move (`domain::entitlement_rounding::round_holding`).
async fn stable_counts_hold(
    pool: &SqlitePool,
    before: &CountRows,
    after: &CountRows,
    what: &str,
) -> Result<(), TestCaseError> {
    let holdings: Vec<(i64, i64, i64, Option<u32>)> = sqlx::query_as(
        "SELECT c.corporate_action_id, c.trade_id, t.holding_account_id, \
                CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END \
         FROM corporate_action_parcel_counts c \
         JOIN trades t ON t.id = c.trade_id \
         JOIN corporate_actions a ON a.id = c.corporate_action_id \
         JOIN holding_accounts ha ON ha.id = t.holding_account_id \
         LEFT JOIN holding_unit_scales o \
                ON o.holding_account_id = t.holding_account_id AND o.listing_id = t.listing_id \
         WHERE a.action_type IN ('ShareSplit', 'BonusIssue')",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut by_holding: BTreeMap<(i64, i64), (u32, Vec<i64>)> = BTreeMap::new();
    for (action, trade, account, scale) in holdings {
        let scale = scale.expect("a stored count has a grid");
        by_holding
            .entry((action, account))
            .or_insert((scale, Vec::new()))
            .1
            .push(trade);
    }
    for ((action, account), (scale, trades)) in by_holding {
        let (new, old) = action_ratio(pool, action).await;
        let step = Decimal::new(1, scale);
        let total: Decimal = trades.iter().map(|t| after[&(action, *t)].1).sum();
        let (mut kept, mut lower, mut must, mut may) =
            (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        let mut moved = false;
        for trade in &trades {
            let (units, count, _) = after[&(action, *trade)];
            let share = units * new / old;
            let floor = (share / step).floor() * step;
            let ceiling = if floor == share { floor } else { floor + step };
            match before.get(&(action, *trade)) {
                Some((was_units, was, _))
                    if *was_units == units
                        && *was > Decimal::ZERO
                        && was.normalize().scale() <= scale
                        && *was >= floor
                        && *was <= ceiling =>
                {
                    kept += *was;
                    moved |= *was != count;
                }
                _ => {
                    lower += floor;
                    if floor.is_zero() {
                        must += step;
                    }
                    if floor != share {
                        may += step;
                    }
                }
            }
        }
        let rest = total - kept;
        prop_assert!(
            !moved || rest < lower + must || rest > lower + may,
            "{what}: action {action} account {account} moved a kept count the others could absorb"
        );
    }
    Ok(())
}

/// A ratio-driven action's (new, old): every `old` units become `new`.
async fn action_ratio(pool: &SqlitePool, action: i64) -> (Decimal, Decimal) {
    match corporate_action::db_get(pool, action)
        .await
        .unwrap()
        .unwrap()
        .kind
    {
        ActionKind::ShareSplit {
            split_new_units,
            split_old_units,
            ..
        } => (split_new_units, split_old_units),
        ActionKind::BonusIssue {
            bonus_units,
            bonus_held_units,
            ..
        } => (bonus_held_units + bonus_units, bonus_held_units),
        ActionKind::ScripForScrip {
            scrip_new_units,
            scrip_old_units,
            ..
        } => (scrip_new_units, scrip_old_units),
        ActionKind::Demerger {
            demerger_new_units,
            demerger_held_units,
            ..
        } => (demerger_new_units, demerger_held_units),
        other => panic!("counts stored for {other:?}"),
    }
}

/// The non-`Exact` rules, for a strategy to pick from.
fn rounding_rule() -> impl Strategy<Value = FractionRule> {
    prop_oneof![
        Just(FractionRule::Down),
        Just(FractionRule::Up),
        Just(FractionRule::Nearest),
        Just(FractionRule::CashInLieu),
    ]
}

/// [`op`]'s world with every ratio-driven action rounded (a non-`Exact`
/// rule), plus cash-in-lieu payments — run with account 1 on whole units, so
/// its holdings round and account 2's (no grid) stay exact.
fn rounded_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (1i64..=2, 1i64..=2_000, 1i64..=5_000)
            .prop_map(|(account, units, cents)| Op::Buy { account, units, cents }),
        2 => (1i64..=2, 1i64..=100, 1i64..=5_000)
            .prop_map(|(account, pct, cents)| Op::Sell { account, pct, cents }),
        1 => (any::<bool>(), 1i64..=100).prop_map(|(to_two, pct)| Op::Transfer { to_two, pct }),
        2 => (1i64..=4, 1i64..=4, rounding_rule())
            .prop_map(|(new, old, rule)| Op::Split { new, old, rule }),
        2 => (1i64..=3, 1i64..=7, rounding_rule())
            .prop_map(|(units, held, rule)| Op::Bonus { units, held, rule }),
        1 => (1i64..=3, 1i64..=3, prop::option::of(1i64..=200), rounding_rule())
            .prop_map(|(new, old, cash_cents, rule)| Op::Scrip { new, old, cash_cents, rule }),
        1 => (1i64..=3, 1i64..=5, 1i64..=99, rounding_rule())
            .prop_map(|(new, held, pct, rule)| Op::Demerger { new, held, pct, rule }),
        3 => (1i64..=2, 0i64..=500).prop_map(|(account, cents)| Op::CashInLieu { account, cents }),
        2 => (1i64..=2, 1i64..=300, 1i64..=5_000)
            .prop_map(|(account, units, cents)| Op::BackBuy { account, units, cents }),
        2 => (1i64..=2, 1i64..=100, 1i64..=5_000)
            .prop_map(|(account, pct, cents)| Op::BackSell { account, pct, cents }),
        // A grid only: an unconstrained (`None`) holding goes exact, and an
        // exchange/demerger of a non-terminating exact holding rounded `Up`
        // can overshoot by a step on its 28th digit — an `Exact`-mode defect
        // of its own (TODO.md), not this world's. Dropping the grid is
        // covered by `corporate_action::tests::rounding`.
        1 => (0u32..=2).prop_map(|scale| Op::Scale { scale: Some(scale) }),
    ]
}

/// One stored count as the invariant reads it: action, account, before,
/// after, in lieu, the action's rule and the account's scale.
type StoredCountRow = (i64, i64, String, String, String, String, Option<u32>);

/// One parcel's (before, after, in lieu).
type ParcelCounts = (Decimal, Decimal, Decimal);

/// The stored-count invariant, over every row a rounded action has stored:
/// per (action, holding account) the counts, less any cash-in-lieu fraction,
/// are on the account's grid and sum to the holding's exact count rounded by
/// the action's rule, and each lies within one grid step of its parcel's
/// exact share (no units-received override is ever generated here).
async fn stored_counts_hold(pool: &SqlitePool) -> Result<(), TestCaseError> {
    let rows: Vec<StoredCountRow> = sqlx::query_as(
        "SELECT c.corporate_action_id, t.holding_account_id, c.units_before, c.units_after, \
                c.in_lieu_units, a.fraction_rule, \
                CASE WHEN o.id IS NULL THEN ha.unit_scale ELSE o.unit_scale END \
         FROM corporate_action_parcel_counts c \
         JOIN trades t ON t.id = c.trade_id \
         JOIN corporate_actions a ON a.id = c.corporate_action_id \
         JOIN holding_accounts ha ON ha.id = t.holding_account_id \
         LEFT JOIN holding_unit_scales o \
                ON o.holding_account_id = t.holding_account_id AND o.listing_id = t.listing_id \
         ORDER BY c.corporate_action_id, t.holding_account_id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut holdings: BTreeMap<(i64, i64), Vec<ParcelCounts>> = BTreeMap::new();
    let mut rules = HashMap::new();
    for (action, account, before, after, in_lieu, rule, scale) in rows {
        let d = |s: &str| s.parse::<Decimal>().unwrap();
        holdings
            .entry((action, account))
            .or_default()
            .push((d(&before), d(&after), d(&in_lieu)));
        rules.insert(action, (rule, scale.expect("a stored count has a grid")));
    }
    for ((action, account), parcels) in holdings {
        let (rule, scale) = &rules[&action];
        let (new, old) = action_ratio(pool, action).await;
        let step = Decimal::new(1, *scale);
        let held: Decimal = parcels.iter().map(|(b, _, _)| *b).sum();
        let exact = held * new / old;
        let expected = match rule.as_str() {
            "Up" => {
                exact.round_dp_with_strategy(*scale, rust_decimal::RoundingStrategy::AwayFromZero)
            }
            "Nearest" => exact.round_dp_with_strategy(
                *scale,
                rust_decimal::RoundingStrategy::MidpointAwayFromZero,
            ),
            _ => exact.round_dp_with_strategy(*scale, rust_decimal::RoundingStrategy::ToZero),
        };
        let mut total = Decimal::ZERO;
        for (before, after, in_lieu) in &parcels {
            let on_grid = after - in_lieu;
            prop_assert_eq!(
                on_grid,
                on_grid.round_dp(*scale),
                "action {} account {}",
                action,
                account
            );
            prop_assert!(
                (on_grid - before * new / old).abs() < step,
                "action {action} account {account}: {on_grid} vs exact {}",
                before * new / old
            );
            total += on_grid;
        }
        prop_assert_eq!(total, expected, "action {} account {}", action, account);
    }
    Ok(())
}

async fn world(steps: &[(Op, bool)]) -> Result<(), TestCaseError> {
    world_on(steps, false).await
}

/// The world `steps` run in; `rounded` puts account 1 on whole units and
/// checks the stored-count invariant after every step.
async fn world_on(steps: &[(Op, bool)], rounded: bool) -> Result<(), TestCaseError> {
    let pool = test_pool().await;
    for (id, ticker) in [(1, "AAA"), (2, "BBB"), (3, "CCC")] {
        test_support::listing(id).ticker(ticker).insert(&pool).await;
    }
    let app = ApiClient::full(&pool);
    app.put_ok("/holding_accounts/2", &json!({ "name": "Second" }))
        .await;
    if rounded {
        app.put_ok(
            "/holding_accounts/1",
            &json!({ "name": "Default", "unit_scale": 0 }),
        )
        .await;
    }
    drp_enrolment::db_upsert(
        &pool,
        &DrpEnrolment {
            id: 1,
            holding_account_id: HoldingAccountId(1),
            listing_id: ListingId(1),
            enrolment_date: wednesday(0),
            unenrolment_date: None,
            residual_handling: ResidualHandling::CarryForward,
        },
    )
    .await
    .unwrap();
    test_support::buy(1, 1)
        .date(wednesday(0) - Days::new(7))
        .qty(Decimal::from(1_000))
        .price(Decimal::new(250, 2))
        .insert(&pool)
        .await;
    for (n, (op, undo)) in steps.iter().enumerate() {
        if let Some(step) = prepare(&pool, n, op).await {
            run(&pool, &app, step, *undo, rounded).await?;
            if rounded {
                stored_counts_hold(&pool).await?;
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(test_support::db_proptest_config())]

    /// After any history of buys, sales, transfers, corporate actions and
    /// their operations (scrip exchange, demerger, buy-back, worthless
    /// shares, rights exercise and sale), DRP reinvestments and renames:
    /// every previewable write's `?dry_run=true` stores nothing and answers
    /// what the real run then does (a refusal alike), a refused write stores
    /// nothing, and undoing a write — `DELETE` on the row, the closing Sell,
    /// the transfer, the rights sale, the reinvestment or the rename — puts
    /// every table but the audit trail back exactly as it was.
    #[test]
    fn every_operation_previews_and_undoes_exactly(
        steps in prop::collection::vec((op(), prop::bool::weighted(0.4)), 1..=10),
    ) {
        test_support::block_on(world(&steps))?;
    }

    /// The same, with every ratio-driven action rounded onto account 1's
    /// whole-unit grid (account 2 has none, so its holdings stay exact) and
    /// cash-in-lieu payments, back-dated Buys and Sells (dated before every
    /// action, so each recomputes what has been rounded since) and changes of
    /// account 1's grid among the operations: each previews exactly as it
    /// writes; each write and each undo keeps every stored count the new
    /// total allows (`stable_counts_hold`); an undo restores every table, and
    /// the stored counts exactly when the write moved none — otherwise every
    /// holding's parcels, units and total (`holdings_restored`); and after
    /// every step the stored counts of each rounded holding sum to its exact
    /// count rounded by the action's rule, on the grid, each within one grid
    /// step of its parcel's exact share.
    #[test]
    fn every_rounded_operation_previews_undoes_and_stores_grid_counts(
        steps in prop::collection::vec((rounded_op(), prop::bool::weighted(0.4)), 1..=10),
    ) {
        test_support::block_on(world_on(&steps, true))?;
    }
}

/// One link of a rollover chain, applied to the `nth` listing still held.
#[derive(Clone, Debug)]
enum Link {
    Scrip {
        nth: usize,
        new: i64,
        old: i64,
        cash_cents: Option<i64>,
    },
    Demerger {
        nth: usize,
        new: i64,
        held: i64,
        pct: i64,
    },
    Transfer {
        nth: usize,
        pct: i64,
    },
    Sell {
        nth: usize,
        pct: i64,
    },
    Split {
        nth: usize,
        new: i64,
        old: i64,
    },
    Bonus {
        nth: usize,
        units: i64,
        held: i64,
    },
    ReturnOfCapital {
        nth: usize,
        cents: i64,
    },
}

fn link() -> impl Strategy<Value = Link> {
    prop_oneof![
        (0usize..4, 1i64..=7, 1i64..=7, prop::option::of(1i64..=300)).prop_map(
            |(nth, new, old, cash_cents)| Link::Scrip {
                nth,
                new,
                old,
                cash_cents
            }
        ),
        (0usize..4, 1i64..=7, 1i64..=7, 1i64..=99).prop_map(|(nth, new, held, pct)| {
            Link::Demerger {
                nth,
                new,
                held,
                pct,
            }
        }),
        (0usize..4, 1i64..=100).prop_map(|(nth, pct)| Link::Transfer { nth, pct }),
        (0usize..4, 1i64..=100).prop_map(|(nth, pct)| Link::Sell { nth, pct }),
        (0usize..4, 1i64..=7, 1i64..=7).prop_map(|(nth, new, old)| Link::Split { nth, new, old }),
        (0usize..4, 1i64..=3, 1i64..=7).prop_map(|(nth, units, held)| Link::Bonus {
            nth,
            units,
            held
        }),
        (0usize..4, 1i64..=2_000).prop_map(|(nth, cents)| Link::ReturnOfCapital { nth, cents }),
    ]
}

/// Every open parcel of `listing`, by account: `(trade id, units)`.
async fn parcels_of(
    pool: &SqlitePool,
    date: NaiveDate,
    listing: i64,
    account: i64,
) -> Vec<(i64, Decimal)> {
    open_parcels::db_open_parcels(pool, Some(date))
        .await
        .unwrap()
        .into_iter()
        .filter(|p| p.listing_id == listing && p.holding_account_id == account)
        .map(|p| (p.trade_id.0, p.remaining_quantity))
        .collect()
}

async fn sell_all(
    app: &ApiClient,
    pool: &SqlitePool,
    date: NaiveDate,
    listing: i64,
    account: i64,
    pct: i64,
    price_cents: i64,
) {
    let allocs = allocations(
        &parcels_of(pool, date, listing, account).await,
        pct,
        "quantity_allocated",
    );
    if allocs.is_empty() {
        return;
    }
    let body = json!({
        "listing_id": listing, "holding_account_id": account, "date": date,
        "quantity": total(&allocs, "quantity_allocated").to_string(),
        "average_price": cents(price_cents), "currency": "AUD", "brokerage": "0",
        "brokerage_currency": "AUD", "fx_rate": "1", "allocations": allocs,
    });
    let sold = app.post("/sells", &body).await;
    assert_eq!(sold.status, StatusCode::CREATED, "{body}: {}", sold.text());
}

/// The cost base each original parcel's lineage realised, keyed by the
/// original's acquisition date, after `links` and a sale of everything left.
/// Also answers the cash every return of capital paid, and the G1 gains
/// reported for what it paid past a cost base.
/// G1 gains are reported at the cent, so it also answers how many parcels
/// a return of capital paid on — each one a figure rounded by half a cent.
async fn chain(
    buys: &[(i64, i64, i64)],
    links: &[Link],
    sale_days: u64,
) -> Result<(BTreeMap<NaiveDate, Decimal>, Decimal, Decimal, usize), TestCaseError> {
    let pool = test_pool().await;
    for id in 1..=12 {
        test_support::listing(id)
            .ticker(&format!("L{id}"))
            .insert(&pool)
            .await;
    }
    let app = ApiClient::full(&pool);
    app.put_ok("/holding_accounts/2", &json!({ "name": "Second" }))
        .await;
    for (i, (units, price_cents, brokerage_cents)) in buys.iter().enumerate() {
        test_support::buy(i as i64 + 1, 1)
            .date(wednesday(i))
            .qty(Decimal::from(*units))
            .price(Decimal::new(*price_cents, 2))
            .brokerage(Decimal::new(*brokerage_cents, 2))
            .insert(&pool)
            .await;
    }
    let mut held: Vec<i64> = vec![1];
    let mut next_listing = 2;
    let mut roc_cash = Decimal::ZERO;
    let mut roc_parcels = 0;
    for (n, l) in links.iter().enumerate() {
        if held.is_empty() {
            break;
        }
        let date = wednesday(10 + 2 * n);
        let pick = |nth: usize| held[nth % held.len()];
        match *l {
            Link::Scrip {
                nth,
                new,
                old,
                cash_cents,
            } => {
                let from = pick(nth);
                let to = next_listing;
                put_action(
                    &pool,
                    100 + n as i64,
                    date,
                    ActionKind::ScripForScrip {
                        fraction_rule: FractionRule::Exact,
                        scrip_listing_id: ListingId(to),
                        scrip_new_units: new.into(),
                        scrip_old_units: old.into(),
                        scrip_cash_per_unit: cash_cents.map(|c| Decimal::new(c, 2)),
                        scrip_market_value: cash_cents.map(|_| Decimal::new(375, 2)),
                        scrip_cash_currency: cash_cents.map(|_| "AUD".into()),
                    },
                )
                .await;
                sqlx::query("UPDATE corporate_actions SET listing_id = ? WHERE id = ?")
                    .bind(from)
                    .bind(100 + n as i64)
                    .execute(&pool)
                    .await
                    .unwrap();
                let done = app
                    .post_empty(format!("/corporate_actions/{}/exchange", 100 + n))
                    .await;
                // A partial sale of a lone unit sells it all.
                if done.status == StatusCode::UNPROCESSABLE_ENTITY
                    && done.text().starts_with("nothing of the")
                {
                    held.retain(|h| *h != from);
                    continue;
                }
                prop_assert_eq!(
                    done.status,
                    StatusCode::CREATED,
                    "exchange {:?}: {}",
                    l,
                    done.text()
                );
                held.retain(|h| *h != from);
                held.push(to);
                next_listing += 1;
            }
            Link::Demerger {
                nth,
                new,
                held: per,
                pct,
            } => {
                let from = pick(nth);
                let to = next_listing;
                put_action(
                    &pool,
                    100 + n as i64,
                    date,
                    ActionKind::Demerger {
                        fraction_rule: FractionRule::Exact,
                        demerger_listing_id: ListingId(to),
                        demerger_new_units: new.into(),
                        demerger_held_units: per.into(),
                        demerger_cost_base_pct: pct.into(),
                        demerger_close_date: None,
                        demerger_close_price: None,
                        demerger_close_sourced_from: None,
                        demerger_close_reason: None,
                    },
                )
                .await;
                sqlx::query("UPDATE corporate_actions SET listing_id = ? WHERE id = ?")
                    .bind(from)
                    .bind(100 + n as i64)
                    .execute(&pool)
                    .await
                    .unwrap();
                let done = app
                    .post_empty(format!("/corporate_actions/{}/demerge", 100 + n))
                    .await;
                // A partial sale of a lone unit sells it all.
                if done.status == StatusCode::UNPROCESSABLE_ENTITY
                    && done.text().starts_with("nothing of the")
                {
                    held.retain(|h| *h != from);
                    continue;
                }
                prop_assert_eq!(
                    done.status,
                    StatusCode::CREATED,
                    "demerge {:?}: {}",
                    l,
                    done.text()
                );
                held.push(to);
                next_listing += 1;
            }
            Link::Transfer { nth, pct } => {
                let listing = pick(nth);
                let allocs = allocations(
                    &parcels_of(&pool, date, listing, 1).await,
                    pct,
                    "quantity_allocated",
                );
                if allocs.is_empty() {
                    continue;
                }
                let body = json!({
                    "listing_id": listing, "date": date, "from_account_id": 1, "to_account_id": 2,
                    "allocations": allocs, "fee_allocations": [],
                });
                let done = app.post("/transfers", &body).await;
                prop_assert_eq!(
                    done.status,
                    StatusCode::CREATED,
                    "{}: {}",
                    body,
                    done.text()
                );
            }
            Link::Split { nth, new, old }
            | Link::Bonus {
                nth,
                units: new,
                held: old,
            } => {
                let kind = if matches!(l, Link::Split { .. }) {
                    ActionKind::ShareSplit {
                        fraction_rule: FractionRule::Exact,
                        split_new_units: new.into(),
                        split_old_units: old.into(),
                    }
                } else {
                    ActionKind::BonusIssue {
                        fraction_rule: FractionRule::Exact,
                        bonus_units: new.into(),
                        bonus_held_units: old.into(),
                    }
                };
                put_action(&pool, 100 + n as i64, date, kind).await;
                sqlx::query("UPDATE corporate_actions SET listing_id = ? WHERE id = ?")
                    .bind(pick(nth))
                    .bind(100 + n as i64)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            Link::ReturnOfCapital { nth, cents: c } => {
                let listing = pick(nth);
                let paid_on: Vec<Decimal> = open_parcels::db_open_parcels(&pool, Some(date))
                    .await
                    .unwrap()
                    .iter()
                    .filter(|p| p.listing_id == listing)
                    .map(|p| p.remaining_quantity)
                    .collect();
                roc_parcels += paid_on.len();
                let units: Decimal = paid_on.iter().sum();
                put_action(
                    &pool,
                    100 + n as i64,
                    date,
                    ActionKind::ReturnOfCapital {
                        amount_per_unit: Decimal::new(c, 2),
                        currency: "AUD".into(),
                        record_date: None,
                    },
                )
                .await;
                sqlx::query("UPDATE corporate_actions SET listing_id = ? WHERE id = ?")
                    .bind(listing)
                    .bind(100 + n as i64)
                    .execute(&pool)
                    .await
                    .unwrap();
                roc_cash += units * Decimal::new(c, 2);
            }
            Link::Sell { nth, pct } => {
                for account in [1, 2] {
                    sell_all(&app, &pool, date, pick(nth), account, pct, 333).await;
                }
            }
        }
    }
    let sale = wednesday(10 + 2 * links.len()) + Days::new(sale_days);
    for listing in &held {
        for account in [1, 2] {
            sell_all(&app, &pool, sale, *listing, account, 100, 777).await;
        }
    }
    let left = open_parcels::db_open_parcels(&pool, None).await.unwrap();
    prop_assert!(
        left.is_empty(),
        "still open after selling everything: {:?}",
        left.iter()
            .map(|p| (p.listing_id, p.remaining_quantity))
            .collect::<Vec<_>>()
    );

    let mut realised: BTreeMap<NaiveDate, Decimal> = BTreeMap::new();
    for disposal in realised_gains::db_realised_gains(&pool).await.unwrap() {
        for parcel in &disposal.parcels {
            *realised.entry(parcel.acquisition_date).or_default() += parcel.cost_base;
            let held_over_a_year = discount_eligible(parcel.acquisition_date, disposal.sale_date);
            if parcel.proceeds > parcel.cost_base && !held_over_a_year {
                prop_assert!(
                    disposal.non_discountable_gain > Decimal::ZERO,
                    "{:?}",
                    disposal
                );
            }
        }
    }
    // Nothing changed behind any operation after it ran, so the
    // rollover-consistency check has nothing to say about one beyond the
    // note it always carries on a rollover with a cash component.
    for alert in rollover_consistency::db_rollover_alerts(&pool)
        .await
        .unwrap()
    {
        let flagged: Vec<&String> = alert
            .problems
            .iter()
            .filter(|p| !p.contains("carries a cash component"))
            .collect();
        prop_assert!(flagged.is_empty(), "{:?}: {:?}", alert, flagged);
    }
    let g1: Decimal = net_capital_gain::db_net_capital_gain(&pool)
        .await
        .unwrap()
        .iter()
        .map(|y| y.cgt_event_g1_gain)
        .sum();
    Ok((realised, roc_cash, g1, roc_parcels))
}

proptest! {
    #![proptest_config(test_support::db_proptest_config())]

    /// A rollover — scrip-for-scrip (with or without a cash component) or a
    /// demerger — carries the original's cost base and acquisition date to
    /// the replacement, so after any chain of them, between transfers and
    /// partial sales, splits and bonus issues, selling everything that is left realises exactly each
    /// original parcel's cost base, every unit of it dated from the day the
    /// original was bought; and with returns of capital along the way, the
    /// cost bases realised fall short of it by exactly what was paid less
    /// what was reported as G1 gains.
    #[test]
    fn a_rollover_chain_realises_each_original_cost_base_from_its_own_date(
        buys in prop::collection::vec((1i64..=5_000, 1i64..=10_000, 0i64..=3_000), 1..=3),
        links in prop::collection::vec(link(), 1..=6),
        sale_days in prop::sample::select(vec![0u64, 196, 399, 798]),
    ) {
        test_support::block_on(async move {
            let (realised, roc_cash, g1, roc_parcels) = chain(&buys, &links, sale_days).await?;
            let tolerance = Decimal::new(1, 10);
            let cost = |i: usize| {
                let (units, price_cents, brokerage_cents) = buys[i];
                Decimal::from(units) * Decimal::new(price_cents, 2) + Decimal::new(brokerage_cents, 2)
            };
            if roc_cash.is_zero() {
                for i in 0..buys.len() {
                    let got = realised.get(&wednesday(i)).copied().unwrap_or_default();
                    prop_assert!((got - cost(i)).abs() <= tolerance, "parcel bought {}: cost {}, realised {}", wednesday(i), cost(i), got);
                }
            }
            // Each unit's cost base falls by what a return of capital paid on
            // it until nil, and the rest is a G1 gain (s 104-135).
            let expected = (0..buys.len()).map(cost).sum::<Decimal>() - roc_cash + g1;
            let got: Decimal = realised.values().copied().sum();
            let cents = Decimal::new(5, 3) * Decimal::from(roc_parcels);
            prop_assert!((got - expected).abs() <= tolerance + cents, "realised {}, expected cost - {} paid + {} G1 = {}", got, roc_cash, g1, expected);
            let dates: Vec<_> = (0..buys.len()).map(wednesday).collect();
            for date in realised.keys() {
                prop_assert!(dates.contains(date), "a disposal dated from {}, which no original parcel was bought on", date);
            }
            Ok(())
        })?;
    }
}
