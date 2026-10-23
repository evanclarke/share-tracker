//! The machine-readable API description: `GET /openapi.json`, an OpenAPI 3.1
//! document generated from the route registrations themselves. Every handler
//! is a `#[utoipa::path]` function registered through `utoipa_axum`'s
//! `routes!` (see `app::api_routes`), so the route axum serves and the
//! operation the document carries are **one declaration**: the path and verb
//! are the annotation's, the request body and the path/query parameters are
//! read off the handler's own extractors, the success responses off its return
//! type (`infra::respond`), and the schemas from the *real* request/response
//! types (each derives [`utoipa::ToSchema`] — no mirror types). Only the
//! summary is hand-written, beside the handler it describes.
//!
//! Why it exists: the whole contract used to be the prose of `docs/API.md`
//! (1,936 lines), with the critical global rules — money/quantity as JSON
//! strings, `deny_unknown_fields` on every body — stated only in prose at the
//! end. A machine client (an LLM, a script) had nothing to read but English.
//! [`publish`] completes the generated operations into the served document,
//! and the tests at the bottom pin what the types cannot: a body that stops
//! denying unknown fields or a money field that starts rendering as a number
//! fails `every_request_body_schema_denies_unknown_fields` /
//! `a_money_field_is_a_json_string`, and a route registered with axum's plain
//! `.route(…)` (served, but undocumented) fails
//! `every_route_is_registered_through_routes`.
//!
//! The path spellings are the axum ones (`{id}`), relative to the deployment's
//! `base_path` — the same strings `docs/API.md` uses. `/static/*.js` is the one
//! class of route no annotation can describe (the paths are built in a loop
//! over `web::JS_MODULES`), so [`publish`] reads them from that same list.
//!
//! Behind `[auth]`: the router below is merged into `app::router` like every
//! other one, so `require_auth` gates it and `/openapi.json` is deliberately
//! *not* on that layer's login-page allowlist.

#[cfg(test)]
use crate::app;
use crate::entities::amit_adjustment::{AmitAdjustment, AmitAdjustmentBody};
use crate::entities::amit_adjustment_generation::{GenerateBody, GeneratedAdjustments};
use crate::entities::amma::{AmmaStatement, AmmaStatementBody};
use crate::entities::attachment::Attachment;
use crate::entities::buyback_participation::{Participation, ParticipationBody};
use crate::entities::cgt_settings::{CgtSettings, CgtSettingsBody};
use crate::entities::closing_price::{
    BackfillBody, BackfillSummary, ClearBody, ClearSummary, ClosingPrice, FetchBody,
    ManualPriceBody,
};
use crate::entities::corporate_action::{CorporateAction, CorporateActionBody};
use crate::entities::currencies::{Currency, ImportSummary};
use crate::entities::demerger::Demerge;
use crate::entities::distribution_event::DistributionEvent;
use crate::entities::drp_enrolment::{DrpEnrolment, DrpEnrolmentBody};
use crate::entities::drp_reinvestment::ReinvestBody;
use crate::entities::ess_statement::{EssStatement, EssStatementBody};
use crate::entities::exchange::{Exchange, ExchangeBody};
use crate::entities::exchange_holiday::{ExchangeHoliday, ExchangeHolidayBody};
use crate::entities::holding_account::{HoldingAccount, HoldingAccountBody};
use crate::entities::income::{Income, IncomeBody};
use crate::entities::inheritance::{Inheritance, InheritanceBody};
use crate::entities::interest_income::{InterestIncome, InterestIncomeBody};
use crate::entities::investment_expense::{InvestmentExpense, InvestmentExpenseBody};
use crate::entities::listing::{Listing, ListingBody};
use crate::entities::listing_rename::{ListingRename, RenameBody};
use crate::entities::mic_registry::MicEntry;
use crate::entities::parcel_allocation::ParcelAllocation;
use crate::entities::rba_fx_rate::{CorrectionBody, ImportOutcome, RbaFxRate};
use crate::entities::rights_exercise::ExerciseBody;
use crate::entities::rights_sale::{RightsSale, SellRightsBody};
use crate::entities::sell::SellBody;
use crate::entities::tax_year_settings::{TaxYearSettings, TaxYearSettingsBody};
use crate::entities::trade::{Trade, TradeBody};
use crate::entities::transfer::{Transfer, TransferBody, TransferGroup};
use crate::entities::worthless::Recognise;
use crate::entities::{mic_registry, rba_fx_rate, scrip_exchange};
use crate::infra::auth::LoginForm;
use crate::infra::respond::JavaScript;
use crate::infra::scheduler::JobStatus;
use crate::reports::activity::ActivityResponse;
use crate::reports::amit_adjustment_cross_check::AmitAdjustmentAlert;
use crate::reports::amit_cash_cross_check::AmitCashAlert;
use crate::reports::attachments::AttachmentIndexRow;
use crate::reports::e4_cross_check::E4CrossCheckAlert;
use crate::reports::franking_at_risk::{FrankingAtRiskAlert, FrankingWhatIfAlert};
use crate::reports::fx_coverage::FxCoverageAlert;
use crate::reports::health::HealthReport;
use crate::reports::indexation_cross_check::IndexationCrossCheck;
use crate::reports::mic_validation::ExchangeMicStatus;
use crate::reports::net_capital_gain::{NetCapitalGainYear, WhatIfRequest, WhatIfResponse};
use crate::reports::open_parcels::OpenParcel;
use crate::reports::parcel_optimiser::OptimiserResponse;
use crate::reports::performance::{HoldingPerformance, PerformanceRequest};
use crate::reports::period_performance::PeriodPerformance;
use crate::reports::portfolio::{HoldingOverview, OverviewRequest};
use crate::reports::realised_gains::RealisedGainLoss;
use crate::reports::rollover_consistency::RolloverAlert;
use crate::reports::row_history::RowHistoryResponse;
use crate::reports::settlement_coverage::SettlementCoverageAlert;
use crate::reports::snapshot;
use crate::reports::snapshot::{
    HoldingSeries, RegenerateBody, RegenerateRange, RegenerateSummary, SeriesPoint, Snapshot,
    SnapshotMeta,
};
use crate::reports::tax_report::TaxReport;
use crate::reports::tax_summary::TaxYearSummary;
use crate::reports::unrealised_gains::{UnrealisedGain, UnrealisedGainsRequest};
use crate::reports::wash_sales::WashSaleAlert;
use crate::web;
use axum::http::header;
use axum::response::IntoResponse;
use axum::{Router, response};
use std::collections::{BTreeMap, BTreeSet};
use std::iter;
use std::sync::Arc;
use utoipa::openapi::ServerBuilder;

use crate::infra::auth::AuthMode;
use crate::infra::config::BasePath;
use crate::infra::extract::Query;
use crate::infra::http::ApiError;
use crate::infra::respond::{JsonBody, Payload, PlainText};
use sqlx::SqlitePool;
use std::fmt::Write as _;
use utoipa::openapi::content::{Content, ContentBuilder};
use utoipa::openapi::path::{
    HttpMethod, Operation, OperationBuilder, Parameter, ParameterBuilder, ParameterIn,
};
use utoipa::openapi::response::{Response, ResponseBuilder, ResponsesBuilder};
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use utoipa::openapi::security::{
    ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme,
};
use utoipa::openapi::tag::{Tag, TagBuilder};
use utoipa::openapi::{
    Components, ComponentsBuilder, InfoBuilder, OpenApi, OpenApiBuilder, PathItem, RefOr, Required,
};
use utoipa::{PartialSchema, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The document's `info.description`: the two global rules every endpoint
/// obeys, stated in prose *and* carried structurally by the schemas (a money
/// field is a `string`; a request body has `additionalProperties: false`) —
/// plus the never-JSON error matrix, the reading-a-list contract (ascending
/// order, the four POST-bodied report reads, the sole paginated endpoint, and
/// the per-entity query filters), and the PUT upsert outcome rule (201 with
/// the created row, or 204 on a replace).
/// The wording is pinned by `the_two_global_rules_are_stated_in_the_description`,
/// `the_error_body_matrix_is_stated_in_the_description`,
/// `the_list_reading_contract_is_stated_in_the_description` and
/// `the_put_outcome_rule_is_stated_in_the_description`.
const DESCRIPTION: &str = "\
The share-tracker JSON API, for the web UI and machine clients alike. Every \
route app::router serves is listed here, under its axum path spelling ({id} \
placeholders), relative to the deployment's base_path.

Two global rules apply to every endpoint. They are stated here and carried by \
the schemas below, not left to prose alone:

- Money and quantities travel as JSON strings, in both directions. Every \
monetary or quantity field (a rust_decimal::Decimal in the code) is a decimal \
string such as \"1234.5600\", never a JSON number: a number is read as an f64 \
and silently loses digits past about the fifteenth significant one. Every such \
field is typed string here, and sending it as a number is a 422 naming the \
field (SCENARIOS W-a).

- Every request body denies unknown fields. Each request-body schema carries \
additionalProperties: false, so a misspelt or unexpected field is a 422 naming \
the offending field rather than a silently-ignored default writing a zero into \
a tax figure. A query string is held to the same rule, but the refusal is a 400 \
naming the field rather than a 422, because it is the query decoder that rejects \
it before the handler runs. Every such refusal reads 'cannot read the query \
string: <reason>' (a body: 'cannot read the request body: <reason>').

Errors are never JSON. A rejected request answers either a text/plain; \
charset=utf-8 body with the reason — 400 (a malformed path parameter, query \
string or body), 401, 403 (the request guard: a cross-site write, or with no \
[auth] a Host name that is not localhost, an IP address or in allowed_hosts), \
404 naming the missing row or prerequisite (a GET or DELETE of one row, an \
operation, or a read whose parameter names a missing row), 413, 415 (a JSON body sent \
without Content-Type: application/json), 422, 429 on POST /login once a source \
has exhausted its failed-attempt budget (the body carries the reason and a \
Retry-After header the remaining whole seconds; a browser login POST is refused \
429 too, with the sign-in page as the body instead of the plain-text reason), a \
failed POST /jobs/{name}'s 500, 502, 503 — or a deliberately empty body: the 404 \
of a path no route matches (so an empty 404 means the URL is wrong, a text one \
that the row is absent), a 405, and an internal 500. docs/API.md's \
\"Error-body contract\" section carries the full matrix.

Reading a collection is one more contract, stated here and in docs/API.md's \
\"Reading a list\" section. First, list endpoints return rows ascending — by \
id, by natural key, or by date then id — except the deliberate newest-first \
browse surfaces (/closing_prices, /distribution_events, a listing's rename \
chain, a job's run history, and the /reports/row_history trail). This is the \
reverse of the web UI, which sorts its own tables newest-first client-side. \
Second, a read is GET with its parameters in the query string; only four \
report reads keep a POST body, because their parameter is a map or a list a \
query string cannot carry: /portfolio/overview, /portfolio/performance and \
/portfolio/unrealised_gains (a prices price-override map) and \
/portfolio/net_capital_gain/what_if (an allocations list). Third, \
/reports/row_history is the only paginated endpoint: without row_id it answers \
{\"entries\":[…],\"page_size\":n,\"next_before_id\":id|null}, where before_id \
returns entries older than that trail id and limit is 1-1000 (default 100); \
with row_id it answers that row's whole trail as a bare JSON array. Fourth, \
an entity list may be narrowed by query filters. /listings takes \
?exchange_mic= and ?security_type=. The dated workhorse lists take an \
inclusive ?from=/?to= over their own date column, plus ?listing_id= where \
the row has a listing and ?holding_account_id= where it has a holding \
account: /trades, /income, /investment_expenses, /amma_statements, \
/ess_statements, /inheritances, /corporate_actions, /transfers and \
/distribution_events (the last three have no holding account); \
/interest_income takes ?holding_account_id= and the date range; \
/drp_enrolments takes ?listing_id= and ?holding_account_id=; \
/amit_adjustments takes ?amma_statement_id= and ?trade_id=, and \
/parcel_allocations ?sale_trade_id= and ?purchase_trade_id=. \
/rights_sales takes ?listing_id= (the rights issue's listing), \
?holding_account_id=, ?rights_action_id= and the date range; \
/exchange_holidays and /exchange_holidays/{mic} take the date range over \
holiday_date. The long-standing /closing_prices (?listing_id=, ?from=, ?to=, ?status= — ok or \
error, omitted is every row: the rows carrying a price, which is one call \
rather than two, but not quite the series a valuation reads — a row before \
its listing's unpriced_before is stored ok and superseded) and /attachments \
(owner ids, include_linked) filters are unchanged. An unrecognised \
parameter is a 400 naming it on every list route — including a list that accepts no filter, which refuses any \
parameter at all rather than silently ignoring it.

A PUT upsert reports its outcome. PUT /<collection>/{id} answers 201 Created \
carrying the created row — exactly the body POST /<collection> answers — when \
the id was free, and 204 No Content when it replaced an existing row. There is \
deliberately no If-Match/ETag: the status is the created-vs-replaced signal, so \
a client that must not clobber a row it has not seen reads it before writing. \
Two PUTs differ: /transfers/{id} is create-only and always answers 201 with the \
executed group, because a bare 204 would hide the created Sell/Buy ids; and \
/rba_fx_rates/{id} only ever corrects an existing row, so always answers 204 \
(new rates arrive through POST /rba_fx_rates/import).

The document itself is generated from the route registrations and the serde \
structs (src/api_spec.rs) and is pinned by that module's tests. It can be read in pieces: \
GET /openapi/index is a compact route index (verb, path, operationId and summary \
per line), and GET /openapi.json?operation=<operationId> or ?tag=<tag> serves one \
operation's or one tag's slice, each a whole document carrying only the schemas it \
reaches.";

/// The verbs the server uses, as the document's path items carry them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verb {
    Get,
    Post,
    Put,
    Delete,
}

impl Verb {
    const ALL: [Verb; 4] = [Verb::Get, Verb::Post, Verb::Put, Verb::Delete];

    /// The operation this verb names on `item`, if the path serves it.
    fn of(self, item: &mut PathItem) -> &mut Option<Operation> {
        match self {
            Verb::Get => &mut item.get,
            Verb::Post => &mut item.post,
            Verb::Put => &mut item.put,
            Verb::Delete => &mut item.delete,
        }
    }
}

/// Which `docs/API.md` `## ` section documents a route, by path prefix: an
/// operation's one tag is the tag of the **longest** prefix that matches it on
/// a segment boundary, so an operation nested under another resource's path
/// (`/income/{id}/reinvest`, documented under DRP reinvestment) can be filed
/// under its own section. Read by [`tag_for`]; the order is `docs/API.md`'s,
/// and is the order of the document's top-level `tags` list.
///
/// `every_operation_has_a_unique_id_and_one_documented_tag` pins every value
/// to a real `## ` heading of `docs/API.md`, so a section rename cannot leave a
/// tag naming nothing — and an agent slicing the contract by tag lands on the
/// same grouping as a reader of the prose.
const TAGS: &[(&str, &str)] = &[
    ("/", "Web frontend"),
    ("/static", "Web frontend"),
    ("/openapi.json", "OpenAPI description"),
    ("/openapi", "OpenAPI description"),
    ("/login", "Authentication"),
    ("/logout", "Authentication"),
    ("/exchanges", "Exchanges"),
    ("/exchange_holidays", "Exchange holidays"),
    ("/listings", "Listings"),
    ("/holding_accounts", "Holding accounts"),
    ("/rba_fx_rates", "RBA FX rates"),
    ("/mic_registry", "MIC registry"),
    ("/currencies", "Currencies"),
    ("/closing_prices", "Closing prices"),
    ("/distribution_events", "Distribution calendar"),
    ("/report_snapshots", "Report snapshots"),
    ("/jobs", "Jobs"),
    ("/trades", "Trades"),
    ("/income", "Income"),
    ("/interest_income", "Interest income"),
    ("/investment_expenses", "Investment expenses"),
    ("/amma_statements", "AMMA statements"),
    ("/amit_adjustments", "AMIT adjustments"),
    ("/ess_statements", "ESS statements"),
    ("/attachments", "Attachments"),
    ("/drp_enrolments", "DRP enrolments"),
    ("/cgt_settings", "CGT settings"),
    ("/tax_year_settings", "Tax year settings"),
    ("/corporate_actions", "Corporate actions"),
    ("/rights_sales", "Corporate actions"),
    ("/income/{id}/reinvest", "DRP reinvestment"),
    ("/sells", "Sells"),
    ("/transfers", "Transfers"),
    ("/inheritances", "Inheritances"),
    ("/parcel_allocations", "Parcel allocations"),
    ("/portfolio", "Portfolio reports"),
    ("/reports", "Portfolio reports"),
];

/// The `docs/API.md` section a route is tagged with: the longest [`TAGS`]
/// prefix matching `path` on a segment boundary. `/` matches only the root
/// itself — it is the web frontend's page, not a catch-all. A path no prefix
/// matches is a route nobody has filed yet, which the tag test fails on by
/// name; it is tagged with the empty string rather than panicking, because
/// the document is built while the router is assembled.
fn tag_for(path: &str) -> &'static str {
    TAGS.iter()
        .filter(|(prefix, _)| {
            path == *prefix
                || (*prefix != "/"
                    && path
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('/')))
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map_or("", |&(_, tag)| tag)
}

/// The operation's `operationId` — the name an OpenAPI-to-tool adapter gives
/// the tool — derived from the verb and the path so it is stable and needs no
/// column of its own: a verb word, the path's literal segments in PascalCase,
/// then `By` + its path parameters (`getTradesById`,
/// `getExchangeHolidaysByMicAndDate`, `createCorporateActionsParticipateById`).
///
/// The verb word says what the call does rather than repeating the HTTP verb:
/// a `GET` answering an array is `list`, any other `GET` is `get`; a `POST`
/// answering `201` is `create` and any other `POST` (a report, an import, a
/// job trigger) is `run`; `PUT` is `upsert`; `DELETE` is `delete`. The root
/// page, which has no segment to name it, is `getIndex`. Uniqueness is not
/// assumed: `every_operation_has_a_unique_id_and_one_documented_tag` pins it.
fn operation_id(verb: Verb, path: &str, statuses: &[u16], answers_array: bool) -> String {
    let word = match verb {
        Verb::Get if answers_array => "list",
        Verb::Get => "get",
        Verb::Post if statuses.contains(&201) => "create",
        Verb::Post => "run",
        Verb::Put => "upsert",
        Verb::Delete => "delete",
    };
    let pascal = |segment: &str| -> String {
        segment
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut chars = word.chars();
                chars.next().map_or(String::new(), |first| {
                    iter::once(first.to_ascii_uppercase())
                        .chain(chars)
                        .collect()
                })
            })
            .collect()
    };
    let segments = path.split('/').filter(|s| !s.is_empty());
    let (params, literals): (Vec<&str>, Vec<&str>) = segments.partition(|s| s.starts_with('{'));
    let mut id = word.to_owned();
    if literals.is_empty() {
        id.push_str("Index");
    }
    for literal in literals {
        id.push_str(&pascal(literal));
    }
    for (i, param) in params.iter().enumerate() {
        id.push_str(if i == 0 { "By" } else { "And" });
        id.push_str(&pascal(param));
    }
    id
}

/// One published operation, as [`route_index`] lists it: verb, path, summary.
fn operations(document: &OpenApi) -> Vec<(Verb, String, String, String)> {
    let mut out = Vec::new();
    for (path, item) in &document.paths.paths {
        let mut item = item.clone();
        for verb in Verb::ALL {
            if let Some(op) = verb.of(&mut item) {
                out.push((
                    verb,
                    path.clone(),
                    op.operation_id.clone().unwrap_or_default(),
                    op.summary.clone().unwrap_or_default(),
                ));
            }
        }
    }
    out
}

/// The compact route index served at `GET /openapi/index`: one line per
/// published operation — verb, path, `operationId`, summary — grouped under
/// its tag in [`TAGS`] order, each group headed by the `?tag=` request that
/// fetches just that slice of the document.
///
/// It exists because the whole document is too large for an agent to read
/// (~400 KB); this is a few thousand tokens, enough to choose the operation
/// and then fetch only its tag's slice. It carries no schemas and adds no
/// facts of its own: every line is read from the published document itself.
fn route_index(document: &OpenApi, base_path: &BasePath) -> String {
    let operations = operations(document);
    let mut out = format!(
        "share-tracker API {} — route index ({} operations)\n\
         Paths are relative to {}. The full OpenAPI 3.1 contract is GET /openapi.json; one \
         operation and the schemas it reaches is GET /openapi.json?operation=<operationId> \
         (the third column below), and one section's is GET /openapi.json?tag=<tag>, the tag \
         URL-encoded as each heading shows.\n\
         Money and quantities are JSON strings, every request body refuses unknown fields, \
         and a PUT replaces the whole row.\n",
        env!("CARGO_PKG_VERSION"),
        operations.len(),
        base_path.home(),
    );
    let mut seen: Vec<&str> = Vec::new();
    for &(_, tag) in TAGS {
        if seen.contains(&tag) {
            continue;
        }
        seen.push(tag);
        let group: Vec<_> = operations
            .iter()
            .filter(|(_, path, ..)| tag_for(path) == tag)
            .collect();
        if group.is_empty() {
            continue;
        }
        write!(
            out,
            "\n## {tag} — GET /openapi.json?tag={}\n",
            tag.replace(' ', "%20")
        )
        .expect("writing to a String cannot fail");
        for (verb, path, id, summary) in group {
            writeln!(
                out,
                "{:<6} {path}  {id} — {summary}",
                format!("{verb:?}").to_uppercase(),
            )
            .expect("writing to a String cannot fail");
        }
    }
    out
}

/// A slice of the serialised document: only the operations `keep` accepts,
/// only the top-level `tags` they carry, and only the component schemas they
/// reach — followed through every `$ref`, transitively, so the slice is a
/// whole document on its own. Everything else (`info`, `servers`, the security
/// schemes and requirement) is kept as is. `None` when `keep` accepts no
/// operation, which the handler answers `400`.
fn slice_where(
    document: &serde_json::Value,
    keep: impl Fn(&serde_json::Value) -> bool,
) -> Option<serde_json::Value> {
    use serde_json::{Map, Value};

    /// Push every `#/components/schemas/<name>` a value refers to.
    fn refs(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "$ref"
                        && let Some(name) = child
                            .as_str()
                            .and_then(|r| r.strip_prefix("#/components/schemas/"))
                    {
                        out.push(name.to_owned());
                    }
                    refs(child, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    refs(item, out);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    let mut paths = Map::new();
    for (path, item) in document["paths"].as_object()? {
        let kept: Map<String, Value> = item
            .as_object()?
            .iter()
            .filter(|(_, op)| keep(op))
            .map(|(verb, op)| (verb.clone(), op.clone()))
            .collect();
        if !kept.is_empty() {
            paths.insert(path.clone(), Value::Object(kept));
        }
    }
    if paths.is_empty() {
        return None;
    }
    let paths = Value::Object(paths);
    let used = |tag: &Value| {
        paths.as_object().into_iter().flatten().any(|(_, item)| {
            item.as_object().into_iter().flatten().any(|(_, op)| {
                op["tags"]
                    .as_array()
                    .is_some_and(|tags| tags.contains(&tag["name"]))
            })
        })
    };
    let tags: Vec<Value> = document["tags"]
        .as_array()?
        .iter()
        .filter(|tag| used(tag))
        .cloned()
        .collect();
    let mut pending: Vec<String> = Vec::new();
    refs(&paths, &mut pending);
    let all = &document["components"]["schemas"];
    let mut reached = Map::new();
    while let Some(name) = pending.pop() {
        if reached.contains_key(&name) {
            continue;
        }
        if let Some(schema) = all.get(&name) {
            refs(schema, &mut pending);
            reached.insert(name, schema.clone());
        }
    }
    let mut slice = Map::new();
    for (key, value) in document.as_object()? {
        let value = match key.as_str() {
            "paths" => paths.clone(),
            "tags" => Value::Array(tags.clone()),
            "components" => {
                let mut components = value.as_object()?.clone();
                components.insert("schemas".to_owned(), Value::Object(reached.clone()));
                Value::Object(components)
            }
            _ => value.clone(),
        };
        slice.insert(key.clone(), value);
    }
    Some(Value::Object(slice))
}

/// One tag's slice of the document ([`slice_where`] over the operations
/// carrying `tag`).
fn tag_slice(document: &serde_json::Value, tag: &str) -> Option<serde_json::Value> {
    slice_where(document, |op| {
        op["tags"]
            .as_array()
            .is_some_and(|tags| tags.iter().any(|t| t == tag))
    })
}

/// One operation's slice of the document ([`slice_where`] over the operation
/// whose `operationId` is `id`).
fn operation_slice(document: &serde_json::Value, id: &str) -> Option<serde_json::Value> {
    slice_where(document, |op| op["operationId"] == id)
}

/// The OpenAPI 3.1 document — the auth-on, root-path form the tests read.
#[cfg(test)]
pub fn document() -> OpenApi {
    document_for(&BasePath::ROOT, AuthMode::Required)
}

/// [`document`] for a deployment, built exactly as [`router`] builds the one
/// it serves: from [`crate::app::api_routes`]' generated operations plus this
/// module's own two.
#[cfg(test)]
pub fn document_for(base_path: &BasePath, mode: AuthMode) -> OpenApi {
    let mut generated = app::api_routes(base_path, mode).into_openapi();
    generated.merge(own_routes().into_openapi());
    publish(generated, base_path, mode)
}

/// The published document: the operations utoipa generated from the
/// `#[utoipa::path]` handlers — path, verb and summary from the annotation;
/// request body, query parameters and success responses read off the
/// handler's own signature (see `infra::respond`) — completed with what every
/// route shares or its spelling decides:
///
/// - its `operationId` ([`operation_id`]) and its one tag ([`tag_for`]);
/// - a required string path parameter per `{name}` segment
///   ([`path_parameters`]);
/// - the non-success responses ([`non_success_responses`]);
/// - a request body's description, and no schema for a non-JSON payload;
/// - the handler's doc comment dropped from the operation — it is written for
///   the code's reader, and the summary is what the document publishes;
/// - one operation per `/static/*.js` module, read from `web::JS_MODULES` (the
///   routes registered in a loop, which no annotation can describe);
/// - `info`, the `servers` entry naming `base_path` (so a client resolves the
///   paths against the prefix the app is really mounted under), the tag list,
///   the component schemas and, with `[auth]`, the security schemes and
///   requirement. A deployment without `[auth]` merges no `/login` or
///   `/logout` route, so the document cannot advertise one.
fn publish(generated: OpenApi, base_path: &BasePath, mode: AuthMode) -> OpenApi {
    let mut paths = generated.paths;
    for (path, _) in web::JS_MODULES {
        paths.add_path_operation(
            path,
            vec![HttpMethod::Get],
            OperationBuilder::new()
                .summary(Some("A served frontend ES module (JavaScript)."))
                .responses(
                    ResponsesBuilder::new().responses_from_into_responses::<Payload<JavaScript>>(),
                )
                .build(),
        );
    }
    let mut used_tags = BTreeSet::new();
    for (path, item) in paths.paths.iter_mut() {
        for verb in Verb::ALL {
            if let Some(op) = verb.of(item) {
                complete(op, verb, path);
                used_tags.insert(tag_for(path));
            }
        }
    }
    // The document-level tag list, in `TAGS` order (which is `docs/API.md`'s),
    // carrying only the tags this deployment's operations use — without
    // `[auth]` there is no Authentication route to group.
    let mut tags: Vec<Tag> = Vec::new();
    for &(_, tag) in TAGS {
        if used_tags.contains(tag) && !tags.iter().any(|t| t.name == tag) {
            tags.push(TagBuilder::new().name(tag).build());
        }
    }
    let server_url = base_path.home();
    let mut builder = OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("share-tracker API")
                .version(env!("CARGO_PKG_VERSION"))
                .description(Some(DESCRIPTION))
                .build(),
        )
        .paths(paths)
        .tags(Some(tags))
        .components(Some(components(generated.components, mode)))
        .servers(Some(vec![ServerBuilder::new().url(server_url).build()]));
    match mode {
        AuthMode::Required => {
            builder = builder.security(Some(vec![
                SecurityRequirement::new("sessionCookie", Vec::<String>::new()),
                SecurityRequirement::new("bearerAuth", Vec::<String>::new()),
            ]));
        }
        AuthMode::Open => {}
    }
    builder.build()
}

/// Complete one generated operation (see [`publish`]).
fn complete(op: &mut Operation, verb: Verb, path: &str) {
    let is_array = |content: &RefOr<Content>| {
        matches!(content, RefOr::T(content)
            if matches!(content.schema, Some(RefOr::T(Schema::Array(_)))))
    };
    let statuses: Vec<u16> = op
        .responses
        .responses
        .keys()
        .filter_map(|status| status.parse().ok())
        .collect();
    let answers_array = matches!(
        op.responses.responses.get("200"),
        Some(RefOr::T(response))
            if response.content.get("application/json").is_some_and(is_array)
    );
    op.operation_id = Some(operation_id(verb, path, &statuses, answers_array));
    op.tags = Some(vec![tag_for(path).to_owned()]);
    op.description = None;
    let mut parameters: Vec<RefOr<Parameter>> =
        path_parameters(path).into_iter().map(RefOr::T).collect();
    parameters.extend(
        op.parameters
            .take()
            .into_iter()
            .flatten()
            .filter(|p| !matches!(p, RefOr::T(p) if p.parameter_in == ParameterIn::Path)),
    );
    op.parameters = Some(parameters);
    let body = match op.request_body.as_mut() {
        Some(RefOr::T(body)) => {
            let structured = body
                .content
                .keys()
                .all(|media| media == "application/json" || media == FORM);
            for (media, content) in body.content.iter_mut() {
                if media != "application/json"
                    && media != FORM
                    && let RefOr::T(content) = content
                {
                    content.schema = None;
                }
            }
            let array = body.content.values().any(is_array);
            body.description = Some(
                match (body.content.contains_key(FORM), structured, array) {
                    (true, ..) => "The form request body.",
                    (false, false, _) => {
                        "The raw request body: not JSON, so it carries no schema here."
                    }
                    (false, true, true) => "A JSON array request body.",
                    (false, true, false) => "The JSON request body.",
                }
                .to_owned(),
            );
            body.required = Some(Required::True);
            Some(structured)
        }
        Some(RefOr::Ref(_)) => Some(true),
        None => None,
    };
    for (status, response) in non_success_responses(verb, path, body) {
        op.responses
            .responses
            .insert(status.to_string(), RefOr::T(response));
    }
}

/// The `application/x-www-form-urlencoded` media type (the login form).
const FORM: &str = "application/x-www-form-urlencoded";

/// The non-success responses an operation documents, by what its route is:
/// `body` is `None` for a route taking no request body, and otherwise whether
/// that body is structured (JSON or a form) rather than a raw payload.
///
/// Every route can answer a generic internal failure — recorded so a
/// generated client handles it rather than treating a 500 as transport noise.
/// The rest follow from the route: a body can be refused (`422`, `413`,
/// `415`), a path parameter can name a missing row (`404`), and a feed import
/// or provider fetch can find its upstream unreachable (`502`). Each is
/// recorded by description, because no error body is JSON.
fn non_success_responses(verb: Verb, path: &str, body: Option<bool>) -> Vec<(u16, Response)> {
    let described = |text: &str| ResponseBuilder::new().description(text).build();
    let mut out = Vec::new();
    match body {
        // The deny-unknown-fields rule and the money-as-string rule are both
        // enforced there.
        Some(true) => out.push((
            422,
            described(
                "The body was rejected: an unknown or misspelt field, or a money/quantity \
                 field sent as a JSON number. The body is the plain-text reason, naming the \
                 offending field.",
            ),
        )),
        // A multipart upload or a bare-text feed: not a JSON struct, so the
        // rule above cannot describe it, but it answers 422 when the payload
        // cannot be read or parsed.
        Some(false) if verb == Verb::Post => out.push((
            422,
            described(
                "The payload could not be read or parsed: the body is the plain-text reason.",
            ),
        )),
        _ => {}
    }
    if path.contains('{') && matches!(verb, Verb::Get | Verb::Post | Verb::Delete) {
        out.push((
            404,
            described("No such row: the body is the plain-text reason naming it."),
        ));
    }
    if verb == Verb::Post
        && (path.ends_with("/import") || path.ends_with("/fetch") || path.ends_with("/backfill"))
    {
        out.push((
            502,
            described(
                "The upstream feed or price provider could not be reached: the body is the \
                 plain-text reason.",
            ),
        ));
    }
    out.push((
        500,
        described("An internal failure. The body is empty; the cause is in the server log."),
    ));
    // A route that takes a body can answer 413 (over axum's 2 MiB body limit;
    // the attachment upload raises its own route) and 415 (a body sent with
    // the wrong content type) before its handler runs.
    if body.is_some() {
        out.push((
            413,
            described(
                "The request body is over the size limit. The body is the plain-text reason.",
            ),
        ));
        out.push((
            415,
            described(
                "The request body's content type is not the one this route takes. The body is \
                 the plain-text reason.",
            ),
        ));
    }
    out
}

/// The document's reusable schemas: every type a handler takes or answers,
/// plus everything those types reference recursively. A type with no entry
/// here cannot be reached by a `$ref`, and a `$ref` whose name has no entry
/// fails `every_referenced_schema_is_registered_and_uniquely_named`.
///
/// A name reached twice with the **same** schema is expected — the recursive
/// collection registers it once per path — so the merge takes the first and
/// moves on. Two *different* schemas under one name is the error the
/// uniqueness test pins, deliberately not a runtime `assert!` in a request
/// path (which would panic inside a handler for an invariant the test already
/// covers). The `BTreeMap` also makes the merge O(n log n) rather than the
/// `Vec::iter().find` scan this replaced.
fn components(generated: Option<Components>, mode: AuthMode) -> Components {
    let mut by_name: BTreeMap<String, RefOr<Schema>> = BTreeMap::new();
    for (name, schema) in component_schemas() {
        by_name.entry(name).or_insert(schema);
    }
    // The schemas utoipa collected from the handlers' own request bodies —
    // already registered above, unless a body type is reached only there.
    for (name, schema) in generated.into_iter().flat_map(|c| c.schemas) {
        by_name.entry(name).or_insert(schema);
    }
    let mut components = ComponentsBuilder::new().schemas_from_iter(by_name).build();
    if mode == AuthMode::Required {
        // The two credentials docs/API.md documents: the session cookie the
        // browser carries, and the bearer token the deployment scripts send
        // (`Authorization: Bearer <api_token>`). Named here so a generated
        // client can be configured rather than guessing.
        let mut schemes = BTreeMap::new();
        schemes.insert(
            "bearerAuth".to_owned(),
            RefOr::T(SecurityScheme::Http(
                HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build(),
            )),
        );
        schemes.insert(
            "sessionCookie".to_owned(),
            RefOr::T(SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(
                "st_session",
            )))),
        );
        components.security_schemes = schemes;
    }
    components
}

/// Every type a handler answers or takes, registered as a named component
/// with everything it references recursively — and, through `components!`,
/// the [`Component`](crate::infra::respond::Component) impl that lets a handler
/// return it at all: a response type missing here does not compile.
macro_rules! components {
    ($($ty:ty),* $(,)?) => {
        $(impl crate::infra::respond::Component for $ty {})*

        /// Every registered type as `(component name, schema)` pairs, collected
        /// recursively from the derives.
        fn component_schemas() -> Vec<(String, RefOr<Schema>)> {
            let mut schemas: Vec<(String, RefOr<Schema>)> = Vec::new();
            $(
                schemas.push((
                    <$ty as ToSchema>::name().into_owned(),
                    <$ty as PartialSchema>::schema(),
                ));
                <$ty as ToSchema>::schemas(&mut schemas);
            )*
            schemas
        }
    };
}

components!(
    // Request bodies.
    ListingBody,
    TradeBody,
    SellBody,
    IncomeBody,
    InterestIncomeBody,
    InvestmentExpenseBody,
    AmmaStatementBody,
    AmitAdjustmentBody,
    HoldingAccountBody,
    InheritanceBody,
    DrpEnrolmentBody,
    EssStatementBody,
    TransferBody,
    CorporateActionBody,
    ExchangeBody,
    ExchangeHolidayBody,
    CgtSettingsBody,
    TaxYearSettingsBody,
    ManualPriceBody,
    FetchBody,
    BackfillBody,
    ClearBody,
    CorrectionBody,
    ReinvestBody,
    RenameBody,
    ExerciseBody,
    SellRightsBody,
    ParticipationBody,
    GenerateBody,
    snapshot::GenerateBody,
    RegenerateBody,
    OverviewRequest,
    PerformanceRequest,
    UnrealisedGainsRequest,
    WhatIfRequest,
    LoginForm,
    // Entity rows and operation results.
    Listing,
    Trade,
    Transfer,
    TransferGroup,
    Income,
    InterestIncome,
    InvestmentExpense,
    AmmaStatement,
    AmitAdjustment,
    HoldingAccount,
    Inheritance,
    DrpEnrolment,
    EssStatement,
    CorporateAction,
    Exchange,
    ExchangeHoliday,
    ClosingPrice,
    CgtSettings,
    TaxYearSettings,
    ParcelAllocation,
    Attachment,
    DistributionEvent,
    Currency,
    MicEntry,
    RbaFxRate,
    RightsSale,
    ListingRename,
    JobStatus,
    Participation,
    Demerge,
    scrip_exchange::Exchange,
    Recognise,
    GeneratedAdjustments,
    BackfillSummary,
    ClearSummary,
    ImportSummary,
    mic_registry::ImportSummary,
    rba_fx_rate::ImportSummary,
    // Report responses.
    HoldingOverview,
    HoldingPerformance,
    PeriodPerformance,
    ActivityResponse,
    OpenParcel,
    UnrealisedGain,
    RealisedGainLoss,
    NetCapitalGainYear,
    WhatIfResponse,
    OptimiserResponse,
    TaxYearSummary,
    ExchangeMicStatus,
    SettlementCoverageAlert,
    E4CrossCheckAlert,
    IndexationCrossCheck,
    AmitAdjustmentAlert,
    RowHistoryResponse,
    RolloverAlert,
    AmitCashAlert,
    WashSaleAlert,
    FrankingAtRiskAlert,
    FrankingWhatIfAlert,
    FxCoverageAlert,
    HealthReport,
    TaxReport,
    AttachmentIndexRow,
    SnapshotMeta,
    SeriesPoint,
    HoldingSeries,
    RegenerateSummary,
    ImportOutcome,
    RegenerateRange,
    Snapshot,
);

/// A required string path parameter per `{name}` segment of the path — read
/// out of the axum spelling itself, so the document can never name a
/// parameter the route does not have.
fn path_parameters(path: &str) -> Vec<Parameter> {
    path.split('/')
        .filter_map(|segment| {
            segment
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
        })
        .map(|name| {
            ParameterBuilder::new()
                .name(name)
                .parameter_in(ParameterIn::Path)
                .required(Required::True)
                .schema(Some(ObjectBuilder::new().schema_type(Type::String)))
                .build()
        })
        .collect()
}

/// `GET /openapi.json` and `GET /openapi/index`, documenting `generated` —
/// the operations [`crate::app::api_routes`] registered — plus themselves.
/// Merged into `app::router` like every other router, so `[auth]`'s
/// `require_auth` gates both and they are deliberately absent from that
/// layer's login-page allowlist.
///
/// The document (200+ schemas, ~170 paths) is completed and serialised
/// **once**, when the router is assembled, and each request clones the
/// finished JSON string.
pub fn router(generated: OpenApi, base_path: &BasePath, mode: AuthMode) -> Router<SqlitePool> {
    let (router, own) = own_routes().split_for_parts();
    let mut generated = generated;
    generated.merge(own);
    let published = publish(generated, base_path, mode);
    let document = serde_json::to_value(&published).expect("the document serialises");
    let served = Arc::new(Served {
        whole: document.to_string(),
        index: route_index(&published, base_path),
        document,
    });
    router.layer(axum::Extension(served))
}

/// This module's own two routes.
fn own_routes() -> OpenApiRouter<SqlitePool> {
    OpenApiRouter::new()
        .routes(routes!(serve_document))
        .routes(routes!(serve_index))
}

/// What the two routes answer, built once at router assembly: the whole
/// document already serialised (the common request only clones a string),
/// the document as a value for the `?tag=`/`?operation=` slices, and the
/// route index.
struct Served {
    whole: String,
    document: serde_json::Value,
    index: String,
}

/// `GET /openapi.json`'s query string: at most one of the two narrowings.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct DocumentQuery {
    /// Narrow the document to one tag — a `docs/API.md` section name such as
    /// `Trades` or `Portfolio reports`, URL-encoded — serving only that tag's
    /// operations and the component schemas they reach. A tag no operation
    /// carries is refused `400`, naming the valid tags. Omitted (with
    /// `operation` omitted too), the whole document.
    tag: Option<String>,
    /// Narrow the document to the one operation with this `operationId`
    /// (`listTrades`, `runPortfolioOverview` — `GET /openapi/index` lists
    /// them all) and the component schemas it reaches. An id no operation
    /// carries is refused `400`; so is sending both `tag` and `operation`.
    operation: Option<String>,
}

/// `GET /openapi.json`: the whole document, or one tag's or one operation's
/// slice of it.
#[utoipa::path(
    get,
    path = "/openapi.json",
    summary = "This OpenAPI 3.1 document, generated from the route registrations and the serde structs; ?tag= narrows it to one tag's operations, or ?operation= to one operationId, with the schemas they reach. Behind [auth] like every other route.",
    params(DocumentQuery)
)]
async fn serve_document(
    axum::Extension(served): axum::Extension<Arc<Served>>,
    Query(query): Query<DocumentQuery>,
) -> Result<DocumentJson, ApiError> {
    let body = match (query.tag, query.operation) {
        (None, None) => served.whole.clone(),
        (Some(tag), None) => tag_slice(&served.document, &tag)
            .ok_or_else(|| {
                let known: Vec<&str> = served.document["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t["name"].as_str())
                    .collect();
                ApiError::BadRequest(format!(
                    "no operation is tagged `{tag}`; the tags are: {}",
                    known.join(", ")
                ))
            })?
            .to_string(),
        (None, Some(id)) => operation_slice(&served.document, &id)
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "no operation has the operationId `{id}`; GET /openapi/index lists them"
                ))
            })?
            .to_string(),
        (Some(_), Some(_)) => {
            return Err(ApiError::BadRequest(
                "send at most one of ?tag= and ?operation=".to_owned(),
            ));
        }
    };
    Ok(DocumentJson(body))
}

/// The serialised document (or a slice of it): JSON, but no struct models it,
/// so it is described as a free-form object.
struct DocumentJson(String);

impl IntoResponse for DocumentJson {
    fn into_response(self) -> response::Response {
        ([(header::CONTENT_TYPE, "application/json")], self.0).into_response()
    }
}

impl utoipa::IntoResponses for DocumentJson {
    fn responses() -> BTreeMap<String, RefOr<Response>> {
        ResponsesBuilder::new()
            .response(
                "200",
                ResponseBuilder::new()
                    .description(
                        "The OpenAPI 3.1 document for this API, as described by info above.",
                    )
                    .content(
                        "application/json",
                        ContentBuilder::new()
                            .schema(Some(<serde_json::Value as JsonBody>::schema()))
                            .build(),
                    ),
            )
            .build()
            .into()
    }
}

/// `GET /openapi/index`: the compact route index, as plain text.
#[utoipa::path(
    get,
    path = "/openapi/index",
    summary = "The compact route index (text/plain): one line per operation — verb, path, operationId, summary — grouped by tag, each group headed by the request for that tag's slice of /openapi.json."
)]
async fn serve_index(axum::Extension(served): axum::Extension<Arc<Served>>) -> Payload<PlainText> {
    Payload::new("text/plain; charset=utf-8", served.index.clone())
}

/// Every **`GET`-one** route the document carries: a `GET` on a path with a
/// parameter whose success body is a single object (not an array, which is how
/// a path-narrowed list like `/exchange_holidays/{mic}` is told apart). Read by
/// the missing-row 404 test below so a new GET-one is covered without a hand-kept
/// list.
#[cfg(test)]
pub(crate) fn documented_get_one_routes() -> Vec<String> {
    let document = serde_json::to_value(document()).expect("the document serialises");
    let mut out: Vec<String> = document["paths"]
        .as_object()
        .expect("paths is an object")
        .iter()
        .filter(|(path, item)| {
            path.contains('{')
                && item["get"]["responses"]["200"]["content"]["application/json"]["schema"]
                    .get("$ref")
                    .is_some()
        })
        .map(|(path, _)| path.clone())
        .collect();
    out.sort();
    out
}

/// Every **id-keyed collection create** the document carries — a
/// parameterless `POST /<collection>` answering `201 Created` — sorted.
///
/// Read by `doc_checks` so the Response-codes `201` row is *derived* from the
/// document the live router's registrations generate, rather than transcribed
/// into a second list that a new entity silently escapes.
///
/// Two parameterless `201` `POST`s are operations rather than collection
/// creates — `/attachments` (an upload) and `/closing_prices/fetch` (a fetch)
/// — which the `201` row names in its operation clauses instead of its
/// collection list. They are listed here so a *new* operation-shaped create
/// fails this test until it is classified rather than quietly widening the
/// collection set.
#[cfg(test)]
pub(crate) fn documented_id_keyed_collection_creates() -> Vec<String> {
    const OPERATION_CREATES: &[&str] = &["/attachments", "/closing_prices/fetch"];
    let document = serde_json::to_value(document()).expect("the document serialises");
    let mut out: Vec<String> = document["paths"]
        .as_object()
        .expect("paths is an object")
        .iter()
        .filter(|(path, item)| {
            item["post"]["responses"].get("201").is_some()
                && !path.contains('{')
                && !OPERATION_CREATES.contains(&path.as_str())
        })
        .map(|(path, _)| path.clone())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::route_tables;
    use crate::infra::http;
    use crate::reports::row_history;
    use crate::test_support::ApiClient;
    use crate::{test_support, web};
    use axum::http::StatusCode;
    use serde_json::Value;
    use std::collections::{BTreeMap, BTreeSet, HashSet};
    use std::sync::LazyLock;

    /// The document under test, as JSON.
    fn doc() -> Value {
        serde_json::to_value(document()).expect("the document serialises")
    }

    /// Every `(path, METHOD)` the document declares, sorted.
    fn documented_routes(doc: &Value) -> Vec<(String, String)> {
        let mut routes = Vec::new();
        let paths = doc["paths"].as_object().expect("paths is an object");
        for (path, item) in paths {
            for method in item.as_object().expect("a path item is an object").keys() {
                let method = method.to_ascii_uppercase();
                if matches!(method.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
                    routes.push((path.clone(), method));
                }
            }
        }
        routes.sort();
        routes
    }

    /// A request or response payload, as the published document records it —
    /// read back out of [`document`] so the tests below can ask about a route's
    /// shape without re-walking its JSON each time.
    #[derive(Clone, Debug, PartialEq)]
    enum Body {
        /// No payload at all.
        None,
        /// `application/json`, a `$ref` to the named component.
        Json(String),
        /// `application/json`, an array of `$ref`s to the named component.
        JsonArray(String),
        /// `application/json`, an array of integers.
        JsonIntegers,
        /// `application/json`, a free-form object; the string is the response
        /// description.
        JsonFree(String),
        /// `application/x-www-form-urlencoded`, a `$ref` to the named component.
        Form(String),
        /// A non-JSON payload, by media type.
        Other(String),
    }

    /// One published operation: `(verb, path, success statuses, summary,
    /// request body, success response)`.
    type RouteRow = (Verb, String, Vec<u16>, String, Body, Body);

    /// The body a request or response `content` map describes.
    fn body_of(content: &Value, description: &str) -> Body {
        let Some((media, entry)) = content.as_object().and_then(|c| c.iter().next()) else {
            return Body::None;
        };
        let schema = &entry["schema"];
        let name = |schema: &Value| {
            schema["$ref"]
                .as_str()
                .map(|r| r.trim_start_matches("#/components/schemas/").to_owned())
        };
        match media.as_str() {
            "application/json" => {
                if let Some(name) = name(schema) {
                    Body::Json(name)
                } else if schema["type"] == "array" {
                    name(&schema["items"]).map_or(Body::JsonIntegers, Body::JsonArray)
                } else {
                    Body::JsonFree(description.to_owned())
                }
            }
            FORM => Body::Form(name(schema).expect("a form body names its schema")),
            other => Body::Other(other.to_owned()),
        }
    }

    /// Every published operation of the auth-on document, in path order.
    static OPERATIONS: LazyLock<Vec<RouteRow>> = LazyLock::new(|| {
        let doc = doc();
        let mut rows = Vec::new();
        for (path, item) in doc["paths"].as_object().expect("paths is an object") {
            for (method, verb) in [
                ("get", Verb::Get),
                ("post", Verb::Post),
                ("put", Verb::Put),
                ("delete", Verb::Delete),
            ] {
                let Some(op) = item.get(method) else {
                    continue;
                };
                let responses = op["responses"].as_object().expect("responses");
                let statuses: Vec<u16> = responses
                    .keys()
                    .filter_map(|s| s.parse().ok())
                    .filter(|s| *s < 400)
                    .collect();
                let success = responses
                    .iter()
                    .find(|(status, _)| matches!(status.as_str(), "200" | "201"))
                    .map_or(Body::None, |(_, r)| {
                        body_of(&r["content"], r["description"].as_str().unwrap_or(""))
                    });
                rows.push((
                    verb,
                    path.clone(),
                    statuses,
                    op["summary"].as_str().unwrap_or("").to_owned(),
                    body_of(&op["requestBody"]["content"], ""),
                    success,
                ));
            }
        }
        rows
    });

    /// The routes registered **outside** `routes!` — through axum's own
    /// `.route(…)`, which `OpenApiRouter` passes through undocumented — each
    /// with why the document still covers it (or deliberately does not).
    /// Everything else is a `#[utoipa::path]` handler registered by `routes!`,
    /// whose route and document entry are one declaration, so this short list
    /// is all that can drift.
    const UNDOCUMENTED_REGISTRATIONS: [(&str, &str); 2] = [
        (
            "app.rs",
            "the base_path redirect (`{base_path}/`) exists only when a prefix is configured; \
             the document describes the unprefixed paths `nest` serves, so it is deliberately \
             absent",
        ),
        (
            "web.rs",
            "the `/static/*.js` loop builds its paths from `JS_MODULES`, and `publish` documents \
             the module routes from that same list",
        ),
    ];

    /// No route is registered with axum's plain `.route(…)` outside
    /// [`UNDOCUMENTED_REGISTRATIONS`]: a handler added that way would be served
    /// with no operation in the document.
    #[test]
    fn every_route_is_registered_through_routes() {
        let needle = format!(".{}(", "route");
        let mut sites: Vec<String> = test_support::rust_sources()
            .iter()
            .filter(|source| source.production.contains(&needle))
            .map(|source| source.path.clone())
            .filter(|file| file != "api_spec.rs")
            .collect();
        sites.sort();
        let classified: Vec<String> = UNDOCUMENTED_REGISTRATIONS
            .iter()
            .map(|(file, _)| (*file).to_owned())
            .collect();
        assert_eq!(
            sites, classified,
            "a route registered with plain `.route(…)` is served but not documented — register \
             a `#[utoipa::path]` handler with `routes!`, or classify the site in \
             UNDOCUMENTED_REGISTRATIONS"
        );
        let doc = doc();
        for (path, _) in web::JS_MODULES {
            assert!(
                doc["paths"][path]["get"].is_object(),
                "the served module {path} is not documented"
            );
        }
    }

    /// A URL a client can write to (`PUT`) or remove (`DELETE`) is one it can
    /// also read with a `GET` — otherwise the only way to see what a write is
    /// about to replace is to scan a list for it (2026-10-04 API sweep). The
    /// exceptions are URLs that do not address a stored row of their own, each
    /// with where its state is read instead.
    #[test]
    fn every_writable_url_is_readable() {
        const NOT_A_READABLE_ROW: &[(&str, &str)] = &[
            (
                "/sells/{id}",
                "a Sell is a trade: it is read at GET /trades/{id} (and its \
                 allocations at /parcel_allocations); /sells is the write path \
                 that keeps the trade and its allocations atomic",
            ),
            (
                "/income/{id}/reinvest",
                "an operation on an income row, not a row: its state is the \
                 row's reinvest_trade_id, read at GET /income/{id}",
            ),
        ];
        let readable: BTreeSet<&str> = OPERATIONS
            .iter()
            .filter(|row| row.0 == Verb::Get)
            .map(|row| row.1.as_str())
            .collect();
        let mut unreadable: Vec<&str> = OPERATIONS
            .iter()
            .filter(|row| matches!(row.0, Verb::Put | Verb::Delete))
            .map(|row| row.1.as_str())
            .filter(|path| !readable.contains(path))
            .filter(|path| !NOT_A_READABLE_ROW.iter().any(|(p, _)| p == path))
            .collect();
        unreadable.dedup();
        assert!(
            unreadable.is_empty(),
            "writable URLs with no GET — add the GET-one or classify them in \
             NOT_A_READABLE_ROW with where they are read: {unreadable:?}"
        );
        for (path, _) in NOT_A_READABLE_ROW {
            assert!(
                !readable.contains(path),
                "{path} is readable now; drop it from NOT_A_READABLE_ROW"
            );
        }
    }

    /// The `201` rule (docs/API.md, Creating a record): a `POST` answers
    /// `201 Created` only with the resource it created — a row the API serves
    /// from a `GET`, or a group of the rows one operation created together.
    /// Anything else a `POST` answers (a batch summary of what an import,
    /// backfill or snapshot run stored, a report, a preview) is `200`, because
    /// it is not a resource a client could read back (2026-10-04 API sweep).
    #[test]
    fn every_201_post_answers_the_resource_it_created() {
        const CREATED_GROUPS: &[(&str, &str)] = &[
            (
                "TransferGroup",
                "the transfer-out Sell and its transfer-in Buys",
            ),
            (
                "Participation",
                "the buy-back's closing Sell and its dividend income row",
            ),
            (
                "Demerge",
                "the closing Sell and the head and demerged replacement parcels",
            ),
            (
                "ScripExchange",
                "the closing Sell and the replacement parcels",
            ),
            ("Recognise", "the closing Sell at nil proceeds"),
            (
                "GeneratedAdjustments",
                "the AMIT adjustment rows written for the statement",
            ),
        ];
        let readable: BTreeSet<&str> = OPERATIONS
            .iter()
            .filter(|row| row.0 == Verb::Get)
            .filter_map(|row| match &row.5 {
                Body::Json(name) | Body::JsonArray(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let mut offenders = Vec::new();
        for (verb, path, statuses, _, _, response) in OPERATIONS.iter() {
            if *verb != Verb::Post || !statuses.contains(&201) {
                continue;
            }
            let created = match response {
                Body::Json(name) => {
                    readable.contains(name.as_str())
                        || CREATED_GROUPS.iter().any(|(group, _)| group == name)
                }
                _ => false,
            };
            if !created {
                offenders.push(path.as_str());
            }
        }
        assert!(
            offenders.is_empty(),
            "POSTs answering 201 with something other than the resource they created — \
             answer 200, or classify a new created group in CREATED_GROUPS: {offenders:?}"
        );
        for (group, _) in CREATED_GROUPS {
            assert!(
                !readable.contains(group),
                "{group} is served by a GET now; drop it from CREATED_GROUPS"
            );
        }
    }

    /// A representative money field is a JSON **string**, followed through the
    /// `$ref` into `components.schemas`: the trade body's `average_price`,
    /// which the code declares as `Decimal`.
    #[test]
    fn a_money_field_is_a_json_string() {
        let doc = doc();
        let field = &doc["components"]["schemas"]["TradeBody"]["properties"]["average_price"];
        assert!(
            !field.is_null(),
            "TradeBody.average_price should be in components.schemas; got {field}"
        );
        assert_eq!(
            field["type"], "string",
            "a money field must be typed string, never number: {field}"
        );
    }

    /// Every request-body schema carries `additionalProperties: false`, the
    /// structural half of the deny-unknown-fields rule (the derive reads
    /// `#[serde(deny_unknown_fields)]`).
    ///
    /// The bodies are read out of [`OPERATIONS`] rather than transcribed, so a
    /// route added with a new body schema is covered the moment it is
    /// registered — there is no list here to forget to update.
    #[test]
    fn every_request_body_schema_denies_unknown_fields() {
        let doc = doc();
        let schemas = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas is an object");
        let mut bodies: Vec<&str> = OPERATIONS
            .iter()
            .filter_map(|(_, _, _, _, request, _)| match request {
                Body::Json(name) | Body::Form(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        bodies.sort_unstable();
        bodies.dedup();
        assert!(
            bodies.len() > 30,
            "only {} request bodies were read out of the document — it has stopped \
             registering them",
            bodies.len()
        );
        for name in bodies {
            let schema = &schemas[name];
            assert_eq!(
                schema["additionalProperties"], false,
                "request body `{name}` must deny unknown fields (additionalProperties: false)"
            );
        }
    }

    /// The two global rules ride in `info.description` as well as in the
    /// schemas — a machine client that reads only the prose still learns them.
    ///
    /// Both halves are checked: that the description states each rule, and that
    /// the document it is the preamble to actually keeps it. The second half is
    /// the point — a prose pin on its own could only ever say the sentence is
    /// still typed there, so the same walks that
    /// [`no_component_schema_advertises_a_json_number`] and
    /// [`every_request_body_schema_denies_unknown_fields`] make are made here
    /// over the schemas the description is promising for, which is what makes
    /// the promise true rather than present.
    #[test]
    fn the_two_global_rules_are_stated_in_the_description() {
        let doc = doc();
        let description = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        for rule in [
            "Money and quantities travel as JSON strings, in both directions.",
            "Every request body denies unknown fields.",
            "additionalProperties: false",
            "422",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }

        // …and the document keeps both promises.
        let mut numbers = Vec::new();
        collect_number_schemas(&doc["components"]["schemas"], &mut numbers);
        assert!(
            numbers.is_empty(),
            "the description promises money and quantities travel as strings, but these schemas \
             advertise a JSON number: {numbers:?}"
        );
        let schemas = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas is an object");
        let mut bodies = 0;
        for (_, _, _, _, request, _) in OPERATIONS.iter() {
            let (Body::Json(name) | Body::Form(name)) = request else {
                continue;
            };
            assert_eq!(
                schemas[name]["additionalProperties"], false,
                "the description promises every request body denies unknown fields, but `{name}` \
                 does not"
            );
            bodies += 1;
        }
        assert!(
            bodies > 30,
            "only {bodies} request bodies checked — the walk has stopped finding them"
        );
    }

    /// The error-body matrix — the never-JSON rule, the media type, and every
    /// status's body shape — rides in `info.description`, which is the
    /// machine-client surface. `docs/API.md`'s "Error-body contract" section
    /// is the long form (`doc_checks` pins that copy); this is the compact
    /// twin a client reading only the generated document gets.
    ///
    /// The status list is **cross-checked against the code**: every
    /// body-carrying `ApiError` shape (read from `infra::http`'s one sample
    /// table) must be named here, so a new variant fails rather than leaving
    /// the compact contract silently short.
    #[test]
    fn the_error_body_matrix_is_stated_in_the_description() {
        let doc = doc();
        let description = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        assert!(description.contains("Errors are never JSON."));
        assert!(description.contains("text/plain; charset=utf-8"));
        for (code, has_body) in http::documented_error_shapes() {
            if !has_body {
                continue;
            }
            // The 500 is named through the shape it comes from (the manual job
            // trigger), not as a bare code — which the empty-bodied 500 shares.
            let needle = if code == 500 {
                "a failed POST /jobs/{name}'s 500".to_owned()
            } else {
                code.to_string()
            };
            assert!(
                description.contains(&needle),
                "info.description must name the body-carrying {code}: {description}"
            );
        }
        // The one body-carrying status no `ApiError` variant answers.
        assert!(
            description.contains("415"),
            "info.description must name axum's 415: {description}"
        );
        // …and the empty-bodied ones, both 404s and both 500s named apart. The
        // 404 is qualified on both sides, because the two shapes are told apart
        // by whether a route matched: a missing row is named in the body
        // (`every_get_one_route_names_the_missing_row_in_its_404` drives the
        // GETs), while only a path no route matches is empty.
        assert!(description.contains(
            "a deliberately empty body: the 404 of a path no route matches (so an empty \
             404 means the URL is wrong, a text one that the row is absent), a 405, and an \
             internal 500"
        ));
        assert!(
            description.contains(
                "404 naming the missing row or prerequisite (a GET or DELETE of one row, an \
                 operation, or a read whose parameter names a missing row)"
            ),
            "info.description must say a missing row's 404 carries its reason: {description}"
        );
    }

    /// The reading-a-list contract — ascending order with its newest-first
    /// browse exceptions, the four report reads that keep a POST body, and the
    /// one paginated endpoint with both its shapes — rides in
    /// `info.description`, the machine-client surface. `docs/API.md`'s
    /// "Reading a list" section is the long form (`doc_checks` pins that copy);
    /// this is the compact twin a client reading only the generated document
    /// gets, and it must name each endpoint so a dropped one fails here.
    ///
    /// Two of the three halves are cross-checked rather than merely present:
    /// the POST-bodied report reads are read out of [`OPERATIONS`] (below), and the
    /// page-size claim is **formatted from
    /// `reports::row_history::DEFAULT_BROWSE_LIMIT`/`MAX_BROWSE_LIMIT`**, the
    /// consts the handler validates against, so raising the cap cannot leave the
    /// prose behind.
    ///
    /// The ordering clause is a **requirement pin only** — it asserts the
    /// sentence is there, and cannot catch a list that started ordering the
    /// other way. That behaviour is pinned where it happens, per surface
    /// (`listing_rename`'s `api_list_renames_returns_newest_first`,
    /// `closing_price`'s and `distribution_event`'s list tests,
    /// `scheduler::db`'s run history, `reports::row_history`'s trail order) and,
    /// for the client side of it, by `web`'s
    /// `tables_open_newest_first_on_their_own_date_column`. Deriving the set
    /// here would mean reflecting over every list's `ORDER_BY` — including the
    /// hand-written queries' SQL — which no scan can do honestly, so it is
    /// stated rather than pretended.
    #[test]
    fn the_list_reading_contract_is_stated_in_the_description() {
        let doc = doc();
        let description = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        for rule in [
            // Ordering: the rule, then the newest-first exceptions by name.
            "list endpoints return rows ascending",
            "except the deliberate newest-first browse surfaces (/closing_prices, \
             /distribution_events, a listing's rename chain, a job's run history, and the \
             /reports/row_history trail)",
            "This is the reverse of the web UI, which sorts its own tables newest-first \
             client-side.",
            // Pagination: the one endpoint, both shapes and the cursor facts.
            "/reports/row_history is the only paginated endpoint",
            "with row_id it answers that row's whole trail as a bare JSON array",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }
        // The page-size claim, formatted from the consts the handler enforces.
        let bounds = format!(
            "before_id returns entries older than that trail id and limit is 1-{} (default {})",
            row_history::MAX_BROWSE_LIMIT,
            row_history::DEFAULT_BROWSE_LIMIT
        );
        assert!(
            description.contains(&bounds),
            "info.description must state `{bounds}`; got:\n{description}"
        );
        // The POST-bodied report reads are **derived from the document**,
        // not from a copy of the sentence that names them: the compact contract
        // must name every one, and no new `POST /portfolio/*` report read can
        // quietly join the set without being documented here.
        let mut posts: Vec<String> = OPERATIONS
            .iter()
            .filter(|(verb, path, _, _, _, _)| {
                *verb == Verb::Post && path.starts_with("/portfolio/")
            })
            .map(|(_, path, _, _, _, _)| (*path).to_string())
            .collect();
        posts.sort();
        assert_eq!(
            posts,
            [
                "/portfolio/net_capital_gain/what_if",
                "/portfolio/overview",
                "/portfolio/performance",
                "/portfolio/unrealised_gains",
            ],
            "the POST-bodied report reads have changed — the description must name the new set"
        );
        for path in &posts {
            assert!(
                description.contains(path.as_str()),
                "info.description must name the POST-bodied report read {path}"
            );
        }
    }

    /// Every operation in the emitted document — auth on, so `/login` and
    /// `/logout` are included, and the `/static/*.js` modules too — carries an
    /// `operationId`, the ids are unique across the document, and each carries
    /// exactly one tag, which is a real `## ` heading of `docs/API.md` and
    /// appears in the document's top-level `tags` list. That list in turn
    /// names only tags some operation uses, each once.
    #[test]
    fn every_operation_has_a_unique_id_and_one_documented_tag() {
        let api_md = include_str!("../docs/API.md");
        let sections: HashSet<&str> = api_md
            .lines()
            .filter_map(|line| line.strip_prefix("## "))
            .collect();
        let doc = doc();
        let listed: Vec<&str> = doc["tags"]
            .as_array()
            .expect("the document lists its tags")
            .iter()
            .map(|tag| tag["name"].as_str().expect("a tag has a name"))
            .collect();
        let mut ids = BTreeMap::new();
        let mut used = BTreeSet::new();
        let mut operations = 0;
        for (path, item) in doc["paths"].as_object().expect("paths is an object") {
            for (method, op) in item.as_object().expect("a path item is an object") {
                operations += 1;
                let id = op["operationId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{method} {path} has no operationId"));
                assert!(
                    id.chars().all(|c| c.is_ascii_alphanumeric()),
                    "{method} {path}'s operationId {id:?} is not a plain identifier"
                );
                if let Some(other) = ids.insert(id.to_owned(), format!("{method} {path}")) {
                    panic!("{method} {path} and {other} share the operationId {id}");
                }
                let tags = op["tags"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{method} {path} has no tags"));
                assert_eq!(tags.len(), 1, "{method} {path} must carry exactly one tag");
                let tag = tags[0].as_str().expect("a tag is a string");
                assert!(
                    sections.contains(tag),
                    "{method} {path}'s tag {tag:?} is not a `## ` section of docs/API.md"
                );
                assert!(
                    listed.contains(&tag),
                    "{method} {path}'s tag {tag:?} is missing from the top-level tags"
                );
                used.insert(tag);
            }
        }
        assert!(operations > 150, "only {operations} operations were walked");
        let mut seen = HashSet::new();
        for tag in &listed {
            assert!(seen.insert(tag), "the top-level tags list {tag:?} twice");
            assert!(
                used.contains(tag),
                "the top-level tag {tag:?} has no operation"
            );
        }
        // The derivation, spot-checked on each verb word and on a nested
        // operation filed under its own section.
        let op = |path: &str, method: &str| doc["paths"][path][method].clone();
        assert_eq!(op("/trades", "get")["operationId"], "listTrades");
        assert_eq!(op("/trades/{id}", "get")["operationId"], "getTradesById");
        assert_eq!(op("/sells", "post")["operationId"], "createSells");
        assert_eq!(
            op("/listings/{id}", "put")["operationId"],
            "upsertListingsById"
        );
        assert_eq!(
            op("/portfolio/overview", "post")["operationId"],
            "runPortfolioOverview"
        );
        assert_eq!(
            op("/exchange_holidays/{mic}/{date}", "delete")["operationId"],
            "deleteExchangeHolidaysByMicAndDate"
        );
        assert_eq!(op("/", "get")["operationId"], "getIndex");
        assert_eq!(
            op("/income/{id}/reinvest", "post")["tags"][0],
            "DRP reinvestment"
        );
        assert_eq!(op("/income/{id}", "get")["tags"][0], "Income");
        assert_eq!(op("/reports/health", "get")["tags"][0], "Portfolio reports");
    }

    /// Without `[auth]` the Authentication tag goes with its routes: the
    /// top-level list names only tags some published operation uses.
    #[test]
    fn the_tag_list_follows_the_published_routes() {
        let doc = serde_json::to_value(document_for(&BasePath::ROOT, AuthMode::Open))
            .expect("the document serialises");
        let listed: Vec<&str> = doc["tags"]
            .as_array()
            .expect("the document lists its tags")
            .iter()
            .filter_map(|tag| tag["name"].as_str())
            .collect();
        assert!(!listed.contains(&"Authentication"), "{listed:?}");
        assert!(listed.contains(&"Trades"), "{listed:?}");
    }

    /// Every success status an operation documents carries a body exactly when
    /// it has one to carry: a `200` or `201` names its content, and a `204` or
    /// `303` names none. The statuses and bodies come from the handlers'
    /// return types (`infra::respond`), so this pins those types' own
    /// `IntoResponses` — a new response type that forgot its body, or gave a
    /// `204` one, fails here.
    #[test]
    fn every_success_response_carries_its_body() {
        let doc = doc();
        let mut checked = 0;
        for (path, item) in doc["paths"].as_object().expect("paths") {
            for (method, op) in item.as_object().expect("a path item") {
                for (status, response) in op["responses"].as_object().expect("responses") {
                    let content = response.get("content");
                    match status.as_str() {
                        "200" | "201" => assert!(
                            content.is_some_and(|c| c.as_object().is_some_and(|c| !c.is_empty())),
                            "{method} {path}'s {status} has no body"
                        ),
                        "204" | "303" => assert!(
                            content.is_none(),
                            "{method} {path}'s {status} must carry no body, found {content:?}"
                        ),
                        _ => continue,
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 150, "only {checked} success responses checked");
    }

    /// Every property of every schema a request body reaches — the bodies
    /// [`OPERATIONS`] names and every schema they `$ref`, transitively (a Sell's
    /// allocation rows, a what-if's parcels) — carries a non-empty
    /// `description`. That text is each field's `///` doc comment, and it is
    /// what an agent filling a body from the OpenAPI document has to go on:
    /// the unit, the convention (`foreign per 1 AUD`), what an omission
    /// defaults to. A new body field without one fails here, naming it.
    ///
    /// An `Option<Enum>` field is emitted as `oneOf: [{$ref, description},
    /// {type: null}]` — utoipa puts the comment on the `$ref` branch — so a
    /// description on any `oneOf`/`allOf`/`anyOf` branch counts too.
    #[test]
    fn every_request_body_property_is_described() {
        fn described(property: &Value) -> bool {
            let has = |v: &Value| {
                v.get("description")
                    .and_then(Value::as_str)
                    .is_some_and(|d| !d.trim().is_empty())
            };
            has(property)
                || ["oneOf", "allOf", "anyOf"].iter().any(|k| {
                    property
                        .get(*k)
                        .and_then(Value::as_array)
                        .is_some_and(|branches| branches.iter().any(has))
                })
        }
        /// Every `$ref`'d component name anywhere under `v`, and every
        /// `properties` map met on the way (a tagged enum's variants each
        /// carry their own).
        fn walk<'a>(v: &'a Value, refs: &mut Vec<String>, props: &mut Vec<&'a Value>) {
            match v {
                Value::Object(map) => {
                    if let Some(Value::String(r)) = map.get("$ref") {
                        refs.push(r.trim_start_matches("#/components/schemas/").to_owned());
                    }
                    if let Some(properties) = map.get("properties") {
                        props.push(properties);
                    }
                    for c in map.values() {
                        walk(c, refs, props);
                    }
                }
                Value::Array(items) => {
                    for c in items {
                        walk(c, refs, props);
                    }
                }
                _ => {}
            }
        }

        let doc = doc();
        let schemas = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas is an object");
        let mut queue: Vec<String> = OPERATIONS
            .iter()
            .filter_map(|(_, _, _, _, request, _)| match request {
                Body::Json(name) | Body::JsonArray(name) | Body::Form(name) => {
                    Some(name.to_string())
                }
                _ => None,
            })
            .collect();
        let mut seen = BTreeSet::new();
        let mut undescribed = BTreeSet::new();
        let mut checked = 0;
        while let Some(name) = queue.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let schema = schemas
                .get(&name)
                .unwrap_or_else(|| panic!("request schema {name} is not a component"));
            let mut props = Vec::new();
            walk(schema, &mut queue, &mut props);
            for properties in props {
                for (field, property) in properties.as_object().expect("properties is an object") {
                    checked += 1;
                    if !described(property) {
                        undescribed.insert(format!("{name}.{field}"));
                    }
                }
            }
        }
        assert!(
            undescribed.is_empty(),
            "request-body properties with no description (add a `///` doc comment \
             to the field): {undescribed:?}"
        );
        // The bodies and their nested rows: far more than a handful, so an
        // empty walk cannot pass vacuously.
        assert!(
            seen.len() > 40,
            "only {} request schemas walked",
            seen.len()
        );
        assert!(checked > 200, "only {checked} request properties checked");
    }

    /// Every `PUT` route records the statuses it can answer: the create/replace
    /// pair `[201, 204]` for the upserts, and the single status the two
    /// documented exceptions can give. A new `PUT` route that records neither
    /// fails here, so the create-vs-replace signal cannot go missing from the
    /// machine-client contract.
    #[test]
    fn every_put_route_documents_its_outcome() {
        use crate::entities::route_tables::{PUT_ROUTES, PutOutcome};

        // The entity route classification is the **one** source for which PUTs
        // report the create/replace pair and which are the two deliberate
        // exceptions — read, not re-declared here, so the document cannot lie
        // while both tests stay green.
        let expected_statuses = |path: &str| -> &'static [u16] {
            match PUT_ROUTES
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, outcome)| *outcome)
            {
                Some(PutOutcome::CreateThenReplace) => &[201, 204],
                // Create-only: always 201 with the executed group.
                Some(PutOutcome::AlwaysCreatedGroup) => &[201],
                // A correction of an existing row: never creates, so always 204.
                Some(PutOutcome::NeverCreates) => &[204],
                None => {
                    panic!("PUT {path} is not classified in entities::route_tables::PUT_ROUTES")
                }
            }
        };
        let doc = doc();
        let mut puts = 0;
        for (verb, path, statuses, _, _, _) in OPERATIONS.iter() {
            if *verb != Verb::Put {
                continue;
            }
            puts += 1;
            assert_eq!(
                statuses.as_slice(),
                expected_statuses(path),
                "PUT {path} records statuses that disagree with its entities::route_tables::PUT_ROUTES \
                 classification"
            );
            let responses = doc["paths"][path.as_str()]["put"]["responses"]
                .as_object()
                .unwrap_or_else(|| panic!("PUT {path} has no documented responses"));
            for status in statuses {
                let documented = responses
                    .get(&status.to_string())
                    .unwrap_or_else(|| panic!("PUT {path} does not document {status}"));
                // A 201 is the created row (a body); a 204 never has one.
                if *status == 201 {
                    assert!(
                        documented.get("content").is_some(),
                        "PUT {path}'s 201 must carry the created row schema"
                    );
                } else {
                    assert!(
                        documented.get("content").is_none(),
                        "PUT {path}'s 204 must carry no body"
                    );
                }
            }
        }
        assert_eq!(
            puts,
            PUT_ROUTES.len(),
            "the documented PUT routes and the classified PUT routes must be the same set; a \
             new `PUT` route must be classified in entities::route_tables::PUT_ROUTES"
        );
    }

    /// The PUT outcome rule rides in `info.description`, the machine-client
    /// surface, so a client reading only the generated document learns that a
    /// `PUT` reports whether it created or replaced.
    ///
    /// The two single-status exceptions are **read out of
    /// `entities::route_tables::PUT_ROUTES`** rather than transcribed: a third one, or a
    /// reclassified existing one, changes what the description has to say and
    /// fails here until it does. The general rule stays a prose pin — it is
    /// pinned behaviourally by `entities::tests::put::every_put_route_reports_create_then_replace`
    /// and structurally by [`every_put_route_documents_its_outcome`].
    #[test]
    fn the_put_outcome_rule_is_stated_in_the_description() {
        use crate::entities::route_tables::{PUT_ROUTES, PutOutcome};
        let doc = doc();
        let description = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        for rule in [
            "A PUT upsert reports its outcome.",
            "answers 201 Created carrying the created row",
            "204 No Content when it replaced an existing row",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }

        let mut exceptions = 0;
        for (path, outcome) in PUT_ROUTES {
            let status = match outcome {
                PutOutcome::CreateThenReplace => continue,
                PutOutcome::AlwaysCreatedGroup => 201,
                PutOutcome::NeverCreates => 204,
            };
            let at = description
                .find(path)
                .unwrap_or_else(|| panic!("info.description must name the {path} exception"));
            let sentence = description[at..].split('.').next().unwrap_or_default();
            assert!(
                sentence.contains(&format!("always answers {status}")),
                "info.description must say {path} always answers {status}; it says: {sentence}"
            );
            exceptions += 1;
        }
        assert_eq!(
            exceptions, 2,
            "the single-status PUT exceptions have changed; the description must describe the \
             new set"
        );
    }

    /// The 2026-09-24 REST-audit item "Rate-limit / lock out `POST /login`"
    /// (B6): the generated document must carry the lockout, because a machine
    /// client reading only `GET /openapi.json` is exactly the caller the item
    /// was written for — it needs to know a `429` with a `Retry-After` is a
    /// possible answer, and that retrying immediately will not work.
    ///
    /// Three surfaces, one fact: the route's summary, the operation's own
    /// documented `429` response, and the `info.description` error paragraph.
    #[test]
    fn the_login_lockout_is_documented() {
        let login = OPERATIONS
            .iter()
            .find(|(verb, path, _, _, _, _)| *verb == Verb::Post && *path == "/login")
            .expect("the POST /login route is documented");
        for fact in ["429", "Retry-After", "failed-attempt budget"] {
            assert!(
                login.3.contains(fact),
                "the POST /login summary must state `{fact}`: {}",
                login.3
            );
        }

        let doc = doc();
        let responses = doc["paths"]["/login"]["post"]["responses"]
            .as_object()
            .expect("POST /login documents its responses");
        let locked = responses
            .get("429")
            .expect("POST /login must document its 429");
        let description = locked["description"]
            .as_str()
            .expect("the 429 response has a description");
        assert!(
            description.contains("Retry-After"),
            "the documented 429 must name Retry-After: {description}"
        );
        assert!(
            description.contains("sign-in page"),
            "the documented 429 must state the browser's page answer: {description}"
        );
        // No other route carries it — the lockout is a login fact, and a 429
        // invented on the entity routes would describe behaviour no handler
        // has.
        let mut with_429 = Vec::new();
        for (path, item) in doc["paths"].as_object().expect("paths is an object") {
            for (method, operation) in item.as_object().expect("a path item is an object") {
                if operation["responses"].get("429").is_some() {
                    with_429.push(format!("{} {}", method.to_ascii_uppercase(), path));
                }
            }
        }
        assert_eq!(
            with_429,
            vec!["POST /login".to_owned()],
            "only POST /login answers 429"
        );

        // The error paragraph a client reading only `info.description` gets.
        let info = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        assert!(
            info.contains(
                "429 on POST /login once a source has exhausted its failed-attempt budget"
            ),
            "info.description must state the login 429: {info}"
        );
        assert!(info.contains("Retry-After"));
    }

    /// Every `$ref` in the document resolves to a component schema, and no
    /// component name carries two different schemas (which would make a `$ref`
    /// describe the wrong fields). A name reached twice from two routes is
    /// fine — the recursive collection registers it once per path — so only
    /// *differing* schemas under one name are an error.
    #[test]
    fn every_referenced_schema_is_registered_and_uniquely_named() {
        let schemas = component_schemas();
        let mut by_name: BTreeMap<&str, &RefOr<Schema>> = BTreeMap::new();
        for (name, schema) in &schemas {
            match by_name.get(name.as_str()) {
                None => {
                    by_name.insert(name.as_str(), schema);
                }
                Some(existing) => assert!(
                    *existing == schema,
                    "two different schemas are registered under the component name `{name}`; \
                     rename one with #[schema(as = …)]"
                ),
            }
        }
        assert!(
            by_name.len() > 200,
            "only {} component names were registered — the schema collection has stopped \
             collecting",
            by_name.len()
        );

        let doc = doc();
        let registered = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas is an object");
        let mut unresolved = Vec::new();
        collect_refs(&doc, &mut |name| {
            if !registered.contains_key(name) {
                unresolved.push(name.to_owned());
            }
        });
        assert!(
            unresolved.is_empty(),
            "these $refs name no registered component schema: {unresolved:?}"
        );
    }

    /// Walk every `$ref` string in a JSON value and hand its schema name to
    /// `visit`.
    fn collect_refs(value: &Value, visit: &mut impl FnMut(&str)) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "$ref" {
                        if let Some(name) = child.as_str().and_then(|r| r.rsplit('/').next()) {
                            visit(name);
                        }
                    } else {
                        collect_refs(child, visit);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_refs(item, visit);
                }
            }
            _ => {}
        }
    }

    /// The document carries the `openapi` version, `info` and `paths` a client
    /// needs.
    #[test]
    fn the_document_has_the_openapi_envelope() {
        let doc = doc();
        assert!(
            doc["openapi"]
                .as_str()
                .is_some_and(|version| version.starts_with("3.1"))
        );
        assert!(doc["info"]["title"].is_string());
        assert!(doc["info"]["version"].is_string());
        assert!(doc["paths"].is_object());
    }

    /// Every filtered entity list's parameters reach the document as real
    /// `in: query` parameters, and so do the report reads' own. The document
    /// declared **no** query parameter at all before this, so a generated
    /// client could not narrow a list or pass a report's date and answered a
    /// `422` the contract never mentioned.
    #[test]
    fn every_query_parameter_reaches_the_document() {
        let doc = doc();
        let query_names = |path: &str, method: &str| -> Vec<String> {
            doc["paths"][path][method]["parameters"]
                .as_array()
                .map(|params| {
                    params
                        .iter()
                        .filter(|p| p["in"] == "query")
                        .filter_map(|p| p["name"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        for (path, params) in route_tables::filtered_list_routes() {
            let declared = query_names(path, "get");
            for param in params {
                assert!(
                    declared.iter().any(|d| d == param),
                    "the document must declare ?{param}= on GET {path}: {declared:?}"
                );
            }
        }
        // The report reads the review named, each of which survives only as
        // English in the summary without this.
        for (path, param) in [
            ("/portfolio/open_parcels", "as_of_date"),
            ("/reports/tax_report", "tax_year"),
            ("/reports/row_history", "before_id"),
            ("/portfolio/activity", "listing_id"),
        ] {
            assert!(
                query_names(path, "get").iter().any(|d| d == param),
                "the document must declare ?{param}= on GET {path}"
            );
        }
    }

    /// The `?name=` tokens a summary mentions — the prose side of the query
    /// parameters a route's `Query<T>` type declares.
    fn summary_query_names(summary: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut rest = summary;
        while let Some(at) = rest.find('?') {
            rest = &rest[at + 1..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() || !rest[name.len()..].starts_with('=') {
                continue;
            }
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// Every `?name=` a route summary mentions is a parameter that route's
    /// `Query<T>` type really has, and every parameter the type has is
    /// mentioned. The document is generated from the type (so a client gets the
    /// truth), and this is what stops the prose beside it drifting into
    /// describing a parameter that does not exist — which is how the first
    /// version of this feature shipped two snapshot routes' parameters swapped.
    #[test]
    fn every_query_parameter_matches_its_summary() {
        let doc = doc();
        for (verb, path, _, summary, _, _) in OPERATIONS.iter() {
            let method = format!("{verb:?}").to_ascii_lowercase();
            let mut declared: Vec<String> = doc["paths"][path.as_str()][&method]["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["in"] == "query")
                .filter_map(|p| p["name"].as_str().map(str::to_owned))
                .collect();
            let mut mentioned = summary_query_names(summary);
            declared.sort();
            mentioned.sort();
            assert_eq!(
                declared, mentioned,
                "{verb:?} {path}: the summary's ?name= tokens and the Query type's \
                 fields must be the same set"
            );
        }
    }

    /// A parameter a request is refused for omitting is `required` in the
    /// document, and one with a non-string type says so. Both come from the
    /// Rust type via `IntoParams`; the first version of this derived parameters
    /// from prose and could only emit "optional string", so `?tax_year=` — whose
    /// absence is a `400` — was advertised as optional, and `docs/API.md`'s
    /// "Required: yes" contradicted the document generated beside it.
    #[test]
    fn a_required_query_parameter_is_documented_as_required() {
        let doc = doc();
        let param = |path: &str, name: &str| -> serde_json::Value {
            doc["paths"][path]["get"]["parameters"]
                .as_array()
                .expect("the route must carry parameters")
                .iter()
                .find(|p| p["name"] == name)
                .unwrap_or_else(|| panic!("GET {path} must declare ?{name}="))
                .clone()
        };
        let tax_year = param("/reports/tax_report", "tax_year");
        assert_eq!(tax_year["required"], serde_json::json!(true));
        assert_eq!(tax_year["schema"]["type"], serde_json::json!("integer"));
        let listing_id = param("/portfolio/activity", "listing_id");
        assert_eq!(listing_id["required"], serde_json::json!(true));
        // A typed id is a `$ref` to its integer component (`infra::id`).
        let reference = listing_id["schema"]["$ref"]
            .as_str()
            .expect("a typed id parameter refers to its component");
        let component = reference.trim_start_matches("#/components/schemas/");
        assert_eq!(
            doc["components"]["schemas"][component]["type"],
            serde_json::json!("integer")
        );
        // …and an `Option` field is optional, so the two are really being told
        // apart rather than everything being stamped the same way.
        let as_of = param("/portfolio/open_parcels", "as_of_date");
        assert_eq!(as_of["required"], serde_json::json!(false));
        let price = param("/portfolio/activity", "price");
        assert_eq!(price["required"], serde_json::json!(false));
        // Money stays a string in the document, as everywhere else — an
        // optional field's type is the nullable pair, so what matters is that
        // it is the string and not a JSON number.
        let ty = price["schema"]["type"].to_string();
        assert!(ty.contains("string"), "price must be a string, got {ty}");
        assert!(!ty.contains("number"), "money must never be a JSON number");
    }

    /// The document carries the **non-success** responses a client must handle,
    /// not only the success ones: a `404` on every path-addressed
    /// GET/POST/DELETE, and a `502` on the feed-import and provider-fetch
    /// routes. It used to be success-only, so a generated client treated every
    /// documented 404 (and the imports' 502) as an unexpected status.
    #[test]
    fn non_success_responses_are_documented() {
        let doc = doc();
        let has = |path: &str, method: &str, status: &str| -> bool {
            doc["paths"][path][method]["responses"]
                .get(status)
                .is_some()
        };
        // A path-addressed read, delete and operation each carry a 404.
        assert!(has("/listings/{id}", "get", "404"));
        assert!(has("/listings/{id}", "delete", "404"));
        assert!(has("/income/{id}/reinvest", "post", "404"));
        // The two upstream-facing creates carry a 502.
        assert!(has("/rba_fx_rates/import", "post", "502"));
        assert!(has("/closing_prices/fetch", "post", "502"));
        // …and the bare-text import feeds carry the 422 a bad payload answers.
        assert!(has("/rba_fx_rates/import", "post", "422"));
        assert!(has("/currencies/import", "post", "422"));
        // Every route documents a generic 500; a body-taking route also
        // documents 413 and 415, which axum answers before the handler runs.
        assert!(has("/listings", "get", "500"));
        assert!(has("/listings", "post", "413"));
        assert!(has("/listings", "post", "415"));
        assert!(
            !has("/listings", "get", "413"),
            "a body-less GET has no 413"
        );
    }

    /// The whole document — schemas, not just the one field an assertion names —
    /// advertises no JSON number, so no money or quantity field can be sent or
    /// read as an `f64`. The `rust_decimal` codec renders every `Decimal` as a
    /// string; this is the non-vacuous version of the `TradeBody.average_price`
    /// spot check, and it fails on an added `f64` field or a switch to the
    /// `decimal_float` feature.
    #[test]
    fn no_component_schema_advertises_a_json_number() {
        let doc = doc();
        let mut numbers = Vec::new();
        collect_number_schemas(&doc["components"]["schemas"], &mut numbers);
        assert!(
            numbers.is_empty(),
            "these schemas advertise a JSON number, so a tax figure could travel as an f64: \
             {numbers:?}"
        );
    }

    fn collect_number_schemas(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if map.get("type").and_then(Value::as_str) == Some("number") {
                    out.push(serde_json::to_string(value).unwrap_or_default());
                }
                for child in map.values() {
                    collect_number_schemas(child, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_number_schemas(item, out);
                }
            }
            _ => {}
        }
    }

    /// The document publishes where it is mounted and what authenticates it: a
    /// `base_path` deployment must not list root-relative paths (which would
    /// 404), and an `[auth]` deployment must name the cookie and bearer schemes
    /// a client has to send. An auth-less deployment advertises neither, and
    /// omits the `/login`/`/logout` routes it does not serve.
    #[test]
    fn the_document_publishes_its_server_and_security() {
        let rooted = serde_json::to_value(document_for(&BasePath::ROOT, AuthMode::Required))
            .expect("serialises");
        assert_eq!(rooted["servers"][0]["url"], "/");
        assert!(rooted["components"]["securitySchemes"]["bearerAuth"].is_object());
        assert!(rooted["components"]["securitySchemes"]["sessionCookie"].is_object());
        assert!(
            rooted["security"].as_array().is_some_and(|s| !s.is_empty()),
            "an auth deployment must carry a global security requirement"
        );

        let prefixed = serde_json::to_value(document_for(
            &BasePath::of("/share_tracker"),
            AuthMode::Required,
        ))
        .expect("ser");
        assert_eq!(
            prefixed["servers"][0]["url"], "/share_tracker",
            "a base-path deployment must publish the prefix its paths resolve against"
        );

        let open = serde_json::to_value(document_for(&BasePath::ROOT, AuthMode::Open))
            .expect("serialises");
        assert!(
            open["components"]
                .get("securitySchemes")
                .is_none_or(|s| s.as_object().is_none_or(|m| m.is_empty())),
            "an auth-less deployment must declare no security scheme"
        );
        assert!(open["paths"].get("/login").is_none());
        assert!(open["paths"].get("/logout").is_none());
    }

    /// Serve check: the full application answers `GET /openapi.json` with the
    /// document, through `ApiClient::full` (whose auth-less router serves it).
    #[tokio::test]
    async fn get_openapi_json_serves_the_document() {
        let pool = test_support::test_pool().await;
        let client = ApiClient::full(&pool);
        let body: Value = client.get_json("/openapi.json").await;
        assert_eq!(body["openapi"].as_str(), Some("3.1.0"));
        assert!(body["info"]["title"].is_string());
        assert!(body["paths"]["/openapi.json"]["get"].is_object());
    }

    /// The route index lists exactly the operations the document publishes —
    /// one line each, carrying the document's own `operationId` — under the
    /// heading of the operation's tag, for both deployment shapes (without
    /// `[auth]` neither lists `/login`/`/logout`).
    #[test]
    fn the_route_index_lists_exactly_the_published_operations() {
        for mode in [AuthMode::Required, AuthMode::Open] {
            let document = document_for(&BasePath::ROOT, mode);
            let doc = serde_json::to_value(&document).expect("serialises");
            let index = route_index(&document, &BasePath::ROOT);
            let mut listed: Vec<(String, String)> = Vec::new();
            let mut heading = String::new();
            for line in index.lines() {
                if let Some(rest) = line.strip_prefix("## ") {
                    heading = rest.split(" — ").next().unwrap_or_default().to_owned();
                    continue;
                }
                let mut words = line.split_whitespace();
                let (Some(verb), Some(path), Some(id)) = (words.next(), words.next(), words.next())
                else {
                    continue;
                };
                if !matches!(verb, "GET" | "POST" | "PUT" | "DELETE") {
                    continue;
                }
                let op = &doc["paths"][path][verb.to_ascii_lowercase()];
                assert!(
                    op.is_object(),
                    "the index lists {verb} {path}, which is not published"
                );
                assert_eq!(
                    op["operationId"].as_str(),
                    Some(id),
                    "{verb} {path}'s operationId"
                );
                assert_eq!(
                    op["tags"][0].as_str(),
                    Some(heading.as_str()),
                    "{verb} {path}'s tag"
                );
                assert!(
                    line.contains(op["summary"].as_str().unwrap_or_default()),
                    "{verb} {path}'s index line must carry its summary"
                );
                listed.push((path.to_owned(), verb.to_owned()));
            }
            listed.sort();
            let before = listed.len();
            listed.dedup();
            assert_eq!(before, listed.len(), "the index lists an operation twice");
            assert_eq!(listed, documented_routes(&doc), "{mode:?}");
            assert_eq!(
                listed.iter().any(|(path, _)| path == "/login"),
                mode == AuthMode::Required,
                "/login is listed only when [auth] serves it"
            );
        }
    }

    /// Assert `slice` is a whole document carrying exactly `expected` of
    /// `doc`'s operations, unchanged: every `$ref` resolves inside it, it
    /// carries no schema nothing in it reaches, its top-level `tags` are the
    /// ones those operations use (in the document's order), and the rest of
    /// the envelope is the document's own.
    fn assert_self_contained_slice(
        doc: &Value,
        slice: &Value,
        expected: &[(String, String)],
        what: &str,
    ) {
        fn refs(value: &Value, out: &mut BTreeSet<String>) {
            match value {
                Value::Object(map) => {
                    for (key, child) in map {
                        if key == "$ref" {
                            out.insert(child.as_str().unwrap_or_default().to_owned());
                        }
                        refs(child, out);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        refs(item, out);
                    }
                }
                _ => {}
            }
        }
        let op = |doc: &Value, path: &str, method: &str| -> Value {
            doc["paths"][path][method.to_ascii_lowercase()].clone()
        };
        let routes = documented_routes(slice);
        assert_eq!(routes, expected, "{what} must carry exactly its operations");
        let mut used_tags: Vec<Value> = Vec::new();
        for (path, method) in &routes {
            assert_eq!(
                op(slice, path, method),
                op(doc, path, method),
                "{what}: {method} {path} must be carried unchanged"
            );
            used_tags.push(serde_json::json!({ "name": op(doc, path, method)["tags"][0] }));
        }
        let expected_tags: Vec<Value> = doc["tags"]
            .as_array()
            .expect("top-level tags")
            .iter()
            .filter(|tag| used_tags.contains(tag))
            .cloned()
            .collect();
        assert_eq!(
            slice["tags"],
            Value::Array(expected_tags),
            "{what}'s top-level tags"
        );

        let mut found = BTreeSet::new();
        refs(slice, &mut found);
        for reference in &found {
            let name = reference
                .strip_prefix("#/components/schemas/")
                .unwrap_or_else(|| panic!("{what} has a non-schema $ref {reference}"));
            assert!(
                slice["components"]["schemas"][name].is_object(),
                "{what}'s $ref {reference} does not resolve inside it"
            );
        }
        let carried: BTreeSet<String> = slice["components"]["schemas"]
            .as_object()
            .expect("schemas")
            .keys()
            .map(|name| format!("#/components/schemas/{name}"))
            .collect();
        assert_eq!(
            carried, found,
            "{what} carries a schema nothing in it reaches"
        );
        for key in ["openapi", "info", "servers", "security"] {
            assert_eq!(slice[key], doc[key], "{what}'s {key}");
        }
        assert_eq!(
            slice["components"]["securitySchemes"], doc["components"]["securitySchemes"],
            "{what}'s security schemes"
        );
    }

    /// Every tag's slice is a whole document carrying exactly the full
    /// document's operations with that tag — so none from another tag — and
    /// together the slices cover every operation. A tag no operation carries
    /// has no slice.
    #[test]
    fn a_tag_slice_is_a_self_contained_document_of_that_tag_alone() {
        let doc = doc();
        let tags: Vec<&str> = doc["tags"]
            .as_array()
            .expect("top-level tags")
            .iter()
            .map(|t| t["name"].as_str().expect("a tag name"))
            .collect();
        assert!(
            tags.len() > 20,
            "the document has stopped tagging its operations"
        );
        let mut covered: Vec<(String, String)> = Vec::new();
        for tag in tags {
            let slice = tag_slice(&doc, tag).unwrap_or_else(|| panic!("{tag} has no slice"));
            let expected: Vec<(String, String)> = documented_routes(&doc)
                .into_iter()
                .filter(|(path, method)| {
                    doc["paths"][path][method.to_ascii_lowercase()]["tags"][0] == tag
                })
                .collect();
            assert!(!expected.is_empty());
            assert_self_contained_slice(&doc, &slice, &expected, &format!("the {tag} slice"));
            covered.extend(expected);
        }
        covered.sort();
        assert_eq!(
            covered,
            documented_routes(&doc),
            "the slices must cover every operation"
        );
        assert!(tag_slice(&doc, "Nope").is_none());
    }

    /// Every operation's slice is a whole document carrying that operation
    /// alone, and a much smaller one than the document — the point of it is
    /// that an agent can read one operation's contract whole.
    #[test]
    fn an_operation_slice_is_a_self_contained_document_of_that_operation_alone() {
        let doc = doc();
        let whole = doc.to_string().len();
        for (path, method) in documented_routes(&doc) {
            let id = doc["paths"][&path][method.to_ascii_lowercase()]["operationId"]
                .as_str()
                .expect("an operationId")
                .to_owned();
            let slice = operation_slice(&doc, &id).unwrap_or_else(|| panic!("{id} has no slice"));
            assert_self_contained_slice(
                &doc,
                &slice,
                &[(path.clone(), method.clone())],
                &format!("the {id} slice"),
            );
            assert!(
                slice.to_string().len() < whole / 4,
                "the {id} slice is over a quarter of the whole document"
            );
        }
        assert!(operation_slice(&doc, "nope").is_none());
    }

    /// Serve check for the agent surface: `?tag=` answers that tag's slice, an
    /// unknown tag is a `400` naming the real ones, and `GET /openapi/index`
    /// answers the plain-text index — all through the full application.
    #[tokio::test]
    async fn the_tag_slice_and_the_route_index_are_served() {
        let pool = test_support::test_pool().await;
        let client = ApiClient::full(&pool);
        let slice: Value = client
            .get_json("/openapi.json?tag=Portfolio%20reports")
            .await;
        assert!(slice["paths"]["/portfolio/overview"]["post"].is_object());
        assert!(slice["paths"]["/trades"].is_null());
        assert_eq!(
            slice["tags"],
            serde_json::json!([{ "name": "Portfolio reports" }])
        );

        let unknown = client.get("/openapi.json?tag=Nope").await;
        assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
        assert!(
            unknown.text().contains("no operation is tagged `Nope`"),
            "{}",
            unknown.text()
        );
        assert!(unknown.text().contains("Trades"), "{}", unknown.text());

        let typo = client.get("/openapi.json?tags=Trades").await;
        assert_eq!(typo.status, StatusCode::BAD_REQUEST);

        let one: Value = client.get_json("/openapi.json?operation=createSells").await;
        assert_eq!(
            documented_routes(&one),
            [("/sells".to_owned(), "POST".to_owned())]
        );
        assert!(one["components"]["schemas"]["SellBody"].is_object());
        let unknown = client.get("/openapi.json?operation=nope").await;
        assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
        assert!(unknown.text().contains("`nope`"), "{}", unknown.text());
        let both = client
            .get("/openapi.json?operation=createSells&tag=Sells")
            .await;
        assert_eq!(both.status, StatusCode::BAD_REQUEST);

        let index = client.get("/openapi/index").await;
        assert_eq!(index.status, StatusCode::OK);
        assert_eq!(index.headers["content-type"], "text/plain; charset=utf-8");
        assert!(
            index.text().contains("GET    /trades  listTrades — "),
            "{}",
            index.text()
        );
        assert!(
            index
                .text()
                .contains("## Trades — GET /openapi.json?tag=Trades")
        );
    }

    /// The filtering half of the reading-a-list contract — which entity list
    /// takes which query parameters, and that an unrecognised one is a `400`
    /// whether the list filters or not — rides in `info.description`, the
    /// machine-client surface. `docs/API.md`'s "Reading a list" section is the
    /// long form (`doc_checks` pins that copy); this is the compact twin a
    /// client reading only the generated document gets.
    ///
    /// Which lists and which parameters is **read out of
    /// `entities::route_tables::LIST_ROUTES`**, both ways, rather than transcribed: every list
    /// that takes a filter is named in the paragraph and every list that takes
    /// none is not, and every filter name in the table is spelled `?name=`
    /// there. So a new filtered list, a new filter, or a filter removed from a
    /// list fails here until the paragraph says so — which is the half a pin on
    /// the sentences alone could never catch.
    #[test]
    fn the_list_filtering_contract_is_stated_in_the_description() {
        use crate::entities::route_tables::LIST_ROUTES;
        let doc = doc();
        let description = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        for rule in [
            "an entity list may be narrowed by query filters.",
            "/listings takes ?exchange_mic= and ?security_type=",
            "inclusive ?from=/?to= over their own date column",
            "/interest_income takes ?holding_account_id= and the date range",
            "/drp_enrolments takes ?listing_id= and ?holding_account_id=",
            "/amit_adjustments takes ?amma_statement_id= and ?trade_id=",
            "/parcel_allocations ?sale_trade_id= and ?purchase_trade_id=",
            "The long-standing /closing_prices (?listing_id=, ?from=, ?to=, ?status= — ok or \
             error, omitted is every row: the rows carrying a price, which is one call rather \
             than two, but not quite the series a valuation reads — a row before its listing's \
             unpriced_before is stored ok and superseded) and /attachments (owner ids, \
             include_linked) filters are unchanged.",
            "/rights_sales takes ?listing_id= (the rights issue's listing)",
            "/exchange_holidays and /exchange_holidays/{mic} take the date range over \
             holiday_date.",
            "An unrecognised parameter is a 400 naming it on every list route —",
            "including a list that accepts no filter, which refuses any parameter at all rather \
             than silently ignoring it.",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }

        // The filtering paragraph alone: naming a path anywhere else in the
        // description (a report read, an operation) is not a filter claim.
        let from = description
            .find("an entity list may be narrowed")
            .expect("the filtering paragraph");
        let to = description[from..]
            .find("silently ignoring it.")
            .expect("the paragraph's last sentence");
        let paragraph = &description[from..from + to];

        // A path is "named" only as a whole path: `/listings` must not count as
        // naming `/listings/{id}/renames`, nor the other way about.
        let names = |path: &str| {
            paragraph.match_indices(path).any(|(at, _)| {
                !matches!(paragraph[at + path.len()..].chars().next(),
                    Some(c) if c.is_alphanumeric() || c == '_' || c == '/')
            })
        };
        let mut filtered = 0;
        for route in LIST_ROUTES {
            if route.filters.is_empty() {
                assert!(
                    !names(route.path),
                    "the filtering paragraph names {}, which takes no filter",
                    route.path
                );
                continue;
            }
            assert!(
                names(route.path),
                "{} takes {:?}, so the filtering paragraph must name it",
                route.path,
                route.filters
            );
            filtered += 1;
        }
        assert_eq!(
            filtered, 19,
            "the set of filtered list routes has changed; the paragraph must describe the new one"
        );

        // …and every filter name is spelled out, bar the owner ids the
        // /attachments clause describes collectively (there are six of them, one
        // per owning entity, and `?trade_id=` for an attachment is the same
        // parameter the trades list takes).
        const DESCRIBED_COLLECTIVELY: &[&str] = &[
            "income_id",
            "ess_statement_id",
            "interest_income_id",
            "corporate_action_id",
            "include_linked",
        ];
        for route in LIST_ROUTES {
            for filter in route.filters {
                if DESCRIBED_COLLECTIVELY.contains(filter) {
                    continue;
                }
                assert!(
                    paragraph.contains(&format!("?{filter}=")),
                    "the filtering paragraph must spell `?{filter}=` ({} takes it)",
                    route.path
                );
            }
        }
    }

    /// Every filtered list's summary names the parameters it accepts — the
    /// per-route half of the same contract, and the one a client browsing
    /// `paths` reads. The list and its parameters are **read from**
    /// `route_tables::filtered_list_routes()` (the classification table the
    /// filtering tests drive), not transcribed here, so a filter added to an
    /// entity fails until its summary documents it.
    #[test]
    fn every_filtered_list_summary_names_its_filters() {
        let filtered = route_tables::filtered_list_routes();
        assert!(
            !filtered.is_empty(),
            "the entity filter classification table came back empty"
        );
        for (path, params) in filtered {
            let row = OPERATIONS
                .iter()
                .find(|(verb, p, _, _, _, _)| *verb == Verb::Get && *p == path)
                .unwrap_or_else(|| panic!("no GET route is documented at {path}"));
            let summary = &row.3;
            for param in params {
                assert!(
                    summary.contains(&format!("?{param}=")),
                    "the {path} summary must name ?{param}=: {summary}"
                );
            }
        }
    }

    /// The 2026-09-24 REST-audit item "Make the as-at default explicit" (B8):
    /// every valuation report exposes the same `as_of_date`, the summaries say
    /// that omitting it is today's live position, and the OpenAPI schema
    /// carries it as an **optional** field rather than a required one (a
    /// required date would break every caller that omits it).
    #[test]
    fn the_valuation_reports_expose_as_of_date() {
        // The route summaries name the parameter and the default. The
        // overview/performance/unrealised-gains bodies take it; open-parcels
        // takes it as a query parameter.
        for path in [
            "/portfolio/overview",
            "/portfolio/performance",
            "/portfolio/unrealised_gains",
        ] {
            let row = OPERATIONS
                .iter()
                .find(|(_, p, _, _, _, _)| *p == path)
                .unwrap_or_else(|| panic!("no route is documented at {path}"));
            assert!(
                row.3.contains("as_of_date"),
                "the {path} summary must name as_of_date: {}",
                row.3
            );
            assert!(
                row.3.contains("today's live position"),
                "the {path} summary must state the omitted-date default: {}",
                row.3
            );
        }
        let open_parcels = OPERATIONS
            .iter()
            .find(|(_, p, _, _, _, _)| *p == "/portfolio/open_parcels")
            .expect("the open-parcels route is documented");
        assert!(
            open_parcels.3.contains("?as_of_date="),
            "the open-parcels summary must name ?as_of_date=: {}",
            open_parcels.3
        );

        // The generated document carries the field on the request schemas, and
        // it is optional there.
        let doc = doc();
        for schema in [
            "OverviewRequest",
            "PerformanceRequest",
            "UnrealisedGainsRequest",
        ] {
            let props = &doc["components"]["schemas"][schema]["properties"];
            assert!(
                props.get("as_of_date").is_some(),
                "{schema} must expose as_of_date in the OpenAPI schema"
            );
            let required = doc["components"]["schemas"][schema]["required"].as_array();
            if let Some(required) = required {
                assert!(
                    !required.iter().any(|r| r == "as_of_date"),
                    "{schema}.as_of_date must stay optional"
                );
            }
        }
    }

    /// A read whose **parameter** (not its path) names a row that is not there
    /// answers a `404` with a plain-text reason too —
    /// `GET /portfolio/activity?listing_id=` is the one such read. Driven
    /// beside the GET-one walk below because the generic `404` response
    /// `operation` attaches is keyed off a path parameter, which this route
    /// does not have.
    #[tokio::test]
    async fn a_read_whose_parameter_names_a_missing_row_answers_a_text_404() {
        use crate::test_support::{ApiClient, test_pool};
        use axum::http::StatusCode;

        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let resp = client.get("/portfolio/activity?listing_id=9999").await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
        assert_eq!(
            resp.text(),
            "listing 9999 not found",
            "a read whose parameter names a missing row must say which one"
        );
    }

    /// Every request-body schema's `required` list is exactly the fields the
    /// decoder requires. serde reports a missing field one at a time (it stops
    /// at the first), so the schema is where a client learns the whole list in
    /// one read — `GET /openapi.json?operation=<id>` — and this is what makes
    /// that list trustworthy.
    ///
    /// Empirical, through the real router rather than read off the structs: a
    /// body built from the schema with **every** property set (one item per
    /// array, recursively) must decode — the handler may refuse it on its
    /// merits, but never with `cannot read the request body` — and removing any
    /// one property from it must give `missing field` naming it exactly when the
    /// schema lists it as required, at whatever depth it sits.
    #[tokio::test]
    async fn every_request_body_schemas_required_list_is_what_the_decoder_requires() {
        use crate::test_support::{ApiClient, test_pool};
        use serde_json::{Map, json};

        fn resolve<'a>(schemas: &'a Map<String, Value>, v: &'a Value) -> &'a Value {
            match v.get("$ref").and_then(Value::as_str) {
                Some(r) => resolve(
                    schemas,
                    &schemas[r.trim_start_matches("#/components/schemas/")],
                ),
                None => v,
            }
        }
        /// A decodable value of `schema`, every property present.
        fn sample(schemas: &Map<String, Value>, schema: &Value) -> Value {
            let schema = resolve(schemas, schema);
            if let Some(values) = schema.get("enum").and_then(Value::as_array) {
                return values
                    .iter()
                    .find(|v| !v.is_null())
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            for key in ["oneOf", "anyOf", "allOf"] {
                if let Some(branches) = schema.get(key).and_then(Value::as_array) {
                    let branch = branches
                        .iter()
                        .find(|b| b.get("type").and_then(Value::as_str) != Some("null"))
                        .expect("a non-null branch");
                    return sample(schemas, branch);
                }
            }
            let ty = match &schema["type"] {
                Value::String(t) => t.as_str(),
                Value::Array(ts) => ts
                    .iter()
                    .filter_map(Value::as_str)
                    .find(|t| *t != "null")
                    .expect("a non-null type"),
                other => panic!("schema without a type: {other} in {schema}"),
            };
            match ty {
                "object" => Value::Object(
                    schema["properties"]
                        .as_object()
                        .map(|props| {
                            props
                                .iter()
                                .map(|(k, p)| (k.clone(), sample(schemas, p)))
                                .collect()
                        })
                        .unwrap_or_default(),
                ),
                "array" => json!([sample(schemas, &schema["items"])]),
                "boolean" => json!(false),
                "integer" | "number" => json!(1),
                "string" => match schema.get("format").and_then(Value::as_str) {
                    Some("date") => json!("2024-01-02"),
                    Some("date-time") => json!("2024-01-02T00:00:00Z"),
                    _ => json!("1"),
                },
                other => panic!("unhandled schema type {other}"),
            }
        }
        /// Every property in `value` (built by `sample` from `schema`), as the
        /// JSON-pointer path to it, its name, and whether its object's schema
        /// requires it.
        fn properties(
            schemas: &Map<String, Value>,
            schema: &Value,
            value: &Value,
            at: &str,
            out: &mut Vec<(String, String, bool)>,
        ) {
            let schema = resolve(schemas, schema);
            for key in ["oneOf", "anyOf", "allOf"] {
                if let Some(branches) = schema.get(key).and_then(Value::as_array) {
                    if let Some(b) = branches
                        .iter()
                        .find(|b| b.get("type").and_then(Value::as_str) != Some("null"))
                    {
                        properties(schemas, b, value, at, out);
                    }
                    return;
                }
            }
            match value {
                Value::Object(map) => {
                    let required: Vec<&str> = schema
                        .get("required")
                        .and_then(Value::as_array)
                        .map(|r| r.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    for (k, v) in map {
                        let path = format!("{at}/{k}");
                        out.push((path.clone(), k.clone(), required.contains(&k.as_str())));
                        properties(schemas, &schema["properties"][k], v, &path, out);
                    }
                }
                Value::Array(items) => {
                    for (i, v) in items.iter().enumerate() {
                        properties(schemas, &schema["items"], v, &format!("{at}/{i}"), out);
                    }
                }
                _ => {}
            }
        }
        fn remove(value: &mut Value, path: &str) {
            let (parent, last) = path.rsplit_once('/').expect("a property path");
            value
                .pointer_mut(parent)
                .and_then(Value::as_object_mut)
                .expect("the property's object")
                .remove(last);
        }

        let doc = doc();
        let schemas = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas is an object");
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let mut wrong = Vec::new();
        let mut checked = 0;
        for &(verb, ref path, _, _, ref request, _) in OPERATIONS.iter() {
            let schema = match request {
                Body::Json(name) => json!({ "$ref": format!("#/components/schemas/{name}") }),
                Body::JsonArray(name) => json!({
                    "type": "array",
                    "items": { "$ref": format!("#/components/schemas/{name}") },
                }),
                _ => continue,
            };
            let mut uri = path.to_string();
            for (name, value) in [
                ("{mic}", "XASX"),
                ("{date}", "2024-01-02"),
                ("{price_date}", "2024-01-02"),
                ("{tax_year}", "2024"),
                ("{listing_id}", "1"),
                ("{id}", "1"),
            ] {
                uri = uri.replace(name, value);
            }
            assert!(!uri.contains('{'), "unsubstituted parameter in {uri}");
            // Only whether the body *decodes* is asked: a sample the handler
            // goes on to accept (and store) disturbs nothing that question
            // reads.
            let send = |body: Value| {
                let client = &client;
                let uri = uri.clone();
                async move {
                    let resp = match verb {
                        Verb::Put => client.put(&uri, &body).await,
                        Verb::Post => client.post(&uri, &body).await,
                        other => panic!("a request body on a {other:?}"),
                    };
                    resp.text().to_owned()
                }
            };
            let full = sample(schemas, &schema);
            let reply = send(full.clone()).await;
            if reply.starts_with("cannot read the request body") {
                wrong.push(format!("{verb:?} {path}: every property set → {reply}"));
                continue;
            }
            let mut props = Vec::new();
            properties(schemas, &schema, &full, "", &mut props);
            for (pointer, name, required) in props {
                let mut body = full.clone();
                remove(&mut body, &pointer);
                let reply = send(body).await;
                let missing = reply.starts_with("cannot read the request body")
                    && reply.contains(&format!("missing field `{name}`"));
                if missing != required {
                    wrong.push(format!(
                        "{verb:?} {path} {pointer}: schema says required={required}, \
                         without it → {reply:?}"
                    ));
                }
                checked += 1;
            }
        }
        assert!(
            wrong.is_empty(),
            "request-body `required` lists disagree with the decoder:\n{}",
            wrong.join("\n")
        );
        assert!(checked > 300, "only {checked} properties checked");
    }

    /// Every `GET`-one answers a missing key with a `404` whose plain-text body
    /// names what was missing — so a client can tell an absent row from a
    /// mistyped path, whose `404` (no route matched) is the only empty one.
    /// Driven from the document, so a new GET-one is covered without a
    /// hand-kept list.
    #[tokio::test]
    async fn every_get_one_route_names_the_missing_row_in_its_404() {
        use crate::test_support::{ApiClient, test_pool};
        use axum::http::StatusCode;

        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
        let unrouted = client.get("/no-such-collection/9999").await;
        assert_eq!(unrouted.status, StatusCode::NOT_FOUND);
        assert_eq!(unrouted.text(), "", "a path no route matches stays empty");
        // The one path-addressed GET whose success body is not JSON, so the
        // table walk below does not reach it.
        let content = client.get("/attachments/9999/content").await;
        assert_eq!(content.status, StatusCode::NOT_FOUND);
        assert_eq!(content.text(), "no attachment with that id");
        let mut checked = 0;
        for template in documented_get_one_routes() {
            let mut uri = template.clone();
            for (name, value) in [
                ("{id}", "9999"),
                ("{tax_year}", "9999"),
                ("{listing_id}", "9999"),
                ("{rename_id}", "9999"),
                ("{code}", "ZZZZ"),
                ("{mic}", "ZZZZ"),
                ("{date}", "2020-01-01"),
                ("{price_date}", "2020-01-01"),
                ("{report}", "overview"),
            ] {
                uri = uri.replace(name, value);
            }
            assert!(!uri.contains('{'), "unsubstituted parameter in {uri}");
            let resp = client.get(&uri).await;
            assert_eq!(resp.status, StatusCode::NOT_FOUND, "GET {uri}");
            assert_eq!(
                resp.headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok()),
                Some("text/plain; charset=utf-8"),
                "GET {uri}"
            );
            assert!(
                resp.text().starts_with("no "),
                "a GET-one's 404 must name the missing row: GET {uri} → {:?}",
                resp.text()
            );
            checked += 1;
        }
        assert!(
            checked >= 20,
            "the walk found only {checked} GET-one routes — it has stopped reading the table"
        );
    }
}
