//! Every write naming a listing or holding account: a body naming one that
//! does not exist is the client's mistake, so it answers `422` with a
//! plain-text reason — never the `500` a read run ahead of the insert's
//! foreign-key check used to turn its `RowNotFound` into.

use super::*;
use crate::api_spec;
use crate::test_support::{amma, buy, dec, listing, ymd};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The id every case swaps in for a listing or holding account: no fixture
/// row carries it.
const MISSING: i64 = 999;

/// One write route, with a body the base world accepts.
struct Case {
    /// `"put"` or `"post"`.
    verb: &'static str,
    path: String,
    body: Value,
    /// Rows the write needs beyond [`seed`]'s, written through the API
    /// first: the corporate action an operation acts on.
    setup: Vec<(&'static str, Value)>,
    /// The same path with its listing segment naming [`MISSING`], for a
    /// route keyed by the listing it writes under — a `404`, as any path
    /// parameter naming no row is.
    missing_path: Option<String>,
}

impl Case {
    fn new(verb: &'static str, path: impl Into<String>, body: Value) -> Self {
        Self {
            verb,
            path: path.into(),
            body,
            setup: Vec::new(),
            missing_path: None,
        }
    }

    fn after(mut self, path: &'static str, body: Value) -> Self {
        self.setup.push((path, body));
        self
    }

    fn missing_path(mut self, path: &str) -> Self {
        self.missing_path = Some(path.to_owned());
        self
    }

    /// The case's documented route: `verb` and the `paths` key its concrete
    /// path instantiates.
    fn documented_route(&self, templates: &[String]) -> (String, String) {
        let segments: Vec<&str> = self.path.split('/').collect();
        let template = templates
            .iter()
            .find(|template| {
                let pattern: Vec<&str> = template.split('/').collect();
                pattern.len() == segments.len()
                    && pattern
                        .iter()
                        .zip(&segments)
                        .all(|(p, s)| p.starts_with('{') || p == s)
            })
            .unwrap_or_else(|| panic!("no documented path matches {}", self.path));
        (self.verb.to_owned(), template.clone())
    }
}

/// The `PUT /x/{id}` upsert and the `POST /x` create of one entity, which
/// take the same body.
fn upsert_and_create(collection: &'static str, body: Value) -> [Case; 2] {
    [
        Case::new("put", format!("{collection}/10"), body.clone()),
        Case::new("post", collection, body),
    ]
}

/// Is `key` a reference to a listing or a holding account?
fn names_a_listing_or_account(key: &str) -> bool {
    key.ends_with("listing_id") || key.ends_with("account_id")
}

/// The world every case writes into: accounts 1 and 2 on whole units,
/// listings 1 (AMIT) and 2 (a rollover target), 3 (non-AMIT, for franked
/// income) and 4 (USD-quoted, for an ESS statement), a 100-unit Buy of
/// listing 1 in each account (trades 1 and 2), and an AMMA statement.
async fn seed(pool: &SqlitePool) -> ApiClient {
    let app = ApiClient::full(pool);
    for (id, name) in [(1, "Default"), (2, "Second")] {
        app.put_ok(
            format!("/holding_accounts/{id}"),
            &json!({ "name": name, "unit_scale": 0 }),
        )
        .await;
    }
    listing(1).amit(true).insert(pool).await;
    listing(2).ticker("TWO").insert(pool).await;
    listing(3).ticker("NAM").insert(pool).await;
    listing(4).ticker("USDL").currency("USD").insert(pool).await;
    for (id, account) in [(1, 1), (2, 2)] {
        buy(id, 1)
            .qty(dec("100"))
            .date(ymd(2024, 1, 10))
            .settlement(ymd(2024, 1, 12))
            .account(account)
            .insert(pool)
            .await;
    }
    amma(1, 1).insert(pool).await;
    app
}

/// Every write route whose body (or path) names a listing or holding
/// account, with a body [`seed`]'s world accepts.
fn cases() -> Vec<Case> {
    let rights_issue = json!({
        "action_type": "RightsIssue", "listing_id": 1, "date": "2024-03-01",
        "rights_units": "1", "rights_held_units": "4", "exercise_price": "1.80",
        "currency": "AUD", "renounceable": true,
    });
    let mut cases: Vec<Case> = [
        upsert_and_create(
            "/trades",
            json!({
                "trade_type": "Buy", "date": "2024-03-04", "listing_id": 1,
                "holding_account_id": 1, "average_price": "12.34", "quantity": "100",
                "currency": "AUD", "brokerage": "19.95", "brokerage_includes_gst": true,
                "brokerage_currency": "AUD", "fx_rate": "1",
            }),
        ),
        upsert_and_create(
            "/sells",
            json!({
                "date": "2024-06-03", "listing_id": 1, "holding_account_id": 1,
                "average_price": "15.67", "quantity": "60", "currency": "AUD",
                "brokerage": "9.95", "brokerage_includes_gst": true,
                "brokerage_currency": "AUD", "fx_rate": "1",
                "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "60" }],
            }),
        ),
        upsert_and_create(
            "/amma_statements",
            json!({
                "listing_id": 1, "holding_account_id": 1, "tax_year_end_date": "2024-06-30",
                "date_received": "2024-08-15", "units_held": "100",
                "franked_dividends": "12.34", "franking_credits": "5.29",
                "cost_base_adjustment": "0.12", "currency": "AUD",
            }),
        ),
        upsert_and_create(
            "/income",
            json!({
                "listing_id": 3, "holding_account_id": 1, "date_paid": "2024-03-04",
                "franked_amount": "123.46", "franking_credits": "52.91",
                "unfranked_amount": "1.05", "currency": "AUD",
            }),
        ),
        upsert_and_create(
            "/interest_income",
            json!({
                "holding_account_id": 1, "date_paid": "2024-03-04", "amount": "45.67",
                "currency": "AUD", "source": "Some Bank",
            }),
        ),
        upsert_and_create(
            "/investment_expenses",
            json!({
                "listing_id": 1, "holding_account_id": 1, "date_incurred": "2024-03-04",
                "expense_type": "ManagementFee", "amount": "87.91", "currency": "AUD",
                "description": "Annual fee",
            }),
        ),
        upsert_and_create(
            "/drp_enrolments",
            json!({
                "listing_id": 1, "holding_account_id": 1, "enrolment_date": "2024-02-01",
                "residual_handling": "PayOut",
            }),
        ),
        upsert_and_create(
            "/ess_statements",
            json!({
                "listing_id": 4, "holding_account_id": 1, "taxing_point_date": "2024-03-04",
                "quantity": "250", "market_value_per_share": "12.34",
                "deferral_discount": "3085", "currency": "USD", "fx_rate": "0.6666666667",
                "aud_deferral_discount": "4627.50",
            }),
        ),
        upsert_and_create(
            "/inheritances",
            json!({
                "listing_id": 1, "holding_account_id": 1, "quantity": "500",
                "date_of_death": "2024-02-14", "cost_base_rule": "MarketValueAtDeath",
                "cost_base": "6172.84", "currency": "AUD", "fx_rate": "1",
            }),
        ),
        upsert_and_create(
            "/holding_unit_scales",
            json!({ "holding_account_id": 1, "listing_id": 1, "unit_scale": 0 }),
        ),
        upsert_and_create(
            "/corporate_actions",
            json!({
                "action_type": "ScripForScrip", "fraction_rule": "Exact", "listing_id": 1,
                "date": "2024-07-10", "scrip_listing_id": 2, "scrip_new_units": "1",
                "scrip_old_units": "1",
            }),
        ),
        upsert_and_create(
            "/transfers",
            json!({
                "listing_id": 1, "date": "2024-06-03", "from_account_id": 2,
                "to_account_id": 1,
                "allocations": [{ "purchase_trade_id": 2, "quantity_allocated": "100" }],
            }),
        ),
        upsert_and_create(
            "/entitlement_overrides",
            json!({
                "corporate_action_id": 30, "holding_account_id": 1, "units_received": "33",
            }),
        )
        .map(|case| {
            case.after(
                "/corporate_actions/30",
                json!({
                    "listing_id": 1, "date": "2024-06-03", "action_type": "ShareSplit",
                    "split_new_units": "1", "split_old_units": "3", "fraction_rule": "Down",
                }),
            )
        }),
    ]
    .into_iter()
    .flatten()
    .collect();

    cases.extend([
        // A demerger names its spun-off listing as well as its head.
        Case::new(
            "put",
            "/corporate_actions/11",
            json!({
                "action_type": "Demerger", "fraction_rule": "Exact", "listing_id": 1,
                "date": "2024-07-22", "demerger_listing_id": 2, "demerger_new_units": "1",
                "demerger_held_units": "5", "demerger_cost_base_pct": "10",
            }),
        ),
        Case::new(
            "post",
            "/corporate_actions/31/cash_in_lieu",
            json!({ "holding_account_id": 1, "date": "2024-06-20", "proceeds": "2.00" }),
        )
        .after(
            "/corporate_actions/31",
            json!({
                "listing_id": 1, "date": "2024-06-03", "action_type": "ShareSplit",
                "split_new_units": "1", "split_old_units": "3", "fraction_rule": "CashInLieu",
            }),
        ),
        Case::new(
            "post",
            "/corporate_actions/32/exercise",
            json!({ "date": "2024-03-20", "units": "25", "holding_account_id": 1 }),
        )
        .after("/corporate_actions/32", rights_issue.clone()),
        Case::new(
            "post",
            "/corporate_actions/32/sell_rights",
            json!({
                "date": "2024-03-15", "units": "25", "proceeds_per_right": "0.20",
                "holding_account_id": 1,
                "allocations": [{ "purchase_trade_id": 1, "units": "25" }],
            }),
        )
        .after("/corporate_actions/32", rights_issue),
        Case::new(
            "post",
            "/corporate_actions/33/participate",
            json!({
                "date": "2024-11-29", "units": "100", "holding_account_id": 1,
                "allocations": [{ "purchase_trade_id": 1, "quantity_allocated": "100" }],
            }),
        )
        .after(
            "/corporate_actions/33",
            json!({
                "action_type": "BuyBack", "listing_id": 1, "date": "2024-11-29",
                "buyback_price": "9.60", "buyback_dividend": "1.40",
                "buyback_franking_credit": "0.60", "buyback_market_value": "10.20",
                "currency": "AUD",
            }),
        ),
        Case::new(
            "put",
            "/closing_prices/1/2026-06-04",
            json!({
                "price": "62.48", "sourced_from": "asx.com.au closing report",
                "reason": "provider serves no candle for the day",
            }),
        )
        .missing_path(&format!("/closing_prices/{MISSING}/2026-06-04")),
        Case::new(
            "post",
            "/closing_prices/fetch",
            json!({ "listing_id": 1, "price_date": "2026-06-04" }),
        ),
        Case::new(
            "post",
            "/closing_prices/backfill",
            json!({ "listing_id": 1, "from": "2026-06-01", "to": "2026-06-04" }),
        ),
        Case::new(
            "post",
            "/closing_prices/clear_unpriced_before",
            json!({ "listing_id": 1 }),
        )
        .after(
            "/listings/1",
            json!({
                "exchange_mic": "XASX", "ticker": "T1", "name": "Test 1",
                "security_type": "ETF", "currency": "AUD", "amit": true,
                "unpriced_before": "2020-01-02",
            }),
        ),
        Case::new(
            "post",
            "/portfolio/net_capital_gain/what_if",
            json!({
                "listing_id": 1, "holding_account_id": 1, "units": "50", "proceeds": "600",
                "date": "2024-06-14", "strategy": "harvest_losses",
            }),
        ),
    ]);
    cases
}

/// Every `PUT`/`POST` the generated document carries whose request body has
/// a property naming a listing or holding account, or whose path is keyed by
/// a listing — the routes [`cases`] must cover.
fn documented_reference_writes() -> BTreeSet<(String, String)> {
    let doc = serde_json::to_value(api_spec::document()).expect("the document serialises");
    let schemas = &doc["components"]["schemas"];

    /// Does `schema` (following every `$ref`, once each) have a property
    /// naming a listing or holding account?
    fn names_one(schema: &Value, schemas: &Value, seen: &mut BTreeSet<String>) -> bool {
        match schema {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref") {
                    let name = reference.rsplit('/').next().unwrap_or_default().to_owned();
                    if seen.insert(name.clone()) && names_one(&schemas[&name], schemas, seen) {
                        return true;
                    }
                }
                if let Some(Value::Object(properties)) = map.get("properties")
                    && properties.keys().any(|k| names_a_listing_or_account(k))
                {
                    return true;
                }
                map.values().any(|v| names_one(v, schemas, seen))
            }
            Value::Array(items) => items.iter().any(|v| names_one(v, schemas, seen)),
            _ => false,
        }
    }

    let mut routes = BTreeSet::new();
    for (path, item) in doc["paths"].as_object().expect("paths is an object") {
        for verb in ["put", "post"] {
            let Some(op) = item.get(verb) else { continue };
            if path.contains("{listing_id}")
                || names_one(&op["requestBody"], schemas, &mut BTreeSet::new())
            {
                routes.insert((verb.to_owned(), path.clone()));
            }
        }
    }
    routes
}

/// [`cases`] covers exactly the documented writes that name a listing or
/// holding account, so a new one fails here until it has a case — and with
/// it the `422` contract below.
#[test]
fn every_write_naming_a_listing_or_account_has_a_case() {
    let documented = documented_reference_writes();
    let templates: Vec<String> = documented.iter().map(|(_, p)| p.clone()).collect();
    let covered: BTreeSet<(String, String)> = cases()
        .iter()
        .map(|case| case.documented_route(&templates))
        .collect();
    assert_eq!(
        documented, covered,
        "the documented writes naming a listing or holding account and the cases must agree"
    );
}

/// A write whose body names a listing or holding account that does not exist
/// answers `422` with a plain-text reason, on every route that takes one (and
/// one keyed by a missing listing in its *path* the `404` a path parameter
/// owes, with its reason) — and the same request naming rows that do exist
/// succeeds, so it is the missing reference, not the body, that each refusal
/// is about.
///
/// Found 2026-10-25: `PUT`/`POST /trades` and `/sells` answered `500` — the
/// settlement-period read ran ahead of the insert's foreign-key check and its
/// `RowNotFound` was an internal error — and the three closing-price
/// operations answered `404` for a body naming no listing.
#[tokio::test]
async fn a_write_naming_a_missing_listing_or_account_is_422() {
    let mut failures = Vec::new();
    for case in cases() {
        let pool = test_pool().await;
        let app = seed(&pool).await;
        for (path, body) in &case.setup {
            app.put_ok(*path, body).await;
        }
        let send = |path: String, body: Value| {
            let app = &app;
            async move {
                match case.verb {
                    "put" => app.put(path, &body).await,
                    _ => app.post(path, &body).await,
                }
            }
        };

        // (what is missing, the request path, its body, the status owed).
        let mut attempts: Vec<(String, String, Value, StatusCode)> = case
            .body
            .as_object()
            .expect("every case body is an object")
            .keys()
            .filter(|key| names_a_listing_or_account(key))
            .map(|key| {
                let mut body = case.body.clone();
                body[key] = json!(MISSING);
                (
                    key.clone(),
                    case.path.clone(),
                    body,
                    StatusCode::UNPROCESSABLE_ENTITY,
                )
            })
            .collect();
        if let Some(path) = &case.missing_path {
            attempts.push((
                "listing in the path".to_owned(),
                path.clone(),
                case.body.clone(),
                StatusCode::NOT_FOUND,
            ));
        }
        assert!(!attempts.is_empty(), "{} names nothing to miss", case.path);

        for (field, path, body, owed) in attempts {
            let resp = send(path.clone(), body).await;
            let (status, text) = resp.status_and_body();
            if status != owed || text.is_empty() {
                failures.push(format!(
                    "{} {path} naming a missing {field}: {status} {text:?}",
                    case.verb
                ));
            }
        }
        let resp = send(case.path.clone(), case.body.clone()).await;
        if !resp.status.is_success() {
            failures.push(format!(
                "{} {} (the baseline) was refused: {} {:?}",
                case.verb,
                case.path,
                resp.status,
                resp.text()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
