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

use axum::{Json, Router, routing::get};
use sqlx::SqlitePool;
use utoipa::openapi::{
    Components, ComponentsBuilder, InfoBuilder, OpenApi, OpenApiBuilder, Paths, RefOr, Required,
    content::ContentBuilder,
    path::{HttpMethod, Operation, OperationBuilder, Parameter, ParameterBuilder, ParameterIn},
    request_body::{RequestBody, RequestBodyBuilder},
    response::{Response, ResponseBuilder, ResponsesBuilder},
    schema::{AdditionalProperties, ArrayBuilder, ObjectBuilder, Ref, Schema, Type},
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
a tax figure. Query parameters follow the same rule on the routes that take \
them.

Errors are never JSON. A rejected request answers either a text/plain; \
charset=utf-8 body with the reason — 400 (a malformed path parameter, query \
string or body), 401, 404 on a delete or operation, 413, 415 (a JSON body sent \
without Content-Type: application/json), 422, a failed POST /jobs/{name}'s 500, \
502, 503 — or a deliberately empty body: a GET's 404, a 405, and an internal \
500. docs/API.md's \"Error-body contract\" section carries the full matrix.

Reading a collection is one more contract, stated here and in docs/API.md's \
\"Reading a list\" section. First, list endpoints return rows ascending — by \
id, by natural key, or by date then id — except the deliberate newest-first \
browse surfaces (/closing_prices, /distribution_events, a listing's rename \
chain, a job's run history, and the /reports/row_history trail). This is the \
reverse of the web UI, which sorts its own tables newest-first client-side. \
Second, a read is GET with its parameters in the query string; only four \
report reads keep a POST body, because their parameter is a map or a list a \
query string cannot carry: /portfolio/overview, /portfolio/performance and \
/portfolio/unrealised-gains (a prices price-override map) and \
/portfolio/net-capital-gain/what-if (an allocations list). Third, \
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
/parcel_allocations ?sale_trade_id= and ?purchase_trade_id=. The \
long-standing /closing_prices (?listing_id=, ?from=, ?to=, ?status= — ok or \
error, omitted is every row, so a valuation client gets the clean series in \
one call) and /attachments \
(owner ids, include_linked) filters are unchanged. An unrecognised \
parameter is a 400 naming it on every list route that decodes a query \
string — including a list that accepts no filter, which refuses any \
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
src/api_spec.rs and is pinned by that module's tests.";

/// The four HTTP verbs the server uses, as the table spells them.
#[derive(Clone, Copy, PartialEq, Eq)]
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
        &[201],
        "Create a Sell with its parcel allocations; the created row is returned as GET /trades/{id} would present it.",
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
        "List every interest-income row, optionally narrowed by ?holding_account_id= and an inclusive ?from=/?to= over date_paid.",
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
        "List every investment-expense row, optionally narrowed by ?listing_id=, ?holding_account_id= and an inclusive ?from=/?to= over date_incurred.",
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
        &[201],
        "Execute a transfer: the Sell and transfer-in Buys are written atomically and the whole group is returned.",
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
        "List every rights sale.",
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
        "List every exchange holiday.",
        Body::None,
        Body::JsonArray("ExchangeHoliday"),
    ),
    (
        Verb::Get,
        "/exchange_holidays/{mic}",
        &[200],
        "List one exchange's holiday calendar.",
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
        "Correct the stored rate at this id (the only field a correction may change). Always 204: this route can never create a row — new rates arrive through POST /rba_fx_rates/import.",
        Body::Json("CorrectionBody"),
        Body::None,
    ),
    (
        Verb::Post,
        "/rba_fx_rates/import",
        &[200],
        "Import the RBA F11 rates; the body is a bare (text) CSV document, and an empty body fetches the live source.",
        Body::Other("text/plain"),
        Body::Json("RbaImportSummary"),
    ),
    (
        Verb::Get,
        "/closing_prices",
        &[200],
        "List stored closing prices, optionally narrowed by ?listing_id= / ?from= / ?to= / ?status= (ok or error; omitted is every row).",
        Body::None,
        Body::JsonArray("ClosingPrice"),
    ),
    (
        Verb::Post,
        "/closing_prices/fetch",
        &[201],
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
        "List stored attachments, optionally filtered by owner (?trade_id=, ?income_id=, ...).",
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
        &[201],
        "Generate the AMIT adjustments an AMMA statement implies (201; a preview run answers 200 with the same body and writes nothing).",
        Body::Json("AmitGenerateBody"),
        Body::Json("GeneratedAdjustments"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/participate",
        &[201],
        "Participate in a buy-back: the closing Sell and its dividend-component income row are written together and returned.",
        Body::Json("ParticipationBody"),
        Body::Json("Participation"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/demerge",
        &[201],
        "Apportion the head listing's open parcels between head and demerged listing, and return the created replacement parcels.",
        Body::None,
        Body::Json("Demerge"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/exchange",
        &[201],
        "Substitute every open parcel of a scrip-for-scrip action's original listing, and return the created replacement parcels.",
        Body::None,
        Body::Json("ScripExchange"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/exercise",
        &[201],
        "Exercise a rights issue into a new Buy carrying the rights cost, and return the created trade.",
        Body::Json("ExerciseBody"),
        Body::Json("Trade"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/sell_rights",
        &[201],
        "Sell the rights of a renounceable issue and return the created rights sale.",
        Body::Json("SellRightsBody"),
        Body::Json("RightsSale"),
    ),
    (
        Verb::Post,
        "/corporate_actions/{id}/recognise",
        &[201],
        "Recognise a worthless-shares loss by closing every open parcel at nil, and return the closing Sell.",
        Body::None,
        Body::Json("Recognise"),
    ),
    (
        Verb::Post,
        "/income/{id}/reinvest",
        &[201],
        "Create the DRP trade for a distribution and link it; the created trade is returned.",
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
        &[201],
        "Vest an ESS statement into the cost-base-reset Buy it implies, and return the created trade.",
        Body::None,
        Body::Json("Trade"),
    ),
    (
        Verb::Post,
        "/listings/{id}/rename",
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
        "Verify credentials; 303 to the home path with the session cookie, or 200 re-rendering the page with the error. Unauthenticated: on the allowlist.",
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
        "This OpenAPI 3.1 document, generated from the route table and the serde structs. Behind [auth] like every other route.",
        Body::None,
        Body::JsonFree("The OpenAPI 3.1 document for this API, as described by info above."),
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
        "/portfolio/period-performance",
        &[200],
        "Portfolio return over a period, with FX attribution; ?from= / ?to= bound the window.",
        Body::None,
        Body::Json("PeriodPerformance"),
    ),
    (
        Verb::Get,
        "/portfolio/activity",
        &[200],
        "One listing's activity ledger; ?listing_id= is required, with optional ?price= and as-of bounds.",
        Body::None,
        Body::Json("ActivityResponse"),
    ),
    (
        Verb::Get,
        "/portfolio/open-parcels",
        &[200],
        "Every open parcel, per parcel rather than aggregated; ?as_of_date= is the valuation date (omitted = today's live position).",
        Body::None,
        Body::JsonArray("OpenParcel"),
    ),
    (
        Verb::Post,
        "/portfolio/unrealised-gains",
        &[200],
        "Unrealised gains and losses; body carries the optional price-override map, live flag and as_of_date (omitted = today's live position).",
        Body::Json("UnrealisedGainsRequest"),
        Body::JsonArray("UnrealisedGain"),
    ),
    (
        Verb::Get,
        "/portfolio/realised-gains",
        &[200],
        "Realised gains and losses per disposal.",
        Body::None,
        Body::JsonArray("RealisedGainLoss"),
    ),
    (
        Verb::Get,
        "/portfolio/net-capital-gain",
        &[200],
        "Net capital gain per financial year, with the discount and loss-netting order applied.",
        Body::None,
        Body::JsonArray("NetCapitalGainYear"),
    ),
    (
        Verb::Get,
        "/portfolio/net-capital-gain/export",
        &[200],
        "The net capital gain report as CSV.",
        Body::None,
        Body::Other("text/csv"),
    ),
    (
        Verb::Post,
        "/portfolio/net-capital-gain/what-if",
        &[200],
        "What-if net capital gain for a contemplated disposal; the body's allocations are a list of per-parcel inputs.",
        Body::Json("WhatIfRequest"),
        Body::Json("WhatIfResponse"),
    ),
    (
        Verb::Get,
        "/portfolio/parcel-optimiser",
        &[200],
        "Which parcels to sell for a target, under a chosen strategy; ?listing_id=, ?units=, ?sale_date=, ?price= and ?holding_account_id= drive it.",
        Body::None,
        Body::Json("OptimiserResponse"),
    ),
    (
        Verb::Get,
        "/portfolio/tax-summary",
        &[200],
        "The per-financial-year tax summary.",
        Body::None,
        Body::JsonArray("TaxYearSummary"),
    ),
    (
        Verb::Get,
        "/portfolio/tax-summary/export",
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
        "/reports/franking_at_risk/what-if",
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
        "The Annual Tax Report for one financial year; ?tax_year= selects it, defaulting to the one in progress.",
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
        "One report's stored series over a date range; ?report= is required.",
        Body::None,
        Body::JsonArray("SeriesPoint"),
    ),
    (
        Verb::Get,
        "/report_snapshots/holding-series",
        &[200],
        "One holding's stored valuation series; ?listing_id= and ?holding_account_id= drive it.",
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

/// The OpenAPI 3.1 document, assembled from [`ROUTES`] and the schemas the
/// referenced types derive.
pub fn document() -> OpenApi {
    let mut paths = Paths::new();
    for &(verb, path, statuses, summary, request, response) in ROUTES {
        let operation = operation(path, statuses, summary, request, response);
        paths.add_path_operation(path, vec![verb.http()], operation);
    }
    // The `/static/*.js` routes are registered in a loop over `JS_MODULES`,
    // so no literal path string exists to scan; read them from that same
    // list rather than transcribing the paths.
    for (path, _) in crate::web::JS_MODULES {
        let operation = operation(
            path,
            &[200],
            "A served frontend ES module (JavaScript).",
            Body::None,
            Body::Other("text/javascript"),
        );
        paths.add_path_operation(path, vec![HttpMethod::Get], operation);
    }
    OpenApiBuilder::new()
        .info(
            InfoBuilder::new()
                .title("share-tracker API")
                .version(env!("CARGO_PKG_VERSION"))
                .description(Some(DESCRIPTION))
                .build(),
        )
        .paths(paths)
        .components(Some(components()))
        .build()
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
fn components() -> Components {
    let mut unique: Vec<(String, RefOr<Schema>)> = Vec::new();
    for (name, schema) in component_schemas() {
        match unique.iter().find(|(existing, _)| *existing == name) {
            // Two distinct Rust types with the same name would silently
            // overwrite one another in the component map, and whichever
            // `$ref` lost would describe the wrong fields. Rename one with
            // `#[schema(as = …)]`; the uniqueness test names the pair.
            Some((_, existing)) => assert!(
                *existing == schema,
                "two different schemas are registered under the component name `{name}`"
            ),
            None => unique.push((name, schema)),
        }
    }
    ComponentsBuilder::new().schemas_from_iter(unique).build()
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
/// because it reports whether it created or replaced. A `201` is the only
/// status that carries `response`; a `204` never has a body.
fn operation(
    path: &str,
    statuses: &[u16],
    summary: &str,
    request: Body,
    response: Body,
) -> Operation {
    let mut responses = ResponsesBuilder::new();
    for &status in statuses {
        let body = if status == 201 { response } else { Body::None };
        responses = responses.response(status.to_string(), success_response(status, body));
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
    }
    let mut builder = OperationBuilder::new()
        .summary(Some(summary))
        .parameters(Some(path_parameters(path)))
        .responses(responses.build());
    if let Some(body) = request_body(request) {
        builder = builder.request_body(Some(body));
    }
    builder.build()
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

/// The document as JSON — what `GET /openapi.json` answers.
async fn openapi_json() -> Json<OpenApi> {
    Json(document())
}

/// `GET /openapi.json`. Merged into `app::router` like every other router, so
/// `[auth]`'s `require_auth` gates it and it is deliberately absent from that
/// layer's login-page allowlist.
pub fn router() -> Router<SqlitePool> {
    Router::new().route("/openapi.json", get(openapi_json))
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

    /// Strip `#[cfg(test)] mod …{ … }` blocks, so a route registered by a
    /// test (the panic layer's `/boom`) is not mistaken for a served one.
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
                    out.push('\n');
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
    }

    /// The error-body matrix — the never-JSON rule, the media type, and every
    /// status's body shape — rides in `info.description`, which is the
    /// machine-client surface. `docs/API.md`'s "Error-body contract" section
    /// is the long form (`doc_checks` pins that copy); this is the compact
    /// twin a client reading only the generated document gets, and it must
    /// name each status so a dropped one fails here.
    #[test]
    fn the_error_body_matrix_is_stated_in_the_description() {
        let doc = doc();
        let description = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        for rule in [
            "Errors are never JSON.",
            "text/plain; charset=utf-8",
            // The text-carrying statuses, the job trigger's 500 among them.
            "400 (a malformed path parameter, query string or body), 401, 404 on a delete or \
             operation, 413, 415 (a JSON body sent without Content-Type: application/json), 422, a \
             failed POST /jobs/{name}'s 500, 502, 503",
            // …and the empty-bodied ones, both 404s and both 500s named apart.
            "a deliberately empty body: a GET's 404, a 405, and an internal 500",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }
    }

    /// The reading-a-list contract — ascending order with its newest-first
    /// browse exceptions, the four report reads that keep a POST body, and the
    /// one paginated endpoint with both its shapes — rides in
    /// `info.description`, the machine-client surface. `docs/API.md`'s
    /// "Reading a list" section is the long form (`doc_checks` pins that copy);
    /// this is the compact twin a client reading only the generated document
    /// gets, and it must name each endpoint so a dropped one fails here.
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
            // The POST-for-read set, each endpoint named.
            "/portfolio/overview, /portfolio/performance and /portfolio/unrealised-gains (a \
             prices price-override map) and /portfolio/net-capital-gain/what-if (an allocations \
             list)",
            // Pagination: the one endpoint, both shapes and the cursor facts.
            "/reports/row_history is the only paginated endpoint",
            "with row_id it answers that row's whole trail as a bare JSON array",
            "before_id returns entries older than that trail id and limit is 1-1000 (default 100)",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }
    }

    /// Every `PUT` route records the statuses it can answer: the create/replace
    /// pair `[201, 204]` for the upserts, and the single status the two
    /// documented exceptions can give. A new `PUT` route that records neither
    /// fails here, so the create-vs-replace signal cannot go missing from the
    /// machine-client contract.
    #[test]
    fn every_put_route_documents_its_outcome() {
        // The two `PUT`s that deliberately do not report the pair, with the one
        // status each can answer.
        const EXCEPTIONS: &[(&str, &[u16])] = &[
            // Create-only: always 201 with the executed group.
            ("/transfers/{id}", &[201]),
            // A correction of an existing row: never creates, so always 204.
            ("/rba_fx_rates/{id}", &[204]),
        ];
        let doc = doc();
        let mut puts = 0;
        for &(verb, path, statuses, _, _, _) in ROUTES {
            if verb != Verb::Put {
                continue;
            }
            puts += 1;
            match EXCEPTIONS.iter().find(|(p, _)| *p == path) {
                Some((_, expected)) => assert_eq!(
                    statuses, *expected,
                    "PUT {path} is a classified exception with the wrong statuses"
                ),
                None => assert_eq!(
                    statuses,
                    &[201, 204],
                    "PUT {path} must record 201 Created then 204 No Content"
                ),
            }
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
            puts, 20,
            "the route table should carry every PUT route; a new one must be classified here"
        );
    }

    /// The PUT outcome rule rides in `info.description`, the machine-client
    /// surface, so a client reading only the generated document learns that a
    /// `PUT` reports whether it created or replaced.
    #[test]
    fn the_put_outcome_rule_is_stated_in_the_description() {
        let doc = doc();
        let description = doc["info"]["description"]
            .as_str()
            .expect("info.description is a string");
        for rule in [
            "A PUT upsert reports its outcome.",
            "answers 201 Created carrying the created row",
            "204 No Content when it replaced an existing row",
            "/transfers/{id} is create-only and always answers 201",
            "/rba_fx_rates/{id} only ever corrects an existing row, so always answers 204",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }
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

    /// The filtering half of the reading-a-list contract — which entity list
    /// takes which query parameters, and that an unrecognised one is a `400`
    /// whether the list filters or not — rides in `info.description`, the
    /// machine-client surface. `docs/API.md`'s "Reading a list" section is the
    /// long form (`doc_checks` pins that copy); this is the compact twin a
    /// client reading only the generated document gets.
    #[test]
    fn the_list_filtering_contract_is_stated_in_the_description() {
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
             error, omitted is every row, so a valuation client gets the clean series in one \
             call) and /attachments (owner ids, include_linked) filters are unchanged.",
            "An unrecognised parameter is a 400 naming it on every list route that decodes a \
             query string",
            "including a list that accepts no filter, which refuses any parameter at all rather \
             than silently ignoring it.",
        ] {
            assert!(
                description.contains(rule),
                "info.description must state `{rule}`; got:\n{description}"
            );
        }
    }

    /// Every filtered list's summary names the parameters it accepts — the
    /// per-route half of the same contract, and the one a client browsing
    /// `paths` reads. Driven by a table, and each entry must be a real `GET`
    /// in [`ROUTES`], so a renamed or dropped route fails here rather than
    /// silently passing.
    #[test]
    fn every_filtered_list_summary_names_its_filters() {
        const FILTERED: &[(&str, &[&str])] = &[
            ("/listings", &["?exchange_mic=", "?security_type="]),
            (
                "/trades",
                &["?listing_id=", "?holding_account_id=", "?from=", "?to="],
            ),
            (
                "/income",
                &["?listing_id=", "?holding_account_id=", "?from=", "?to="],
            ),
            (
                "/interest_income",
                &["?holding_account_id=", "?from=", "?to="],
            ),
            (
                "/investment_expenses",
                &["?listing_id=", "?holding_account_id=", "?from=", "?to="],
            ),
            (
                "/amma_statements",
                &["?listing_id=", "?holding_account_id=", "?from=", "?to="],
            ),
            (
                "/ess_statements",
                &["?listing_id=", "?holding_account_id=", "?from=", "?to="],
            ),
            (
                "/inheritances",
                &["?listing_id=", "?holding_account_id=", "?from=", "?to="],
            ),
            ("/corporate_actions", &["?listing_id=", "?from=", "?to="]),
            ("/transfers", &["?listing_id=", "?from=", "?to="]),
            ("/distribution_events", &["?listing_id=", "?from=", "?to="]),
            ("/drp_enrolments", &["?listing_id=", "?holding_account_id="]),
            ("/amit_adjustments", &["?amma_statement_id=", "?trade_id="]),
            (
                "/parcel_allocations",
                &["?sale_trade_id=", "?purchase_trade_id="],
            ),
            // The long-standing filters the hand-written lists already took; the
            // closing-price list's `?status=` is the 2026-09-24 REST-audit item
            // B7 (the errored-row filter).
            (
                "/closing_prices",
                &["?listing_id=", "?from=", "?to=", "?status="],
            ),
            ("/attachments", &["?trade_id="]),
        ];
        let mut checked = 0;
        for (path, params) in FILTERED {
            let row = ROUTES
                .iter()
                .find(|(verb, p, _, _, _, _)| *verb == Verb::Get && p == path)
                .unwrap_or_else(|| panic!("no GET route is documented at {path}"));
            let summary = row.3;
            for param in *params {
                assert!(
                    summary.contains(param),
                    "the {path} summary must name {param}: {summary}"
                );
            }
            checked += 1;
        }
        assert_eq!(
            checked, 16,
            "every filtered list route must be named here, and no other"
        );
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
            "/portfolio/unrealised-gains",
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
            .find(|(_, p, _, _, _, _)| *p == "/portfolio/open-parcels")
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
}
