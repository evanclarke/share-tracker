//! The machine-readable API description: `GET /openapi.json`, an OpenAPI 3.1
//! document assembled from the *real* request/response types (each derives
//! [`utoipa::ToSchema`] — no mirror types) and the route table below, which
//! names every route `app::router` serves.
//!
//! Why it exists: the whole contract used to be the prose of `docs/API.md`
//! (1,936 lines), with the critical global rules — money/quantity as JSON
//! strings, `deny_unknown_fields` on every body — stated only in prose at the
//! end. A machine client (an LLM, a script) had nothing to read but English.
//! This module turns the route table and the serde structs into a document a
//! client can consume, and the tests at the bottom pin it so it cannot drift:
//! a route added to the sources without a table row fails
//! `every_served_route_is_documented_and_nothing_else_is`, and a body that
//! stops denying unknown fields or a money field that starts rendering as a
//! number fails `every_request_body_schema_denies_unknown_fields` /
//! `a_money_field_is_a_json_string`.
//!
//! The path spellings are the axum ones (`{id}`), relative to the deployment's
//! `base_path` — the same strings `docs/API.md` uses. `/static/*.js` is the one
//! class of route a source scan cannot see (the paths are built in code from
//! `web::JS_MODULES`), so [`document`] reads them from that same list and the
//! coverage test classifies them explicitly.
//!
//! Behind `[auth]`: the router below is merged into `app::router` like every
//! other one, so `require_auth` gates it and `/openapi.json` is deliberately
//! *not* on that layer's login-page allowlist.

use axum::{Router, response::IntoResponse, routing::get};

use crate::infra::extract::Query;
use crate::infra::http::ApiError;
use sqlx::SqlitePool;
use utoipa::openapi::{
    Components, ComponentsBuilder, InfoBuilder, OpenApi, OpenApiBuilder, Paths, RefOr, Required,
    content::ContentBuilder,
    path::{HttpMethod, Operation, OperationBuilder, Parameter, ParameterBuilder, ParameterIn},
    request_body::{RequestBody, RequestBodyBuilder},
    response::{Response, ResponseBuilder, ResponsesBuilder},
    schema::{AdditionalProperties, ArrayBuilder, ObjectBuilder, Ref, Schema, Type},
    security::{
        ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityRequirement, SecurityScheme,
    },
    tag::{Tag, TagBuilder},
};
use utoipa::{PartialSchema, ToSchema};

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
404 on a delete, an operation, or a read whose parameter \
names a missing row (GET /portfolio/activity?listing_id=), 413, 415 (a JSON body sent \
without Content-Type: application/json), 422, 429 on POST /login once a source \
has exhausted its failed-attempt budget (the body carries the reason and a \
Retry-After header the remaining whole seconds; a browser login POST is refused \
429 too, with the sign-in page as the body instead of the plain-text reason), a \
failed POST /jobs/{name}'s 500, 502, 503 — or a deliberately empty body: the 404 \
of a GET addressed at one missing row, a 405, and an internal 500. docs/API.md's \
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

The document itself is generated from the route table and the serde structs in \
src/api_spec.rs and is pinned by that module's tests. It can be read in pieces: \
GET /openapi/index is a compact route index (verb, path, operationId and summary \
per line), and GET /openapi.json?operation=<operationId> or ?tag=<tag> serves one \
operation's or one tag's slice, each a whole document carrying only the schemas it \
reaches.";

/// The four HTTP verbs the server uses, as the table spells them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verb {
    Get,
    Post,
    Put,
    Delete,
}

impl Verb {
    fn http(self) -> HttpMethod {
        match self {
            Verb::Get => HttpMethod::Get,
            Verb::Post => HttpMethod::Post,
            Verb::Put => HttpMethod::Put,
            Verb::Delete => HttpMethod::Delete,
        }
    }
}

/// A request or response payload, as the table records it.
#[derive(Clone, Copy)]
enum Body {
    /// No payload at all.
    None,
    /// An `application/json` body/response that is a `$ref` to the named
    /// component schema.
    Json(&'static str),
    /// An `application/json` array whose items are a `$ref` to the named
    /// component schema.
    JsonArray(&'static str),
    /// An `application/json` array of integers (`Vec<i32>`).
    JsonIntegers,
    /// A free-form `application/json` value no single struct models; the
    /// string is the description of what it is.
    JsonFree(&'static str),
    /// An `application/x-www-form-urlencoded` body that is a `$ref` to the
    /// named component schema.
    Form(&'static str),
    /// A non-JSON payload, by media type (CSV, HTML, JavaScript, an uploaded
    /// file). Recorded as a media type with no schema: the bytes are not
    /// JSON-Schema-shaped.
    Other(&'static str),
}

/// One row of the route table, in the order the builder reads it:
/// `(verb, axum path, success statuses, summary, request body, success response)`.
///
/// Every row is a real route: the path is the literal registered in the module
/// the summary describes, the success statuses are the ones the handler can
/// answer, and the schemas are the handler's actual extractor and return types.
/// A route whose request is not a JSON struct says so in its summary and records
/// the media type it does take.
///
/// Most routes record one status. A `PUT` upsert records two — `&[201, 204]` —
/// because it reports its outcome: `201 Created` carrying `response` (the
/// created row) or `204 No Content`. The two exceptions
/// (`/transfers/{id}` is create-only and always `201` with the executed group;
/// `/rba_fx_rates/{id}` only ever corrects an existing row, so always `204`)
/// record their single status, and `every_put_route_documents_its_outcome`
/// pins that classification.
type RouteRow = (Verb, &'static str, &'static [u16], &'static str, Body, Body);

/// Every route `app::router` serves, plus `GET /openapi.json` itself.
///
/// Grouped entity routes first (the CRUD shape), then the operation routes,
/// the scheduler and auth routes, the web frontend, and the reports. Compare
/// against the sources: `every_served_route_is_documented_and_nothing_else_is`
/// walks `src/**/*.rs` for route registrations and fails either way.
const ROUTES: &[RouteRow] = &[
    // ---- Entities: the CRUD shape -----------------------------------------
    // GET list (200, array), POST create (201, created row), GET one (200),
    // PUT upsert (201 with the created row on create, 204 on replace — the
    // two deliberate exceptions are `/transfers/{id}` and `/rba_fx_rates/{id}`),
    // DELETE (204).
    (
        Verb::Get,
        "/listings",
        &[200],
        "List every listing, optionally narrowed by ?exchange_mic= or ?security_type=.",
        Body::None,
        Body::JsonArray("Listing"),
    ),
    (
        Verb::Post,
        "/listings",
        &[201],
        "Create a listing; the database assigns the id and the created row is returned.",
        Body::Json("ListingBody"),
        Body::Json("Listing"),
    ),
    (
        Verb::Get,
        "/listings/{id}",
        &[200],
        "Fetch one listing, or 404.",
        Body::None,
        Body::Json("Listing"),
    ),
    (
        Verb::Put,
        "/listings/{id}",
        &[201, 204],
        "Create or replace the listing at this id.",
        Body::Json("ListingBody"),
        Body::Json("Listing"),
    ),
    (
        Verb::Delete,
        "/listings/{id}",
        &[204],
        "Delete the listing; 422 while a trade or other row still references it.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/trades",
        &[200],
        "List every trade (Buys, DRPs, Sells), ascending date then id, optionally narrowed by ?listing_id=, ?holding_account_id= and an inclusive ?from=/?to= over the trade date.",
        Body::None,
        Body::JsonArray("Trade"),
    ),
    (
        Verb::Post,
        "/trades",
        &[201],
        "Create a trade; the database assigns the id and the created row is returned.",
        Body::Json("TradeBody"),
        Body::Json("Trade"),
    ),
    (
        Verb::Get,
        "/trades/{id}",
        &[200],
        "Fetch one trade, or 404.",
        Body::None,
        Body::Json("Trade"),
    ),
    (
        Verb::Put,
        "/trades/{id}",
        &[201, 204],
        "Create or replace the trade at this id (the Buy/DRP parcel path).",
        Body::Json("TradeBody"),
        Body::Json("Trade"),
    ),
    (
        Verb::Delete,
        "/trades/{id}",
        &[204],
        "Delete the trade; 422 while a sale allocation or a derived row still depends on it.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Post,
        "/sells",
        &[200, 201],
        "Create a Sell with its parcel allocations; the created row is returned as GET /trades/{id} would present it (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::Json("SellBody"),
        Body::Json("Trade"),
    ),
    (
        Verb::Put,
        "/sells/{id}",
        &[201, 204],
        "Create or replace the Sell at this id.",
        Body::Json("SellBody"),
        Body::Json("Trade"),
    ),
    (
        Verb::Delete,
        "/sells/{id}",
        &[204],
        "Delete the Sell; 422 if it is not a Sell, or a replacement parcel it created is consumed later.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/income",
        &[200],
        "List every income (distribution) row, optionally narrowed by ?listing_id=, ?holding_account_id= and an inclusive ?from=/?to= over date_paid.",
        Body::None,
        Body::JsonArray("Income"),
    ),
    (
        Verb::Post,
        "/income",
        &[201],
        "Create an income row; the created row is returned.",
        Body::Json("IncomeBody"),
        Body::Json("Income"),
    ),
    (
        Verb::Get,
        "/income/{id}",
        &[200],
        "Fetch one income row, or 404.",
        Body::None,
        Body::Json("Income"),
    ),
    (
        Verb::Put,
        "/income/{id}",
        &[201, 204],
        "Create or replace the income row at this id.",
        Body::Json("IncomeBody"),
        Body::Json("Income"),
    ),
    (
        Verb::Delete,
        "/income/{id}",
        &[204],
        "Delete the income row.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/interest_income",
        &[200],
        "List every interest-income row, optionally narrowed by ?holding_account_id= and an inclusive \
         ?from=/?to= over date_paid. An owner filter matches recorded values only: \
         holding_account_id is nullable, and a row without one is in no ?holding_account_id= \
         slice — so summing the slices is less than the unfiltered list.",
        Body::None,
        Body::JsonArray("InterestIncome"),
    ),
    (
        Verb::Post,
        "/interest_income",
        &[201],
        "Create an interest-income row; the created row is returned.",
        Body::Json("InterestIncomeBody"),
        Body::Json("InterestIncome"),
    ),
    (
        Verb::Get,
        "/interest_income/{id}",
        &[200],
        "Fetch one interest-income row, or 404.",
        Body::None,
        Body::Json("InterestIncome"),
    ),
    (
        Verb::Put,
        "/interest_income/{id}",
        &[201, 204],
        "Create or replace the interest-income row at this id.",
        Body::Json("InterestIncomeBody"),
        Body::Json("InterestIncome"),
    ),
    (
        Verb::Delete,
        "/interest_income/{id}",
        &[204],
        "Delete the interest-income row.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/investment_expenses",
        &[200],
        "List every investment-expense row, optionally narrowed by ?listing_id=, \
         ?holding_account_id= and an inclusive ?from=/?to= over date_incurred. An owner filter \
         matches recorded values only: both owner columns are nullable for a portfolio-wide \
         expense (an adviser's whole-of-portfolio fee), and such a row is in no ?listing_id= or \
         ?holding_account_id= slice — so summing the slices is less than the unfiltered list.",
        Body::None,
        Body::JsonArray("InvestmentExpense"),
    ),
    (
        Verb::Post,
        "/investment_expenses",
        &[201],
        "Create an investment-expense row; the created row is returned.",
        Body::Json("InvestmentExpenseBody"),
        Body::Json("InvestmentExpense"),
    ),
    (
        Verb::Get,
        "/investment_expenses/{id}",
        &[200],
        "Fetch one investment-expense row, or 404.",
        Body::None,
        Body::Json("InvestmentExpense"),
    ),
    (
        Verb::Put,
        "/investment_expenses/{id}",
        &[201, 204],
        "Create or replace the investment-expense row at this id.",
        Body::Json("InvestmentExpenseBody"),
        Body::Json("InvestmentExpense"),
    ),
    (
        Verb::Delete,
        "/investment_expenses/{id}",
        &[204],
        "Delete the investment-expense row.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/amma_statements",
        &[200],
        "List every AMMA statement, optionally narrowed by ?listing_id=, ?holding_account_id= and an inclusive ?from=/?to= over tax_year_end_date.",
        Body::None,
        Body::JsonArray("AmmaStatement"),
    ),
    (
        Verb::Post,
        "/amma_statements",
        &[201],
        "Create an AMMA statement; the created row is returned.",
        Body::Json("AmmaStatementBody"),
        Body::Json("AmmaStatement"),
    ),
    (
        Verb::Get,
        "/amma_statements/{id}",
        &[200],
        "Fetch one AMMA statement, or 404.",
        Body::None,
        Body::Json("AmmaStatement"),
    ),
    (
        Verb::Put,
        "/amma_statements/{id}",
        &[201, 204],
        "Create or replace the AMMA statement at this id.",
        Body::Json("AmmaStatementBody"),
        Body::Json("AmmaStatement"),
    ),
    (
        Verb::Delete,
        "/amma_statements/{id}",
        &[204],
        "Delete the AMMA statement.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/amit_adjustments",
        &[200],
        "List every AMIT cost-base adjustment, optionally narrowed by ?amma_statement_id= and ?trade_id=.",
        Body::None,
        Body::JsonArray("AmitAdjustment"),
    ),
    (
        Verb::Post,
        "/amit_adjustments",
        &[201],
        "Create an AMIT adjustment; the created row is returned.",
        Body::Json("AmitAdjustmentBody"),
        Body::Json("AmitAdjustment"),
    ),
    (
        Verb::Get,
        "/amit_adjustments/{id}",
        &[200],
        "Fetch one AMIT adjustment, or 404.",
        Body::None,
        Body::Json("AmitAdjustment"),
    ),
    (
        Verb::Put,
        "/amit_adjustments/{id}",
        &[201, 204],
        "Create or replace the AMIT adjustment at this id.",
        Body::Json("AmitAdjustmentBody"),
        Body::Json("AmitAdjustment"),
    ),
    (
        Verb::Delete,
        "/amit_adjustments/{id}",
        &[204],
        "Delete the AMIT adjustment.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/holding_accounts",
        &[200],
        "List every holding account.",
        Body::None,
        Body::JsonArray("HoldingAccount"),
    ),
    (
        Verb::Post,
        "/holding_accounts",
        &[201],
        "Create a holding account; the created row is returned.",
        Body::Json("HoldingAccountBody"),
        Body::Json("HoldingAccount"),
    ),
    (
        Verb::Get,
        "/holding_accounts/{id}",
        &[200],
        "Fetch one holding account, or 404.",
        Body::None,
        Body::Json("HoldingAccount"),
    ),
    (
        Verb::Put,
        "/holding_accounts/{id}",
        &[201, 204],
        "Create or replace the holding account at this id.",
        Body::Json("HoldingAccountBody"),
        Body::Json("HoldingAccount"),
    ),
    (
        Verb::Delete,
        "/holding_accounts/{id}",
        &[204],
        "Delete the holding account; 422 while a trade or statement still references it.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/inheritances",
        &[200],
        "List every inheritance record, optionally narrowed by ?listing_id=, ?holding_account_id= and an inclusive ?from=/?to= over date_of_death.",
        Body::None,
        Body::JsonArray("Inheritance"),
    ),
    (
        Verb::Post,
        "/inheritances",
        &[201],
        "Create an inheritance record; the created row is returned.",
        Body::Json("InheritanceBody"),
        Body::Json("Inheritance"),
    ),
    (
        Verb::Get,
        "/inheritances/{id}",
        &[200],
        "Fetch one inheritance record, or 404.",
        Body::None,
        Body::Json("Inheritance"),
    ),
    (
        Verb::Put,
        "/inheritances/{id}",
        &[201, 204],
        "Create or replace the inheritance record at this id.",
        Body::Json("InheritanceBody"),
        Body::Json("Inheritance"),
    ),
    (
        Verb::Delete,
        "/inheritances/{id}",
        &[204],
        "Delete the inheritance record.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/drp_enrolments",
        &[200],
        "List every DRP enrolment period, optionally narrowed by ?listing_id= and ?holding_account_id=.",
        Body::None,
        Body::JsonArray("DrpEnrolment"),
    ),
    (
        Verb::Post,
        "/drp_enrolments",
        &[201],
        "Create a DRP enrolment period; the created row is returned.",
        Body::Json("DrpEnrolmentBody"),
        Body::Json("DrpEnrolment"),
    ),
    (
        Verb::Get,
        "/drp_enrolments/{id}",
        &[200],
        "Fetch one DRP enrolment period, or 404.",
        Body::None,
        Body::Json("DrpEnrolment"),
    ),
    (
        Verb::Put,
        "/drp_enrolments/{id}",
        &[201, 204],
        "Create or replace the DRP enrolment period at this id.",
        Body::Json("DrpEnrolmentBody"),
        Body::Json("DrpEnrolment"),
    ),
    (
        Verb::Delete,
        "/drp_enrolments/{id}",
        &[204],
        "Delete the DRP enrolment period.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/ess_statements",
        &[200],
        "List every ESS statement, optionally narrowed by ?listing_id=, ?holding_account_id= and an inclusive ?from=/?to= over taxing_point_date.",
        Body::None,
        Body::JsonArray("EssStatement"),
    ),
    (
        Verb::Post,
        "/ess_statements",
        &[201],
        "Create an ESS statement; the created row is returned.",
        Body::Json("EssStatementBody"),
        Body::Json("EssStatement"),
    ),
    (
        Verb::Get,
        "/ess_statements/{id}",
        &[200],
        "Fetch one ESS statement, or 404.",
        Body::None,
        Body::Json("EssStatement"),
    ),
    (
        Verb::Put,
        "/ess_statements/{id}",
        &[201, 204],
        "Create or replace the ESS statement at this id.",
        Body::Json("EssStatementBody"),
        Body::Json("EssStatement"),
    ),
    (
        Verb::Delete,
        "/ess_statements/{id}",
        &[204],
        "Delete the ESS statement; 422 while its vested Buy is still there.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/transfers",
        &[200],
        "List every holding-account transfer, optionally narrowed by ?listing_id= and an inclusive ?from=/?to= over the transfer date.",
        Body::None,
        Body::JsonArray("Transfer"),
    ),
    (
        Verb::Post,
        "/transfers",
        &[200, 201],
        "Execute a transfer: the Sell and transfer-in Buys are written atomically and the whole group is returned (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::Json("TransferBody"),
        Body::Json("TransferGroup"),
    ),
    (
        Verb::Get,
        "/transfers/{id}",
        &[200],
        "Fetch one transfer, or 404.",
        Body::None,
        Body::Json("Transfer"),
    ),
    (
        Verb::Put,
        "/transfers/{id}",
        &[201],
        "Create the transfer at this id; the executed group is returned. The one PUT that is always 201: a transfer is create-only (a re-PUT is 422), and the response must carry the created Sell/Buy ids a bare 204 would hide.",
        Body::Json("TransferBody"),
        Body::Json("TransferGroup"),
    ),
    (
        Verb::Delete,
        "/transfers/{id}",
        &[204],
        "Delete the transfer and its derived trades; 422 while a transferred-in parcel is consumed later.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/corporate_actions",
        &[200],
        "List every corporate action, optionally narrowed by ?listing_id= and an inclusive ?from=/?to= over the action date.",
        Body::None,
        Body::JsonArray("CorporateAction"),
    ),
    (
        Verb::Post,
        "/corporate_actions",
        &[201],
        "Create a corporate action; the created row is returned.",
        Body::Json("CorporateActionBody"),
        Body::Json("CorporateAction"),
    ),
    (
        Verb::Get,
        "/corporate_actions/{id}",
        &[200],
        "Fetch one corporate action, or 404.",
        Body::None,
        Body::Json("CorporateAction"),
    ),
    (
        Verb::Put,
        "/corporate_actions/{id}",
        &[201, 204],
        "Create or replace the corporate action at this id.",
        Body::Json("CorporateActionBody"),
        Body::Json("CorporateAction"),
    ),
    (
        Verb::Delete,
        "/corporate_actions/{id}",
        &[204],
        "Delete the corporate action; 422 while a trade references it.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/rights_sales",
        &[200],
        "List rights sales, optionally narrowed by ?listing_id= (the rights issue's listing) / \
         ?holding_account_id= / ?rights_action_id= / ?from= / ?to= (inclusive, over the sale date).",
        Body::None,
        Body::JsonArray("RightsSale"),
    ),
    (
        Verb::Get,
        "/rights_sales/{id}",
        &[200],
        "Fetch one rights sale, or 404.",
        Body::None,
        Body::Json("RightsSale"),
    ),
    (
        Verb::Delete,
        "/rights_sales/{id}",
        &[204],
        "Delete the rights sale.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/exchange_holidays",
        &[200],
        "List every exchange's holidays, optionally narrowed by ?from= / ?to= (inclusive, over \
         holiday_date).",
        Body::None,
        Body::JsonArray("ExchangeHoliday"),
    ),
    (
        Verb::Get,
        "/exchange_holidays/{mic}",
        &[200],
        "List one exchange's holiday calendar, optionally narrowed by ?from= / ?to= (inclusive, \
         over holiday_date).",
        Body::None,
        Body::JsonArray("ExchangeHoliday"),
    ),
    (
        Verb::Get,
        "/exchange_holidays/{mic}/{date}",
        &[200],
        "Fetch one holiday, or 404.",
        Body::None,
        Body::Json("ExchangeHoliday"),
    ),
    (
        Verb::Put,
        "/exchange_holidays/{mic}/{date}",
        &[201, 204],
        "Create or replace the holiday at this exchange and date.",
        Body::Json("ExchangeHolidayBody"),
        Body::Json("ExchangeHoliday"),
    ),
    (
        Verb::Delete,
        "/exchange_holidays/{mic}/{date}",
        &[204],
        "Delete the holiday.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/exchanges",
        &[200],
        "List every curated exchange.",
        Body::None,
        Body::JsonArray("Exchange"),
    ),
    (
        Verb::Get,
        "/exchanges/{mic}",
        &[200],
        "Fetch one exchange, or 404.",
        Body::None,
        Body::Json("Exchange"),
    ),
    (
        Verb::Put,
        "/exchanges/{mic}",
        &[201, 204],
        "Create or replace the exchange at this MIC.",
        Body::Json("ExchangeBody"),
        Body::Json("Exchange"),
    ),
    (
        Verb::Delete,
        "/exchanges/{mic}",
        &[204],
        "Delete the exchange.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/currencies",
        &[200],
        "List every currency.",
        Body::None,
        Body::JsonArray("Currency"),
    ),
    (
        Verb::Get,
        "/currencies/{code}",
        &[200],
        "Fetch one currency by ISO 4217 code, or 404.",
        Body::None,
        Body::Json("Currency"),
    ),
    (
        Verb::Post,
        "/currencies/import",
        &[200],
        "Import the currency reference data; the body is a bare (text) ISO 4217 XML or ISO 24165 JSON document, and an empty body fetches the live sources.",
        Body::Other("text/plain"),
        Body::Json("CurrencyImportSummary"),
    ),
    (
        Verb::Get,
        "/mic_registry",
        &[200],
        "List every exchange MIC registry entry.",
        Body::None,
        Body::JsonArray("MicEntry"),
    ),
    (
        Verb::Get,
        "/mic_registry/{mic}",
        &[200],
        "Fetch one MIC registry entry, or 404.",
        Body::None,
        Body::Json("MicEntry"),
    ),
    (
        Verb::Post,
        "/mic_registry/import",
        &[200],
        "Import the ISO 10383 MIC registry; the body is a bare (text) CSV document, and an empty body fetches the live source.",
        Body::Other("text/plain"),
        Body::Json("MicImportSummary"),
    ),
    (
        Verb::Get,
        "/rba_fx_rates",
        &[200],
        "List every stored ATO/RBA FX rate.",
        Body::None,
        Body::JsonArray("RbaFxRate"),
    ),
    (
        Verb::Get,
        "/rba_fx_rates/{id}",
        &[200],
        "Fetch one FX rate by id, or 404.",
        Body::None,
        Body::Json("RbaFxRate"),
    ),
    (
        Verb::Put,
        "/rba_fx_rates/{id}",
        &[204],
        "Correct the stored rate at this id (the only field a correction may change). Always 204 when the id exists — this route can never create a row, so it never answers 201 (new rates arrive through POST /rba_fx_rates/import) — and 404 when it does not.",
        Body::Json("CorrectionBody"),
        Body::None,
    ),
    (
        Verb::Post,
        "/rba_fx_rates/import",
        &[200],
        "Import the RBA F11 rates; the body is a bare (text) CSV document, and an empty body fetches the live source.",
        Body::Other("text/plain"),
        Body::Json("RbaImportOutcome"),
    ),
    (
        Verb::Get,
        "/closing_prices",
        &[200],
        "List stored closing prices, optionally narrowed by ?listing_id= / ?from= / ?to= / ?status= \
         (ok or error; omitted is every row). ?status=ok is the rows that carry a price, which is \
         not the same as the rows a valuation reads: a row before its listing's unpriced_before is \
         stored ok and superseded, and no valuation uses it.",
        Body::None,
        Body::JsonArray("ClosingPrice"),
    ),
    (
        Verb::Post,
        "/closing_prices/fetch",
        &[200, 201],
        "Fetch and store one day's close from the price provider; the stored row is returned.",
        Body::Json("FetchBody"),
        Body::Json("ClosingPrice"),
    ),
    (
        Verb::Post,
        "/closing_prices/backfill",
        &[200],
        "Backfill a listing's price history over a date range and report what was stored.",
        Body::Json("BackfillBody"),
        Body::Json("BackfillSummary"),
    ),
    (
        Verb::Post,
        "/closing_prices/clear_unpriced_before",
        &[200],
        "Clear the span of a listing's stored prices that its own unpriced_before marker declares superseded, and report how many rows were removed.",
        Body::Json("ClearBody"),
        Body::Json("ClearSummary"),
    ),
    (
        Verb::Get,
        "/closing_prices/{listing_id}/{price_date}",
        &[200],
        "Fetch the stored price row for one (listing, day), errored or ok, or 404.",
        Body::None,
        Body::Json("ClosingPrice"),
    ),
    (
        Verb::Put,
        "/closing_prices/{listing_id}/{price_date}",
        &[201, 204],
        "Store a hand-entered price for one (listing, day) with its provenance: 201 with the stored row when no price was there, 204 when one was replaced.",
        Body::Json("ManualPriceBody"),
        Body::Json("ClosingPrice"),
    ),
    (
        Verb::Delete,
        "/closing_prices/{listing_id}/{price_date}",
        &[204],
        "Delete a stored price row that is errored or superseded; 422 for an ok row.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/cgt_settings",
        &[200],
        "List the CGT settings rows.",
        Body::None,
        Body::JsonArray("CgtSettings"),
    ),
    (
        Verb::Get,
        "/cgt_settings/{id}",
        &[200],
        "Fetch one CGT settings row, or 404.",
        Body::None,
        Body::Json("CgtSettings"),
    ),
    (
        Verb::Put,
        "/cgt_settings/{id}",
        &[201, 204],
        "Create or replace the CGT settings row at this id.",
        Body::Json("CgtSettingsBody"),
        Body::Json("CgtSettings"),
    ),
    (
        Verb::Delete,
        "/cgt_settings/{id}",
        &[204],
        "Delete the CGT settings row.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/tax_year_settings",
        &[200],
        "List the per-tax-year eligibility settings.",
        Body::None,
        Body::JsonArray("TaxYearSettings"),
    ),
    (
        Verb::Get,
        "/tax_year_settings/{tax_year}",
        &[200],
        "Fetch one tax year's settings, or 404.",
        Body::None,
        Body::Json("TaxYearSettings"),
    ),
    (
        Verb::Put,
        "/tax_year_settings/{tax_year}",
        &[201, 204],
        "Create or replace the settings for this tax year.",
        Body::Json("TaxYearSettingsBody"),
        Body::Json("TaxYearSettings"),
    ),
    (
        Verb::Delete,
        "/tax_year_settings/{tax_year}",
        &[204],
        "Delete the settings for this tax year.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/parcel_allocations",
        &[200],
        "List every sale's parcel allocations, optionally narrowed by ?sale_trade_id= and ?purchase_trade_id=.",
        Body::None,
        Body::JsonArray("ParcelAllocation"),
    ),
    (
        Verb::Get,
        "/parcel_allocations/{id}",
        &[200],
        "Fetch one parcel allocation, or 404.",
        Body::None,
        Body::Json("ParcelAllocation"),
    ),
    (
        Verb::Get,
        "/attachments",
        &[200],
        "List stored attachments, optionally filtered by owner (?trade_id=, ?income_id=, \
         ?amma_statement_id=, ?ess_statement_id=, ?interest_income_id=, ?corporate_action_id=) \
         or request an owner's linked documents (?include_linked=true, with ?trade_id=).",
        Body::None,
        Body::JsonArray("Attachment"),
    ),
    (
        Verb::Post,
        "/attachments",
        &[201],
        "Upload an attachment; multipart/form-data carrying the file and exactly one owner field. The stored row is returned.",
        Body::Other("multipart/form-data"),
        Body::Json("Attachment"),
    ),
    (
        Verb::Get,
        "/attachments/{id}",
        &[200],
        "Fetch one attachment's metadata, or 404.",
        Body::None,
        Body::Json("Attachment"),
    ),
    (
        Verb::Delete,
        "/attachments/{id}",
        &[204],
        "Delete the attachment and its stored bytes.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Get,
        "/attachments/{id}/content",
        &[200],
        "Download an attachment's bytes under the content type it was stored with; ?disposition=inline lets the browser render it in place.",
        Body::None,
        Body::Other("application/octet-stream"),
    ),
    (
        Verb::Get,
        "/distribution_events",
        &[200],
        "List cached distribution events, optionally narrowed by ?listing_id= and an inclusive ?from=/?to= over ex_date.",
        Body::None,
        Body::JsonArray("DistributionEvent"),
    ),
    (
        Verb::Get,
        "/distribution_events/{id}",
        &[200],
        "Fetch one cached distribution event, or 404.",
        Body::None,
        Body::Json("DistributionEvent"),
    ),
    // ---- Operation routes --------------------------------------------------
    (
        Verb::Post,
        "/amma_statements/{id}/generate_adjustments",
        &[200, 201],
        "Generate the AMIT adjustments an AMMA statement implies (201; a preview run answers 200 with the same body and writes nothing).",
        Body::Json("AmitGenerateBody"),
        Body::Json("GeneratedAdjustments"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/participate",
        &[200, 201],
        "Participate in a buy-back: the closing Sell and its dividend-component income row are written together and returned (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::Json("ParticipationBody"),
        Body::Json("Participation"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/demerge",
        &[200, 201],
        "Apportion the head listing's open parcels between head and demerged listing, and return the created replacement parcels (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::None,
        Body::Json("Demerge"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/exchange",
        &[200, 201],
        "Substitute every open parcel of a scrip-for-scrip action's original listing, and return the created replacement parcels (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::None,
        Body::Json("ScripExchange"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/exercise",
        &[200, 201],
        "Exercise a rights issue into a new Buy carrying the rights cost, and return the created trade (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::Json("ExerciseBody"),
        Body::Json("Trade"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/sell_rights",
        &[200, 201],
        "Sell the rights of a renounceable issue and return the created rights sale (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::Json("SellRightsBody"),
        Body::Json("RightsSale"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/recognise",
        &[200, 201],
        "Recognise a worthless-shares loss by closing every open parcel at nil, and return the closing Sell (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::None,
        Body::Json("Recognise"),
    ),
    (
        Verb::Post,
        "/income/{id}/reinvest",
        &[200, 201],
        "Create the DRP trade for a distribution and link it; the created trade is returned (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::Json("ReinvestBody"),
        Body::Json("Trade"),
    ),
    (
        Verb::Delete,
        "/income/{id}/reinvest",
        &[204],
        "Undo the DRP reinvestment; 422 if a later trade drew on it.",
        Body::None,
        Body::None,
    ),
    (
        Verb::Post,
        "/ess_statements/{id}/vest",
        &[200, 201],
        "Vest an ESS statement into the cost-base-reset Buy it implies, and return the created trade (?dry_run=true previews it: 200 with the same body, nothing stored).",
        Body::None,
        Body::Json("Trade"),
    ),
    (
        Verb::Post,
        "/listings/{id}/renames",
        &[201],
        "Rename a listing from a date and return the recorded rename.",
        Body::Json("RenameBody"),
        Body::Json("ListingRename"),
    ),
    (
        Verb::Get,
        "/listings/{id}/renames",
        &[200],
        "List one listing's recorded renames.",
        Body::None,
        Body::JsonArray("ListingRename"),
    ),
    (
        Verb::Get,
        "/listings/{id}/renames/{rename_id}",
        &[200],
        "Fetch one recorded rename of this listing, or 404.",
        Body::None,
        Body::Json("ListingRename"),
    ),
    (
        Verb::Delete,
        "/listings/{id}/renames/{rename_id}",
        &[204],
        "Undo a recorded rename.",
        Body::None,
        Body::None,
    ),
    // ---- Scheduler ---------------------------------------------------------
    (
        Verb::Get,
        "/jobs",
        &[200],
        "List the registered maintenance jobs with their schedule, trigger kind and run history.",
        Body::None,
        Body::JsonArray("JobStatus"),
    ),
    (
        Verb::Post,
        "/jobs/{name}",
        &[204],
        "Trigger a registered job by name now; 404 naming the registered names, or 500 carrying the job's own failure text. Takes an optional ?suffix= / ?skip_command= in the query string, not a body.",
        Body::None,
        Body::None,
    ),
    // ---- Auth (only merged in when [auth] is configured) -------------------
    (
        Verb::Get,
        "/login",
        &[200],
        "The sign-in page (HTML). Unauthenticated: on require_auth's allowlist.",
        Body::None,
        Body::Other("text/html"),
    ),
    (
        Verb::Post,
        "/login",
        &[303],
        "Verify credentials; 303 to the home path with the session cookie, or 200 re-rendering the page with the error. A source over its failed-attempt budget is refused 429 with a Retry-After (a browser gets the page carrying the lockout message instead). Unauthenticated: on the allowlist.",
        Body::Form("LoginForm"),
        Body::Other("text/html"),
    ),
    (
        Verb::Post,
        "/logout",
        &[303],
        "Tell the browser to drop the session cookie (the page itself is not gated).",
        Body::None,
        Body::Other("text/html"),
    ),
    // ---- Web frontend (the paths are prefixed by base_path at deploy time) --
    (
        Verb::Get,
        "/",
        &[200],
        "The single-page app shell (HTML).",
        Body::None,
        Body::Other("text/html"),
    ),
    (
        Verb::Get,
        "/static/style.css",
        &[200],
        "The app stylesheet. Unauthenticated: on require_auth's allowlist.",
        Body::None,
        Body::Other("text/css"),
    ),
    // ---- The document itself ----------------------------------------------
    (
        Verb::Get,
        "/openapi.json",
        &[200],
        "This OpenAPI 3.1 document, generated from the route table and the serde structs; ?tag= narrows it to one tag's operations, or ?operation= to one operationId, with the schemas they reach. Behind [auth] like every other route.",
        Body::None,
        Body::JsonFree("The OpenAPI 3.1 document for this API, as described by info above."),
    ),
    (
        Verb::Get,
        "/openapi/index",
        &[200],
        "The compact route index (text/plain): one line per operation — verb, path, operationId, summary — grouped by tag, each group headed by the request for that tag's slice of /openapi.json.",
        Body::None,
        Body::Other("text/plain"),
    ),
    // ---- Reports -----------------------------------------------------------
    // Reads: the GET reports take their parameters in the query string; the
    // POST ones take a JSON body (a price-override map or a contemplated
    // disposal, which a query string cannot carry). All answer 200.
    (
        Verb::Post,
        "/portfolio/overview",
        &[200],
        "Open holdings per (listing, holding account) with quantity, cost base and value; body carries the optional price-override map, live flag and as_of_date (omitted = today's live position).",
        Body::Json("OverviewRequest"),
        Body::JsonArray("HoldingOverview"),
    ),
    (
        Verb::Post,
        "/portfolio/performance",
        &[200],
        "Per-holding performance: accumulated figures and dated cash flows; body carries the optional price-override map, live flag and as_of_date (omitted = today's live position).",
        Body::Json("PerformanceRequest"),
        Body::JsonArray("HoldingPerformance"),
    ),
    (
        Verb::Get,
        "/portfolio/period_performance",
        &[200],
        "Portfolio return over a period, with FX attribution; ?from= / ?to= bound the window.",
        Body::None,
        Body::Json("PeriodPerformance"),
    ),
    (
        Verb::Get,
        "/portfolio/activity",
        &[200],
        "One listing's activity ledger; ?listing_id= is required, with an optional ?price= for the holding summary (absent, it is live-fetched).",
        Body::None,
        Body::Json("ActivityResponse"),
    ),
    (
        Verb::Get,
        "/portfolio/open_parcels",
        &[200],
        "Every open parcel, per parcel rather than aggregated; ?as_of_date= is the valuation date (omitted = today's live position).",
        Body::None,
        Body::JsonArray("OpenParcel"),
    ),
    (
        Verb::Post,
        "/portfolio/unrealised_gains",
        &[200],
        "Unrealised gains and losses; body carries the optional price-override map, live flag and as_of_date (omitted = today's live position).",
        Body::Json("UnrealisedGainsRequest"),
        Body::JsonArray("UnrealisedGain"),
    ),
    (
        Verb::Get,
        "/portfolio/realised_gains",
        &[200],
        "Realised gains and losses per disposal.",
        Body::None,
        Body::JsonArray("RealisedGainLoss"),
    ),
    (
        Verb::Get,
        "/portfolio/net_capital_gain",
        &[200],
        "Net capital gain per financial year, with the discount and loss-netting order applied.",
        Body::None,
        Body::JsonArray("NetCapitalGainYear"),
    ),
    (
        Verb::Get,
        "/portfolio/net_capital_gain/export",
        &[200],
        "The net capital gain report as CSV.",
        Body::None,
        Body::Other("text/csv"),
    ),
    (
        Verb::Post,
        "/portfolio/net_capital_gain/what_if",
        &[200],
        "What-if net capital gain for a contemplated disposal; the body's allocations are a list of per-parcel inputs.",
        Body::Json("WhatIfRequest"),
        Body::Json("WhatIfResponse"),
    ),
    (
        Verb::Get,
        "/portfolio/parcel_optimiser",
        &[200],
        "Which parcels to sell for a target, under a chosen strategy; ?listing_id=, ?units=, ?sale_date=, ?price= and ?holding_account_id= drive it.",
        Body::None,
        Body::Json("OptimiserResponse"),
    ),
    (
        Verb::Get,
        "/portfolio/tax_summary",
        &[200],
        "The per-financial-year tax summary.",
        Body::None,
        Body::JsonArray("TaxYearSummary"),
    ),
    (
        Verb::Get,
        "/portfolio/tax_summary/export",
        &[200],
        "The tax summary as CSV.",
        Body::None,
        Body::Other("text/csv"),
    ),
    (
        Verb::Get,
        "/reports/exchange_mic_validation",
        &[200],
        "Validate every curated exchange's MIC against the registry.",
        Body::None,
        Body::JsonArray("ExchangeMicStatus"),
    ),
    (
        Verb::Get,
        "/reports/settlement_holiday_coverage",
        &[200],
        "Flag every trade whose settlement date cannot be trusted (no calendar, or a missing holiday).",
        Body::None,
        Body::JsonArray("SettlementCoverageAlert"),
    ),
    (
        Verb::Get,
        "/reports/e4_cross_check",
        &[200],
        "Cross-check the CGT event E4 amounts against the trust income rows.",
        Body::None,
        Body::JsonArray("E4CrossCheckAlert"),
    ),
    (
        Verb::Get,
        "/reports/indexation_cross_check",
        &[200],
        "Cross-check the indexation factors against the ATO's published table.",
        Body::None,
        Body::Json("IndexationCrossCheck"),
    ),
    (
        Verb::Get,
        "/reports/amit_adjustment_cross_check",
        &[200],
        "Flag AMMA statements whose adjustments do not reconcile with the adjusted parcels.",
        Body::None,
        Body::JsonArray("AmitAdjustmentAlert"),
    ),
    (
        Verb::Get,
        "/reports/row_history",
        &[200],
        "The append-only audit trail; a browse array, or the cursor-paginated {entries, page_size, next_before_id} shape. ?table=, ?row_id=, ?before_id= and ?limit= drive it.",
        Body::None,
        Body::Json("RowHistoryResponse"),
    ),
    (
        Verb::Get,
        "/reports/rollover_consistency",
        &[200],
        "Flag rollover groups whose replacement parcels do not reconcile.",
        Body::None,
        Body::JsonArray("RolloverAlert"),
    ),
    (
        Verb::Get,
        "/reports/amit_cash_cross_check",
        &[200],
        "Flag (AMIT listing, account, financial year) combinations with cash that no statement accounts for.",
        Body::None,
        Body::JsonArray("AmitCashAlert"),
    ),
    (
        Verb::Get,
        "/reports/wash_sales",
        &[200],
        "Flag loss-realising sales with a re-purchase inside the wash-sale window; ?window_days= sets the window.",
        Body::None,
        Body::JsonArray("WashSaleAlert"),
    ),
    (
        Verb::Get,
        "/reports/franking_at_risk",
        &[200],
        "Franking credits at risk from the holding-period rule.",
        Body::None,
        Body::JsonArray("FrankingAtRiskAlert"),
    ),
    (
        Verb::Get,
        "/reports/franking_at_risk/what_if",
        &[200],
        "What-if franking-at-risk for a contemplated sale; ?listing_id=, ?sale_date= and ?units= drive it.",
        Body::None,
        Body::JsonArray("FrankingWhatIfAlert"),
    ),
    (
        Verb::Get,
        "/reports/fx_coverage",
        &[200],
        "Every record that needs an AUD conversion and the rate it resolves to.",
        Body::None,
        Body::JsonArray("FxCoverageAlert"),
    ),
    (
        Verb::Get,
        "/reports/health",
        &[200],
        "The data-health report behind the UI's banner: stale prices and FX, failed and stalled jobs, unpriced days, duplicates and more.",
        Body::None,
        Body::Json("HealthReport"),
    ),
    (
        Verb::Get,
        "/reports/tax_report",
        &[200],
        "The Annual Tax Report for one financial year; ?tax_year= selects it and is required.",
        Body::None,
        Body::Json("TaxReport"),
    ),
    (
        Verb::Get,
        "/reports/tax_report/years",
        &[200],
        "Every financial year with a recorded fact, for the annual report's year picker.",
        Body::None,
        Body::JsonIntegers,
    ),
    (
        Verb::Get,
        "/reports/attachments",
        &[200],
        "Every stored attachment joined out to its owning activity.",
        Body::None,
        Body::JsonArray("AttachmentIndexRow"),
    ),
    (
        Verb::Get,
        "/report_snapshots",
        &[200],
        "List stored report snapshots, optionally narrowed by ?report= and ?from= / ?to=.",
        Body::None,
        Body::JsonArray("SnapshotMeta"),
    ),
    (
        Verb::Get,
        "/report_snapshots/series",
        &[200],
        "The portfolio's stored snapshot totals as a time series, oldest first; narrow it to one listing with ?listing_id=.",
        Body::None,
        Body::JsonArray("SeriesPoint"),
    ),
    (
        Verb::Get,
        "/report_snapshots/holding_series",
        &[200],
        "Each holding's stored unit-price series; ?from=/?to= bound the snapshot dates read.",
        Body::None,
        Body::JsonArray("HoldingSeries"),
    ),
    (
        Verb::Post,
        "/report_snapshots/generate",
        &[200],
        "Generate (or re-generate) snapshots for a date range; the resulting snapshot metadata is returned.",
        Body::Json("SnapshotGenerateBody"),
        Body::JsonArray("SnapshotMeta"),
    ),
    (
        Verb::Post,
        "/report_snapshots/regenerate_all",
        &[200],
        "Regenerate every held date in a range (default: first-ever-held through the latest valuable date), reporting per-date blockers in the summary.",
        Body::Json("RegenerateBody"),
        Body::Json("RegenerateSummary"),
    ),
    (
        Verb::Get,
        "/report_snapshots/regenerate_range",
        &[200],
        "The default bulk-regeneration bounds, for the UI to prefill before submitting.",
        Body::None,
        Body::Json("RegenerateRange"),
    ),
    (
        Verb::Post,
        "/report_snapshots/regenerate_provisional",
        &[200],
        "Regenerate every snapshot flagged provisional, now that a later FX rate may have arrived.",
        Body::None,
        Body::Json("RegenerateSummary"),
    ),
    (
        Verb::Get,
        "/report_snapshots/{report}/{date}",
        &[200],
        "Fetch one stored snapshot's payload, or 404.",
        Body::None,
        Body::Json("Snapshot"),
    ),
];

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
fn operation_id(verb: Verb, path: &str, statuses: &[u16], response: Body) -> String {
    let word = match verb {
        Verb::Get if matches!(response, Body::JsonArray(_) | Body::JsonIntegers) => "list",
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
                    first.to_ascii_uppercase().to_string() + chars.as_str()
                })
            })
            .collect()
    };
    let segments = path.split('/').filter(|s| !s.is_empty());
    let (params, literals): (Vec<&str>, Vec<&str>) = segments.partition(|s| s.starts_with('{'));
    let mut id = word.to_string();
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

/// Every operation a deployment publishes, in [`ROUTES`] order: the table's
/// rows (less `/login` and `/logout` without `[auth]`, which such a deployment
/// does not serve), then one row per `/static/*.js` module. The one list both
/// the document and the [`route_index`] are built from, so the two cannot
/// disagree about what is served.
fn published_rows(auth: bool) -> Vec<RouteRow> {
    let mut rows: Vec<RouteRow> = ROUTES
        .iter()
        .filter(|(_, path, ..)| auth || !matches!(*path, "/login" | "/logout"))
        .copied()
        .collect();
    // The `/static/*.js` routes are registered in a loop over `JS_MODULES`,
    // so no literal path string exists to scan; read them from that same
    // list rather than transcribing the paths.
    for (path, _) in crate::web::JS_MODULES {
        rows.push((
            Verb::Get,
            path,
            &[200],
            "A served frontend ES module (JavaScript).",
            Body::None,
            Body::Other("text/javascript"),
        ));
    }
    rows
}

/// The compact route index served at `GET /openapi/index`: one line per
/// published operation — verb, path, `operationId`, summary — grouped under
/// its tag in [`TAGS`] order, each group headed by the `?tag=` request that
/// fetches just that slice of the document.
///
/// It exists because the whole document is too large for an agent to read
/// (~400 KB); this is a few thousand tokens, enough to choose the operation
/// and then fetch only its tag's slice. It carries no schemas and adds no
/// facts of its own: every line is read from [`published_rows`], the same
/// list the document is built from.
fn route_index(base_path: &str, auth: bool) -> String {
    let rows = published_rows(auth);
    let mut out = format!(
        "share-tracker API {} — route index ({} operations)\n\
         Paths are relative to {}. The full OpenAPI 3.1 contract is GET /openapi.json; one \
         operation and the schemas it reaches is GET /openapi.json?operation=<operationId> \
         (the third column below), and one section's is GET /openapi.json?tag=<tag>, the tag \
         URL-encoded as each heading shows.\n\
         Money and quantities are JSON strings, every request body refuses unknown fields, \
         and a PUT replaces the whole row.\n",
        env!("CARGO_PKG_VERSION"),
        rows.len(),
        if base_path.is_empty() { "/" } else { base_path },
    );
    let mut seen: Vec<&str> = Vec::new();
    for &(_, tag) in TAGS {
        if seen.contains(&tag) {
            continue;
        }
        seen.push(tag);
        let group: Vec<&RouteRow> = rows.iter().filter(|row| tag_for(row.1) == tag).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "\n## {tag} — GET /openapi.json?tag={}\n",
            tag.replace(' ', "%20")
        ));
        for &&(verb, path, statuses, summary, _, response) in &group {
            out.push_str(&format!(
                "{:<6} {path}  {} — {summary}\n",
                format!("{verb:?}").to_uppercase(),
                operation_id(verb, path, statuses, response),
            ));
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
                        out.push(name.to_string());
                    }
                    refs(child, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| refs(item, out)),
            _ => {}
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
                components.insert("schemas".to_string(), Value::Object(reached.clone()));
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

/// The OpenAPI 3.1 document, assembled from [`ROUTES`] and the schemas the
/// referenced types derive — the auth-on, root-path form the tests read.
#[cfg(test)]
pub fn document() -> OpenApi {
    document_for("", true)
}

/// [`document`] for a deployment: `base_path` becomes the document's `servers`
/// entry (so a client resolves the paths against the prefix the app is really
/// mounted under), and `auth` decides whether the authentication scheme and the
/// `[auth]`-only routes are published. A deployment without `[auth]` has no
/// `/login` or `/logout` route at all, so advertising them would send a client
/// at a 404.
pub fn document_for(base_path: &str, auth: bool) -> OpenApi {
    let mut paths = Paths::new();
    let mut used_tags = std::collections::BTreeSet::new();
    for (verb, path, statuses, summary, request, response) in published_rows(auth) {
        let operation = operation(verb, path, statuses, summary, request, response);
        paths.add_path_operation(path, vec![verb.http()], operation);
        used_tags.insert(tag_for(path));
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
    let server_url = if base_path.is_empty() { "/" } else { base_path };
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
        .components(Some(components(auth)))
        .servers(Some(vec![
            utoipa::openapi::ServerBuilder::new()
                .url(server_url)
                .build(),
        ]));
    if auth {
        builder = builder.security(Some(vec![
            SecurityRequirement::new("sessionCookie", Vec::<String>::new()),
            SecurityRequirement::new("bearerAuth", Vec::<String>::new()),
        ]));
    }
    builder.build()
}

/// Register a type's own schema and every schema it recursively references.
macro_rules! register_schemas {
    ($out:expr, $($ty:ty),* $(,)?) => {
        $(
            $out.push((
                <$ty as ToSchema>::name().into_owned(),
                <$ty as PartialSchema>::schema(),
            ));
            <$ty as ToSchema>::schemas(&mut $out);
        )*
    };
}

/// The document's reusable schemas: every type the route table references,
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
fn components(auth: bool) -> Components {
    let mut by_name: std::collections::BTreeMap<String, RefOr<Schema>> =
        std::collections::BTreeMap::new();
    for (name, schema) in component_schemas() {
        by_name.entry(name).or_insert(schema);
    }
    let mut components = ComponentsBuilder::new().schemas_from_iter(by_name).build();
    if auth {
        // The two credentials docs/API.md documents: the session cookie the
        // browser carries, and the bearer token the deployment scripts send
        // (`Authorization: Bearer <api_token>`). Named here so a generated
        // client can be configured rather than guessing.
        let mut schemes = std::collections::BTreeMap::new();
        schemes.insert(
            "bearerAuth".to_string(),
            RefOr::T(SecurityScheme::Http(
                HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build(),
            )),
        );
        schemes.insert(
            "sessionCookie".to_string(),
            RefOr::T(SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(
                "st_session",
            )))),
        );
        components.security_schemes = schemes;
    }
    components
}

/// Every type the route table references, as `(component name, schema)` pairs
/// collected recursively from the derives.
fn component_schemas() -> Vec<(String, RefOr<Schema>)> {
    let mut schemas: Vec<(String, RefOr<Schema>)> = Vec::new();
    register_schemas!(
        schemas,
        // Request bodies.
        crate::entities::listing::ListingBody,
        crate::entities::trade::TradeBody,
        crate::entities::sell::SellBody,
        crate::entities::income::IncomeBody,
        crate::entities::interest_income::InterestIncomeBody,
        crate::entities::investment_expense::InvestmentExpenseBody,
        crate::entities::amma::AmmaStatementBody,
        crate::entities::amit_adjustment::AmitAdjustmentBody,
        crate::entities::holding_account::HoldingAccountBody,
        crate::entities::inheritance::InheritanceBody,
        crate::entities::drp_enrolment::DrpEnrolmentBody,
        crate::entities::ess_statement::EssStatementBody,
        crate::entities::transfer::TransferBody,
        crate::entities::corporate_action::CorporateActionBody,
        crate::entities::exchange::ExchangeBody,
        crate::entities::exchange_holiday::ExchangeHolidayBody,
        crate::entities::cgt_settings::CgtSettingsBody,
        crate::entities::tax_year_settings::TaxYearSettingsBody,
        crate::entities::closing_price::ManualPriceBody,
        crate::entities::closing_price::FetchBody,
        crate::entities::closing_price::BackfillBody,
        crate::entities::closing_price::ClearBody,
        crate::entities::rba_fx_rate::CorrectionBody,
        crate::entities::drp_reinvestment::ReinvestBody,
        crate::entities::listing_rename::RenameBody,
        crate::entities::rights_exercise::ExerciseBody,
        crate::entities::rights_sale::SellRightsBody,
        crate::entities::buyback_participation::ParticipationBody,
        crate::entities::amit_adjustment_generation::GenerateBody,
        crate::reports::snapshot::GenerateBody,
        crate::reports::snapshot::RegenerateBody,
        crate::reports::portfolio::OverviewRequest,
        crate::reports::performance::PerformanceRequest,
        crate::reports::unrealised_gains::UnrealisedGainsRequest,
        crate::reports::net_capital_gain::WhatIfRequest,
        crate::infra::auth::LoginForm,
        // Entity rows and operation results.
        crate::entities::listing::Listing,
        crate::entities::trade::Trade,
        crate::entities::transfer::Transfer,
        crate::entities::transfer::TransferGroup,
        crate::entities::income::Income,
        crate::entities::interest_income::InterestIncome,
        crate::entities::investment_expense::InvestmentExpense,
        crate::entities::amma::AmmaStatement,
        crate::entities::amit_adjustment::AmitAdjustment,
        crate::entities::holding_account::HoldingAccount,
        crate::entities::inheritance::Inheritance,
        crate::entities::drp_enrolment::DrpEnrolment,
        crate::entities::ess_statement::EssStatement,
        crate::entities::corporate_action::CorporateAction,
        crate::entities::exchange::Exchange,
        crate::entities::exchange_holiday::ExchangeHoliday,
        crate::entities::closing_price::ClosingPrice,
        crate::entities::cgt_settings::CgtSettings,
        crate::entities::tax_year_settings::TaxYearSettings,
        crate::entities::parcel_allocation::ParcelAllocation,
        crate::entities::attachment::Attachment,
        crate::entities::distribution_event::DistributionEvent,
        crate::entities::currencies::Currency,
        crate::entities::mic_registry::MicEntry,
        crate::entities::rba_fx_rate::RbaFxRate,
        crate::entities::rights_sale::RightsSale,
        crate::entities::listing_rename::ListingRename,
        crate::infra::scheduler::JobStatus,
        crate::entities::buyback_participation::Participation,
        crate::entities::demerger::Demerge,
        crate::entities::scrip_exchange::Exchange,
        crate::entities::worthless::Recognise,
        crate::entities::amit_adjustment_generation::GeneratedAdjustments,
        crate::entities::closing_price::BackfillSummary,
        crate::entities::closing_price::ClearSummary,
        crate::entities::currencies::ImportSummary,
        crate::entities::mic_registry::ImportSummary,
        crate::entities::rba_fx_rate::ImportSummary,
        // Report responses.
        crate::reports::portfolio::HoldingOverview,
        crate::reports::performance::HoldingPerformance,
        crate::reports::period_performance::PeriodPerformance,
        crate::reports::activity::ActivityResponse,
        crate::reports::open_parcels::OpenParcel,
        crate::reports::unrealised_gains::UnrealisedGain,
        crate::reports::realised_gains::RealisedGainLoss,
        crate::reports::net_capital_gain::NetCapitalGainYear,
        crate::reports::net_capital_gain::WhatIfResponse,
        crate::reports::parcel_optimiser::OptimiserResponse,
        crate::reports::tax_summary::TaxYearSummary,
        crate::reports::mic_validation::ExchangeMicStatus,
        crate::reports::settlement_coverage::SettlementCoverageAlert,
        crate::reports::e4_cross_check::E4CrossCheckAlert,
        crate::reports::indexation_cross_check::IndexationCrossCheck,
        crate::reports::amit_adjustment_cross_check::AmitAdjustmentAlert,
        crate::reports::row_history::RowHistoryResponse,
        crate::reports::rollover_consistency::RolloverAlert,
        crate::reports::amit_cash_cross_check::AmitCashAlert,
        crate::reports::wash_sales::WashSaleAlert,
        crate::reports::franking_at_risk::FrankingAtRiskAlert,
        crate::reports::franking_at_risk::FrankingWhatIfAlert,
        crate::reports::fx_coverage::FxCoverageAlert,
        crate::reports::health::HealthReport,
        crate::reports::tax_report::TaxReport,
        crate::reports::attachments::AttachmentIndexRow,
        crate::reports::snapshot::SnapshotMeta,
        crate::reports::snapshot::SeriesPoint,
        crate::reports::snapshot::HoldingSeries,
        crate::reports::snapshot::RegenerateSummary,
        crate::entities::rba_fx_rate::ImportOutcome,
        crate::reports::snapshot::RegenerateRange,
        crate::reports::snapshot::Snapshot,
    );
    schemas
}

/// One operation: its summary, the path parameters its spelling declares, and
/// its request and success responses.
///
/// `statuses` is the success set the route can answer. A `PUT` upsert carries
/// two — `201` with `response` (the created row) and `204` with no content —
/// because it reports whether it created or replaced. A `200` or `201`
/// carries `response`; a `204` never has a body, and neither does a `303`
/// (the login/logout redirect — its `Location` is the whole answer).
fn operation(
    verb: Verb,
    path: &str,
    statuses: &[u16],
    summary: &str,
    request: Body,
    response: Body,
) -> Operation {
    let mut responses = ResponsesBuilder::new();
    for &status in statuses {
        let body = if matches!(status, 200 | 201) {
            response
        } else {
            Body::None
        };
        responses = responses.response(status.to_string(), success_response(status, body));
    }
    // `POST /login` can answer 429 — the failed-attempt lockout
    // (`infra::auth`) — and it is not a JSON body, so it is recorded here by
    // description rather than as a schema. The browser path is refused 429 too
    // — it differs only in rendering the sign-in page as the body; both shapes
    // are stated in `docs/API.md`'s Error-body contract.
    if path == "/login" && verb == Verb::Post {
        responses = responses.response(
            "429".to_string(),
            ResponseBuilder::new()
                .description(
                    "This source has exhausted its failed-attempt budget: the body is the \
                     plain-text reason and Retry-After names the remaining whole seconds, rounded \
                     up. A browser login POST (Accept: text/html) is refused 429 too, with the \
                     sign-in page as the body instead of the plain-text reason. The same 429 \
                     answers when every password-verify slot is busy — the server verifies at \
                     most two passwords at once, so a flood cannot allocate unbounded memory on \
                     this unauthenticated path — and then Retry-After is 1.",
                )
                .build(),
        );
    }
    // Every JSON/form body can answer 422 — the deny-unknown-fields rule and
    // the money-as-string rule are both enforced there.
    if matches!(
        request,
        Body::Json(_) | Body::JsonArray(_) | Body::Form(_) | Body::JsonFree(_)
    ) {
        responses = responses.response(
            "422".to_string(),
            ResponseBuilder::new()
                .description(
                    "The body was rejected: an unknown or misspelt field, or a money/quantity \
                     field sent as a JSON number. The body is the plain-text reason, naming the \
                     offending field.",
                )
                .build(),
        );
    } else if verb == Verb::Post && matches!(request, Body::Other(_)) {
        // A multipart upload or a bare-text feed: not a JSON struct, so the
        // deny-unknown-fields rule above cannot describe it, but it answers 422
        // when the payload cannot be read or parsed.
        responses = responses.response(
            "422".to_string(),
            ResponseBuilder::new()
                .description(
                    "The payload could not be read or parsed: the body is the plain-text reason.",
                )
                .build(),
        );
    }
    // A route addressed by a path parameter can answer 404 — the row the path
    // names does not exist. A GET-one's is the **empty** body; a DELETE's or an
    // operation's carries the plain-text reason. Recorded here rather than in
    // every row, and by description because neither shape is a schema.
    if path.contains('{') && matches!(verb, Verb::Get | Verb::Post | Verb::Delete) {
        responses = responses.response(
            "404".to_string(),
            ResponseBuilder::new()
                .description(if verb == Verb::Get {
                    "No such row. The body is empty."
                } else {
                    "No such row: the body is the plain-text reason."
                })
                .build(),
        );
    }
    // The feed-import and provider-fetch routes can answer 502 when their
    // upstream cannot be reached (`ApiError::BadGateway`).
    if matches!(verb, Verb::Post)
        && (path.ends_with("/import") || path.ends_with("/fetch") || path.ends_with("/backfill"))
    {
        responses = responses.response(
            "502".to_string(),
            ResponseBuilder::new()
                .description(
                    "The upstream feed or price provider could not be reached: the body is the \
                     plain-text reason.",
                )
                .build(),
        );
    }
    // Every route can answer a generic internal failure — recorded once here so
    // a generated client handles it rather than treating a 500 as transport
    // noise. The body is empty; the cause is in the server log.
    responses = responses.response(
        "500".to_string(),
        ResponseBuilder::new()
            .description("An internal failure. The body is empty; the cause is in the server log.")
            .build(),
    );
    // A route that takes a body can answer 413 (over axum's 2 MiB body limit;
    // the attachment upload raises its own route) and 415 (a body sent with the
    // wrong content type) before its handler runs.
    if !matches!(request, Body::None) {
        responses = responses.response(
            "413".to_string(),
            ResponseBuilder::new()
                .description(
                    "The request body is over the size limit. The body is the plain-text reason.",
                )
                .build(),
        );
        responses = responses.response(
            "415".to_string(),
            ResponseBuilder::new()
                .description(
                    "The request body's content type is not the one this route takes. The body is \
                     the plain-text reason.",
                )
                .build(),
        );
    }
    let mut builder = OperationBuilder::new()
        .operation_id(Some(operation_id(verb, path, statuses, response)))
        .tag(tag_for(path))
        .summary(Some(summary))
        .parameters(Some(operation_parameters(verb, path)))
        .responses(responses.build());
    if let Some(body) = request_body(request) {
        builder = builder.request_body(Some(body));
    }
    builder.build()
}

/// Every parameter of an operation: a required string per `{name}` path
/// segment, plus the query parameters the route's own `Query<T>` type declares.
///
/// The query half is derived from that Rust type through `utoipa::IntoParams`,
/// not from the summary prose: the type is what axum actually decodes, so the
/// document's name, `required` and schema for each parameter cannot disagree
/// with the handler. A plain field is required, an `Option` is not, and
/// `i64`/`NaiveDate`/`Decimal` reach the document as integer/date/string rather
/// than as one flat "string, optional" shape. Reading `?name=` out of the
/// summary — which the first version of this did — could only ever guess both,
/// and guessed wrong: every parameter came out optional, including
/// `?tax_year=`, which a request is refused `400` for omitting.
///
/// [`tests::every_query_parameter_matches_its_summary`] keeps the prose honest
/// in the other direction, so a summary cannot name a parameter the type does
/// not have.
fn operation_parameters(verb: Verb, path: &str) -> Vec<Parameter> {
    let mut parameters = path_parameters(path);
    parameters.extend(query_parameters(verb, path));
    parameters
}

/// The query parameters a route declares, as its `Query<T>` type describes
/// them. One arm per route that decodes a query string; a route absent from
/// the match takes none, and `NoFilter` lists (the reference and settings
/// tables) are deliberately among them.
///
/// `into_params` is given `|| Some(ParameterIn::Query)` so every field is
/// placed in the query string rather than needing a per-field attribute.
fn query_parameters(verb: Verb, path: &str) -> Vec<Parameter> {
    use utoipa::IntoParams;

    fn of<T: IntoParams>() -> Vec<Parameter> {
        T::into_params(|| Some(ParameterIn::Query))
    }

    // Keyed on the verb too, because some query-decoding routes are not a
    // GET: `POST /jobs/{name}` takes `?suffix=`/`?skip_command=`, and the
    // previewable writes take `?dry_run=`.
    if verb == Verb::Post {
        return match path {
            "/jobs/{name}" => of::<crate::infra::scheduler::JobParams>(),
            // The computed writes a `?dry_run=true` previews.
            "/sells"
            | "/transfers"
            | "/corporate_actions/{id}/participate"
            | "/corporate_actions/{id}/demerge"
            | "/corporate_actions/{id}/exchange"
            | "/corporate_actions/{id}/exercise"
            | "/corporate_actions/{id}/sell_rights"
            | "/corporate_actions/{id}/recognise"
            | "/income/{id}/reinvest"
            | "/ess_statements/{id}/vest" => of::<crate::infra::http::DryRunQuery>(),
            _ => Vec::new(),
        };
    }
    if verb != Verb::Get {
        return Vec::new();
    }
    match path {
        "/openapi.json" => of::<DocumentQuery>(),
        // Entity lists: the `CrudEntity::Filter` type behind
        // `http::list_handler`, one per filtered list (`entities::LIST_ROUTES`
        // classifies every list route, filtered or not).
        "/listings" => of::<crate::entities::listing::ListingListQuery>(),
        "/trades" => of::<crate::entities::trade::TradeListQuery>(),
        "/income" => of::<crate::entities::income::IncomeListQuery>(),
        "/interest_income" => of::<crate::entities::interest_income::InterestIncomeListQuery>(),
        "/investment_expenses" => {
            of::<crate::entities::investment_expense::InvestmentExpenseListQuery>()
        }
        "/amma_statements" => of::<crate::entities::amma::AmmaListQuery>(),
        "/amit_adjustments" => of::<crate::entities::amit_adjustment::AmitAdjustmentListQuery>(),
        "/corporate_actions" => of::<crate::entities::corporate_action::CorporateActionListQuery>(),
        "/distribution_events" => {
            of::<crate::entities::distribution_event::DistributionEventListQuery>()
        }
        "/drp_enrolments" => of::<crate::entities::drp_enrolment::DrpEnrolmentListQuery>(),
        "/ess_statements" => of::<crate::entities::ess_statement::EssStatementListQuery>(),
        "/inheritances" => of::<crate::entities::inheritance::InheritanceListQuery>(),
        "/parcel_allocations" => {
            of::<crate::entities::parcel_allocation::ParcelAllocationListQuery>()
        }
        "/transfers" => of::<crate::entities::transfer::TransferListQuery>(),
        // Hand-written lists and reads.
        "/attachments" => of::<crate::entities::attachment::ListQuery>(),
        "/attachments/{id}/content" => of::<crate::entities::attachment::ContentQuery>(),
        "/closing_prices" => of::<crate::entities::closing_price::ListParams>(),
        "/rights_sales" => of::<crate::entities::rights_sale::ListParams>(),
        "/exchange_holidays" | "/exchange_holidays/{mic}" => {
            of::<crate::entities::exchange_holiday::ListParams>()
        }
        "/report_snapshots" => of::<crate::reports::snapshot::ListParams>(),
        "/report_snapshots/series" => of::<crate::reports::snapshot::SeriesParams>(),
        "/report_snapshots/holding_series" => of::<crate::reports::snapshot::HoldingSeriesParams>(),
        "/portfolio/activity" => of::<crate::reports::activity::ActivityRequest>(),
        "/portfolio/open_parcels" => of::<crate::reports::open_parcels::OpenParcelsQuery>(),
        "/portfolio/parcel_optimiser" => of::<crate::reports::parcel_optimiser::OptimiserRequest>(),
        "/portfolio/period_performance" => {
            of::<crate::reports::period_performance::PeriodRequest>()
        }
        "/reports/row_history" => of::<crate::reports::row_history::RowHistoryRequest>(),
        "/reports/tax_report" => of::<crate::reports::tax_report::TaxReportRequest>(),
        "/reports/wash_sales" => of::<crate::reports::wash_sales::WashSalesRequest>(),
        "/reports/franking_at_risk/what_if" => {
            of::<crate::reports::franking_at_risk::WhatIfRequest>()
        }
        _ => Vec::new(),
    }
}

/// The `?name=` tokens a summary mentions — the prose side of
/// [`query_parameters`], used only by the test that keeps the two in step.
#[cfg(test)]
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

/// The success response, with the content [`Body`] names.
fn success_response(status: u16, response: Body) -> Response {
    let description = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        303 => "See Other",
        other => panic!("the route table documents no success status {other}"),
    };
    let builder = ResponseBuilder::new().description(description);
    match response {
        Body::None => builder.build(),
        Body::Json(name) => builder
            .content("application/json", json_content(schema_ref(name)))
            .build(),
        Body::JsonArray(name) => builder
            .content("application/json", json_content(array_of(schema_ref(name))))
            .build(),
        Body::JsonIntegers => builder
            .content(
                "application/json",
                json_content(array_of(
                    ObjectBuilder::new().schema_type(Type::Integer).into(),
                )),
            )
            .build(),
        Body::JsonFree(description) => builder
            .description(description)
            .content("application/json", json_content(free_form_object()))
            .build(),
        Body::Form(_) => unreachable!("a response is never a form body"),
        Body::Other(media_type) => builder
            .content(media_type, ContentBuilder::new().build())
            .build(),
    }
}

/// The request body, if the route takes one.
fn request_body(request: Body) -> Option<RequestBody> {
    let (media_type, content, description) = match request {
        Body::None => return None,
        Body::Json(name) => (
            "application/json",
            json_content(schema_ref(name)),
            "The JSON request body.",
        ),
        Body::JsonArray(name) => (
            "application/json",
            json_content(array_of(schema_ref(name))),
            "A JSON array request body.",
        ),
        Body::JsonIntegers => (
            "application/json",
            json_content(array_of(
                ObjectBuilder::new().schema_type(Type::Integer).into(),
            )),
            "A JSON array of integers.",
        ),
        Body::JsonFree(description) => (
            "application/json",
            json_content(free_form_object()),
            description,
        ),
        Body::Form(name) => (
            "application/x-www-form-urlencoded",
            ContentBuilder::new().schema(Some(schema_ref(name))).build(),
            "The form request body.",
        ),
        Body::Other(media_type) => (
            media_type,
            ContentBuilder::new().build(),
            "The raw request body: not JSON, so it carries no schema here.",
        ),
    };
    Some(
        RequestBodyBuilder::new()
            .description(Some(description))
            .content(media_type, content)
            .required(Some(Required::True))
            .build(),
    )
}

/// A `$ref` to a component schema.
fn schema_ref(name: &str) -> RefOr<Schema> {
    RefOr::Ref(Ref::from_schema_name(name))
}

/// A JSON array whose items are `items`.
fn array_of(items: RefOr<Schema>) -> RefOr<Schema> {
    RefOr::T(Schema::Array(ArrayBuilder::new().items(items).build()))
}

/// An object schema allowing any property (`{"type": "object"}` with no
/// `additionalProperties: false`).
fn free_form_object() -> RefOr<Schema> {
    RefOr::T(Schema::Object(
        ObjectBuilder::new()
            .schema_type(Type::Object)
            .additional_properties(Some(AdditionalProperties::FreeForm(true)))
            .build(),
    ))
}

/// A JSON `Content` object for one schema.
fn json_content(schema: RefOr<Schema>) -> utoipa::openapi::Content {
    ContentBuilder::new().schema(Some(schema)).build()
}

/// `GET /openapi.json`. Merged into `app::router` like every other router, so
/// `[auth]`'s `require_auth` gates it and it is deliberately absent from that
/// layer's login-page allowlist.
///
/// The document (200+ schemas, ~130 paths) is built and serialised **once**,
/// when the router is assembled, and each request clones the finished JSON
/// string — not rebuilt per request as it was.
pub fn router(base_path: &str, auth: bool) -> Router<SqlitePool> {
    let document =
        serde_json::to_value(document_for(base_path, auth)).expect("the document serialises");
    let served = std::sync::Arc::new(Served {
        whole: document.to_string(),
        index: route_index(base_path, auth),
        document,
    });
    Router::new()
        .route("/openapi.json", get(serve_document))
        .route("/openapi/index", get(serve_index))
        .layer(axum::Extension(served))
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
async fn serve_document(
    axum::Extension(served): axum::Extension<std::sync::Arc<Served>>,
    Query(query): Query<DocumentQuery>,
) -> Result<axum::response::Response, ApiError> {
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
                "send at most one of ?tag= and ?operation=".to_string(),
            ));
        }
    };
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}

/// `GET /openapi/index`: the compact route index, as plain text.
async fn serve_index(
    axum::Extension(served): axum::Extension<std::sync::Arc<Served>>,
) -> axum::response::Response {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        served.index.clone(),
    )
        .into_response()
}

/// Every **id-keyed collection create** the route table documents — a
/// parameterless `POST /<collection>` answering `201 Created` — sorted.
///
/// Read by `doc_checks` so the Response-codes `201` row is *derived* from the
/// same route table `every_served_route_is_documented_and_nothing_else_is`
/// pins to the live router, rather than transcribed into a second list that a
/// new entity silently escapes.
///
/// Two parameterless `201` `POST`s are operations rather than collection
/// creates — `/attachments` (an upload) and `/closing_prices/fetch` (a fetch)
/// — which the `201` row names in its operation clauses instead of its
/// collection list. They are listed here so a *new* operation-shaped create
/// fails this test until it is classified rather than quietly widening the
/// collection set.
/// Every **`GET`-one** route the document carries: a `GET` on a path with a
/// parameter whose success body is a single object (not an array, which is how
/// a path-narrowed list like `/exchange_holidays/{mic}` is told apart). Read by
/// the empty-404 test below so a new GET-one is covered without a hand-kept
/// list.
#[cfg(test)]
pub(crate) fn documented_get_one_routes() -> Vec<String> {
    ROUTES
        .iter()
        .filter(|(verb, path, _, _, _, response)| {
            *verb == Verb::Get && path.contains('{') && matches!(response, Body::Json(_))
        })
        .map(|(_, path, _, _, _, _)| (*path).to_string())
        .collect()
}

#[cfg(test)]
pub(crate) fn documented_id_keyed_collection_creates() -> Vec<String> {
    const OPERATION_CREATES: &[&str] = &["/attachments", "/closing_prices/fetch"];
    let mut out: Vec<String> = ROUTES
        .iter()
        .filter(|(verb, path, statuses, _, _, _)| {
            *verb == Verb::Post
                && statuses.contains(&201)
                && !path.contains('{')
                && !OPERATION_CREATES.contains(path)
        })
        .map(|(_, path, _, _, _, _)| (*path).to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::PathBuf;

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

    /// Blank `#[cfg(test)] mod …{ … }` blocks, so a route registered by a
    /// test (the panic layer's `/boom`) is not mistaken for a served one.
    ///
    /// Blanked byte-for-byte, like [`crate::test_support::code_only`] (which
    /// the handler scans compose over this): a caller that finds a token in the
    /// result reads the surrounding source out of the raw text at the same
    /// offset, and the route path it wants is a string literal only the raw
    /// text still has.
    fn strip_test_modules(source: &str) -> String {
        let bytes = source.as_bytes();
        let mut out = String::with_capacity(source.len());
        let mut i = 0;
        while i < bytes.len() {
            if source[i..].starts_with("#[cfg(test)]")
                && let Some(brace) = source[i..].find('{')
            {
                let between = &source[i..i + brace];
                if between.contains("mod ") && !between.contains(';') {
                    let mut depth = 0i32;
                    let mut j = i + brace;
                    while j < bytes.len() {
                        match bytes[j] {
                            b'{' => depth += 1,
                            b'}' => {
                                depth -= 1;
                                if depth == 0 {
                                    j += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    for c in source[i..j].chars() {
                        if c == '\n' {
                            out.push('\n');
                        } else {
                            for _ in 0..c.len_utf8() {
                                out.push(' ');
                            }
                        }
                    }
                    i = j;
                    continue;
                }
            }
            let ch = source[i..].chars().next().expect("a char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    /// Every `.rs` file under `src`, path relative to `src`.
    fn source_files() -> Vec<(String, String)> {
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut walk = vec![src.clone()];
        while let Some(dir) = walk.pop() {
            for entry in std::fs::read_dir(&dir).expect("src is readable").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let rel = path
                        .strip_prefix(&src)
                        .expect("under src")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    found.push((
                        rel,
                        std::fs::read_to_string(&path).expect("a source file is readable"),
                    ));
                }
            }
        }
        found.sort();
        found
    }

    /// One `.route(…)` registration found in the sources.
    enum SourceRoute {
        /// A literal path with the verbs registered on it.
        Literal(String, Vec<&'static str>),
        /// A registration whose path is built in code (the file is the
        /// walk entry's own path).
        Dynamic,
    }

    /// Read every `.route(…)` call out of the sources. The path is the first
    /// string literal in the call (a literal registration) or absent (one
    /// built in code). The verbs are whichever of
    /// `get(`/`post(`/`put(`/`delete(`/`any(` the call names. The needle is
    /// assembled rather than written out so this module does not match itself.
    fn source_routes() -> Vec<(String, SourceRoute)> {
        let needle = format!(".{}(", "route");
        let verbs = [
            ("get(", "GET"),
            ("post(", "POST"),
            ("put(", "PUT"),
            ("delete(", "DELETE"),
            ("any(", "ANY"),
        ];
        let mut found = Vec::new();
        for (file, raw) in source_files() {
            let body = strip_test_modules(&raw);
            let mut rest = body.as_str();
            while let Some(at) = rest.find(&needle) {
                rest = &rest[at + needle.len()..];
                // The call's own arguments, up to its matching `)`.
                let mut depth = 1i32;
                let mut end = rest.len();
                for (i, c) in rest.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let args = &rest[..end];
                rest = &rest[end..];
                let literal = args.find('"').and_then(|open| {
                    let after = &args[open + 1..];
                    after
                        .find('"')
                        .map(|close| (open, after[..close].to_string()))
                });
                match literal {
                    // Only a literal that starts the call is the path.
                    Some((open, path)) if args[..open].trim().is_empty() => {
                        let tail = &args[open + 1 + path.len()..];
                        let mut registered: Vec<&'static str> = verbs
                            .iter()
                            .filter(|(needle, _)| tail.contains(*needle))
                            .map(|(_, verb)| *verb)
                            .collect();
                        registered.sort_unstable();
                        registered.dedup();
                        assert!(
                            !registered.is_empty(),
                            "{file}: `.route(\"{path}\", …)` registers no HTTP verb"
                        );
                        found.push((file.clone(), SourceRoute::Literal(path, registered)));
                    }
                    _ => found.push((file.clone(), SourceRoute::Dynamic)),
                }
            }
        }
        found
    }

    /// Registrations the scan cannot read because their path is built in code,
    /// each with why and how the document still covers it. Named rather than
    /// skipped, the way `reports::SNAPSHOT_WRITES` names its POSTs, so a new
    /// dynamic registration fails here until it is classified.
    const DYNAMIC_ROUTE_SITES: [(&str, &str); 2] = [
        (
            "app.rs",
            "the base_path redirect (`&format!(\"{base_path}/\")`) exists only when a prefix is \
             configured; the document describes the unprefixed paths `nest` serves, so it is \
             deliberately absent",
        ),
        (
            "web.rs",
            "the `/static/*.js` loop builds its paths from `JS_MODULES`; `document` reads the same \
             const, so the module routes are documented from the one list rather than a copy",
        ),
    ];

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
        let readable: std::collections::BTreeSet<&str> = ROUTES
            .iter()
            .filter(|row| row.0 == Verb::Get)
            .map(|row| row.1)
            .collect();
        let mut unreadable: Vec<&str> = ROUTES
            .iter()
            .filter(|row| matches!(row.0, Verb::Put | Verb::Delete))
            .map(|row| row.1)
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
        let readable: std::collections::BTreeSet<&str> = ROUTES
            .iter()
            .filter(|row| row.0 == Verb::Get)
            .filter_map(|row| match row.5 {
                Body::Json(name) | Body::JsonArray(name) => Some(name),
                _ => None,
            })
            .collect();
        let mut offenders = Vec::new();
        for (verb, path, statuses, _, _, response) in ROUTES {
            if *verb != Verb::Post || !statuses.contains(&201) {
                continue;
            }
            let created = match response {
                Body::Json(name) => {
                    readable.contains(name) || CREATED_GROUPS.iter().any(|(group, _)| group == name)
                }
                _ => false,
            };
            if !created {
                offenders.push(*path);
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

    /// The coverage pin, both ways: every `.route(…)` registration the sources
    /// make appears in the document under that method, and the document names
    /// no path/method the sources do not register.
    ///
    /// The `/static/*.js` module routes are added to the expected set from
    /// `JS_MODULES`, the same const the document reads; the base-path redirect
    /// is classified as a dynamic site the document deliberately omits.
    #[test]
    fn every_served_route_is_documented_and_nothing_else_is() {
        let doc = doc();
        let documented = documented_routes(&doc);

        let mut expected: Vec<(String, String)> = Vec::new();
        let mut dynamic_sites: Vec<String> = Vec::new();
        for (file, route) in source_routes() {
            match route {
                SourceRoute::Literal(path, verbs) => {
                    for verb in verbs {
                        expected.push((path.clone(), verb.to_string()));
                    }
                }
                SourceRoute::Dynamic => {
                    dynamic_sites.push(file.rsplit('/').next().unwrap_or(&file).to_string());
                }
            }
        }
        assert!(
            !expected.is_empty(),
            "the scan found no literal route registrations at all — it has stopped parsing"
        );
        assert!(
            expected.iter().any(|(path, _)| path == "/openapi.json"),
            "the scan missed this module's own `/openapi.json` registration"
        );
        // The module routes, read from the same list the document builds them
        // from (the scan cannot see the loop's string literal).
        for (path, _) in crate::web::JS_MODULES {
            expected.push((path.to_string(), "GET".to_string()));
        }
        expected.sort();
        expected.dedup();

        let missing: Vec<&(String, String)> = expected
            .iter()
            .filter(|r| !documented.contains(r))
            .collect();
        assert!(
            missing.is_empty(),
            "the document does not cover these served routes: {missing:?}"
        );
        let extra: Vec<&(String, String)> = documented
            .iter()
            .filter(|r| !expected.contains(r))
            .collect();
        assert!(
            extra.is_empty(),
            "the document names routes the sources do not register: {extra:?}"
        );

        // …and the classification cannot rot: every dynamic registration in
        // the sources is one this module names, with its reason.
        dynamic_sites.sort();
        dynamic_sites.dedup();
        let mut classified: Vec<String> = DYNAMIC_ROUTE_SITES
            .iter()
            .map(|(file, _)| (*file).to_string())
            .collect();
        classified.sort();
        classified.dedup();
        assert_eq!(
            dynamic_sites, classified,
            "the sources register dynamic routes that api_spec's DYNAMIC_ROUTE_SITES does not \
             classify (or vice versa)"
        );
    }

    /// The shape of a [`Body`], for comparing a derived body against a
    /// recorded one: the variant, and the component-schema name it names.
    fn body_shape(body: &Body) -> (&'static str, Option<&'static str>) {
        match body {
            Body::None => ("no body", None),
            Body::Json(name) => ("a JSON object", Some(name)),
            Body::JsonArray(name) => ("a JSON array", Some(name)),
            Body::JsonIntegers => ("a JSON array of integers", None),
            Body::JsonFree(_) => ("a free-form JSON value", None),
            Body::Form(name) => ("a form body", Some(name)),
            Body::Other(media) => ("a non-JSON payload", Some(media)),
        }
    }

    /// One registration of a generic CRUD handler found in the sources: the
    /// path it is registered on, which handler, and the model type it is
    /// instantiated with.
    struct GenericRoute {
        file: String,
        path: String,
        handler: &'static str,
        model: String,
    }

    /// Every `http::list_handler::<M>` / `get_handler::<M>` /
    /// `delete_handler::<M>` registration in the sources.
    ///
    /// The needles are the handler names alone, not the qualified
    /// `http::list_handler::<`, for the reason
    /// `entities::tests::shared_list_route_paths` gives: an entity that imports
    /// the handler registers `get(list_handler::<X>)`, which is the same route
    /// written differently, and the longer needle would not see it.
    fn generic_crud_registrations() -> Vec<GenericRoute> {
        // Assembled so this module's own text is not a registration.
        let needles: Vec<(String, &'static str)> = ["list", "get", "delete"]
            .iter()
            .map(|verb| (format!("{verb}_handler::{}", '<'), *verb))
            .collect();
        let mut found = Vec::new();
        for (file, body) in crate::test_support::rust_sources() {
            // The needles are looked for in the *code*, so a doc comment
            // explaining the scan and a test quoting the needle as a string
            // literal are not registrations. `code_only` blanks both
            // byte-for-byte, so an offset into it addresses the same place in
            // `body` — which is where the route path, itself a string literal,
            // has to be read from.
            let code = crate::test_support::code_only(&body);
            for (needle, handler) in &needles {
                for (at, _) in code.match_indices(needle.as_str()) {
                    let before = &code[..at];
                    let route_at = before
                        .rfind(".route(")
                        .expect("a generic handler is registered by a `.route(…)` call");
                    // The path is a string literal, so it survives only in the
                    // raw source — at the same offset.
                    let rest = body[route_at + ".route(".len()..].trim_start();
                    let path = rest
                        .strip_prefix('"')
                        .expect("the route path is a string literal")
                        .split('"')
                        .next()
                        .expect("a closing quote")
                        .to_string();
                    let model = code[at + needle.len()..]
                        .split('>')
                        .next()
                        .expect("a closing angle bracket")
                        .trim()
                        .to_string();
                    found.push(GenericRoute {
                        file: file.clone(),
                        path,
                        handler,
                        model,
                    });
                }
            }
        }
        found
    }

    /// The three fields a generic-CRUD row does **not** have to be typed by
    /// hand are derived from the registration itself and compared against
    /// [`ROUTES`]: its success status, its request body (there is none), and
    /// its response schema, which is the handler's own type parameter.
    ///
    /// This is the answer to the review finding that ~110 rows' statuses and
    /// schemas were hand-maintained with nothing checking them — the proof it
    /// drifts being `59bc2e6`, which had to hand-edit 536 lines when the `PUT`
    /// statuses changed, with nothing failing had it not. The ~55 registrations
    /// that go through `http::list_handler`/`get_handler`/`delete_handler` are
    /// the largest class and need no new metadata: the handler fixes the status
    /// (`200`/`200`/`204`) and takes no body, and `CrudEntity`'s type parameter
    /// *is* the schema name (`every_component_schema_is_referenced` is what
    /// then ties that name to a real `ToSchema` derive).
    ///
    /// What is deliberately **not** derived: the summaries, which are prose no
    /// scan can write, and the hand-written verbs' bodies, covered by
    /// [`every_request_body_matches_its_handlers_extractor`] instead.
    #[test]
    fn the_generic_crud_routes_derive_their_status_and_schemas() {
        let mut checked = 0;
        for route in generic_crud_registrations() {
            let (verb, statuses, response) = match route.handler {
                "list" => (
                    Verb::Get,
                    &[200u16][..],
                    ("a JSON array", Some(&route.model)),
                ),
                "get" => (
                    Verb::Get,
                    &[200u16][..],
                    ("a JSON object", Some(&route.model)),
                ),
                "delete" => (Verb::Delete, &[204u16][..], ("no body", None)),
                other => panic!("unclassified generic handler {other}"),
            };
            let row = ROUTES
                .iter()
                .find(|(v, p, ..)| *v == verb && *p == route.path)
                .unwrap_or_else(|| {
                    panic!(
                        "{}: {verb:?} {} is registered with {}_handler but ROUTES has no row \
                         for it",
                        route.file, route.path, route.handler
                    )
                });
            let (_, _, recorded_statuses, _, request, recorded_response) = row;
            assert_eq!(
                *recorded_statuses, statuses,
                "{verb:?} {} goes through http::{}_handler, which answers {statuses:?}",
                route.path, route.handler
            );
            assert_eq!(
                body_shape(request).0,
                "no body",
                "{verb:?} {} goes through http::{}_handler, which reads no request body",
                route.path,
                route.handler
            );
            let (kind, name) = body_shape(recorded_response);
            assert_eq!(
                (kind, name.map(str::to_string)),
                (response.0, response.1.cloned()),
                "{verb:?} {} is registered as {}_handler::<{}>, so that is its response",
                route.path,
                route.handler,
                route.model
            );
            checked += 1;
        }
        assert!(
            checked >= 50,
            "only {checked} generic CRUD registrations found — the scan has stopped parsing"
        );
    }

    /// The text between the parentheses that open at `from`, which must be the
    /// `(` itself — the call's or the parameter list's own arguments.
    fn balanced(source: &str, from: usize) -> &str {
        let rest = &source[from + 1..];
        let mut depth = 1i32;
        for (i, c) in rest.char_indices() {
            match c {
                '(' | '<' => depth += 1,
                ')' | '>' => {
                    depth -= 1;
                    if depth == 0 {
                        return &rest[..i];
                    }
                }
                _ => {}
            }
        }
        rest
    }

    /// The `{ … }` block that opens at or after `from`, balanced on braces —
    /// a handler's body. ([`balanced`] counts `<`/`>` as nesting too, which a
    /// body's comparisons and turbofish would throw off.)
    fn block_after(source: &str, from: usize) -> &str {
        let Some(open) = source[from..].find('{').map(|i| from + i) else {
            return "";
        };
        let rest = &source[open + 1..];
        let mut depth = 1i32;
        for (i, c) in rest.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &rest[..i];
                    }
                }
                _ => {}
            }
        }
        rest
    }

    /// The `Json<T>` / `Form<T>` extractor a handler's parameter list declares,
    /// as the `(variant, schema name)` pair [`body_shape`] would print for it.
    ///
    /// The schema name is `T`'s own name unless the struct renames itself with
    /// `#[schema(as = …)]`, which one body does (`GenerateBody` is
    /// `AmitGenerateBody` in the document, its module-local name being too
    /// generic to publish) — so the attribute is read out of the declaration
    /// rather than the row being allowed to disagree with the type.
    fn extractor_body(params: &str, file_code: &str) -> Option<(&'static str, String)> {
        for (needle, kind) in [("Json<", "a JSON object"), ("Form<", "a form body")] {
            if let Some(at) = params.find(needle) {
                let ty = balanced(params, at + needle.len() - 1).trim();
                return Some((kind, schema_name_of(ty, file_code)));
            }
        }
        None
    }

    /// The success response a handler's **return type** names, in
    /// [`body_shape`] terms: the `Json<…>` inside the `Result`, which is what
    /// the handler serialises on the success path.
    ///
    /// `None` where the return type is not a `Json` — a `Response` built by
    /// hand (an asset, a redirect, a file download), a `StatusCode`, or the
    /// upsert's own outcome type. Those rows say what they answer in their
    /// summary and are classified rather than derived.
    fn returned_body(signature: &str, file_code: &str) -> Option<(&'static str, String)> {
        // A `PUT` upsert answers `UpsertResponse<E>`: the created row on a
        // create, nothing on a replace — so `E` is the documented response.
        if let Some(at) = signature.find("UpsertResponse<") {
            let inner = balanced(signature, at + "UpsertResponse<".len() - 1).trim();
            return Some(("a JSON object", schema_name_of(inner, file_code)));
        }
        // A handler whose success value is a bare `StatusCode` has no body at
        // all — every `DELETE`, the job trigger, the FX correction `PUT`.
        if signature.contains("StatusCode") && !signature.contains("Json<") {
            return Some(("no body", String::new()));
        }
        let at = signature.find("Json<")?;
        let inner = balanced(signature, at + "Json<".len() - 1).trim();
        if let Some(item) = inner.strip_prefix("Vec<").and_then(|r| r.strip_suffix('>')) {
            let item = item.trim();
            if item == "i32" {
                return Some(("a JSON array of integers", String::new()));
            }
            return Some(("a JSON array", schema_name_of(item, file_code)));
        }
        if matches!(inner, "Value" | "serde_json::Value") {
            return Some(("a free-form JSON value", String::new()));
        }
        Some(("a JSON object", schema_name_of(inner, file_code)))
    }

    /// `ty`'s name in the document: itself, or whatever its
    /// `#[schema(as = …)]` renames it to.
    fn schema_name_of(ty: &str, file_code: &str) -> String {
        let declaration = format!("struct {ty} ");
        if let Some(at) = file_code.find(&declaration) {
            let attributes = &file_code[..at];
            let needle = "#[schema(as = ";
            if let Some(from) = attributes.rfind(needle) {
                // Only the attributes of *this* declaration: anything before
                // the previous item's end belongs to another type.
                let between = &attributes[from..];
                if between.lines().count() <= 6
                    && let Some(renamed) = between[needle.len()..].split(')').next()
                {
                    return renamed.trim().to_string();
                }
            }
        }
        ty.to_string()
    }

    /// Registrations whose handler the scan cannot follow to a parameter list,
    /// each with why the row it serves is still right. Named rather than
    /// skipped, so a new unfollowable registration fails here until it is
    /// classified.
    ///
    /// All four are **closures** rather than named functions, each because the
    /// route captures something the router was built with (the configured base
    /// path, the auth state, the shell HTML) — so there is no `fn` whose
    /// parameter list could be read, and three of them take no request body at
    /// all.
    const UNRESOLVED_HANDLERS: &[(&str, &str)] = &[
        (
            "infra/auth.rs: Get /login",
            "a closure over the configured base path, rendering the sign-in page; no request body",
        ),
        (
            "infra/auth.rs: Post /login",
            "a closure over the auth state that hands the whole `Request` to `login_handler`, \
             which decodes `Form::<LoginForm>` itself — it has to own the failure to answer \
             either the page or a plain-text reason by `Accept`, so the extractor is inside the \
             body rather than in a parameter list. The row's `Form(\"LoginForm\")` is that type",
        ),
        (
            "infra/auth.rs: Post /logout",
            "a closure over the auth state; the cookie is the whole request, so no body",
        ),
        (
            "web.rs: Get /",
            "a closure over the `include_str!` shell; no request body",
        ),
    ];

    /// One hand-written route the scan resolved: the registration, and the
    /// handler function behind it.
    struct ResolvedRoute {
        site: String,
        verb: Verb,
        path: String,
        handler: String,
        /// The handler's parameter list, as written.
        params: String,
        /// Its return type, `->` to the opening brace.
        returns: String,
        /// Its body, for the success status it names explicitly.
        body: String,
        /// The whole (comment- and literal-blanked) file, for reading a
        /// `#[schema(as = …)]` off a type declared in it.
        file_code: std::rc::Rc<String>,
    }

    /// Every `.route("…", verb(handler))` registration whose handler is a named
    /// function in the same file, with that function's signature — the source
    /// the two derivation tests below read the request and response bodies out
    /// of. The second return is the registrations whose handler could not be
    /// followed, for [`UNRESOLVED_HANDLERS`] to account for.
    ///
    /// The generic CRUD handlers are skipped: they take no body and their
    /// response is their type parameter, which
    /// [`the_generic_crud_routes_derive_their_status_and_schemas`] derives.
    fn resolved_handler_routes() -> (Vec<ResolvedRoute>, Vec<String>) {
        let verbs = [
            ("get(", Verb::Get),
            ("post(", Verb::Post),
            ("put(", Verb::Put),
            ("delete(", Verb::Delete),
        ];
        let mut resolved: Vec<ResolvedRoute> = Vec::new();
        let mut unresolved: Vec<String> = Vec::new();
        for (file, raw) in crate::test_support::rust_sources() {
            // Test modules blanked first (the panic layer's `/boom` is not a
            // served route), then comments and string literals — both
            // byte-for-byte, so an offset into `code` is an offset into `raw`.
            let code = std::rc::Rc::new(crate::test_support::code_only(&strip_test_modules(&raw)));
            let needle = format!(".{}(", "route");
            let mut at = 0;
            while let Some(found) = code[at..].find(&needle) {
                let call = at + found;
                at = call + needle.len();
                let args_code = balanced(&code, call + needle.len() - 1);
                // The path is a string literal, so it is only in the raw text —
                // at the same offset, `code_only` being byte-for-byte.
                let args_raw = balanced(&raw, call + needle.len() - 1);
                let Some(path) = args_raw
                    .trim_start()
                    .strip_prefix('"')
                    .and_then(|rest| rest.split('"').next())
                else {
                    // A path built in code; `DYNAMIC_ROUTE_SITES` classifies these.
                    continue;
                };
                for (verb_needle, verb) in verbs {
                    let Some(verb_at) = args_code.find(verb_needle) else {
                        continue;
                    };
                    let handler = balanced(args_code, verb_at + verb_needle.len() - 1).trim();
                    // The generic CRUD handlers read no body and are pinned by
                    // the derivation test above.
                    if handler.contains("_handler::") {
                        continue;
                    }
                    let name = handler.rsplit("::").next().unwrap_or(handler);
                    let signature = format!("fn {name}(");
                    let site = format!("{file}: {verb:?} {path}");
                    let Some(fn_at) = code.find(&signature) else {
                        unresolved.push(site);
                        continue;
                    };
                    let params = balanced(&code, fn_at + signature.len() - 1);
                    // The return type: `->` to the body's opening brace.
                    let after = &code[fn_at + signature.len() + params.len()..];
                    let returns: String = after
                        .find("->")
                        .and_then(|arrow| {
                            after[arrow..]
                                .find('{')
                                .map(|end| &after[arrow..arrow + end])
                        })
                        .unwrap_or("")
                        .to_string();
                    resolved.push(ResolvedRoute {
                        site,
                        verb,
                        path: path.to_string(),
                        handler: name.to_string(),
                        params: params.to_string(),
                        body: block_after(
                            &code,
                            fn_at + signature.len() + params.len() + returns.len(),
                        )
                        .to_string(),
                        returns,
                        file_code: std::rc::Rc::clone(&code),
                    });
                }
            }
        }
        assert!(
            resolved.len() >= 60,
            "only {} hand-written handlers resolved — the scan has stopped parsing",
            resolved.len()
        );
        unresolved.sort();
        unresolved.dedup();
        (resolved, unresolved)
    }

    /// The row a resolved registration documents.
    fn row_for(route: &ResolvedRoute) -> &'static RouteRow {
        ROUTES
            .iter()
            .find(|(v, p, ..)| *v == route.verb && *p == route.path)
            .unwrap_or_else(|| panic!("{:?} {} has no row in ROUTES", route.verb, route.path))
    }

    /// The paths [`UNRESOLVED_HANDLERS`] excuses from a both-ways check.
    fn excused_paths() -> Vec<&'static str> {
        UNRESOLVED_HANDLERS
            .iter()
            .map(|(site, _)| site.rsplit(' ').next().unwrap_or(site))
            .collect()
    }

    /// Every registration names a handler the scan can follow, or is classified
    /// in [`UNRESOLVED_HANDLERS`] with why its row is right.
    #[test]
    fn every_registration_resolves_to_a_handler_or_is_classified() {
        let (_, unresolved) = resolved_handler_routes();
        let classified: Vec<String> = UNRESOLVED_HANDLERS
            .iter()
            .map(|(site, _)| (*site).to_string())
            .collect();
        assert_eq!(
            unresolved, classified,
            "these registrations name a handler the scan cannot follow to a parameter list; \
             classify each in UNRESOLVED_HANDLERS with why its row is right"
        );
    }

    /// Routes whose handler builds its `Response` by hand, so the return type
    /// says nothing about the body — each with what it actually answers, which
    /// is what its row records.
    ///
    /// All seven set a content type (and, for the download, a
    /// `Content-Disposition`) that no `Json`/`UpsertResponse`/`StatusCode`
    /// return could carry. There is nothing to derive for them; the point of
    /// naming them is that the list is exhaustive — a *new* hand-built response
    /// fails this test rather than joining ~110 rows nothing checks.
    const RESPONSES_NOT_DERIVABLE: &[(&str, &str)] = &[
        (
            "api_spec.rs: Get /openapi.json",
            "the OpenAPI document (or one tag's slice of it) serialised once at router build \
             time, served as an application/json string rather than re-encoded per request",
        ),
        (
            "api_spec.rs: Get /openapi/index",
            "the route index as text/plain",
        ),
        (
            "entities/attachment.rs: Get /attachments/{id}/content",
            "the stored bytes under the content type they were stored with, plus a \
             Content-Disposition the `?disposition=` parameter chooses",
        ),
        (
            "entities/attachment.rs: Post /attachments",
            "201 with the stored metadata row, built by hand because the multipart upload's \
             own failures are answered from the same function",
        ),
        (
            "reports/net_capital_gain.rs: Get /portfolio/net_capital_gain/export",
            "a text/csv download",
        ),
        (
            "reports/tax_summary.rs: Get /portfolio/tax_summary/export",
            "a text/csv download",
        ),
        (
            "web.rs: Get /static/style.css",
            "the stylesheet as text/css",
        ),
    ];

    /// The request-body half of the finding: every route's recorded request
    /// schema is the one its handler's own extractor names.
    ///
    /// The scan follows each registration to its handler and takes the
    /// `Json<T>`/`Form<T>` out of the parameter list — the type axum actually
    /// decodes, exactly as [`query_parameters`] takes the query half from the
    /// real `Query<T>`. Both directions: a row naming a schema its handler does
    /// not take fails, and so does a row that records no body for a handler
    /// that declares one.
    #[test]
    fn every_request_body_matches_its_handlers_extractor() {
        let (resolved, _) = resolved_handler_routes();
        let mut covered: Vec<(Verb, String)> = Vec::new();
        for route in &resolved {
            let recorded = body_shape(&row_for(route).4);
            match extractor_body(&route.params, &route.file_code) {
                Some((kind, ty)) => assert_eq!(
                    (recorded.0, recorded.1.map(str::to_string)),
                    (kind, Some(ty.clone())),
                    "{}: handled by `{}`, whose extractor is {kind} of {ty}",
                    route.site,
                    route.handler
                ),
                None => assert!(
                    !matches!(recorded.0, "a JSON object" | "a form body"),
                    "{} records {} of {:?}, but `{}` declares no Json/Form extractor",
                    route.site,
                    recorded.0,
                    recorded.1,
                    route.handler
                ),
            }
            covered.push((route.verb, route.path.clone()));
        }
        // …and no row claims a JSON/form body that no handler was read for.
        let excused = excused_paths();
        let unchecked: Vec<(Verb, &str)> = ROUTES
            .iter()
            .filter(|(.., request, _)| matches!(request, Body::Json(_) | Body::Form(_)))
            .map(|(verb, path, ..)| (*verb, *path))
            .filter(|(verb, path)| {
                !covered.contains(&(*verb, (*path).to_string())) && !excused.contains(path)
            })
            .collect();
        assert!(
            unchecked.is_empty(),
            "these rows record a JSON/form request body that was never read off a handler: \
             {unchecked:?}"
        );
    }

    /// The response half: every route's recorded success body is the one its
    /// handler's **return type** names.
    ///
    /// A handler that answers JSON returns `Result<Json<T>, ApiError>` — or
    /// `Json<Vec<T>>` for a list — so the type is there to be read, the same way
    /// the request half reads the extractor. Where the return type is not a
    /// `Json` there is nothing to derive: a hand-built `Response` (an asset, a
    /// redirect, a download), a `StatusCode`, or the upsert's own
    /// `Upserted`-shaped outcome. Those are the rows whose response is typed by
    /// hand, and each is classified in [`RESPONSES_NOT_DERIVABLE`] with what it
    /// answers instead.
    #[test]
    fn every_response_body_matches_its_handlers_return_type() {
        let (resolved, _) = resolved_handler_routes();
        let mut checked = 0;
        let mut hand_typed: Vec<String> = Vec::new();
        for route in &resolved {
            let row = row_for(route);
            let recorded = body_shape(&row.5);
            match returned_body(&route.returns, &route.file_code) {
                Some((kind, name)) => {
                    let recorded_name = recorded.1.unwrap_or_default().to_string();
                    assert_eq!(
                        (recorded.0, recorded_name),
                        (kind, name.clone()),
                        "{}: `{}` returns {kind}{}, so that is its documented response",
                        route.site,
                        route.handler,
                        if name.is_empty() {
                            String::new()
                        } else {
                            format!(" of {name}")
                        }
                    );
                    checked += 1;
                }
                None => hand_typed.push(route.site.clone()),
            }
        }
        assert!(
            checked >= 40,
            "only {checked} responses derived — the scan has stopped parsing"
        );
        hand_typed.sort();
        hand_typed.dedup();
        let classified: Vec<String> = RESPONSES_NOT_DERIVABLE
            .iter()
            .map(|(site, _)| (*site).to_string())
            .collect();
        assert_eq!(
            hand_typed, classified,
            "these handlers do not return a `Json<…>`, so their row's response is typed by hand; \
             classify each in RESPONSES_NOT_DERIVABLE with what it answers"
        );
    }

    /// The success statuses, derived the two ways they can be.
    ///
    /// By **verb** for the 110 rows where the verb decides it: a read answers
    /// `200` and a delete `204`, which is the API convention (docs/API.md) and
    /// the shape every handler of those verbs has. The `PUT` rows are pinned
    /// separately and twice over by
    /// [`every_put_route_documents_its_outcome`] against
    /// `entities::PUT_ROUTES`.
    ///
    /// By **handler** for the `POST`s, whose status genuinely varies — a create
    /// is `201`, a POST-for-read `200`, the job trigger `204`, a login `303` —
    /// read from the `StatusCode::…` the handler names, or `200` where it names
    /// none and simply returns its JSON.
    #[test]
    fn the_success_statuses_are_derived_from_the_verb_or_the_handler() {
        for &(verb, path, statuses, ..) in ROUTES {
            match verb {
                Verb::Get => assert_eq!(
                    statuses,
                    &[200],
                    "GET {path}: a read answers 200 (docs/API.md, Response codes)"
                ),
                Verb::Delete => assert_eq!(
                    statuses,
                    &[204],
                    "DELETE {path}: a delete answers 204 (docs/API.md, Response codes)"
                ),
                Verb::Post | Verb::Put => {}
            }
        }

        let (resolved, _) = resolved_handler_routes();
        let mut checked = 0;
        for route in resolved.iter().filter(|r| r.verb == Verb::Post) {
            // Every status the handler names, ascending — a handler choosing
            // between two (a create or a replace, a write or a preview) names
            // both, and the row must carry both.
            let named: Vec<u16> = [
                ("StatusCode::OK", 200u16),
                ("StatusCode::CREATED", 201),
                ("StatusCode::NO_CONTENT", 204),
                ("StatusCode::SEE_OTHER", 303),
            ]
            .into_iter()
            .filter(|(spelling, _)| {
                route.body.contains(spelling) || route.returns.contains(spelling)
            })
            .map(|(_, status)| status)
            .collect();
            // No status in the handler at all: it returns its JSON, which axum
            // answers 200 with.
            let derived = if named.is_empty() { vec![200] } else { named };
            assert_eq!(
                row_for(route).2,
                derived.as_slice(),
                "{}: `{}` answers {derived:?}",
                route.site,
                route.handler
            );
            checked += 1;
        }
        assert!(
            checked >= 30,
            "only {checked} POST handlers checked — the scan has stopped parsing"
        );
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
    /// The bodies are read out of [`ROUTES`] rather than transcribed, so a
    /// route added with a new body schema is covered the moment it is
    /// registered — there is no list here to forget to update.
    #[test]
    fn every_request_body_schema_denies_unknown_fields() {
        let doc = doc();
        let schemas = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas is an object");
        let mut bodies: Vec<&str> = ROUTES
            .iter()
            .filter_map(|(_, _, _, _, request, _)| match request {
                Body::Json(name) | Body::Form(name) => Some(*name),
                _ => None,
            })
            .collect();
        bodies.sort_unstable();
        bodies.dedup();
        assert!(
            bodies.len() > 30,
            "only {} request bodies were read out of the route table — the table has stopped \
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
        for (_, _, _, _, request, _) in ROUTES {
            let (Body::Json(name) | Body::Form(name)) = request else {
                continue;
            };
            assert_eq!(
                schemas[*name]["additionalProperties"], false,
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
        for (code, has_body) in crate::infra::http::documented_error_shapes() {
            if !has_body {
                continue;
            }
            // The 500 is named through the shape it comes from (the manual job
            // trigger), not as a bare code — which the empty-bodied 500 shares.
            let needle = if code == 500 {
                "a failed POST /jobs/{name}'s 500".to_string()
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
        // by *what the URL addresses*, not by the verb: a GET aimed at one
        // missing row is empty, while a read whose parameter names a missing row
        // carries the reason (`a_read_whose_parameter_names_a_missing_row_…`
        // drives that one).
        assert!(description.contains(
            "a deliberately empty body: the 404 of a GET addressed at one missing row, \
             a 405, and an internal 500"
        ));
        assert!(
            description.contains(
                "404 on a delete, an operation, or a read whose parameter names a missing row"
            ),
            "info.description must not claim every GET's 404 is empty: {description}"
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
    /// the POST-bodied report reads are read out of [`ROUTES`] (below), and the
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
            crate::reports::row_history::MAX_BROWSE_LIMIT,
            crate::reports::row_history::DEFAULT_BROWSE_LIMIT
        );
        assert!(
            description.contains(&bounds),
            "info.description must state `{bounds}`; got:\n{description}"
        );
        // The POST-bodied report reads are **derived from the route table**,
        // not from a copy of the sentence that names them: the compact contract
        // must name every one, and no new `POST /portfolio/*` report read can
        // quietly join the set without being documented here.
        let mut posts: Vec<String> = ROUTES
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
        let sections: std::collections::HashSet<&str> = api_md
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
        let mut ids = std::collections::BTreeMap::new();
        let mut used = std::collections::BTreeSet::new();
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
                if let Some(other) = ids.insert(id.to_string(), format!("{method} {path}")) {
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
        let mut seen = std::collections::HashSet::new();
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
        let doc = serde_json::to_value(document_for("", false)).expect("the document serialises");
        let listed: Vec<&str> = doc["tags"]
            .as_array()
            .expect("the document lists its tags")
            .iter()
            .filter_map(|tag| tag["name"].as_str())
            .collect();
        assert!(!listed.contains(&"Authentication"), "{listed:?}");
        assert!(listed.contains(&"Trades"), "{listed:?}");
    }

    /// The **emitted** document carries every route's success body: each
    /// `ROUTES` row's response [`Body`] appears under each of its statuses that
    /// has a body (`200`, `201`), and a `204` or `303` carries no `content`.
    ///
    /// The other scans pin the *table* against the handler types; this one
    /// reads the served document back, which is what caught every `200` (every
    /// list, GET-one and report) published as a bare `"OK"` with no schema —
    /// `operation` attached the body to a `201` only. The expected shape is
    /// spelt out per `Body` variant here rather than by calling
    /// `success_response`, so a builder bug cannot agree with itself.
    #[test]
    fn every_success_response_carries_its_routes_body() {
        let doc = doc();
        let mut checked = 0;
        for &(verb, path, statuses, _, _, response) in ROUTES {
            let method = match verb {
                Verb::Get => "get",
                Verb::Post => "post",
                Verb::Put => "put",
                Verb::Delete => "delete",
            };
            for status in statuses {
                let documented = &doc["paths"][path][&method]["responses"][status.to_string()];
                assert!(
                    documented.is_object(),
                    "{method} {path} does not document {status}"
                );
                let content = documented.get("content");
                if matches!(status, 204 | 303) || matches!(response, Body::None) {
                    assert!(
                        content.is_none(),
                        "{method} {path}'s {status} must carry no body, found {content:?}"
                    );
                    continue;
                }
                let content = content
                    .unwrap_or_else(|| panic!("{method} {path}'s {status} has lost its body"));
                let json_schema = &content["application/json"]["schema"];
                match response {
                    Body::None | Body::Form(_) => unreachable!("not a response body"),
                    Body::Json(name) => assert_eq!(
                        json_schema["$ref"],
                        format!("#/components/schemas/{name}"),
                        "{method} {path}'s {status}"
                    ),
                    Body::JsonArray(name) => {
                        assert_eq!(json_schema["type"], "array", "{method} {path}'s {status}");
                        assert_eq!(
                            json_schema["items"]["$ref"],
                            format!("#/components/schemas/{name}"),
                            "{method} {path}'s {status}"
                        );
                    }
                    Body::JsonIntegers => {
                        assert_eq!(json_schema["type"], "array", "{method} {path}'s {status}");
                        assert_eq!(
                            json_schema["items"]["type"], "integer",
                            "{method} {path}'s {status}"
                        );
                    }
                    Body::JsonFree(_) => {
                        assert_eq!(json_schema["type"], "object", "{method} {path}'s {status}")
                    }
                    Body::Other(media_type) => assert!(
                        content.get(media_type).is_some(),
                        "{method} {path}'s {status} must carry {media_type}"
                    ),
                }
                checked += 1;
            }
        }
        // Lists, GET-ones and reports: far more than the 201s alone, which is
        // what the bug left documented.
        assert!(checked > 100, "only {checked} success bodies checked");
    }

    /// Every property of every schema a request body reaches — the bodies
    /// `ROUTES` names and every schema they `$ref`, transitively (a Sell's
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
                        refs.push(r.trim_start_matches("#/components/schemas/").to_string());
                    }
                    if let Some(properties) = map.get("properties") {
                        props.push(properties);
                    }
                    map.values().for_each(|c| walk(c, refs, props));
                }
                Value::Array(items) => items.iter().for_each(|c| walk(c, refs, props)),
                _ => {}
            }
        }

        let doc = doc();
        let schemas = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas is an object");
        let mut queue: Vec<String> = ROUTES
            .iter()
            .filter_map(|&(_, _, _, _, request, _)| match request {
                Body::Json(name) | Body::JsonArray(name) | Body::Form(name) => {
                    Some(name.to_string())
                }
                _ => None,
            })
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        let mut undescribed = std::collections::BTreeSet::new();
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
        // empty walk (a renamed `ROUTES` variant) cannot pass vacuously.
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
        use crate::entities::{PUT_ROUTES, PutOutcome};

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
                None => panic!("PUT {path} is not classified in entities::PUT_ROUTES"),
            }
        };
        let doc = doc();
        let mut puts = 0;
        for &(verb, path, statuses, _, _, _) in ROUTES {
            if verb != Verb::Put {
                continue;
            }
            puts += 1;
            assert_eq!(
                statuses,
                expected_statuses(path),
                "PUT {path} records statuses that disagree with its entities::PUT_ROUTES \
                 classification"
            );
            let responses = doc["paths"][path]["put"]["responses"]
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
             new `PUT` route must be classified in entities::PUT_ROUTES"
        );
    }

    /// The PUT outcome rule rides in `info.description`, the machine-client
    /// surface, so a client reading only the generated document learns that a
    /// `PUT` reports whether it created or replaced.
    ///
    /// The two single-status exceptions are **read out of
    /// `entities::PUT_ROUTES`** rather than transcribed: a third one, or a
    /// reclassified existing one, changes what the description has to say and
    /// fails here until it does. The general rule stays a prose pin — it is
    /// pinned behaviourally by `entities::tests::every_put_route_reports_create_then_replace`
    /// and structurally by [`every_put_route_documents_its_outcome`].
    #[test]
    fn the_put_outcome_rule_is_stated_in_the_description() {
        use crate::entities::{PUT_ROUTES, PutOutcome};
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
        let login = ROUTES
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
            vec!["POST /login".to_string()],
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
        let mut by_name: std::collections::BTreeMap<&str, &RefOr<Schema>> =
            std::collections::BTreeMap::new();
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
                unresolved.push(name.to_string());
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
                        .filter_map(|p| p["name"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        for (path, params) in crate::entities::filtered_list_routes() {
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

    /// Every `?name=` a route summary mentions is a parameter that route's
    /// `Query<T>` type really has, and every parameter the type has is
    /// mentioned. The document is generated from the type (so a client gets the
    /// truth), and this is what stops the prose beside it drifting into
    /// describing a parameter that does not exist — which is how the first
    /// version of this feature shipped two snapshot routes' parameters swapped.
    #[test]
    fn every_query_parameter_matches_its_summary() {
        for &(verb, path, _, summary, _, _) in ROUTES {
            let mut declared: Vec<String> = super::query_parameters(verb, path)
                .iter()
                .map(|p| p.name.clone())
                .collect();
            let mut mentioned = super::summary_query_names(summary);
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
        assert_eq!(listing_id["schema"]["type"], serde_json::json!("integer"));
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
        let rooted = serde_json::to_value(document_for("", true)).expect("serialises");
        assert_eq!(rooted["servers"][0]["url"], "/");
        assert!(rooted["components"]["securitySchemes"]["bearerAuth"].is_object());
        assert!(rooted["components"]["securitySchemes"]["sessionCookie"].is_object());
        assert!(
            rooted["security"].as_array().is_some_and(|s| !s.is_empty()),
            "an auth deployment must carry a global security requirement"
        );

        let prefixed = serde_json::to_value(document_for("/share_tracker", true)).expect("ser");
        assert_eq!(
            prefixed["servers"][0]["url"], "/share_tracker",
            "a base-path deployment must publish the prefix its paths resolve against"
        );

        let open = serde_json::to_value(document_for("", false)).expect("serialises");
        assert!(
            open["components"]
                .get("securitySchemes")
                .is_none_or(|s| s.as_object().is_none_or(|m| m.is_empty())),
            "an auth-less deployment must declare no security scheme"
        );
        assert!(open["paths"].get("/login").is_none());
        assert!(open["paths"].get("/logout").is_none());
    }

    /// No `(verb, path)` is registered twice: `Paths::add_path_operation`
    /// silently overwrites the earlier row, and the coverage test reads the
    /// collapsed map, so a duplicate would document only the later row with
    /// nothing failing.
    #[test]
    fn every_route_row_is_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for (verb, path, ..) in ROUTES {
            assert!(
                seen.insert((format!("{verb:?}"), *path)),
                "ROUTES names `{path}` twice for {verb:?}; the later row silently overwrites \
                 the earlier in the document"
            );
        }
    }

    /// Serve check: the full application answers `GET /openapi.json` with the
    /// document, through `ApiClient::full` (whose auth-less router serves it).
    #[tokio::test]
    async fn get_openapi_json_serves_the_document() {
        let pool = crate::test_support::test_pool().await;
        let client = crate::test_support::ApiClient::full(&pool);
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
        for auth in [true, false] {
            let doc = serde_json::to_value(document_for("", auth)).expect("serialises");
            let index = route_index("", auth);
            let mut listed: Vec<(String, String)> = Vec::new();
            let mut heading = String::new();
            for line in index.lines() {
                if let Some(rest) = line.strip_prefix("## ") {
                    heading = rest.split(" — ").next().unwrap_or_default().to_string();
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
                listed.push((path.to_string(), verb.to_string()));
            }
            listed.sort();
            let before = listed.len();
            listed.dedup();
            assert_eq!(before, listed.len(), "the index lists an operation twice");
            assert_eq!(listed, documented_routes(&doc), "auth = {auth}");
            assert_eq!(
                listed.iter().any(|(path, _)| path == "/login"),
                auth,
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
        fn refs(value: &Value, out: &mut std::collections::BTreeSet<String>) {
            match value {
                Value::Object(map) => {
                    for (key, child) in map {
                        if key == "$ref" {
                            out.insert(child.as_str().unwrap_or_default().to_string());
                        }
                        refs(child, out);
                    }
                }
                Value::Array(items) => items.iter().for_each(|item| refs(item, out)),
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

        let mut found = std::collections::BTreeSet::new();
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
        let carried: std::collections::BTreeSet<String> = slice["components"]["schemas"]
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
                .to_string();
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
        let pool = crate::test_support::test_pool().await;
        let client = crate::test_support::ApiClient::full(&pool);
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
        assert_eq!(unknown.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(
            unknown.text().contains("no operation is tagged `Nope`"),
            "{}",
            unknown.text()
        );
        assert!(unknown.text().contains("Trades"), "{}", unknown.text());

        let typo = client.get("/openapi.json?tags=Trades").await;
        assert_eq!(typo.status, axum::http::StatusCode::BAD_REQUEST);

        let one: Value = client.get_json("/openapi.json?operation=createSells").await;
        assert_eq!(
            documented_routes(&one),
            [("/sells".to_string(), "POST".to_string())]
        );
        assert!(one["components"]["schemas"]["SellBody"].is_object());
        let unknown = client.get("/openapi.json?operation=nope").await;
        assert_eq!(unknown.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(unknown.text().contains("`nope`"), "{}", unknown.text());
        let both = client
            .get("/openapi.json?operation=createSells&tag=Sells")
            .await;
        assert_eq!(both.status, axum::http::StatusCode::BAD_REQUEST);

        let index = client.get("/openapi/index").await;
        assert_eq!(index.status, axum::http::StatusCode::OK);
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
    /// `entities::LIST_ROUTES`**, both ways, rather than transcribed: every list
    /// that takes a filter is named in the paragraph and every list that takes
    /// none is not, and every filter name in the table is spelled `?name=`
    /// there. So a new filtered list, a new filter, or a filter removed from a
    /// list fails here until the paragraph says so — which is the half a pin on
    /// the sentences alone could never catch.
    #[test]
    fn the_list_filtering_contract_is_stated_in_the_description() {
        use crate::entities::LIST_ROUTES;
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
    /// `entities::filtered_list_routes()` (the classification table the
    /// filtering tests drive), not transcribed here, so a filter added to an
    /// entity fails until its summary documents it.
    #[test]
    fn every_filtered_list_summary_names_its_filters() {
        let filtered = crate::entities::filtered_list_routes();
        assert!(
            !filtered.is_empty(),
            "the entity filter classification table came back empty"
        );
        for (path, params) in filtered {
            let row = ROUTES
                .iter()
                .find(|(verb, p, _, _, _, _)| *verb == Verb::Get && *p == path)
                .unwrap_or_else(|| panic!("no GET route is documented at {path}"));
            let summary = row.3;
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
            let row = ROUTES
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
        let open_parcels = ROUTES
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

    /// The other half of the `404` contract: a read whose **parameter** names a
    /// row that is not there answers a `404` **with** a plain-text reason, not
    /// the empty body a `GET`-one gives. `GET /portfolio/activity?listing_id=`
    /// is the only such read, and the Error-body matrix used to say flatly that
    /// "a `GET`'s 404" is empty — so a client generated from the document
    /// discarded "listing 42 not found" as a body that could not exist.
    ///
    /// The scan keeps that "only" honest: a new report that answers
    /// `ApiError::not_found` is a second case the matrix would not cover, so it
    /// fails here until it is classified.
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

        // Exhaustiveness: `src/reports` may answer a text 404 from exactly one
        // file. Recursive, because a report that outgrows one file becomes a
        // directory (`trade.rs`/`closing_price.rs` set that precedent).
        fn sources(dir: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("src/reports must be readable") {
                let path = entry.expect("a readable dir entry").path();
                if path.is_dir() {
                    sources(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let body = std::fs::read_to_string(&path).expect("a readable source");
                    // The declaration itself lives in `infra::http`; here only
                    // call sites matter.
                    if body.contains("ApiError::not_found(") {
                        found.push(
                            path.file_name()
                                .expect("a named file")
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                }
            }
        }
        let mut found = Vec::new();
        sources(std::path::Path::new("src/reports"), &mut found);
        found.sort();
        assert_eq!(
            found,
            vec!["activity.rs".to_string()],
            "a report answering a text 404 must be named in the Error-body \
             matrix's text-carrying 404 row, then added here"
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
        for &(verb, path, _, _, request, _) in ROUTES {
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
                    resp.text().to_string()
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

    /// Every `GET`-one answers a missing key with a bare `404` whose body is
    /// **empty** — the contract the web UI and a machine client both read as
    /// "no such row", and the one `72046a0` moved `GET /rights_sales/{id}` onto.
    /// Driven from the route table, so a new GET-one is covered without a
    /// hand-kept list.
    #[tokio::test]
    async fn every_get_one_route_answers_the_empty_404() {
        use crate::test_support::{ApiClient, test_pool};
        use axum::http::StatusCode;

        let pool = test_pool().await;
        let client = ApiClient::full(&pool);
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
                resp.text(),
                "",
                "a GET-one's 404 must have an empty body: GET {uri}"
            );
            checked += 1;
        }
        assert!(
            checked >= 20,
            "the walk found only {checked} GET-one routes — it has stopped reading the table"
        );
    }
}
