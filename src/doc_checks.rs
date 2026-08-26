//! Tests pinning documentation-only requirements (a TODO item is only done when
//! a test exists for it — CLAUDE.md). Each test asserts its required text —
//! typically a Known-limitations entry in `docs/API.md`, its README surfacing,
//! and the cited ATO mirror — is present, so the documented scope cut can't
//! silently vanish.

const API_MD: &str = include_str!("../docs/API.md");
const README_MD: &str = include_str!("../README.md");
const SCHEMA_MD: &str = include_str!("../docs/SCHEMA.md");
const DENY_TOML: &str = include_str!("../deny.toml");

/// The body of the `# Known limitations` section of `docs/API.md`.
fn known_limitations() -> &'static str {
    let section = API_MD
        .split("# Known limitations")
        .nth(1)
        .expect("docs/API.md has a Known limitations section");
    section
        .split("\n# ")
        .next()
        .expect("split always yields at least one part")
}

/// Docs-sync pin for the strict decoding of request bodies (SCENARIOS V-a):
/// the refusal has its own section, the `422` list points at it, and the one
/// consequence a client has to know about — a read body is no longer PUT-able
/// verbatim — is written down beside it. The behaviour itself is pinned by
/// `infra::http::tests::every_request_body_denies_unknown_fields`; this is the
/// documentation half.
#[test]
fn unrecognised_body_fields_documented() {
    assert!(API_MD.contains("## Unrecognised body fields"));
    assert!(API_MD.contains("Every request body is decoded strictly"));
    assert!(API_MD.contains("refused `422` naming it"));
    assert!(API_MD.contains("a misspelt name is a rejection, not a silently-ignored default"));
    // The `422` row of the response-code table points at the section.
    assert!(API_MD.contains(
        "**any field a request body does not recognise** (see [Unrecognised body \
         fields](#unrecognised-body-fields))"
    ));
    // A query string is refused the same way, at the status its extractor
    // answers with.
    assert!(API_MD.contains("or an unreadable query string on an endpoint that takes one"));
    // The consequence for clients.
    assert!(API_MD.contains("what a `GET` returns cannot be `PUT` back verbatim"));
    assert!(API_MD.contains("`settlement_date_source`"));
}

/// Docs-sync pin for the cent-rounded CSV exports (SCENARIOS W-c): both
/// export paragraphs state the rounding, and the display-rules paragraph no
/// longer claims the CSV exports return full-precision decimals — the sentence
/// that became wrong when the exports started rounding. The behaviour itself is
/// pinned by `reports::export`'s unit tests and the two reports' export tests;
/// this is the documentation half.
#[test]
fn cent_rounded_csv_exports_documented() {
    // Both export paragraphs say what rounds, in the same words.
    assert_eq!(
        API_MD
            .matches(
                "**Money columns are exported to the cent** (2 decimal places, half away from zero"
            )
            .count(),
        2,
        "both /export paragraphs state the cent rounding"
    );
    // …and what does not: the two non-money columns come through untouched.
    // (W-c's other half — "the JSON report above is unaffected" — was
    // withdrawn by W-f, which moved the net-capital-gain rounding into the
    // shared year record; `worksheet_derived_columns_documented` below pins
    // what replaced it.)
    assert!(
        API_MD.contains("`tax_year` and `taxpayer_basis` are not money and are exported verbatim")
    );
    assert!(
        !API_MD
            .contains("The JSON report above is unaffected — it still answers the exact figure.")
    );
    // The display-rules paragraph no longer promises full-precision CSV.
    assert!(
        !API_MD.contains("the JSON API and the CSV exports still return full-precision decimals")
    );
    assert!(API_MD.contains(
        "The two [tax-return-ready CSV exports](#net-capital-gain) apply the same cent rounding \
         to their money columns"
    ));
}

/// Docs-sync pin for the annual tax report's cent-rounded disposal schedule
/// (SCENARIOS W-d): the report section states the convention — the money
/// figures round and every subtotal and total is the sum of the rounded rows,
/// so a printed column adds up — and names both what rounds and what is
/// deliberately left verbatim, since "which columns are money" is the half of
/// the rule a reader cannot infer from a figure. The display-rules paragraph
/// no longer says the CSV exports are the only place outside the browser that
/// rounds, which stopped being true here. The behaviour itself is pinned by
/// `reports::tax_report`'s three W-d tests; this is the documentation half.
#[test]
fn cent_rounded_tax_report_disposals_documented() {
    assert!(API_MD.contains(
        "**The money figures of this section are rounded to the cent, and every per-listing \
         subtotal and grand total is the sum of those rounded rows**"
    ));
    // What rounds, and what does not — with the reason the per-unit and
    // as-entered columns are exempt.
    assert!(API_MD.contains(
        "Rounded are the initial and adjusted cost base, each itemised adjustment's amount, \
         the proceeds, the gain/loss, and the discount amount and discounted gain"
    ));
    assert!(API_MD.contains("Left verbatim, none of them a derived AUD amount"));
    // Rounding here changes no calculation: the source reports stay exact.
    assert!(API_MD.contains(
        "the figures come from [realised gains](#realised-gains), which still answers the exact \
         decimal"
    ));
    // The display-rules paragraph's old exclusivity claim is gone, replaced by
    // one that names this report alongside the exports.
    assert!(!API_MD.contains("they are the one place outside the browser that rounds"));
    assert!(API_MD.contains(
        "the [annual tax report](#annual-tax-report) applies it to the money figures of its \
         disposal schedule"
    ));
}

/// Docs-sync pin for the worksheet columns derived from cent-rounded inputs
/// (SCENARIOS W-f): the two reports whose columns are arithmetically related
/// to one another say so — the net-capital-gain worksheet rounds its *input*
/// columns and derives the rest (so the printed working reaches its own
/// result, and the JSON now carries the export's figures), and the tax
/// summary's total columns are the sum of the cent-rounded lines printed
/// beside them, with the reason its income lines are deliberately left exact.
/// The display-rules paragraph names both. The behaviour itself is pinned by
/// the two reports' W-f tests; this is the documentation half.
#[test]
fn worksheet_derived_columns_documented() {
    // The net-capital-gain worksheet: what rounds, what is derived from it,
    // and the identity a reader checks on the page.
    assert!(API_MD.contains(
        "**The worksheet is kept at the cent, and its derived columns are computed from the \
         rounded inputs.**"
    ));
    assert!(API_MD.contains(
        "`net_discount_eligible_gain − cgt_discount + net_other_gain == net_capital_gain`"
    ));
    // …and the consequence W-c's wording used to deny.
    assert!(API_MD.contains(
        "the **JSON report carries the same rounded figures as the export** — one worksheet, \
         not two"
    ));
    // The tax summary's totals, and why its income lines are not rounded.
    assert!(API_MD.contains("**A total column is the sum of the columns printed beside it.**"));
    assert!(API_MD.contains("The **income lines themselves keep full precision**"));
    // The display-rules paragraph carries the rule and the worked figure.
    assert!(API_MD.contains(
        "**Where columns are arithmetically related to one another, the report rounds too, and \
         derives the rest.**"
    ));
    assert!(API_MD.contains("100.01 − 50.01 is 50.00"));
}

/// Docs-sync pin for the money/quantity encoding rule (SCENARIOS W-a): its own
/// section beside [`unrecognised_body_fields_documented`]'s, the `422` list
/// pointing at it, the reason a JSON number cannot be honoured, and the two
/// things a client could otherwise get wrong (integers are refused too; a read
/// already answers with strings, so the round trip needs no conversion). The
/// behaviour itself is pinned by
/// `infra::http::tests::every_money_request_field_refuses_a_json_number` and
/// the `entities::trade` API tests; this is the documentation half.
#[test]
fn money_as_a_json_number_documented() {
    assert!(API_MD.contains("## Money as a JSON number"));
    assert!(API_MD.contains("Every money and quantity field takes its value as a **JSON string**"));
    assert!(
        API_MD.contains("A bare JSON number is refused `422`, naming the field and the remedy")
    );
    // The reason, with the two figures the finding was raised on.
    assert!(API_MD.contains("A JSON number arrives as an `f64`"));
    assert!(API_MD.contains("{\"quantity\": 100000000.00000001}"));
    assert!(API_MD.contains("99999999.8765432"));
    // Integers are in scope, and so are the price-override maps.
    assert!(API_MD.contains("Integers are refused too"));
    assert!(API_MD.contains("{\"prices\": {\"7\": \"58.12\"}}"));
    // No conversion needed on the way back.
    assert!(API_MD.contains("Reads already answer with strings"));
    // The `422` row of the response-code table points at the section.
    assert!(API_MD.contains(
        "**any money or quantity field sent as a bare JSON number** rather than a decimal \
         string, which would silently lose digits past about the fifteenth significant one \
         (see [Money as a JSON number](#money-as-a-json-number))"
    ));
}

/// Docs-sync pin for the decimal-range refusal (SCENARIOS W-e): its own
/// section beside [`money_as_a_json_number_documented`]'s, the limit quoted,
/// the reason it has to be refused at the write, the list of paths that can
/// reach it, the three that provably cannot, and the edit escape hatch. The
/// behaviour itself is pinned by `domain::cost_base`'s unit tests and one API
/// test per reachable path; this is the documentation half.
#[test]
fn figures_beyond_the_decimal_range_documented() {
    assert!(API_MD.contains("## Figures beyond the decimal range"));
    // The limit itself, quoted rather than described.
    assert!(API_MD.contains("79228162514264337593543950335"));
    // Why the write is the only place it can be refused.
    assert!(API_MD.contains("the product **is** the answer"));
    // Every reachable path is named.
    for path in [
        "`average_price × quantity + brokerage + GST`, one shared check",
        "`cost_base + lpr_expenditure`, a *sum* rather than a product",
        "`quantity × market_value_per_share`, which is both the market value",
        "`exercise_price × units + rights_cost`",
        "`units × reinvestment_price`",
    ] {
        assert!(
            API_MD.contains(path),
            "the 422 path is not documented: {path}"
        );
    }
    // …and so is the reasoning for the ones deliberately left out.
    assert!(API_MD.contains("Three parcel-creating paths are deliberately **not** checked"));
    // An existing offending row stays correctable.
    assert!(
        API_MD.contains(
            "An **edit** is judged on the figures being written, never on the stored ones"
        )
    );
    // The `422` row of the response-code table points at the section.
    assert!(API_MD.contains(
        "**any write whose money figures multiply out beyond the largest decimal that can be \
         stored**"
    ));
}

/// Docs-sync pin for the unit-count half of the same ceiling ("A replacement
/// quantity no `Decimal` can hold"): the section says where each ratio-driven
/// path's refusal sits, why the rights-issue entitlement cap is deliberately
/// not one of them, and that an action already stored in the refused state
/// stays editable — and the per-endpoint `422` lists and the fractional-
/// entitlement promise carry the same exception.
#[test]
fn quantities_beyond_the_decimal_range_documented() {
    assert!(API_MD.contains("### Quantities as well as money"));
    // The result, not the working, is what cannot be represented — which is
    // why re-ordering the arithmetic cannot answer it.
    assert!(API_MD.contains("here it is the **result** that cannot be represented"));
    // The enabling condition, stated rather than left implicit.
    assert!(API_MD.contains("nil-priced parcel of any size is a legal holding"));
    // Where each refusal sits.
    for path in [
        "`POST …/exchange` and `POST …/demerge` refuse before writing anything",
        "so the refusal is at the **action write**, `PUT /corporate_actions/:id`",
        "so `PUT /transfers/:id` refuses a request naming more units than could ever have \
         been held",
    ] {
        assert!(
            API_MD.contains(path),
            "the 422 path is not documented: {path}"
        );
    }
    // …and the one that is deliberately not a refusal, with the reason.
    assert!(API_MD.contains("**entitlement cap** is deliberately *not* refused"));
    assert!(API_MD.contains("the cap saturates to *unbounded* instead and the exercise lands"));
    // An action already in the refused state stays correctable.
    assert!(API_MD.contains(
        "An **edit** is judged on the terms being written here too, so an action already \
         stored in the refused state"
    ));
    // The corporate-action write rule, beside the over-consumption one.
    assert!(API_MD.contains("**Writing terms that re-base a quantity beyond the decimal range.**"));
    // The exception to the exact-fraction promise.
    assert!(API_MD.contains(
        "where the exact figure a ratio produces is past the largest decimal that can be \
         stored, there is no lesser quantity to keep"
    ));
}

/// Docs-sync pin for the mirror rule ("A parcel entered behind a ratio that
/// already fits"): the section says why refusing the action is not sufficient,
/// which parcel-creating paths carry the walk, that the two rollovers are
/// judged on their **destination** listing, which single path is deliberately
/// excluded and why, and that a parcel already stored beyond the range stays
/// correctable.
#[test]
fn a_parcel_entered_behind_a_ratio_that_already_fits_documented() {
    assert!(API_MD.contains("#### The mirror: a parcel entered behind a ratio that already fits"));
    // Why the action-write refusal alone is not enough.
    assert!(API_MD.contains("Refusing the action is necessary but not sufficient."));
    // The two hooks, and what each judges.
    assert!(API_MD.contains(
        "the action write judges a new ratio against every recorded quantity, the parcel write \
         judges a new quantity against every recorded ratio"
    ));
    // Each covered path names the bound it previously met, which is what says
    // why that bound did not already catch this.
    for path in [
        "a `Buy` or `DRP` via [`PUT /trades/:id`](#trades)",
        "an [inheritance](#inheritances) — its own magnitude bound is on `cost_base + \
         lpr_expenditure`, a sum",
        "an [ESS vest](#vesting-an-ess-statement) — the statement's bound is on `quantity × \
         market_value_per_share`",
        "a [rights exercise](#exercising-a-rights-issue) — likewise for `exercise_price × units \
         + rights_cost`",
        "a [DRP reinvestment](#drp-reinvestment), both ways the units are arrived at",
    ] {
        assert!(
            API_MD.contains(path),
            "the covered path is not documented: {path}"
        );
    }
    // The finding the section exists for: the destination listing, not the
    // listing the operation is about.
    assert!(API_MD.contains(
        "checked on the **destination** listing, the one the replacement parcels land on, which \
         is *not* the listing the operation is about"
    ));
    assert!(API_MD.contains(
        "A **1-for-1** exchange onto a listing carrying a 1000-for-1 split of its own answered \
         `201`"
    ));
    // …and the one path deliberately left out, with the argument.
    assert!(API_MD.contains(
        "The eighth, a [transfer](#transfers)'s transfer-in Buy, is deliberately **not** checked"
    ));
    assert!(API_MD.contains(
        "A transfer-in past the range implies a source parcel past it, which is already refused."
    ));
    // A parcel already in the refused state stays correctable.
    assert!(API_MD.contains(
        "An **edit** is judged over the state the write leaves behind, never over the stored row"
    ));
    // The cross-reference from the sibling rule over the same eight writes.
    assert!(API_MD.contains(
        "The same eight writes carry one other rule keyed on the listing's recorded corporate \
         actions rather than its dates"
    ));
}

/// Docs-sync pin for the append-only audit trail (2026-07-13 improvement
/// review): the schema documents the table, its scope decision (which tables
/// are audited and why the rest are not), and the keep-forever retention
/// decision; the API documents the inspection endpoint; the README surfaces
/// the feature and cites the ATO record-keeping guidance it aligns with.
#[test]
fn row_history_audit_trail_documented() {
    // SCHEMA.md: the table and both recorded decisions.
    assert!(SCHEMA_MD.contains("row_history"));
    assert!(SCHEMA_MD.contains("**append-only audit trail**"));
    assert!(SCHEMA_MD.contains("scope decision 2026-07-14"));
    assert!(SCHEMA_MD.contains("Retention (decision 2026-07-14): entries are kept forever"));
    assert!(SCHEMA_MD.contains("deliberately has no pruning job"));
    // API.md: the inspection endpoint, its section, and its 422.
    assert!(API_MD.contains("### Row history"));
    assert!(API_MD.contains("POST /reports/row_history"));
    assert!(API_MD.contains("a row-history request naming a table that is not audited"));
    // The browse form (SCENARIOS U-b): both request shapes, the uniform
    // entry, the cursor, the bounded page, and the ordering rule that makes
    // paging safe.
    assert!(API_MD.contains("**Recent changes (browse).**"));
    assert!(
        API_MD.contains("`{\"entries\": […], \"page_size\": n, \"next_before_id\": id | null}`")
    );
    assert!(API_MD.contains("**uniform across tables**"));
    assert!(API_MD.contains("page size, `1`–`1000`, default `100`"));
    assert!(API_MD.contains("the **cursor**: return only entries older than this trail id"));
    assert!(API_MD.contains("newest first by the trail's own `id`**, never by `changed_at`"));
    assert!(API_MD.contains("`null` **exactly when the page reached the end of the trail**"));
    // The occupant marking (SCENARIOS U-a): the rule that finds the boundary,
    // both entry fields, what the trail cannot know, and the one audited
    // table whose key is a natural one and so has no boundary to find.
    assert!(API_MD.contains("**Whose history is this?**"));
    assert!(API_MD.contains("the `DELETE` and every older entry belong to an earlier occupant**"));
    assert!(API_MD.contains("`occupant` — `1` is the id's most recent occupant"));
    assert!(API_MD.contains(
        "`current_occupant` — `true` when the entry belongs to the record that holds the id **now**"
    ));
    assert!(API_MD.contains("cannot say is *when* the id was taken again"));
    assert!(API_MD.contains("`tax_year_settings` is the one audited table exempt"));
    // README: the feature line, the browse mode, and the ATO citation.
    assert!(README_MD.contains("**browses the recent changes across every audited table**"));
    assert!(README_MD.contains("which record's history each entry actually is"));
    assert!(README_MD.contains("**Append-only audit trail**"));
    assert!(README_MD.contains("docs/ato/cgt-keeping-records-shares.md"));
    assert!(
        include_str!("../docs/ato/cgt-keeping-records-shares.md")
            .contains("keeping-records-of-shares-and-units"),
        "the cited ATO mirror carries its source header"
    );
}

/// Docs-sync pin for the id rule the trail rests on (SCENARIOS U-a): an
/// audited table's id must be `AUTOINCREMENT` **and** the server must let the
/// database assign it, since `AUTOINCREMENT` governs only the ids SQLite picks
/// when an INSERT omits the column. `reports::row_history`'s
/// `every_audited_tables_id_is_autoincrement` enforces the first half against
/// the live schema; this pins that SCHEMA.md states the requirement and the
/// reason, names both exemptions, and that API.md tells the reader which reuse
/// is now impossible and which is still deliberately allowed.
#[test]
fn audited_ids_are_never_reused_documented() {
    // SCHEMA.md: the requirement, the two exemptions, and the server half.
    assert!(SCHEMA_MD.contains(
        "**An audited table's `id` must be `INTEGER PRIMARY KEY AUTOINCREMENT`, \
         and a server-created row must let the database assign it.**"
    ));
    assert!(SCHEMA_MD.contains("an id handed out twice makes one trail out of two records"));
    assert!(SCHEMA_MD.contains("`every_audited_tables_id_is_autoincrement`"));
    assert!(SCHEMA_MD.contains("`tax_year_settings` is keyed on the financial year itself"));
    assert!(SCHEMA_MD.contains("a singleton whose CHECK pins the only id it can have"));
    assert!(SCHEMA_MD.contains(
        "a write path that computes `SELECT COALESCE(MAX(id), 0) + 1` and binds it \
         defeats the column entirely"
    ));
    // API.md: the reuse the server can no longer make, and the one a user can.
    assert!(API_MD.contains("**The server no longer does it.**"));
    assert!(API_MD.contains(
        "now leaves the id to the database, whose `AUTOINCREMENT` columns never \
         re-issue a freed one"
    ));
    assert!(API_MD.contains("A `PUT` on an explicit id remains an upsert"));
    // API.md: a preview carries no id, because none has been assigned.
    assert!(API_MD.contains("Their `id` is **`0`**: a preview writes nothing"));
}

/// Docs-sync pin for linked attachments on provenance-created trades
/// (REQUIREMENTS 2026-07-15): the Attachments section documents the
/// `include_linked` list option, enumerates the three traversed provenance
/// links, and states that ownership is unchanged; the web-UI feature text
/// mentions linked documents; the 422 catalogue covers the option's misuse.
#[test]
fn linked_attachments_documented() {
    // The list-endpoint option and its linked-documents explainer.
    assert!(API_MD.contains("include_linked=true"));
    assert!(API_MD.contains("**Linked documents:**"));
    // All three provenance links are enumerated.
    assert!(API_MD.contains("`income.reinvestment_trade_id`"));
    assert!(API_MD.contains("`income.buyback_trade_id`"));
    assert!(API_MD.contains("`trades.ess_statement_id`"));
    // Attachments stay single-owner; the traversal is read-time only.
    assert!(API_MD.contains("Ownership is unchanged"));
    // The web-UI feature text mentions linked documents.
    assert!(API_MD.contains("linked documents](#attachments)"));
    // The 422 catalogue covers misuse of the option.
    assert!(API_MD.contains("an attachment list combining `include_linked`"));
}

/// Docs-sync pin for provisional snapshots, the job catch-up windows, and
/// the bulk regeneration controls (REQUIREMENTS 2026-07-16): the schema
/// documents the `provisional` column, the API documents the flag in
/// responses, the two regeneration endpoints, the import true-up, the
/// valuation-only FX fallback rule, and the 422 wording; the README surfaces
/// the provisional-then-finalised behaviour and both self-healing windows.
#[test]
fn provisional_snapshots_and_catchup_documented() {
    // SCHEMA.md: the provisional column, distinct from stale.
    assert!(SCHEMA_MD.contains("provisional   INTEGER"));
    assert!(SCHEMA_MD.contains("used a fallback-month FX rate"));
    assert!(SCHEMA_MD.contains("Distinct from stale"));
    // API.md: the flag in list/series responses, the endpoints, the true-up,
    // and the fallback rule (valuation-only, bounded, never a tax figure).
    assert!(API_MD.contains("`stale`, `provisional`"));
    assert!(API_MD.contains("**Provisional snapshots:**"));
    assert!(API_MD.contains("/report_snapshots/regenerate_all"));
    assert!(API_MD.contains("/report_snapshots/regenerate_provisional"));
    assert!(API_MD.contains("snapshot_true_up"));
    assert!(API_MD.contains("**Valuation-only fallback:**"));
    assert!(API_MD.contains("No tax calculation or FY report can reach a fallback-month rate"));
    assert!(API_MD.contains("`fx_provisional: true`"));
    // The 422 catalogue reflects the bounded fallback.
    assert!(API_MD.contains("an FX-rate gap too old for the 2-month valuation fallback"));
    // README: provisional-then-finalised snapshots and the two lookbacks.
    assert!(README_MD.contains("flagged **provisional**"));
    assert!(README_MD.contains("finalised automatically"));
    assert!(README_MD.contains("self-heals the last 14 calendar days"));
    assert!(README_MD.contains("backfills missing dates over a 14-day window"));
}

/// Docs-sync pin for the unpriced-listing fact (SCENARIOS Q-02): the cost of
/// an *unbounded* run of days the provider can never serve — one hand-entered
/// price per listing per trading day, forever — is what the Closing prices
/// section used to leave unsaid, and the way out is now a documented listing
/// field with its own flag on the way back.
#[test]
fn unpriced_listing_and_carried_forward_price_documented() {
    // SCHEMA.md: the column, its two write-time pairings, and the separate
    // snapshot flag.
    assert!(SCHEMA_MD.contains("unpriced_from TEXT (nullable)"));
    assert!(SCHEMA_MD.contains("price_carried_forward INTEGER"));
    assert!(SCHEMA_MD.contains("a stored ok price must exist **before** it"));
    // API.md: the listing field, both 422s, and what happens on each surface.
    assert!(API_MD.contains("**A security the provider has stopped quoting.**"));
    assert!(API_MD.contains("**collection stops fetching the listing.**"));
    assert!(API_MD.contains("**valuation carries the last stored ok close forward**"));
    assert!(API_MD.contains("`price_carried_forward`"));
    // The reason it is not the `provisional` flag: the true-up stays bounded.
    assert!(API_MD.contains("**Carried-forward prices:**"));
    assert!(API_MD.contains("a carried-forward price never clears"));
    // The Closing prices section now states what an unbounded run costs.
    assert!(API_MD.contains("An **unbounded run** of such days"));
    // Health stops nagging about the days the provider can never serve.
    assert!(API_MD.contains("**inside the span its provider serves it**"));
    assert!(API_MD.contains("dated before its [`unpriced_from`](#listings)"));
    // The 422 catalogue carries both refusals and the fetch/backfill guards.
    assert!(API_MD.contains(
        "a listing `unpriced_from` with no stored closing price before it or with a \
             provider-fetched ok price on or after it"
    ));
    assert!(API_MD.contains("that falls on or after its listing's `unpriced_from`"));
    // README: the feature, and the flag on the snapshot series.
    assert!(README_MD.contains("**unpriced from**"));
    assert!(README_MD.contains("carry the last stored close forward"));
    assert!(README_MD.contains("**carried-forward price** flag"));
}

/// Docs-sync pin for the mirror fact (migration 0037): a security whose
/// provider series *begins* at a date, with everything earlier unavailable at
/// any price. The absence of it is what produced 375 hand-entered,
/// knowingly-wrong closing prices in the live database, so the docs must say
/// what the column does, that nothing is substituted, and that the total is
/// therefore incomplete.
#[test]
fn unpriced_before_and_excluded_holdings_documented() {
    // SCHEMA.md: the column, the one pairing rule, and the two new snapshot
    // columns.
    assert!(SCHEMA_MD.contains("unpriced_before TEXT (nullable)"));
    assert!(SCHEMA_MD.contains("holding_excluded INTEGER"));
    assert!(SCHEMA_MD.contains("excluded_holdings TEXT"));
    assert!(SCHEMA_MD.contains("must fall strictly **before** `unpriced_from`"));
    // API.md: the listing field and what happens on each surface.
    assert!(API_MD.contains("**A security whose provider series has not begun.**"));
    assert!(API_MD.contains("**collection never fetches the listing.**"));
    assert!(API_MD.contains("**valuation excludes the holding**"));
    assert!(API_MD.contains("`holding_excluded`"));
    // The decision itself: not symmetric, nothing invented, and both
    // consequences stated rather than left for a reader to discover.
    assert!(API_MD.contains("The two directions are deliberately **not symmetric**."));
    assert!(API_MD.contains("**no figure is invented**"));
    assert!(API_MD.contains("**omits a real holding**"));
    assert!(API_MD.contains("**Excluded holdings:**"));
    // The unbounded-true-up trap, and the all-excluded blocker.
    assert!(API_MD.contains("an excluded holding never clears"));
    assert!(API_MD.contains("is **blocked**, not stored empty"));
    // The 422 catalogue carries the one refusal and the fetch/backfill guard.
    assert!(API_MD.contains(
        "a listing whose `unpriced_before` does not fall strictly before its `unpriced_from`"
    ));
    assert!(API_MD.contains("that falls before its listing's `unpriced_before`"));
    // Health goes quiet over the span at both ends.
    assert!(API_MD.contains("**inside the span its provider serves it**"));
    // README: the feature, and the flag on the snapshot series.
    assert!(README_MD.contains("**unpriced before**"));
    assert!(README_MD.contains("**excluded** from that date's portfolio totals"));
    assert!(README_MD.contains("**excluded holding** flag"));
}

/// Docs-sync pin for the swapped-demerger health check: the API documents the
/// field list, all three clauses of the predicate and what each rules out, the
/// legitimate-shape question, and — honestly — that it would have been silent
/// on the live data before the borrowed prices were cleared; the README
/// surfaces the feature.
#[test]
fn demerger_head_not_continuing_check_documented() {
    assert!(API_MD.contains("`demergers_head_not_continuing` — every recorded"));
    assert!(
        API_MD.contains(r#""head_unpriced_before", "head_first_price_date", "head_held_from""#)
    );
    assert!(
        API_MD.contains(
            r#""demerged_priced_days", "demerged_earliest_date", "demerged_latest_date""#
        )
    );
    // The predicate, and that an absence alone is not it.
    assert!(API_MD.contains("**The predicate is an asymmetry, not an absence**"));
    assert!(API_MD.contains("holds **no `ok` closing price of any origin** dated before"));
    assert!(API_MD.contains("`ok`, provider-**fetched** prices dated *before* it"));
    assert!(API_MD.contains("was **acquired before** the demerger"));
    // The false positives each clause rules out, and the legitimate-shape
    // question asked and answered.
    assert!(API_MD.contains("a database where nothing has been collected lights up"));
    assert!(API_MD.contains("simply never backfilled reads as a defect"));
    assert!(API_MD.contains("**Is there a legitimate demerger of this shape?**"));
    assert!(API_MD.contains("an in-specie distribution of an already-listed holding"));
    // What it would not have caught, stated rather than overclaimed.
    assert!(API_MD.contains("this check would have stayed **silent**"));
    assert!(API_MD.contains("the two are **complements**"));
    assert!(README_MD.contains("**demerger recorded the wrong way round**"));
}

/// Docs-sync pin for the identical-price-series health check: the API
/// documents the field list, the run predicate and its threshold, the two
/// reasons a genuine pair cannot trip it, the per-origin split, and that
/// nothing silences it but fixing the data; the README surfaces the feature.
#[test]
fn duplicate_price_series_check_documented() {
    assert!(API_MD.contains("`duplicate_price_series`"));
    assert!(API_MD.contains("\"identical_days\", \"earliest_date\", \"latest_date\", \"fetched_days\", \"manual_days\", \"other_fetched_days\", \"other_manual_days\""));
    // The predicate: a run of comparisons, not a count of matching days.
    assert!(API_MD.contains("**The predicate is a run, not a total**"));
    assert!(API_MD.contains("Thirty such days (about six weeks of trading) is the threshold"));
    assert!(API_MD.contains("it neither breaks the run nor counts towards it"));
    // The two false-positive guards, and the figures the real data produced.
    assert!(API_MD.contains("A run whose closes **never move** is not reported"));
    assert!(API_MD.contains("the one day (2024-02-08) the two really did both close at 4.12"));
    // The split, and that only fixing the data clears it.
    assert!(API_MD.contains("`fetched_days` / `manual_days` split each side's rows in the run"));
    assert!(API_MD.contains("**Non-blocking, and there is no way to silence it**"));
    assert!(README_MD.contains("**one price series between them**"));
}

/// Docs-sync pin for the duplicate-trade health check (SCENARIOS V-c): the
/// API documents the field list, what the check keys on and why that key was
/// chosen over the figure-based one the rest of the family uses, the scope of
/// the key (per listing, not per holding account), the blank/whitespace rule,
/// that derived trades fall out of it, and — stated rather than glossed — the
/// one thing it cannot catch; the README surfaces the feature and the same
/// limitation. The banner sentence names it too, since the strip is where a
/// user meets it.
#[test]
fn duplicate_trades_check_documented() {
    assert!(API_MD.contains("`duplicate_trades`"));
    assert!(API_MD.contains(
        "\"listing_id\", \"ticker\", \"contract_note_ref\", \"date\", \"trade_count\", \"trade_ids\""
    ));
    // The key, and why it is not the one `duplicate_income` uses.
    assert!(API_MD.contains("**The key is the reference, not the figures**"));
    assert!(API_MD.contains("identifies **one confirmation document**"));
    assert!(API_MD.contains("**this list has no false positives**"));
    // The limitation, stated rather than oversold.
    assert!(API_MD.contains("**only catches trades whose entry recorded the reference**"));
    assert!(API_MD.contains("It is a check on what was typed in, not on the portfolio"));
    // The scope of the key, both halves of it.
    assert!(API_MD.contains("one note can cover a multi-line order"));
    assert!(API_MD.contains("The **holding account is not**"));
    // Blank vs null, and the derived rows that carry neither.
    assert!(API_MD.contains("**trimmed and case-sensitively**"));
    assert!(API_MD.contains("is no reference at all and never groups"));
    assert!(API_MD.contains("fall out of this check by construction"));
    assert!(API_MD.contains("**Non-blocking, and there is no way to silence it**"));
    // The banner sentence, and the README feature line.
    assert!(API_MD.contains("two trades sharing one broker contract note reference"));
    assert!(README_MD.contains("**duplicated trade**"));
    assert!(README_MD.contains("**no false positives**"));
}

/// Docs-sync pin for date-ranged bulk regeneration (REQUIREMENTS 2026-07-25):
/// the API documents the new default-range endpoint, `regenerate_all`'s
/// range/backfill semantics and its 422, and the README surfaces that
/// regenerate-all is date-ranged and defaults to the whole history.
#[test]
fn regenerate_all_date_range_documented() {
    assert!(API_MD.contains("/report_snapshots/regenerate_range"));
    assert!(API_MD.contains("a backfill for dates that never had a snapshot"));
    assert!(API_MD.contains("clamped up to the first-ever-held date"));
    assert!(API_MD.contains("returns `422` if its resolved `from` is after its `to`"));
    assert!(README_MD.contains("date-ranged regenerate-all action"));
}

/// Docs-sync pin for the AMIT cash-only income rows (REQUIREMENTS
/// 2026-06-12): the Income section documents the cash-only rule and its 422s,
/// the Tax summary section documents the exclusion, the cross-check report
/// has its own section, and the README surfaces both features.
#[test]
fn amit_cash_only_rows_documented() {
    // Income section: the cash-only rule and the write-time validation.
    assert!(API_MD.contains("**AMIT cash distributions (cash-only rows):**"));
    assert!(API_MD.contains("the AMMA statement's attribution is the only assessable record"));
    // Tax summary section: the whole-row exclusion.
    assert!(API_MD.contains("**AMIT listing are excluded entirely**"));
    // The cross-check report's own section.
    assert!(API_MD.contains("### AMIT cash cross-check"));
    assert!(API_MD.contains("GET /reports/amit_cash_cross_check"));
    // README features.
    assert!(README_MD.contains("cash-only income rows"));
    assert!(README_MD.contains("**AMIT cash cross-check**"));
}

/// Docs-sync pin for AMIT adjustment cross-check and generation
/// (REQUIREMENTS 2026-08-13): the API documents the generation endpoint with
/// each of its refusals, the cross-check report, the write-time duplicate
/// rejection, and the annual tax report's now-four-part completeness; the
/// schema records the UNIQUE index that backs the invariant; the README
/// surfaces the feature and the completeness wording.
#[test]
fn amit_adjustment_generation_and_cross_check_documented() {
    // The generation endpoint, its reconciliation-not-invariant stance, and
    // its preview mode.
    assert!(API_MD.contains("### Generating AMIT adjustments"));
    assert!(API_MD.contains("POST /amma_statements/:id/generate_adjustments"));
    assert!(API_MD.contains("does not block the write"));
    assert!(API_MD.contains(r#"`"preview": true` computes the same result and writes nothing"#));
    // Each 422 refusal.
    assert!(API_MD.contains("**already has adjustments**"));
    assert!(API_MD.contains("**no parcels** of the statement's listing were open"));
    // SCENARIOS F-04: that refusal names every way to reach it, and points a
    // closed holding at the hand-entered path rather than at missing trades —
    // with the row's destination said for each (SCENARIOS N-06): the parcels the
    // units came from for a sale, the replacement parcels for a move.
    assert!(API_MD.contains("**sold during the year**"));
    assert!(API_MD.contains("**transferred, exchanged or demerged away during the year**"));
    assert!(API_MD.contains("the normal path for the year of a sale"));
    // A split across the covered parcels is *not* a refusal (SCENARIOS
    // B-24): each parcel's stored as-acquired quantity is re-based into the
    // statement year's basis before the per-unit figure is applied.
    assert!(API_MD.contains("**share split** between the covered parcels' acquisition dates"));
    assert!(API_MD.contains("re-based into the year-end basis × `cost_base_adjustment`"));
    // SCENARIOS Y-c: and because a split puts the stored quantity and the
    // reconciling total on different unit bases, every generated row carries
    // *both*, so a caller listing them under the total can show a list that
    // adds up to it.
    assert!(API_MD.contains("**plus its own `units_adjusted`**"));
    assert!(API_MD.contains("`Σ created[].units_adjusted` is the response's `units_adjusted`"));
    // The write-time duplicate invariant and the index behind it.
    assert!(
        API_MD.contains("**another row already adjusts the same parcel on the same statement**")
    );
    // SCENARIOS F-17: and the rollover refusal, with the way round it and the
    // disposals it deliberately does not reach. The way round changed with
    // SCENARIOS N-06: the row goes against the replacement parcel, which the
    // per-account rule now accepts, and generation follows a transfer itself
    // rather than refusing the whole run.
    assert!(API_MD.contains("has already carried the parcel's units into a replacement parcel**"));
    assert!(API_MD.contains("enter the rest **against the replacement parcel**"));
    assert!(API_MD.contains("**A rollover after the year end is followed, not refused.**"));
    assert!(API_MD.contains("`unattributed`"));
    assert!(API_MD.contains("**real disposals** whose gain the reduction does reach"));
    assert!(SCHEMA_MD.contains("UNIQUE (amma_statement_id, trade_id)"));
    // The cross-check report's own section, with each of its four checks.
    assert!(API_MD.contains("### AMIT adjustment cross-check"));
    assert!(API_MD.contains("GET /reports/amit_adjustment_cross_check"));
    // SCENARIOS F-04: the coverage band, not a bare equality — a set covering
    // units sold during the statement's year reconciles.
    assert!(API_MD.contains("the units disposed of during the statement's year"));
    assert!(API_MD.contains("just before a relevant CGT event"));
    for check in [
        "**no adjustments at all**",
        "**coverage mismatch**",
        "**duplicate parcel**",
        "**parcel outside the statement's year**",
    ] {
        assert!(API_MD.contains(check), "missing cross-check bullet {check}");
    }
    // The annual tax report's completeness gate is now five lists (the fifth
    // being the rollover-consistency one, see
    // `rollover_consistency_cross_check_documented`), and stays non-blocking.
    assert!(API_MD.contains("`amit_adjustment_alerts`"));
    assert!(API_MD.contains("`complete` is true only when all five are empty"));
    assert!(API_MD.contains("**`completeness`** — non-blocking (never rejects the request)"));
    // README: the feature line and the completeness wording.
    assert!(README_MD.contains("**AMIT adjustment generation and cross-check**"));
    assert!(README_MD.contains("per-parcel AMIT adjustments reconcile to it"));
}

/// Docs-sync pin for SCENARIOS F-05: which parcels a statement's per-unit
/// `cost_base_adjustment` reaches is a stated convention, not an unwritten
/// one — a parcel bought after the fund's last distribution period is covered
/// like any other, a statement quoting a *total* is divided over the units it
/// covers, and a member wanting another apportionment enters the rows by
/// hand. The ATO mirror it cites states the amount annually and per member.
#[test]
fn the_per_unit_apportionment_across_parcels_is_documented() {
    assert!(API_MD.contains("**Which parcels the per-unit figure reaches.**"));
    assert!(API_MD.contains("**uniformly to every unit held at the statement's"));
    assert!(API_MD.contains("bought *after* the fund's last distribution period"));
    assert!(API_MD.contains("dividing that total over the units the statement covers"));
    // The cited ATO mirror does state it as an annual, member-level amount.
    assert!(
        include_str!("../docs/ato/amit-cost-base-adjustments.md")
            .contains("AMIT cost base net amount for the income year in relation to your units")
    );
}

/// Docs-sync pin for SCENARIOS F-03/F-08: AMMA coverage is asked per holding
/// account, in both the cash cross-check and the annual tax report's
/// completeness section — a registry issues one statement per holder account.
#[test]
fn amma_coverage_is_documented_as_per_holding_account() {
    assert!(API_MD.contains("Coverage is asked **per holding account**"));
    assert!(API_MD.contains("one record per affected (listing, year, account) triple"));
    assert!(API_MD.contains("It is asked **per holding account** (each row carries its"));
    assert!(README_MD.contains("per holding account"));
}

/// Docs-sync pin for SCENARIOS G-11/G-20: what anchors the franking
/// holding-period window, and that a dividend with no such date is reported
/// rather than passing quietly — the promise that makes an empty franking
/// at-risk report readable as an all-clear.
#[test]
fn the_franking_windows_anchor_and_its_untested_rows_are_documented() {
    // The fallback chain, on the Income section that owns the dates.
    assert!(API_MD.contains("the recorded `ex_date`, else — on a trust row — `entitlement_date`"));
    // The DRP participation check resolves the same way.
    assert!(API_MD.contains(
        "falling back to a trust distribution's `entitlement_date` — the distribution period's end"
    ));
    // The report's third status, and what closes it.
    assert!(API_MD.contains(
        "`untested_no_ex_date` — credits are attached but no entitlement date was recorded"
    ));
    assert!(API_MD.contains("an *empty* report can be read as an all-clear"));
    assert!(API_MD.contains("`ex_date_recorded` (false when that is the payment-date fallback)"));
    assert!(
        README_MD.contains(
            "an empty report really does mean every credit the walk can test is claimable"
        )
    );
}

/// Docs-sync pin for SCENARIOS G-14: being a *qualified person* for a franking
/// credit needs more than the 45/90-day count the at-risk walk models — the
/// **30%-at-risk test** (hedges, options, futures) and the **related payments
/// rule** are separate conditions, and the latter is *not* excused by the
/// small-shareholder exemption. Neither is modelled and neither a hedge nor a
/// related payment is recordable, so this is documentation-only (like the C-09
/// rollover scope cut): the Known-limitations entry states both tests, and the
/// two places that report on franking entitlement — the at-risk report's
/// all-clear sentence and the tax summary's `franking_credits` explainer — say
/// what their answer is conditional on, rather than claiming more certainty
/// than the recorded data can support.
#[test]
fn unmodelled_franking_qualified_person_tests_documented() {
    let limitations = known_limitations();
    assert!(
        limitations.contains("**Franking: the 30%-at-risk test and the related payments rule**")
    );
    // Both tests named, with the facts that would drive them unrecordable.
    assert!(limitations.contains("30% or less of the ordinary financial risks of loss"));
    assert!(limitations.contains("hedges, options and futures are not recordable"));
    assert!(limitations.contains("Related payments are not recordable."));
    // The related payments rule applies separately from the holding period, so
    // the A$5,000 exemption does not excuse it — the trap in the ATO wording.
    assert!(limitations.contains("not excused by the A$5,000 threshold"));

    // The at-risk report's all-clear is qualified by what it does not test,
    // while staying an all-clear for what it does (G-11's `untested_no_ex_date`).
    assert!(API_MD.contains("an empty report means every attached credit is claimable **on the tests this report models**"));
    assert!(API_MD.contains("so no dividend leaves the report untested"));
    assert!(
        API_MD
            .contains("assumes the holdings are unhedged and under no related-payment obligation")
    );
    // The same qualification on the tax summary's own franking-credit line.
    assert!(API_MD.contains(
        "`franking_credits` assumes the holdings are unhedged and under no related-payment obligation"
    ));

    // The cited ATO mirror still carries both rules.
    let ato = include_str!("../docs/ato/you-and-your-shares-dividends.md");
    assert!(
        ato.contains(
            "You can't count days on which you have 30% or less of the ordinary financial risks"
        ),
        "the cited ATO mirror still carries the 30%-at-risk test"
    );
    assert!(
        ato.contains(
            "entitled to franking credits for all shares that satisfy the related payments rule"
        ),
        "the cited ATO mirror still carries the related-payments condition on the exemption"
    );
}

#[test]
fn known_limitations_document_gifts_at_market_value() {
    let limitations = known_limitations();
    // Gifts / off-market related-party transfers: a disposal at market value
    // (market-value substitution), entered as a manual Sell or Buy at market value.
    assert!(limitations.contains("**Gifts / off-market related-party transfers**"));
    assert!(limitations.contains("market-value substitution rule"));
    assert!(limitations.contains("manual Sell at market-value proceeds"));
    assert!(limitations.contains("manual Buy at market-value cost"));
    // Cites the mirrored ATO guidance (QC 66021).
    assert!(limitations.contains("docs/ato/capital-proceeds-market-value-substitution.md"));
    assert!(
        include_str!("../docs/ato/capital-proceeds-market-value-substitution.md")
            .contains("QC 66021")
    );
    // Surfaced in the README too.
    assert!(README_MD.contains("gifts / off-market related-party transfers"));
    assert!(README_MD.contains("market-value substitution"));
}

/// Docs-sync pin for the DRP partial-participation scope cut (SCENARIOS
/// I-09): the limitation names the refusal *and* the workaround that produces
/// a defensible cost base — split the distribution into a reinvested row and a
/// cash row — together with the two things that surprise about it: the
/// per-share cross-check cannot be used on the halves, and an even split reads
/// as a duplicate to the health report. A limitation with no way forward sends
/// the user to invent one.
#[test]
fn known_limitations_document_the_partial_drp_workaround() {
    let limitations = known_limitations();
    assert!(limitations.contains("**DRP partial participation**"));
    assert!(limitations.contains("all-or-nothing per (listing, holding account)"));
    // The refusal, and the two-row workaround it points at.
    assert!(
        limitations.contains("Stating the partial allotment as the reinvest `units` is refused")
    );
    assert!(limitations.contains("split the distribution into two [income](#income) rows"));
    assert!(limitations.contains("costed at the dividends actually applied to it"));
    // Cites the ATO rule the workaround rests on.
    assert!(limitations.contains("docs/ato/cgt-dividend-reinvestment-plans.md"));
    assert!(
        include_str!("../docs/ato/cgt-dividend-reinvestment-plans.md").contains("QC 66050"),
        "the cited ATO mirror carries its source header"
    );
    // The two caveats the workaround carries.
    assert!(
        limitations.contains("`amount_per_security` / `securities_held` cross-check off both rows")
    );
    assert!(limitations.contains("duplicate-income warning"));
    // Surfaced in the README's DRP feature line.
    assert!(README_MD.contains("**Partial** participation is out of scope"));
}

#[test]
fn known_limitations_document_pre_cgt_holdings() {
    let limitations = known_limitations();
    // Pre-CGT holdings (acquired before 20 September 1985) are outside CGT and
    // not modelled — and since 2026-07-13 the limitation is enforced at write
    // time: a pre-CGT-dated trade (or an inheritance from a pre-CGT death) is
    // rejected 422 rather than wrongly computing gains on the parcel.
    assert!(limitations.contains("**Pre-CGT holdings**"));
    assert!(limitations.contains("before **20 September 1985** is outside CGT"));
    assert!(limitations.contains("pre-CGT holdings are not modelled"));
    assert!(limitations.contains("enforced at write time 2026-07-13"));
    assert!(limitations.contains("**cannot be entered**"));
    assert!(README_MD.contains("pre-CGT holdings"));
    assert!(README_MD.contains("acquired before 20 September 1985"));
    assert!(README_MD.contains("rejected at write time"));
}

/// Known-limitation pin (REQUIREMENTS 2026-06-12): dividend equivalents on
/// unvested RSU grants are ordinary income when paid (TD 2017/26) and are not
/// modelled — enterable manually as income if paid out in cash.
#[test]
fn known_limitations_document_rsu_dividend_equivalents() {
    let limitations = known_limitations();
    assert!(limitations.contains("**RSU dividend equivalents**"));
    assert!(limitations.contains("dividend equivalents on unvested RSU grants"));
    assert!(limitations.contains("**ordinary income when paid**"));
    // The payment is recordable as its own kind (SCENARIOS J-10) — the docs
    // must say so, and must still name the residual limit (item 1/2).
    assert!(limitations.contains("`income_type: \"EmploymentIncome\"`"));
    assert!(limitations.contains("no surface calls it a dividend"));
    assert!(
        limitations
            .contains("**item 1/2, salary and wages, is not something this system reports**")
    );
    // The entity, tax-summary and printed-document halves of the same change.
    assert!(
        API_MD.contains(
            "A row of **either non-dividend kind** carries **the cash and nothing else**"
        )
    );
    assert!(
        API_MD.contains("`employment_income` (an [`income_type: EmploymentIncome`](#income) row")
    );
    assert!(SCHEMA_MD.contains("Dividend | EmploymentIncome | OtherIncome (CHECK-enforced enum"));
    // Cites the mirrored ATO ruling (TD 2017/26).
    assert!(limitations.contains("docs/ato/ess-dividend-equivalents.md"));
    assert!(include_str!("../docs/ato/ess-dividend-equivalents.md").contains("TD 2017/26"));
    // Surfaced in the README too.
    assert!(README_MD.contains("dividend equivalents on unvested RSU grants"));
    assert!(README_MD.contains("ordinary income when paid"));
    assert!(README_MD.contains("recordable as an income row of type `EmploymentIncome`"));
}

/// Docs-sync pin for the per-year ESS reduction eligibility flag (SCENARIOS
/// J-02, 2026-08-18): the ≤A$180,000 income test stays the user's answer, but
/// it is now *recordable* rather than only documented — the API documents the
/// entity and what an absent row means, the schema documents the table and its
/// audit identity, the tax summary says the ineligible year reports unreduced,
/// and the Known-limitations entry stops calling it merely the user's
/// responsibility.
#[test]
fn per_year_ess_reduction_eligibility_documented() {
    // The entity section, its default, and the reason it is per year.
    assert!(API_MD.contains("## Tax year settings"));
    assert!(API_MD.contains("PUT /tax_year_settings/2026"));
    assert!(API_MD.contains("**An absent row means every setting takes its default**"));
    assert!(API_MD.contains(
        "a single global flag would strip the reduction from years that never crossed the threshold"
    ));
    // The tax summary's own wording, and the printed document's footnote.
    assert!(API_MD.contains("reports its taxed-upfront discount **unreduced**"));
    assert!(
        API_MD.contains(
            "footnotes the condition under its ESS table whenever a reduction was applied"
        )
    );
    // The limitation is now "answered by you", not "unrecordable".
    assert!(
        known_limitations().contains("≤A$180,000 income test is answered by you, not computed")
    );
    // SCHEMA: the table, its year key, and its audit identity.
    assert!(
        SCHEMA_MD.contains("tax_year_settings            Per-financial-year taxpayer settings")
    );
    assert!(SCHEMA_MD.contains("ess_taxed_upfront_reduction_eligible INTEGER"));
    assert!(
        SCHEMA_MD
            .contains("for tax_year_settings, whose identity is the financial year, that year")
    );
    // README surfaces it where the ESS feature is described.
    assert!(README_MD.contains("**Tax Year Settings**"));
    assert!(README_MD.contains("recorded per financial year"));
}

/// Known-limitation pin (SCENARIOS J-04, 2026-08-18): the ESS 30-day rule is
/// **detected and named, never applied**. The scope cut is deliberate — the
/// re-measurement is the employer's amended statement to issue — so the docs
/// must say both halves: that the pattern is flagged (the health list and the
/// banner), and what the user does about it (enter the amended statement over
/// the original, not as a second row).
#[test]
fn known_limitations_document_the_ess_30_day_rule() {
    let limitations = known_limitations();
    assert!(limitations.contains("**The ESS 30-day rule is flagged, never applied**"));
    assert!(limitations.contains("within 30 days after** the deferred taxing point"));
    assert!(limitations.contains("no separate capital gain"));
    // The remedy, and why the tool won't do it for you.
    assert!(limitations.contains("amended ESS statement"));
    assert!(limitations.contains("enter the amended statement **over** the original"));
    // The detection half: the health list is documented as a field of the report.
    assert!(API_MD.contains("`ess_30_day_rule` — every disposal of ESS-vested shares"));
    // (Named in the report's JSON field list; the list itself keeps growing,
    // so the comma rather than the closing brace is what pins it.)
    assert!(API_MD.contains("\"ess_30_day_rule\","));
    // Cites the mirrored ATO guidance, which carries its QC header.
    assert!(limitations.contains("docs/ato/ess-30-day-rule.md"));
    assert!(include_str!("../docs/ato/ess-30-day-rule.md").contains("QC 23058"));
    // Surfaced in the README's health-monitoring feature line.
    assert!(README_MD.contains("**ESS sale inside the 30-day rule's window**"));
}

/// Doc pin (SCENARIOS AA-03): the gift entry convention is only safe if what
/// happens when it is *not* followed is written down. The market-value
/// substitution rule, the fabricated capital loss, the health check that names
/// it, and both exclusions (the operation-written closing Sells, and the free
/// right whose lapse is nil against nil) belong where a reader meets them —
/// the Health field list, the Gifts limitation, and the README's feature line.
#[test]
fn nil_proceeds_disposals_are_documented_with_the_market_value_rule() {
    assert!(
        API_MD.contains("`nil_proceeds_disposals` — every disposal recorded at **nil proceeds**")
    );
    assert!(API_MD.contains("\"non_trading_day_trades\", \"nil_proceeds_disposals\" }`"));
    // The rule it is about, cited to the mirror, which carries its QC header.
    assert!(API_MD.contains("docs/ato/capital-proceeds-market-value-substitution.md"));
    assert!(
        include_str!("../docs/ato/capital-proceeds-market-value-substitution.md")
            .contains("QC 66021")
    );
    // Advisory, and what to do about it.
    assert!(API_MD.contains("correct the proceeds to the market value on `date`"));
    // Both exclusions, so neither reads as an oversight.
    assert!(API_MD.contains("since none is a user entry; and a **free right that lapses**"));
    // The limitation that prescribes the convention says what breaks without it.
    assert!(known_limitations().contains(
        "Entering the nil consideration actually *received* instead is accepted in full and \
         fabricates a capital loss the size of the whole cost base"
    ));
    // Surfaced in the README's health-monitoring feature line.
    assert!(README_MD.contains("**disposal recorded at nil proceeds**"));
}

/// Doc pin (SCENARIOS S-08): a trade dated on a day its exchange did not
/// trade is refused, and the derived write paths that are exempt are named
/// where a reader would look — the 422 catalogue, the Trades and Sells
/// sections, the health report's field, and the schema's `trades.date`
/// comment. The exemption is the part that most needs writing down: it is
/// deliberate, not an oversight, and the health alert is the only thing
/// that covers those rows.
#[test]
fn trading_day_rule_and_its_exemptions_are_documented() {
    // The refusal, in the response-code catalogue beside its date twins.
    assert!(API_MD.contains("a trade or Sell dated on a day its exchange did not trade"));
    // The Trades and Sells sections state the rule in their own terms.
    assert!(API_MD.contains("**A trade's `date` must be a day its exchange actually traded.**"));
    assert!(
        API_MD.contains(
            "A Sell's `date`, like a trade's, must be a day its exchange actually traded"
        )
    );
    // Crypto and an unseeded year are exempt, and say so.
    assert!(
        API_MD.contains(
            "listings trade every day and are exempt; so is a year with no seeded holidays"
        )
    );
    // The derived paths are exempt on purpose, and covered by the alert.
    assert!(API_MD.contains(
        "The **derived** paths that write trade rows directly are deliberately **not** covered"
    ));
    assert!(API_MD.contains(
        "`non_trading_day_trades` — every trade dated on a day its own exchange did not trade"
    ));
    // In the endpoint's returned shape, beside the entry it follows (it is no
    // longer the last field, and pinning it as one would break on the next
    // check added).
    assert!(API_MD.contains("\"ess_30_day_rule\", \"non_trading_day_trades\""));
    // The one place the as-at calendar and the live-exchange settlement
    // calculation disagree is written down where the limitation lives.
    assert!(known_limitations().contains(
        "the calendar a trade is *judged* against and the calendar its settlement is *counted* on can be two different exchanges'"
    ));
    // The column comment carries the same rule.
    assert!(SCHEMA_MD.contains("It must also be a day the listing's own exchange traded"));
    // Surfaced in the README's health-monitoring feature line.
    assert!(README_MD.contains("**trade dated on a day its exchange was shut**"));
}

/// Doc pin (SCENARIOS S-05): the settlement-holiday-coverage report answers
/// two questions now, not one, and its contract sentence had to be corrected
/// — "trades fully inside coverage are omitted" stopped being the whole rule
/// the moment a trade inside coverage could be listed for a settlement date
/// that is not a trading day. The pin covers the corrected sentence, the
/// third `coverage_status` value, the new field, and the two places a reader
/// meets the rule instead: the Trades section (a supplied override is stored
/// as given) and the `trades.settlement_date` column comment.
#[test]
fn settlement_coverage_documents_both_questions_it_answers() {
    // The two questions, as the section poses them.
    assert!(API_MD.contains("**Was the window computed against a complete calendar?**"));
    assert!(API_MD.contains("**Is the settlement date itself a day the exchange traded?**"));
    // The corrected contract sentence: what an empty report does and does not mean.
    assert!(API_MD.contains(
        "an empty report means every settlement window sits inside a seeded calendar and every stored settlement date falls on a day its exchange traded"
    ));
    assert!(
        API_MD.contains(
            "It does **not** mean each stored date is what today's calendar would compute"
        )
    );
    // The payload: a third coverage_status value, and the sibling field that
    // carries the second answer (a row can hold both at once).
    assert!(API_MD.contains(
        "`inside_holiday_coverage` = the window was computed against a complete calendar"
    ));
    assert!(API_MD.contains("`settlement_non_trading_reason`"));
    // Not refused: the Trades section says a supplied value is stored as given.
    assert!(
        API_MD.contains("A `settlement_date` **supplied** in the body is stored exactly as given")
    );
    // The column comment carries the same rule.
    assert!(SCHEMA_MD.contains(
        "supplied, it is stored as given and need only not precede date (422 otherwise), because an explicit value is a deliberate override"
    ));
    // Surfaced in the README's feature line.
    assert!(
        README_MD.contains(
            "every stored settlement **date** is put to the listing's own trading calendar"
        )
    );
}

/// Docs-sync pin (SCENARIOS S-04): the settlement holiday-coverage report's
/// contract sentence was true only while a calendar was *incomplete* — seeding
/// the year it asks for cleared the alert without correcting the settlement
/// dates computed while the year was missing. The repair is the unscheduled
/// `settlement-recompute` job, so the sentence now names it, the job list
/// describes it and says what it will not rewrite, and the schema and the
/// Trades section document the provenance column that makes "what the server
/// computed" answerable at all.
#[test]
fn settlement_recompute_job_documented() {
    // The coverage report's contract sentence points at the repair.
    assert!(
        API_MD.contains("**Run the [`settlement-recompute` job](#jobs) after seeding a calendar**")
    );
    // The job list: registered, unscheduled, and what it leaves alone.
    assert!(API_MD.contains("and `settlement-recompute` (**not scheduled** either"));
    assert!(API_MD.contains(
        "a `settlement_date` supplied on the write is the taxpayer's own assertion and is never \
         rewritten"
    ));
    // The provenance column, in the API's Trades section and the schema.
    assert!(API_MD.contains("the read-only `settlement_date_source` field"));
    assert!(API_MD.contains(
        "Re-supplying the date **already stored** — which is what a `GET` body PUT back verbatim \
         does"
    ));
    assert!(
        SCHEMA_MD.contains("settlement_date_source TEXT (0041)  computed | stated | unrecorded")
    );
    // The README says the same in its feature line and its schedule note.
    assert!(README_MD.contains(
        "the unscheduled `settlement-recompute` job re-derives the settlement dates that were \
         computed while it was missing"
    ));
    assert!(README_MD.contains("So is `settlement-recompute`"));
}

/// Doc pin (SCENARIOS N-08): the 30-day rule turns on a **disposal**, so the
/// health alert's documented scope must say which Sells are one. A
/// holding-account transfer is not (the same owner holds the same interests),
/// and it is the ordinary RSU move, so the alert excludes it and follows the
/// rollover chain to keep a *later* real sale visible. A scrip-exchange or
/// demerger closing Sell is kept and labelled instead of being decided here,
/// because ITAA 1997 s 83A-130's conditions are facts this tool never records.
#[test]
fn health_documents_what_counts_as_a_disposal_for_the_ess_30_day_rule() {
    assert!(API_MD.contains("**What counts as a disposal**"));
    assert!(API_MD.contains("docs/ato/ess-takeovers-and-restructures.md"));
    assert!(API_MD.contains("`disposal_kind: \"TakeoverOrRestructure\"`"));
    assert!(API_MD.contains("The alert **follows the rollover chain**"));
    // The mirrored provision, with the subsections the wording above leans on.
    let mirror = include_str!("../docs/ato/ess-takeovers-and-restructures.md");
    assert!(mirror.contains("SECTION 83A-130") || mirror.contains("s 83A-130"));
    assert!(mirror.contains("**83A-130(2)**"));
    assert!(mirror.contains("**83A-130(5)**"));
    assert!(mirror.contains("**83A-130(9)**"));
    // Indexed in the ATO overview, which is where a reader is told to start.
    assert!(
        include_str!("../docs/ato/OVERVIEW.md").contains("[`ess-takeovers-and-restructures.md`]")
    );
}

/// Doc pin (SCENARIOS N-06, N-07): the three parcel-substituting operations
/// store the cost base their replacement parcels carry, so the docs must say
/// what closes that at write time, what the cross-check catches instead, what it
/// deliberately does not check (a partial-rollover scrip exchange's cash
/// apportionment), and that the annual tax report carries the rows unfiltered by
/// year.
#[test]
fn rollover_consistency_cross_check_documented() {
    assert!(API_MD.contains("### Rollover consistency"));
    assert!(API_MD.contains("GET /reports/rollover_consistency"));
    // What it compares, and the one case it declines to.
    assert!(API_MD.contains("per currency"));
    assert!(API_MD.contains("partial-rollover scrip exchange"));
    assert!(API_MD.contains("listed as *not checked*"));
    // The write-time half of the answer, on the corporate-action side.
    assert!(API_MD.contains(
        "**Recording one of the three read-time events behind a rollover that has already run.**"
    ));
    assert!(API_MD.contains("delete that operation, enter this event, then run it again"));
    // The tax report reads it unfiltered, and says why.
    assert!(API_MD.contains("`rollover_alerts`"));
    assert!(API_MD.contains("this year\'s disposals are costed on"));
    // README feature line.
    assert!(README_MD.contains("**Rollover consistency cross-check**"));
    // The second fault it reports (SCENARIOS V-d): an operation that consumed
    // the whole holding but left a parcel behind, the `kind` that exists only
    // for it, and the write-time refusal that stops any new one appearing.
    assert!(API_MD.contains("**unconsumed parcel**"));
    assert!(API_MD.contains("`WorthlessShares`"));
    assert!(
        API_MD
            .contains("**Entering a parcel behind an operation that consumed the whole holding.**")
    );
    assert!(API_MD.contains("delete that operation, enter the parcel, then run it again"));
    assert!(README_MD.contains("**unconsumed**"));
}

/// Known-limitation pin (SCENARIOS M-12, decided 2026-08-19): the FITO line
/// reaches a foreign-taxed **capital gain** only through the trust path — an
/// AMMA statement's `foreign_tax_credits_capital_gains`, apportioned to its
/// assessable part. Foreign tax on a disposal the taxpayer makes themselves
/// has no field: a Sell carries no foreign-tax column. The option of adding
/// one was weighed and cut, because the assets a source country actually taxes
/// a non-resident on (foreign real property, land-rich interests) are not
/// recordable here either — so the docs must state the gap, why it is narrow,
/// what the taxpayer does instead, and why the income-row workaround is wrong.
#[test]
fn known_limitations_document_foreign_tax_on_a_direct_disposal() {
    let limitations = known_limitations();
    assert!(
        limitations
            .contains("**Foreign tax on a capital gain you realise yourself is not recordable**")
    );
    // The two paths that do reach the offset, and the one that does not.
    assert!(limitations.contains("an [income](#income) row's `foreign_tax_paid`"));
    assert!(limitations.contains("`foreign_tax_credits_capital_gains`"));
    assert!(limitations.contains("a [Sell](#sells) carries **no foreign-tax column**"));
    // Why the gap is narrow rather than an oversight.
    assert!(limitations.contains("**real property** and land-rich interests"));
    assert!(limitations.contains("could not be entered either"));
    // What the taxpayer does instead.
    assert!(limitations.contains("**work that offset out separately and add it to 20O yourself**"));
    assert!(limitations.contains("Division 115 rule this report applies to the AMMA path"));
    // And why the obvious workaround is not one.
    assert!(limitations.contains("**Do not fake it as an income row**"));
    assert!(limitations.contains("join 20O **unapportioned**"));
    // Cites the mirrored ATO guidance, which carries its QC header and records
    // the same decision beside the calculation it bounds.
    assert!(limitations.contains("docs/ato/fito-capital-gains-apportionment.md"));
    let mirror = include_str!("../docs/ato/fito-capital-gains-apportionment.md");
    assert!(mirror.contains("QC 104349"));
    assert!(mirror.contains("**Only the trust path is recordable**"));
    // The tax-summary section says so where the AMMA apportionment is defined.
    assert!(API_MD.contains("**The trust path is the only capital-gains route to this line**"));
    // Surfaced in the README's scope-cut list.
    assert!(
        README_MD.contains("**foreign tax on a capital gain you realise yourself** has no field")
    );
}

/// Known-limitation pin (REQUIREMENTS "Ticker and exchange-code changes",
/// 2026-07-26; narrowed 2026-07-28 to settlement only, once price collection
/// started resolving its symbol and calendar as at the date fetched): an
/// exchange change recorded via `POST /listings/:id/rename` doesn't
/// retroactively pin historical trades to the calendar in force at the time —
/// re-saving a trade dated before the change without an explicit
/// `settlement_date` recomputes it against the listing's *current* exchange.
#[test]
fn known_limitations_document_exchange_change_recomputation() {
    let limitations = known_limitations();
    assert!(limitations.contains(
        "Settlement dates follow the listing's *current* exchange, not the date of the change"
    ));
    assert!(limitations.contains("settlement-holiday calendar"));
    assert!(limitations.contains("`trades.settlement_date` is a stored column"));
    assert!(limitations.contains("recomputes it against the exchange currently on the listing"));
    // The price path is explicitly carved out — it is no longer an instance
    // of this limitation.
    assert!(limitations.contains("no longer shares this limitation"));
}

/// Price collection resolves the provider symbol and the trading calendar as
/// at the date being fetched, from the rename chain (2026-07-28) — the fix
/// that made the fetch and snapshot-valuation paths agree across a rename.
#[test]
fn as_at_price_symbol_resolution_documented() {
    assert!(API_MD.contains("The symbol is resolved **as at the date being fetched**"));
    assert!(API_MD.contains("one provider call per identity"));
    // `price_symbol` is the current spelling, so it must not relabel history.
    assert!(API_MD.contains("applies only to dates in the listing's current identity"));
    // The collection window and the snapshot catch-up window are one length.
    assert!(API_MD.contains("same length as the [report-snapshot](#report-snapshots) catch-up"));
    assert!(README_MD.contains("under the symbol in force on each date"));
}

/// Known-limitation pin (REQUIREMENTS "Ticker and exchange-code changes",
/// 2026-07-26): a rename carries no staleness trigger for `listing_renames`
/// (the ticker is a display label over `listing_id`, never a computed
/// figure), so a snapshot generated before a rename keeps its pre-rename
/// ticker label until regenerated.
#[test]
fn known_limitations_document_snapshot_ticker_labels_are_display_only() {
    let limitations = known_limitations();
    assert!(limitations.contains("Snapshot ticker labels are display-only"));
    assert!(limitations.contains("`listing_renames` deliberately carries no staleness trigger"));
    assert!(limitations.contains("This is display drift only"));
}

/// Known-limitation pin (SCENARIOS R-10, 2026-08-21): a listing's ticker is
/// unique across its whole recorded history — `UNIQUE(exchange_mic, ticker)`
/// and the exchange-less partial index hold across all time — while the rest
/// of the model resolves identity as at a date. So an exchange code reissued
/// to an unrelated company cannot be entered while the first listing is on
/// file, the fake-rename workaround is ruled out, and the entry says what to
/// do instead.
#[test]
fn known_limitations_document_a_reissued_ticker_cannot_be_recorded() {
    let limitations = known_limitations();
    assert!(
        limitations.contains(
            "**A ticker an exchange reissues to a different company cannot be recorded**"
        )
    );
    // The rule, and that it is the whole history rather than today's identity.
    assert!(limitations.contains("unique across its **whole recorded history**"));
    assert!(limitations.contains("`UNIQUE(exchange_mic, ticker)`"));
    assert!(limitations.contains("the partial unique index over the bare ticker"));
    assert!(
        limitations
            .contains("the rest of the model treats a listing's identity as **time-varying**")
    );
    // The refusal, and why the first listing never leaves.
    assert!(
        limitations.contains("UNIQUE constraint failed: listings.exchange_mic, listings.ticker")
    );
    assert!(
        limitations.contains("a disposed holding's parcels, income and price history all stay")
    );
    assert!(limitations.contains("UNIQUE constraint failed: listings.ticker"));
    // The fake rename is ruled out, with both of its consequences.
    assert!(
        limitations.contains("**Recording a rename that never happened is explicitly ruled out**")
    );
    assert!(limitations.contains("yahoo fetch for AAAOLD.AX failed: Not found"));
    assert!(
        limitations
            .contains("prints that invented ticker on the security's disposal and income rows")
    );
    // What to do instead, all of it verified against the live system.
    assert!(limitations.contains("**What to do instead**"));
    assert!(limitations.contains("the same ticker under a different MIC is already accepted"));
    assert!(limitations.contains("a listing with no recorded rename has a single identity span"));
    assert!(limitations.contains("set [`unpriced_from`](#listings) at its last quoted day"));
    assert!(limitations.contains("check `fetched_symbol` on anything backfilled across a reissue"));
    // The marker's bound, stated correctly: a reissued series cannot begin
    // before the delisting that freed the code, so `unpriced_from` covers
    // every date the new company's series can reach. The residual is a
    // provider splicing one series per spelling, which Yahoo does not do
    // (a 2005 backfill under a later-reissued code answers HTTP 400).
    assert!(limitations.contains("That marker covers the whole window a reissue can contaminate"));
    assert!(limitations.contains("one continuous series per ticker spelling"));
}

/// Ticker/exchange renames (REQUIREMENTS "Ticker and exchange-code changes",
/// 2026-07-26): a rename is an explicit, dated, audited event once a listing
/// has history — the rename action, the provider-symbol override, and the
/// as-at ticker resolution used by the tax report and activity ledger are
/// all documented in the Listings section, and the feature is in README.
#[test]
fn listing_rename_action_documented() {
    assert!(API_MD.contains("POST /listings/:id/rename"));
    assert!(API_MD.contains("GET /listings/:id/renames"));
    assert!(API_MD.contains("DELETE /listings/:id/renames/:rename_id"));
    assert!(API_MD.contains("price_symbol"));
    assert!(API_MD.contains("resolves **as at its own date**"));
    assert!(README_MD.contains("Ticker and exchange-code renames"));
}

/// What the activity ledger resolves as at a date (SCENARIOS R-07,
/// 2026-08-21): the ledger has no per-row ticker column — it is one listing's
/// own history — but a scrip-for-scrip/demerger row names its **counterpart**
/// listing at the ticker that listing held on the action's own date. Both the
/// rename section's exception sentence and the row-kind list say so, so the
/// docs describe what `reports::activity` actually does.
#[test]
fn activity_ledger_resolves_counterpart_tickers_as_at_the_rows_date() {
    assert!(
        API_MD.contains(
            "The [listing activity ledger](#listing-activity) has no per-row ticker column"
        )
    );
    assert!(API_MD.contains("never one it was only renamed to later"));
    assert!(
        API_MD.contains("names its **counterpart listing** by the ticker that listing carried")
    );
    assert!(API_MD.contains("**as at the action's own date**"));
}

/// Docs-sync pin for the undo restoring what the rename overwrote (SCENARIOS
/// R-04/R-08, 2026-08-21): the API states which fields come back, why
/// `price_symbol` is among them, that "the listing had no override" is
/// restorable as such, and that a rename recorded before migration 0040
/// recorded neither — its NULLs meaning unrecorded, not "restore to null".
/// The schema documents both columns and the CHECK that makes the reading
/// enforceable.
#[test]
fn rename_undo_restores_what_it_overwrote_documented() {
    assert!(
        API_MD
            .contains("`new_exchange_mic`, `old_name`, `old_price_symbol`, `note`), newest first")
    );
    assert!(API_MD.contains(
        "restoring **all four** fields a rename can change, \
         `ticker`/`exchange_mic`/`name`/`price_symbol`, from its `old_*` columns"
    ));
    assert!(API_MD.contains("`price_symbol` is restored because it is not cosmetic"));
    assert!(API_MD.contains("**A rename recorded before 0040 recorded neither**"));
    assert!(API_MD.contains("means *unrecorded*, never \"restore to null\""));
    assert!(SCHEMA_MD.contains("old_name         TEXT (nullable, 0040)"));
    assert!(SCHEMA_MD.contains("old_price_symbol TEXT (nullable, 0040)"));
    assert!(SCHEMA_MD.contains("CHECK: old_name IS NOT NULL OR old_price_symbol IS NULL"));
}

/// Resolved-limitation pin (REQUIREMENTS 2026-07-13; formerly the
/// "Foreign broker-cash interest classification" Known limitation, 2026-06-12):
/// interest income is classified by payer — Australian-source at question 10
/// (10L), foreign-source (broker-cash/money-market income) at 20E assessable
/// foreign source income with its foreign tax joining the FITO — and the
/// limitation entry is gone from Known limitations.
#[test]
fn docs_document_foreign_interest_source_classification() {
    // The limitation no longer exists — the classification is a feature.
    let limitations = known_limitations();
    assert!(!limitations.contains("**Foreign broker-cash interest classification**"));
    // The interest-income entity section documents the 20E routing…
    assert!(API_MD.contains(
        "instead reports as assessable foreign source income on the `foreign_interest_income` \
         line (question 20, label 20E)"
    ));
    // …and the label mapping carries the foreign interest column at 20E.
    assert!(API_MD.contains("| `interest_income` | `10L` | Australian gross interest"));
    assert!(API_MD.contains(
        "| `foreign_interest_income`, `foreign_source_income`, `amma_foreign_income` | `20E + 20M`"
    ));
    // The mirrored label reference carries both labels.
    let labels = include_str!("../docs/ato/tax-return-labels-2026.md");
    assert!(labels.contains("| 10L | Gross interest"));
    assert!(labels.contains("| 20E | Assessable foreign source income"));
    // Surfaced in the README too.
    assert!(README_MD.contains("**foreign-source** row"));
    assert!(README_MD.contains("20E assessable foreign source income"));
}

/// Docs-sync pin for the collectables / personal-use-asset scope decision
/// (2026-07-29, QC 106842): their capital losses are *quarantined* — a
/// collectable's loss can only reduce a gain from another collectable, and a
/// personal-use asset's loss is disregarded entirely — but this system has one
/// loss pool and no asset-class dimension, so entering one would wrongly
/// offset share gains. That has to be stated, not just omitted, because the
/// failure is silent: the net capital gain simply comes out too low. This is
/// the limitation the Kathleen acceptance test's jewellery leg cites for
/// staying out (`src/ato_examples.rs`).
#[test]
fn known_limitations_document_quarantined_collectable_losses() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Collectables and personal-use assets"));
    assert!(limitations.contains("quarantined"));
    // The two distinct rules, and why entering one here would be wrong.
    assert!(limitations.contains("reduce a capital gain from another collectable"));
    assert!(limitations.contains("never a gain on shares"));
    assert!(limitations.contains("disregarded entirely"));
    assert!(limitations.contains("wrongly offset share gains"));
    // Cites the mirrored ATO source carrying the Kathleen examples.
    assert!(limitations.contains("docs/ato/capital-gains-question-18.md"));
    let mirror = include_str!("../docs/ato/capital-gains-question-18.md");
    assert!(mirror.contains("QC 106842"));
    // The mirror states the step order the net-capital-gain report implements.
    assert!(mirror.contains("before applying the CGT discount"));
    assert!(mirror.contains("$1,260"));
}

/// Docs-sync pin for what the annual tax report's year picker offers
/// (SCENARIOS P-02/P-03/P-04, 2026-08-20): the picker is a closed `<select>`
/// and the only way to reach the report, so it lists every year the report has
/// content for — income by its **assessment date**, every year the
/// net-capital-gain walk emits (the CGT events that are not trades, plus the
/// quiet carry-forward years), and the remaining fact dates — while a year
/// with nothing in it stays absent and no year outside the accepted
/// `tax_year` range is ever offered.
#[test]
fn tax_report_year_picker_scope_documented() {
    assert!(API_MD.contains("every Australian financial year this report has content for"));
    assert!(API_MD.contains("**income by its assessment date**"));
    assert!(API_MD.contains(
        "every year the [net capital gain](#net-capital-gain) report's own year walk emits"
    ));
    assert!(API_MD.contains("realised disposals, rights sales, CGT events E10/G1/C2"));
    assert!(API_MD.contains("plus every quiet year still carrying a capital loss forward"));
    // Honest in both directions, and never offering a year the POST refuses.
    assert!(API_MD.contains("A year with nothing in it and no loss balance is still absent"));
    assert!(API_MD.contains("every listed year is one `POST /reports/tax-report` answers"));
    // The superseded note — "request a quiet year by `tax_year` directly" —
    // described a picker that could not reach such a year, and is gone.
    assert!(!API_MD.contains("request a quiet year by `tax_year` directly"));
}

/// Docs-sync pin for which years the net-capital-gain series covers
/// (SCENARIOS O-03/O-04/O-12, 2026-08-19): a quiet year carrying a capital
/// loss forward is reported — label 18V is reported every year until the loss
/// is used, not only in years with a CGT event — through to the financial year
/// in progress, while a year with neither activity nor a balance stays absent.
/// The annual tax report's `cgt_summary` says the same about its own `null`.
#[test]
fn net_capital_gain_year_series_documented() {
    assert!(API_MD.contains("**Which years get a record.**"));
    assert!(API_MD.contains("plus every quiet year that still carries a capital loss forward"));
    assert!(API_MD.contains(
        "reported every year until the loss is used, not only in years with a CGT event"
    ));
    // The bound, and the opening-loss-only database it has to cover.
    assert!(API_MD.contains("run through to the financial year **in progress**"));
    assert!(API_MD.contains("a pre-system balance attributed to no year"));
    // Still sparse: no row for a year with neither activity nor a balance.
    assert!(API_MD.contains("A year with neither activity nor a balance gets no record at all"));
    assert!(API_MD.contains("**not** a continuous year-by-year series"));
    // The annual tax report's own `null` wording follows from the same walk.
    assert!(API_MD.contains(
        "`null` when the year has neither gain/loss activity recorded nor a capital loss brought forward into it"
    ));
    // The cited mirror carries the step the rule comes from (QC 106842).
    let mirror = include_str!("../docs/ato/capital-gains-question-18.md");
    assert!(mirror.contains("QC 106842"));
    assert!(mirror.contains("**Step 11 — Capital losses carried forward**"));
}

/// Docs-sync pin for the indexation scope cut (SCENARIOS AA-a). Two halves:
/// what is *still* out of scope (the election), and — the finding itself —
/// that the reason once given for it, "the discount almost always gives an
/// individual the better result", is gone and replaced by the actual
/// boundary. The behaviour half is
/// `reports::indexation_cross_check::tests` and
/// `domain::indexation::tests::the_earliest_enterable_acquisition_indexes_at_1_730`.
#[test]
fn known_limitations_document_indexation_method() {
    let limitations = known_limitations();
    // The election (pre-22 September 1999 costs, frozen at Sep 1999) is not
    // modelled; the 50% discount is applied throughout.
    assert!(limitations.contains("**Indexation method**"));
    assert!(limitations.contains("incurred by **21 September 1999**"));
    assert!(limitations.contains("frozen at the 30 September 1999 CPI"));
    assert!(limitations.contains("**election is not modelled**"));
    assert!(limitations.contains("50% discount is applied throughout"));
    // The withdrawn claim, and the boundary that replaced it — the factor for
    // the earliest enterable acquisition and the crossover it implies.
    // The phrase survives in exactly one place: the sentence withdrawing it.
    assert_eq!(limitations.matches("almost always").count(), 1);
    assert!(limitations.contains(
        "This entry used to say the discount \"almost always\" gives the better result. That \
         was wrong for exactly the parcels most likely to be affected, and the claim is \
         withdrawn."
    ));
    assert!(limitations.contains("68.7 ÷ 39.7 = **1.730**"));
    assert!(limitations.contains("proceeds are below **2.460 × cost**"));
    // What exists instead of the election, and the promise it is made under.
    assert!(limitations.contains("[indexation cross-check](#indexation-cross-check)"));
    assert!(limitations.contains("**No reported tax figure is computed from any of it.**"));
    // Cites the mirrored ATO guidance (QC 66024) and the CPI series the
    // factor is derived from (QC 104764).
    assert!(limitations.contains("docs/ato/indexing-the-cost-base.md"));
    assert!(include_str!("../docs/ato/indexing-the-cost-base.md").contains("QC 66024"));
    assert!(limitations.contains("docs/ato/consumer-price-index.md"));
    let cpi_mirror = include_str!("../docs/ato/consumer-price-index.md");
    assert!(cpi_mirror.contains("QC 104764"));
    // The two figures the whole method turns on, in the mirror itself.
    assert!(cpi_mirror.contains("| 1985 | – | – | 39.7 | 40.5 |"));
    assert!(cpi_mirror.contains("| 1999 | 67.8 | 68.1 | 68.7 | n/a (see Note 1) |"));
    assert!(README_MD.contains("indexation method"));
    assert!(README_MD.contains("50% discount is applied throughout"));
}

/// Docs-sync pin for the indexation cross-check report (SCENARIOS AA-a): the
/// section exists, and it states the two things that make it honest rather
/// than merely more figures on a page — that no tax figure it reports is
/// affected by it, and the exact comparison each row is making (per parcel,
/// before capital losses, and a floor on indexation's case rather than the
/// whole answer). The behaviour half is
/// `reports::indexation_cross_check::tests`.
#[test]
fn indexation_cross_check_is_documented() {
    assert!(API_MD.contains("### Indexation cross-check"));
    assert!(API_MD.contains("GET /reports/indexation_cross_check"));
    assert!(API_MD.contains("**Advisory only, and nothing here changes a reported tax figure**"));
    // The comparison, stated.
    assert!(API_MD.contains(
        "Each row compares one parcel **in the absence of capital losses applied against its \
         gain**"
    ));
    assert!(
        API_MD.contains("**Read the rows as a floor on indexation's case, not the whole answer**")
    );
    // Why per parcel rather than per disposal or per year.
    assert!(API_MD.contains(
        "The comparison is stated **per parcel allocation** rather than per disposal or per year"
    ));
    // The two deliberate exclusions.
    assert!(API_MD.contains("since indexation cannot be used on a capital loss at all"));
    assert!(API_MD.contains("A loss allocation is therefore not shown as \"the discount wins\""));
    // The rounding rule the factor is derived under.
    assert!(
        API_MD.contains("limited to 3 decimal places with the fourth decimal rounded up from 5")
    );
    // The seeded reference table is in the schema doc with its source.
    assert!(SCHEMA_MD.contains("cpi_quarters"));
    assert!(SCHEMA_MD.contains("QC 104764"));
}

/// Docs-sync pin for the joint-ownership entry convention (SCENARIOS AA-e,
/// scenario AA-06): a jointly held parcel is entered as *your own share* —
/// half a 1,000-unit registry holding is a 500-unit Buy — and the statement's
/// per-share figures follow, `securities_held` keyed to your own units while
/// `amount_per_security` stays the statement's per-unit rate. The two are
/// cross-checked against the entered cash at write time
/// (`entities::income::check_per_share`), so the wrong keying is a `422`, and
/// the convention is the one *Inherited parcels* already prescribes.
#[test]
fn known_limitations_document_the_joint_ownership_entry_convention() {
    let limitations = known_limitations();
    assert!(limitations.contains("**A jointly held parcel is entered as your own share of it**"));
    // Concrete: your half of a 1,000-unit holding is a 500-unit Buy.
    assert!(
        limitations
            .contains("a 50% interest in a 1,000-unit registry holding is a Buy of **500** units")
    );
    // Which of the two per-share figures moves, and which does not.
    assert!(limitations.contains(
        "`amount_per_security` stays the statement's per-unit rate while `securities_held` is **your own** unit count"
    ));
    // The write-time cross-check is what makes the wrong keying visible.
    assert!(limitations.contains("is a `422` naming the product it computed"));
    // Same convention as the inherited-parcel split, and the same cost.
    assert!(limitations.contains("the convention *Inherited parcels* below already prescribes"));
    assert!(limitations.contains("will not tie back to the registry's holding statement"));
    // The README carries the joint-holding half of the scope cut.
    assert!(README_MD.contains("500 units of a 1,000-unit joint holding"));
}

/// Docs-sync pin for the second-taxpayer remedy (SCENARIOS AA-e, scenario
/// AA-19): one database and one instance per taxpayer (`--db`, `--port`),
/// because the easy wrong answer — a spouse as another holding account —
/// aggregates two taxpayers into one of every per-taxpayer figure and nothing
/// can see it.
#[test]
fn known_limitations_document_the_second_taxpayer_remedy() {
    let limitations = known_limitations();
    assert!(
        limitations.contains("**A second taxpayer is a second database and a second instance**")
    );
    assert!(limitations.contains("each with its own `--db` and `--port`"));
    // The wrong answer, named, and what it silently pools.
    assert!(
        limitations
            .contains("must *not* be done is entering a spouse's or a trust's holdings as another")
    );
    assert!(
        limitations.contains(
            "A$5,000 small-shareholder franking threshold and one A$1,000 FITO de-minimis"
        )
    );
    // Explicit that it cannot be detected.
    assert!(
        limitations
            .contains("aggregating what is in the database is exactly what these reports are for")
    );
    // The README carries the same scope cut beside the other named ones.
    assert!(README_MD.contains("**one taxpayer per database**"));
    assert!(README_MD.contains("never a second holding account"));
    // Both flags the remedy needs are in the README's options table.
    assert!(README_MD.contains("| `--db` |"));
    assert!(README_MD.contains("| `--port` |"));
}

/// Docs-sync pin for the second cost-base element (SCENARIOS AA-e, scenario
/// AA-08): element 2 is wider than the one field it has, the ATO's other
/// incidental costs are named, and the convention that works — fold the cost
/// into the trade's `brokerage`, say what it was in `contract_note_ref` — is
/// documented with its three traps (the GST-inclusive split, the
/// `statement_total` reconciliation, and the disposal side's netting).
#[test]
fn known_limitations_document_the_element_two_incidental_cost_convention() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Element 2 is wider than the one field it has**"));
    // The ATO's element-2 costs a listed-share investor actually meets.
    assert!(limitations.contains("**costs of transfer**"));
    assert!(limitations.contains("**stamp duty or other similar duty**"));
    assert!(
        limitations.contains(
            "**remuneration for a broker, agent, accountant, consultant or legal adviser**"
        )
    );
    // The convention, and that it is exact rather than an approximation.
    assert!(limitations.contains(
        "**The convention is to fold such a cost into the trade's `brokerage` and say what it really was in `contract_note_ref`.**"
    ));
    assert!(limitations.contains("A$500 of transfer duty reports a A$1,500 cost base"));
    // The three traps.
    assert!(limitations.contains("would invent A$45.45 of GST on a A$500 duty"));
    assert!(
        limitations.contains(
            "A supplied `statement_total` must be the total *including* the folded-in cost"
        )
    );
    assert!(limitations.contains("**netted off proceeds rather than added to the cost base**"));
    assert!(limitations.contains("*Where a Sell's brokerage and GST land*"));
    // That passage is where it says it is, in the realised-gains section.
    assert!(API_MD.contains("**Where a Sell's brokerage and GST land.**"));
    // The ATO source the element-2 list is re-derived from.
    assert!(limitations.contains("docs/ato/cgt-cost-base.md"));
    let mirror = include_str!("../docs/ato/cgt-cost-base.md");
    assert!(mirror.contains("Second element: incidental costs"));
    assert!(mirror.contains("stamp duty or other similar duty"));
    assert!(mirror.contains("costs of transfer"));
}

/// Docs-sync pin for the Division 775 forex omission (SCENARIOS AA-e, scenario
/// AA-12): it is its own bullet, sited with the other FX limitations rather
/// than buried in *Crypto assets*, and it says the honest thing — there is no
/// entry path at all, because an [income] row requires a `listing_id`
/// (`entities::income::IncomeBody::listing_id` is an `i64`, not an `Option`)
/// and a currency balance has no listing. The crypto bullet keeps its own
/// load-bearing half — the deferral never reaches a crypto holding — and the
/// two cross-reference each other instead of duplicating.
#[test]
fn known_limitations_document_the_division_775_forex_omission() {
    let limitations = known_limitations();
    assert!(
        limitations
            .contains("- **Foreign-currency cash balances — Division 775 forex gains and losses**")
    );
    assert!(
        limitations
            .contains("assessable ordinary income and deductions under Division 775, not CGT")
    );
    // The honest part: no workaround, and why.
    assert!(limitations.contains("there is no entry path at all"));
    assert!(limitations.contains("an [income](#income) row's `listing_id` is **required**"));
    assert!(limitations.contains("a currency balance has no [listing](#listings) to point at"));
    assert!(limitations.contains("adds it to their return outside this tool"));
    // Sited beside the other FX limitations, and cross-referenced both ways.
    assert!(limitations.contains("*Settlement-window forex on foreign-currency trades* above"));
    assert!(limitations.contains("(*Crypto assets* above)"));
    assert!(
        limitations.contains("has its own bullet below and **never reaches a crypto holding**")
    );
    // The crypto bullet keeps the exclusion itself, with its authorities.
    assert!(limitations.contains("is not 'foreign currency' for Division 775"));
    assert!(limitations.contains("TD 2014/25 and the 2023 statutory exclusion"));
    // Cites the mirrored ATO guidance (QC 18322).
    assert!(limitations.contains("docs/ato/forex-common-transactions.md"));
    assert!(include_str!("../docs/ato/forex-common-transactions.md").contains("QC 18322"));
    // The README carries the no-entry-path scope cut too.
    assert!(README_MD.contains("**foreign-currency cash balances**"));
    assert!(README_MD.contains("**no entry path at all**"));
}

/// Docs-sync pin for the capital-account assumption (SCENARIOS AA-c): every
/// figure this system produces assumes a share **investor** holding CGT assets,
/// never a **share trader** whose shares are trading stock (QC 66047). Nothing
/// stored distinguishes the two, so there is no refusal and no flag to pin —
/// the assumption itself is the deliverable, in `docs/API.md`, in the README's
/// scope-cut paragraph, and cross-referenced against *Taxpayer entity type*,
/// which is the other axis (which taxpayer, and the rate) and does not cover it.
#[test]
fn known_limitations_document_the_investor_not_share_trader_assumption() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Investor, not share trader**"));
    assert!(limitations.contains("**CGT assets held on capital account**"));
    assert!(limitations.contains("**trading stock**"));
    // What goes wrong, concretely, and that nothing can see it.
    assert!(limitations.contains("**assessable as ordinary income**"));
    assert!(limitations.contains("**deductible in the year incurred**"));
    assert!(limitations.contains("**Nothing here can detect which one you are**"));
    // The year-end trading-stock valuation is named as unmodelled.
    assert!(limitations.contains("s 70-35"));
    // The two axes cross-reference each other, so neither reads as the other.
    assert!(limitations.contains("*Investor, not share trader* below"));
    // Cites the mirrored ATO guidance (QC 66047).
    assert!(limitations.contains("docs/ato/share-investing-versus-share-trading.md"));
    let mirror = include_str!("../docs/ato/share-investing-versus-share-trading.md");
    assert!(mirror.contains("QC 66047"));
    assert!(mirror.contains(
        "your shares are treated like trading stock in the ordinary course of a business"
    ));
    assert!(mirror.contains("**CGT event K4**"));
    // The README carries the same scope cut beside the other named ones.
    assert!(README_MD.contains("share **investor** holding CGT assets on capital account"));
    assert!(README_MD.contains("**share trader** carrying on a business"));
    assert!(README_MD.contains("nothing here can tell the two apart"));
}

/// Docs-sync pin for the FX spot-rate override (REQUIREMENTS 2026-06-12, QC
/// 18020): the FX conversion section states the precedence rule honestly —
/// spot override first, monthly RBA rate as the ATO-published convenience
/// default, `fx_rate` fallback, loud failure — and says when the ATO expects
/// a spot rate; the Trades section documents the field and its 422s; the
/// README surfaces the override; and the cited ATO mirrors carry their QC
/// headers.
#[test]
fn fx_spot_rate_override_documented() {
    // The FX conversion section: full precedence and the honesty note.
    assert!(API_MD.contains("**`spot_fx_rate` override**"));
    assert!(API_MD.contains("monthly rate is the ATO-published convenience default"));
    assert!(
        API_MD.contains(
            "**not appropriate for a one-off purchase or sale of a large capital asset**"
        )
    );
    assert!(API_MD.contains("used only when no ATO rate has been imported"));
    // The Trades section: deliberate entry, the 422s, and the rollover carry.
    assert!(
        API_MD
            .contains("rejected with `422` when non-positive or supplied on an AUD-currency trade")
    );
    assert!(API_MD.contains("carry a consumed parcel's override onto its replacement Buys"));
    // Cited mirrors carry their QC headers.
    assert!(API_MD.contains("docs/ato/forex-average-rates.md"));
    assert!(include_str!("../docs/ato/forex-average-rates.md").contains("QC 18020"));
    assert!(API_MD.contains("docs/ato/forex-common-transactions.md"));
    assert!(include_str!("../docs/ato/forex-common-transactions.md").contains("QC 18322"));
    // README features bullet.
    assert!(README_MD.contains("transaction-date spot-rate override"));
    assert!(README_MD.contains("QC 18020"));
}

/// Doc-only resolution pin for cost-base FX timing (2026-07-12 review,
/// decided 2026-07-13 as a documented limitation): `CostBase::into_aud_with`
/// converts the whole breakdown — including AMIT (E10) and return-of-capital
/// (G1) reductions from later rate months — at the parcel's acquisition-month
/// rate, a deliberate simplification of the s 960-50(6) per-transaction
/// translation timing. The Known-limitations entry states the rule, the
/// g1_gains asymmetry (a payment's excess converts at the payment month while
/// its reduction converts at the acquisition month), and when it bites (a
/// non-AUD holding with non-AUD reductions — none in practice); cites the
/// QC 18322 mirror; is cross-linked from the FX-conversion section; and the
/// README surfaces it.
#[test]
fn known_limitations_document_cost_base_fx_timing() {
    let limitations = known_limitations();
    assert!(limitations.contains(
        "**Cost-base FX timing — AMIT/return-of-capital reductions convert at the \
         acquisition-month rate**"
    ));
    assert!(limitations.contains("**one rate: the parcel's (possibly deemed) acquisition month**"));
    // The strict rule and its citation.
    assert!(limitations.contains("s 960-50(6) translation rules"));
    assert!(limitations.contains("docs/ato/forex-common-transactions.md"));
    assert!(include_str!("../docs/ato/forex-common-transactions.md").contains("QC 18322"));
    // The g1_gains asymmetry, both halves.
    assert!(limitations.contains("converts at the **payment month**"));
    assert!(limitations.contains("converts at the **acquisition month**"));
    // When it bites.
    assert!(
        limitations
            .contains("**non-AUD holding receiving non-AUD AMIT/return-of-capital reductions**")
    );
    // Cross-linked from the FX-conversion section.
    assert!(API_MD.contains(
        "including the AMIT/return-of-capital reductions that arose in later rate months \
         (see [Known limitations](#known-limitations))"
    ));
    // Surfaced in the README too.
    assert!(README_MD.contains("cost-base reductions convert at the acquisition-month rate"));
    assert!(README_MD.contains("s 960-50 translation timing"));
}

/// Docs pin for the brokerage-currency invariant (SCENARIOS B-02, decided
/// 2026-08-15): the write-time refusal has its own tests
/// (`trade::tests::api_brokerage_in_another_currency_than_the_trade_returns_422`,
/// the Sell twin, and the open-parcels figure), but the *documented* half —
/// what a user with a foreign-currency fee is supposed to do instead, and why
/// the strict s 960-50 per-leg translation is not modelled — lives only in the
/// docs. The Known-limitations entry states the rule, the workaround and the
/// citation; the Trades section explains what the field means for the cost
/// base; the schema records the constraint; and the README surfaces it.
#[test]
fn known_limitations_document_the_brokerage_currency_invariant() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Brokerage is billed in the trade's own currency**"));
    // The rule, the reason, and the entry route it leaves the user.
    assert!(limitations.contains("`brokerage_currency` differs from its `currency` is rejected"));
    assert!(limitations.contains(
        "**An Australian broker's AUD commission on a US trade is entered converted into the \
         trade's currency**"
    ));
    // The strict rule that is deliberately not modelled, and its mirror.
    assert!(limitations.contains("s 960-50 translates each amount at its own time"));
    assert!(limitations.contains("docs/ato/forex-common-transactions.md"));
    // The Trades section says what the field means for the cost base.
    assert!(API_MD.contains(
        "`brokerage_currency` records the currency the fee was billed in, and **must equal the \
         trade's `currency`**"
    ));
    // The schema carries the constraint on the column itself.
    assert!(SCHEMA_MD.contains("write-time validated to equal `currency` (422 otherwise)"));
    // Surfaced in the README too.
    assert!(README_MD.contains("**brokerage is billed in the trade's own currency**"));

    // SCENARIOS L-08: the crypto shape of the same rule, and which of its
    // three cases is a second CGT event.
    assert!(limitations.contains("**A crypto exchange's fee billed in crypto**"));
    assert!(limitations.contains("netted out of the units you receive"));
    assert!(limitations.contains("taken from the units you sold"));
    assert!(limitations.contains("paid in a third asset you hold"));
    assert!(limitations.contains("*is* a disposal of those units at their market value"));
    assert!(limitations.contains("a trade does not"));
}

/// Docs pin for the return-of-capital currency invariant (SCENARIOS E-07 /
/// E-39, fixed 2026-08-16): both write-time refusals have their own tests
/// (`corporate_action::tests::api_payment_in_another_currency_than_its_parcels_returns_422`
/// and the parcel-side twin in `trade::tests`), but the *documented* half —
/// which parcels the check covers, that the two entry paths refuse the pair
/// from either side, and the one residual route into the mismatch (a rollover
/// carrying a parcel's own currency onto a listing that already has a
/// differing payment) — lives only in the docs.
#[test]
fn return_of_capital_currency_invariant_documented() {
    // The rule and its scope, in the Corporate actions section.
    assert!(API_MD.contains(
        "A `ReturnOfCapital`'s `currency` must match the currency of the parcels it reduces"
    ));
    assert!(API_MD.contains("a differing pair is rejected with `422` naming both currencies"));
    assert!(API_MD.contains(
        "those acquired before its `record_date`, or on or before the payment date when none is \
         recorded"
    ));
    // A rollover-created parcel keeps the original's currency, so the listed
    // currency is the wrong one to reach for — and that is the residual route
    // into the mismatch the reports still fail loudly on.
    assert!(
        API_MD.contains("record its return of capital in the parcels' currency, not the listing's")
    );
    assert!(API_MD.contains(
        "a replacement parcel created *after* a differing payment was recorded is the one \
         remaining way to meet the mismatch"
    ));
    // The parcel side of the same rejection, in the Trades section.
    assert!(API_MD.contains(
        "`PUT /trades/:id` returns `422` if a Buy/DRP's `currency` differs from that of a \
         [return of capital](#corporate-actions) recorded on its listing that reaches it"
    ));
    // The schema carries the constraint on the column itself.
    assert!(SCHEMA_MD.contains(
        "write-time validated to equal the currency of the parcels the payment reaches (422 \
         otherwise)"
    ));
}

/// Docs-sync pin for the backup pipeline hardening (2026-07-13 improvement
/// review) and the later `--backup-command` off-machine-copy hook: the README
/// documents verification (integrity check + migrations match, `.bad`
/// quarantine), the retention policy (newest 8 + 12 monthly keepers,
/// pattern-matched files only), and — the recorded decision for the
/// off-machine copy — that the server never embeds remote credentials or
/// provider-specific upload logic itself, only ever shelling out to an
/// operator-configured command (`--backup-command`) or leaving it to an
/// independent cron job; the jobs API documents the backup job's failure
/// semantics.
#[test]
fn backup_pipeline_documented() {
    // Verification + quarantine.
    assert!(README_MD.contains("must pass `PRAGMA integrity_check`"));
    assert!(README_MD.contains("applied migrations must match the live database's"));
    assert!(README_MD.contains("quarantined by renaming it to `<name>.db.bad`"));
    assert!(README_MD.contains("never left looking like a good backup"));
    // Retention policy, and its only-pattern-matched-files deletion scope.
    assert!(README_MD.contains("**newest 8 backups**"));
    assert!(
        README_MD.contains("**first backup of each calendar month for the 12 most recent months**")
    );
    assert!(README_MD.contains(
        "Pruning deletes only files matching the backup filename pattern for this database"
    ));
    // The off-machine copy decision: no embedded credentials/provider logic —
    // either the `--backup-command` hook or an independent cron job, both
    // driven entirely by operator-supplied configuration.
    assert!(README_MD.contains("### Off-machine copies"));
    assert!(
        README_MD
            .contains("never embeds remote credentials or provider-specific upload logic itself")
    );
    assert!(README_MD.contains("--backup-command"));
    assert!(README_MD.contains("{BACKUP_FILE}"));
    assert!(README_MD.contains("rclone sync /mnt/backups"));
    // The jobs API documents the backup job's verify/prune/failure semantics.
    assert!(API_MD.contains("`POST /jobs/backup` returns `500`"));
    assert!(API_MD.contains("quarantined as `<name>.db.bad`"));
    assert!(API_MD.contains(
        "the newest 8 backups plus the first backup of each of the 12 most recent months"
    ));
}

/// Docs-sync pin for the `?suffix=` param on `POST /jobs/:name` (the
/// update.sh pre-upgrade backup): the jobs API documents its allowed
/// characters and the `422` it produces when invalid, that a suffixed backup
/// is pruned by the same retention policy as any other, and the README
/// documents update.sh's abort-before-`pkg add` behaviour and the
/// `-n`/`--no-backup` escape hatch.
#[test]
fn backup_suffix_param_documented() {
    assert!(API_MD.contains("`POST /jobs/:name?suffix=`"));
    assert!(API_MD.contains(
        "Allowed characters are ASCII letters, digits, `.`, `_`, and `-`, up to 40 characters"
    ));
    assert!(API_MD.contains(
        "A suffixed one-off backup is pruned by the same policy as any other — it is not exempt."
    ));
    assert!(README_MD.contains("aborts before touching the package"));
    assert!(README_MD.contains("`-n`/`--no-backup`"));
    assert!(README_MD.contains("never exempt from pruning"));
}

/// Docs-sync pin for the `?skip_command=` param on `POST /jobs/:name` (the
/// other half of the update.sh pre-upgrade backup): the jobs API documents
/// that the backup is still taken, that the run reports success carrying the
/// note naming what it passed over, and that the suppression lasts for that
/// run only — and the README says the same where the off-machine copy is
/// configured, which is where an operator reading `--backup-command` would
/// look for it.
#[test]
fn backup_skip_command_param_documented() {
    assert!(API_MD.contains("The optional `?skip_command=true` query param"));
    assert!(API_MD.contains("post-backup command skipped at the caller's request"));
    assert!(
        API_MD.contains(
            "The suppression is per-run and never sticky: the configuration is untouched"
        )
    );
    assert!(README_MD.contains("`POST /jobs/backup?skip_command=true`"));
}

/// SCENARIOS T-10. `POST /jobs/:name`'s two failures used to be bare status
/// codes, which the Jobs screen could only toast as "HTTP 404" / "HTTP 500".
/// The docs pin the bodies they now carry — including that this is the one
/// `500` in the API with a body, and why — and that an unrecognised query
/// parameter is refused rather than silently ignored.
#[test]
fn job_trigger_failure_bodies_documented() {
    assert!(API_MD.contains("Neither failure is a bare status code"));
    assert!(API_MD.contains("no job named 'nope'; registered jobs are"));
    assert!(API_MD.contains(
        "the `500` body carries the job's own error text \u{2014} the same string `job_runs.error` records"
    ));
    assert!(API_MD.contains("This is the one `500` in the API with a body"));
    assert!(API_MD.contains(
        "An unrecognised query parameter is refused `422 Unprocessable Entity` naming it"
    ));
    // The Response codes table says the same in its 404, 422 and 500 rows.
    assert!(
        API_MD.contains("`no job named 'nope'; registered jobs are \u{2026}` (`POST /jobs/:name`)")
    );
    assert!(API_MD.contains(
        "a misspelt `?sufix=` is refused, not ignored: it would otherwise take an unlabelled backup"
    ));
    assert!(API_MD.contains(
        "the one `500` that *does* carry a body: the job's own error text, so the UI toast says why"
    ));
}

/// SCENARIOS L-04/L-05/L-06/L-14. The crypto Known-limitations entry names an
/// entry path for every crypto event the ATO taxes, instead of calling three of
/// them "not modelled": the swap (wrapping included), the chain split's
/// nil-cost-base new asset and its abandoned original, the two kinds of
/// airdrop, and where a staking reward's income still has to be carried by
/// hand. Each cites its mirror, and each mirror carries its source header.
#[test]
fn known_limitations_document_the_crypto_entry_paths() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Crypto assets**"));
    assert!(limitations.contains("There is no crypto-specific *operation*"));

    // Swap, and wrapping as the same swap.
    assert!(limitations.contains("**Wrapping or unwrapping** a token is that same swap"));
    assert!(limitations.contains("docs/ato/crypto-wrapping.md"));
    assert!(include_str!("../docs/ato/crypto-wrapping.md").contains("QC 73649"));

    // Chain split: the nil-cost-base new asset, and the C2 close of an
    // abandoned original.
    assert!(limitations.contains("**chain split**'s new asset"));
    assert!(limitations.contains("a Buy at a price of `0`"));
    assert!(limitations.contains("`worthless_event: \"C2Cancellation\"`"));
    assert!(limitations.contains("docs/ato/crypto-chain-splits.md"));
    assert!(include_str!("../docs/ato/crypto-chain-splits.md").contains("QC 69953"));

    // The two airdrops are opposite entries, and the income half's open limit.
    assert!(limitations.contains("**initial-allocation airdrop** is the same nil-cost-base Buy"));
    assert!(limitations.contains("**Staking rewards and established-token airdrops**"));
    assert!(limitations.contains("**item 24, other income**"));
    assert!(limitations.contains("`income_type: \"OtherIncome\"`"));
    // What is genuinely left: item 24 is one figure, with no source split.
    assert!(limitations.contains("not* modelled is any split of that income by source"));
    assert!(limitations.contains("docs/ato/crypto-staking-airdrops.md"));
    assert!(include_str!("../docs/ato/crypto-staking-airdrops.md").contains("QC 69950"));

    // What genuinely is not modelled stays named as such.
    assert!(limitations.contains("**personal-use-asset exemption** is not modelled"));

    // Div 775 never reaches a crypto holding — including a stablecoin.
    assert!(limitations.contains("not 'foreign currency' for Division 775"));
    assert!(limitations.contains("docs/ato/crypto-not-foreign-currency.md"));
    let td = include_str!("../docs/ato/crypto-not-foreign-currency.md");
    assert!(td.contains("TD 2014/25"));
    assert!(td.contains("income years starting 1 July 2021"));

    // Surfaced in the README's crypto feature line.
    assert!(README_MD.contains("have no operation of their own"));

    // …and every new mirror is reachable from the ATO index.
    const ATO_OVERVIEW: &str = include_str!("../docs/ato/OVERVIEW.md");
    for mirror in [
        "crypto-staking-airdrops.md",
        "crypto-chain-splits.md",
        "crypto-wrapping.md",
        "crypto-not-foreign-currency.md",
    ] {
        assert!(
            ATO_OVERVIEW.contains(mirror),
            "OVERVIEW.md indexes {mirror}"
        );
    }
}

/// Doc-only resolution pin for settlement-window forex — CGT events K10/K11
/// (REQUIREMENTS 2026-06-12, NEEDS DECISION resolved 2026-06-12 as out of
/// scope): the Known-limitations entry states the rule (cost-base adjustment
/// on an acquisition, non-discountable K10 gain / K11 loss on a disposal),
/// that outcomes are the taxpayer's manual adjustment, and the interaction
/// with the spot-rate override (nil by construction under same-rate-month
/// monthly rates; per-leg spot rates make the movement visible); cites the
/// QC 17062 mirror; and the README surfaces it.
#[test]
fn known_limitations_document_settlement_window_forex_k10_k11() {
    let limitations = known_limitations();
    assert!(
        limitations.contains(
            "**Settlement-window forex on foreign-currency trades — CGT events K10/K11**"
        )
    );
    assert!(
        limitations.contains(
            "non-discountable capital gain (CGT event K10) or capital loss (CGT event K11)"
        )
    );
    assert!(limitations.contains("the taxpayer's manual adjustment"));
    // The spot-override interaction, both halves.
    assert!(limitations.contains("nil by construction"));
    assert!(limitations.contains("per-leg spot rates"));
    // Cites the mirrored ATO guidance (QC 17062) and its worked examples.
    assert!(limitations.contains("docs/ato/forex-cgt-12-month-rule.md"));
    assert!(limitations.contains("Art Ltd"));
    assert!(limitations.contains("Eleanor"));
    let mirror = include_str!("../docs/ato/forex-cgt-12-month-rule.md");
    assert!(mirror.contains("QC 17062"));
    // Surfaced in the README too.
    assert!(README_MD.contains("CGT events K10/K11"));
}

/// Docs-sync pin for hand-entered closing prices (2026-07-28): the API
/// documents the endpoint, both required provenance fields, and its 422s; the
/// schema documents the three columns and why the table still carries no
/// INSERT/DELETE staleness trigger and stays outside the audit trail; and the
/// Known-limitations entry states the two consequences the user lives with —
/// a manual price is one-way, and its superseded provenance is not retained.
#[test]
fn manual_closing_prices_documented() {
    // API.md: the endpoint, its body, and the one-way rule.
    assert!(API_MD.contains("PUT` | `/closing_prices/:listing_id/:price_date"));
    assert!(API_MD.contains("`sourced_from` (where the figure came from"));
    assert!(API_MD.contains("`reason` (why manual entry was needed"));
    assert!(API_MD.contains("**only ever replaced by another manual price**"));
    assert!(API_MD.contains("re-fetching a day whose stored price was entered manually"));
    // SCHEMA.md: the columns, the trigger reasoning, and the audit exclusion.
    assert!(SCHEMA_MD.contains("fetched | manual (CHECK-enforced enum, 0020)"));
    assert!(SCHEMA_MD.contains("Where a manual price was taken from"));
    assert!(SCHEMA_MD.contains("Why manual entry was needed"));
    assert!(SCHEMA_MD.contains("A **manually entered** price (0020) changes neither premise"));
    // Known limitations: the one-way rule, and that overwriting still loses
    // nothing now that the table is audited (0021).
    let limitations = known_limitations();
    assert!(limitations.contains("**A manually entered price is one-way**"));
    assert!(limitations.contains("cannot be removed — only overwritten"));
    assert!(limitations.contains("Overwriting loses nothing"));
    // README surfaces the feature.
    assert!(README_MD.contains("**priced by hand**"));
}

/// Docs-sync pin for clearing a superseded price span (2026-08-21): the API
/// documents the one relaxation of the ok-row delete rule and why it is
/// narrow, the bulk clear and its listing-bounded span, and both refusals;
/// the schema records the same two deletable kinds behind the table's single
/// staleness trigger; the README names the span as the one place a stored
/// price may be deleted.
#[test]
fn clearing_superseded_closing_prices_documented() {
    let closing_prices = API_MD
        .split("## Closing prices")
        .nth(1)
        .expect("API.md has a Closing prices section")
        .split("\n## ")
        .next()
        .unwrap();
    // The relaxation, its reason, and the asymmetry with `unpriced_from`.
    assert!(closing_prices.contains(
        "**The one relaxation: a date inside the listing's [`unpriced_before`](#listings) span**"
    ));
    assert!(closing_prices.contains("inside that span there is no valued series to hole"));
    assert!(
        closing_prices.contains("gets **no** such relaxation and this asymmetry is deliberate")
    );
    // The bulk form: bounded by the marker, idempotent, audited.
    assert!(API_MD.contains("POST` | `/closing_prices/clear_unpriced_before"));
    assert!(closing_prices.contains("It takes **no date range**"));
    assert!(closing_prices.contains("a second call reports `deleted: 0`"));
    // Both refusals reach the 422 catalogue.
    assert!(API_MD.contains(
        "deleting a closing price that is stored ok rather than errored and is not inside its \
         listing's `unpriced_before` span"
    ));
    assert!(API_MD.contains(
        "clearing a superseded price span on a listing that declares no `unpriced_before`"
    ));
    // SCHEMA.md: the two deletable kinds behind the single UPDATE trigger.
    assert!(SCHEMA_MD.contains("the only deletable rows are ones no stored figure was valued at"));
    // README: the span is the one place a stored price may be deleted.
    assert!(README_MD.contains("the one span in which a stored price may be **deleted**"));
}

/// Docs-sync pin for auditing closing prices (2026-07-28): the schema records
/// why the table joined the audited set, the surrogate key it needed, and what
/// the old composite key became; the API documents the `id` and points a
/// history lookup at it; the README names hand-entered prices among the
/// audited facts.
#[test]
fn audited_closing_prices_documented() {
    // SCHEMA.md: the reversal of the original exclusion and the key change.
    assert!(SCHEMA_MD.contains("`closing_prices` joined the audited set in 0021"));
    assert!(SCHEMA_MD.contains("AUTOINCREMENT surrogate `id`"));
    assert!(SCHEMA_MD.contains("kept as `UNIQUE(listing_id, price_date)`"));
    assert!(SCHEMA_MD.contains("Server-assigned surrogate key"));
    // API.md: the id, what it is for, and that it survives a replacement.
    assert!(API_MD.contains("`{\"table\": \"closing_prices\", \"row_id\": <id>}`"));
    assert!(API_MD.contains("one row's whole revision history sits under one id"));
    assert!(API_MD.contains("Its `id` is the `row_id` to ask for."));
    // README: hand-entered prices are named among the audited facts.
    assert!(README_MD.contains("hand-entered closing prices"));
}

/// Docs-sync pin for auditing the exchange holiday calendar (migration 0039,
/// decided 2026-08-21): the schema records why the table joined the audited
/// set and why `exchanges` did not, plus the surrogate key it needed and what
/// the old composite key became; the API documents the `id`, points a history
/// lookup at it, and states that the audit trigger is deliberately not
/// narrowed the way the staleness one is; and the README names the calendar
/// among the audited facts.
#[test]
fn audited_exchange_holidays_documented() {
    // SCHEMA.md: the reversal, the key change, and the surviving exclusion.
    assert!(SCHEMA_MD.contains("`exchange_holidays` joined in 0039"));
    assert!(SCHEMA_MD.contains("kept as `UNIQUE(mic, holiday_date)`"));
    assert!(
        SCHEMA_MD.contains(
            "**`exchanges` stays out, and its half of the original sentence remains true**"
        )
    );
    // The criterion it meets, stated as the reason rather than implied.
    assert!(SCHEMA_MD.contains(
        "The staleness triggers flag the *effect* of a change; the trail is what retains *what* \
         was changed."
    ));
    // API.md: the id, what it is for, and the two mechanisms' different scope.
    assert!(API_MD.contains("`{\"table\": \"exchange_holidays\", \"row_id\": <id>}`"));
    assert!(API_MD.contains(
        "a name-only `PUT` stales no snapshot (no stored figure moved) but is still recorded"
    ));
    // The A-40 limitation footnote records that a deleted holiday survives.
    assert!(
        known_limitations()
            .contains("the calendar joined the audited tables on 2026-08-21 (migration 0039)")
    );
    // README: the calendar is named among the audited facts.
    assert!(README_MD.contains("the exchange holiday calendar every valuation reads"));
}

/// Docs-sync pin for the fetched-symbol provenance (migration 0038, the
/// `symbol`-override incident): the API states that every fetched row records
/// the symbol it was fetched under and why it is recorded *always* rather than
/// only on a difference, that a manual row and a pre-0038 row carry none, and
/// what the currency cross-check does and does not catch; the schema documents
/// the column; and the residual gap — nothing verifies the symbol names the
/// same security — is a stated Known limitation rather than an implied
/// guarantee.
#[test]
fn fetched_symbol_provenance_documented() {
    let closing_prices = API_MD
        .split("## Closing prices")
        .nth(1)
        .expect("docs/API.md has a Closing prices section")
        .split("\n## ")
        .next()
        .expect("split always yields at least one part");
    assert!(
        closing_prices.contains("**Every fetched row records the symbol it was fetched under**")
    );
    assert!(closing_prices.contains("recorded **always**, not only when it differs"));
    assert!(closing_prices.contains(
        "for any row stored **before** the column existed (unrecorded, and not recoverable after \
         the fact"
    ));
    assert!(closing_prices.contains("stored as an **errored row** naming both currencies"));
    assert!(closing_prices.contains(
        "it does not catch one that reached another security quoted in the same currency"
    ));
    assert!(SCHEMA_MD.contains("fetched_symbol TEXT (nullable, 0038)"));
    assert!(
        API_MD.contains("**Nothing verifies that a fetched symbol names the *same security***")
    );
}

/// Docs-sync pin for the contemporaneous price basis (SCENARIOS Q-14,
/// 2026-08-20): the API states what basis a stored price is in, that it is
/// normalised on entry and re-derived when a re-basing action is recorded,
/// what a manual price does instead, and the precision that survives; the
/// corporate-action entries say a split/bonus issue re-bases stored prices;
/// the schema documents the column and the fetched_at-dates-the-basis rule;
/// and the repair job is named in both the API and the README.
#[test]
fn contemporaneous_price_basis_documented() {
    let closing_prices = API_MD
        .split("## Closing prices")
        .nth(1)
        .expect("docs/API.md has a Closing prices section")
        .split("\n## ")
        .next()
        .expect("split always yields at least one part");
    assert!(closing_prices.contains("**A stored price is in its own trading day's unit basis**"));
    assert!(
        closing_prices.contains(
            "`price = price_as_observed ×` the ratio of every [`ShareSplit`/`BonusIssue`]"
        )
    );
    assert!(closing_prices.contains("in the action write's own transaction"));
    assert!(closing_prices.contains("carries only the provider's ~7 significant digits"));
    assert!(closing_prices.contains("no longer byte-identical to the provider's response"));
    assert!(closing_prices.contains("contemporaneous **by declaration**"));
    // The two re-basing action types say what they now do to stored prices.
    assert!(API_MD.contains(
        "re-bases the listing's stored [closing prices](#closing-prices)** — in the same \
         transaction"
    ));
    assert!(API_MD.contains(
        "It **re-bases the listing's stored [closing prices](#closing-prices)** exactly as a \
         `ShareSplit` does."
    ));
    // SCHEMA.md: the column, its CHECK, and what dates the basis.
    assert!(SCHEMA_MD.contains("price_as_observed TEXT (decimal, nullable, 0034)"));
    assert!(SCHEMA_MD.contains("Also **dates the unit basis** price_as_observed arrived in"));
    // The one-off repair job, in the API's job list and the README's schedule note.
    // (The list gained a second unscheduled job after this one, so the
    // conjunction moved off `price-rebase` — see `settlement_recompute_job_documented`.)
    assert!(
        API_MD.contains("`price-rebase` (see [Closing prices](#closing-prices); **not scheduled**")
    );
    // (Wording moved with SCENARIOS T-09/schedule, which made "deliberately
    // unscheduled" a flag in the registry rather than only a README sentence.)
    assert!(README_MD.contains("`price-rebase` is one of the two manual-only jobs"));
}

/// Docs-sync pin for the demerger price factor (2026-08-20, the finding that
/// followed SCENARIOS Q-14): the API states that the price re-basing set is a
/// superset of the quantity one and lists which action kinds are in it and
/// which are not, the `Demerger` entry documents the stated close and that it
/// moves no quantity, the 422 catalogue carries its refusals, the schema
/// documents the four columns, health documents the warning, and the README
/// says the same in a sentence.
#[test]
fn demerger_price_rebasing_documented() {
    let closing_prices = API_MD
        .split("## Closing prices")
        .nth(1)
        .expect("docs/API.md has a Closing prices section")
        .split("\n## ")
        .next()
        .expect("split always yields at least one part");
    // The invariant is no longer stated unconditionally: the exception list is
    // there, and names both the kinds that restate the series and those that
    // do not.
    assert!(closing_prices.contains("**Which corporate actions restate the series.**"));
    assert!(closing_prices.contains("a strict *superset* of the actions that re-base quantities"));
    assert!(closing_prices.contains("changes **no unit count** on the head listing"));
    assert!(
        closing_prices.contains(
            "`demerger_cost_base_pct` is an ATO cost-base apportionment, not a price ratio"
        )
    );
    assert!(closing_prices.contains("`ScripForScrip` and `WorthlessShares` do **not**"));
    assert!(closing_prices.contains("`ReturnOfCapital`, `RightsIssue` and `BuyBack` do **not**"));
    // The precision the recovered figures actually carry.
    assert!(closing_prices.contains("as accurate as the close you state, not exact"));
    // The action's own entry: the fields, and that they are price-only.
    assert!(API_MD.contains("A demerger also carries an optional **stated pre-demerger close**"));
    assert!(API_MD.contains(
        "It moves no quantity, no cost base and no allocation capacity — a demerger changes no \
         unit count here."
    ));
    // The write-time refusals.
    assert!(API_MD.contains("the demerger's stated close is partial"));
    assert!(API_MD.contains("a close on or after it is already in the post-demerger basis"));
    // Health names the listing whose prices still need it.
    assert!(API_MD.contains("`demergers_missing_close` — every recorded"));
    assert!(API_MD.contains("the rows are `ok`, not errored"));
    // SCHEMA.md: the columns and the superset statement from the other side.
    assert!(SCHEMA_MD.contains("demerger_close_date  TEXT (date, nullable, 0036)"));
    assert!(SCHEMA_MD.contains("demerger_close_price TEXT (decimal, nullable, 0036)"));
    assert!(SCHEMA_MD.contains("demerger_close_sourced_from TEXT (nullable, 0036)"));
    assert!(SCHEMA_MD.contains("demerger_close_reason TEXT (nullable, 0036)"));
    assert!(SCHEMA_MD.contains("a strict SUPERSET of the quantity re-basing one"));
    // README: the user-visible sentence, and the repair job's widened scope.
    assert!(README_MD.contains(
        "A **demerger** restates the provider's series the same way while changing no unit count"
    ));
    assert!(README_MD.contains("or a demerger carrying a stated pre-demerger close is recorded."));
    // The one exception to the referenced-action freeze, and its bounds.
    assert!(API_MD.contains(
        "The **one** exception to the `PUT` half is a [`Demerger`](#corporate-actions)'s stated \
         pre-demerger close"
    ));
    assert!(API_MD.contains("as does a `PUT` that changes nothing"));
}

/// Docs-sync pin for the one exception to the contemporaneous-basis invariant
/// (TODO "The LAC demerger was modelled with the head and the new entity
/// swapped", 2026-08-21): on a demerger's own date the head listing's stored
/// price is the provider's standalone-equivalent rather than an observed
/// close, because the demerged parcel is already held that day — so the walk's
/// strictly-before boundary is deliberate, a split reaches it by the other
/// route, and health's `adjusted_days` counts by the same boundary. The schema
/// carries the same qualification on the `price` column, whose commentary
/// states the invariant from the other side.
#[test]
fn demerger_date_price_basis_exception_documented() {
    let closing_prices = API_MD
        .split("## Closing prices")
        .nth(1)
        .expect("docs/API.md has a Closing prices section")
        .split("\n## ")
        .next()
        .expect("split always yields at least one part");
    // The exception itself, beside the invariant it qualifies.
    assert!(
        closing_prices.contains("**The one exception, and it is on the demerger's own date.**")
    );
    assert!(
        closing_prices
            .contains("`price_basis_ratio` skips an event dated on or before the price date")
    );
    assert!(closing_prices.contains("Leaving it is deliberate, not an off-by-one"));
    assert!(
        closing_prices
            .contains("**on the demerger date the model already holds the demerged parcel**")
    );
    assert!(closing_prices.contains("un-adjusting the row would recover the **combined** entity"));
    // The worked evidence, which is what makes it checkable.
    assert!(closing_prices.contains("16.85 ÷ 6.453301 = `2.611067`"));
    assert!(closing_prices.contains(
        "value the holding at **26.69** a unit against the previous day's 16.85, a 58% overnight \
         jump"
    ));
    assert!(closing_prices.contains("as stored the two sum to **16.19**, a 3.9% move"));
    // A split reaches the same boundary by the other route.
    assert!(
        closing_prices
            .contains("on a split's effective date the price is *already* in the new basis")
    );
    // The health check counts by the same boundary, so both agree.
    assert!(
        closing_prices
            .contains("`demergers_missing_close` counts `adjusted_days` by that same boundary")
    );
    // SCHEMA.md carries the qualification where it asserts the invariant.
    assert!(SCHEMA_MD.contains(
        "The one exception is a **demerger's own date**: the re-base walk covers only the rows \
         dated strictly before the event"
    ));
}

/// Docs-sync pin (REQUIREMENTS 2026-07-13, lossless GST-inclusive
/// round-trip): the Trades section states the read/write round-trip
/// semantics explicitly — with the flag set, `brokerage` is the one
/// GST-inclusive amount on both reads and writes, and a verbatim GET → PUT
/// re-splits to the identical stored pair — and the Sells section carries
/// the same contract for flagged Sells.
#[test]
fn gst_inclusive_round_trip_semantics_documented() {
    assert!(
        API_MD.contains("**Reads present the same shape writes expect (lossless round-trip):**")
    );
    assert!(API_MD.contains("**on both reads and writes**"));
    assert!(API_MD.contains("re-splits the same amount to the **identical stored pair**"));
    // The Sells section states it for flagged Sells as well.
    assert!(API_MD.contains("and the lossless GST-inclusive round-trip"));
    assert!(API_MD.contains("re-`PUT`ting that body to `PUT /sells/:id`"));
}

/// Pins the frontend test strategy (2026-07-13 improvement review): the JS
/// unit tests and the headless UI smoke check are CI steps, not manual-only
/// tools, and the README documents how to run them and the required Node
/// version. The exclusion of `*.test.js` files from the served bundle is
/// pinned in `web.rs` (`js_test_files_are_not_served_and_every_module_is`).
#[test]
fn frontend_tests_run_in_ci() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    // The Node unit-test step, on a pinned Node version.
    assert!(CI_YML.contains("node --test 'src/web/*.test.js'"));
    assert!(CI_YML.contains("node-version: '22'"));
    // The headless smoke-check step.
    assert!(CI_YML.contains("scripts/ui-smoke.sh"));
    // README: how to run them, and the Node version requirement.
    assert!(README_MD.contains("### Tests"));
    assert!(README_MD.contains("node --test 'src/web/*.test.js'"));
    assert!(README_MD.contains("**Node 22 or newer**"));
    assert!(README_MD.contains("scripts/ui-smoke.sh"));
}

/// Pins the supply-chain checks (2026-07-13 improvement review): the RustSec
/// advisory gate (`cargo deny check advisories`) is a CI step driven by the
/// committed `deny.toml`, Dependabot keeps the Cargo and GitHub Actions
/// dependencies patched with weekly grouped PRs, and the README documents the
/// local equivalent and the recorded no-upstream-fix policy.
#[test]
fn supply_chain_checks_run_in_ci() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    assert!(CI_YML.contains("EmbarkStudios/cargo-deny-action"));
    assert!(CI_YML.contains("command: check advisories"));
    // Dependabot: weekly grouped version updates for both ecosystems.
    const DEPENDABOT_YML: &str = include_str!("../.github/dependabot.yml");
    assert!(DEPENDABOT_YML.contains("package-ecosystem: cargo"));
    assert!(DEPENDABOT_YML.contains("package-ecosystem: github-actions"));
    assert!(DEPENDABOT_YML.contains("interval: weekly"));
    assert!(DEPENDABOT_YML.contains("groups:"));
    // deny.toml: the advisories config and the recorded policy.
    assert!(DENY_TOML.contains("[advisories]"));
    assert!(DENY_TOML.contains("Policy for an advisory with no upstream fix"));
    assert!(DENY_TOML.contains("temporary by construction, never"));
    // README: the section, the local equivalent, and the policy decision.
    assert!(README_MD.contains("### Supply-chain checks"));
    assert!(README_MD.contains("cargo deny check advisories"));
    assert!(README_MD.contains("cargo install cargo-deny --locked"));
    assert!(README_MD.contains("**Policy for an advisory with no upstream fix yet**"));
    assert!(README_MD.contains("never permanent"));
}

/// Pins least-privilege `GITHUB_TOKEN` scopes on every workflow (CodeQL
/// `actions/missing-workflow-permissions`, resolved 2026-07-30): a workflow
/// with no `permissions:` block inherits the repository default, so a later
/// change to that repository setting would silently widen what a compromised
/// action could do. Each workflow therefore declares its own floor — `ci.yml`
/// only reads the repo, `release.yml` needs `contents: write` to publish the
/// release — and a new workflow must declare one too.
#[test]
fn workflows_declare_explicit_token_permissions() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    const RELEASE_YML: &str = include_str!("../.github/workflows/release.yml");
    // CI publishes nothing and comments on nothing: read-only is the floor.
    assert!(CI_YML.contains("permissions:\n  contents: read"));
    // Releases create a tag + release, so this one is deliberately broader.
    assert!(RELEASE_YML.contains("permissions:\n  contents: write"));
    // Any workflow added later declares its own scopes rather than inheriting.
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/.github/workflows");
    for entry in std::fs::read_dir(dir).expect("workflows dir is readable") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_none_or(|e| e != "yml" && e != "yaml") {
            continue;
        }
        let body = std::fs::read_to_string(&path).expect("workflow is readable");
        assert!(
            body.contains("\npermissions:"),
            "{} declares no top-level permissions: block",
            path.display()
        );
    }
}

/// Pins third-party actions to an immutable commit SHA (CodeQL
/// `actions/unpinned-tag`, resolved 2026-07-30): a tag is mutable, so `@v2`
/// runs whatever its owner later repoints it at, with the workflow's token.
/// GitHub's own `actions/*` are first-party and exempt, matching the rule.
/// Each pin carries a trailing `# <version>` comment — that is what Dependabot
/// reads to raise the bump, so pinning hardens the workflows without freezing
/// them at today's versions.
#[test]
fn third_party_actions_are_pinned_to_a_commit_sha() {
    const CI_YML: &str = include_str!("../.github/workflows/ci.yml");
    const RELEASE_YML: &str = include_str!("../.github/workflows/release.yml");
    let mut checked = 0;
    for (file, body) in [("ci.yml", CI_YML), ("release.yml", RELEASE_YML)] {
        for line in body.lines() {
            if line.trim_start().starts_with('#') {
                continue; // prose about pinning, not a step
            }
            let Some((_, spec)) = line.split_once("uses:") else {
                continue;
            };
            let spec = spec.trim();
            let (action, reference) = spec
                .split_once('@')
                .unwrap_or_else(|| panic!("{file}: `uses: {spec}` names no ref"));
            if action.starts_with("actions/") {
                continue;
            }
            // Mutability first: on a reverted pin that is the real complaint,
            // and a bare tag has no comment either, so checking the comment
            // first would report the lesser problem.
            let (sha, version) = match reference.split_once('#') {
                Some((sha, version)) => (sha.trim(), Some(version.trim())),
                None => (reference.trim(), None),
            };
            assert!(
                sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
                "{file}: `{action}` is pinned to `{sha}`, not a 40-character commit SHA — a tag is mutable",
            );
            let version = version.unwrap_or_else(|| {
                panic!("{file}: `{action}`'s pin carries no `# <version>` comment for Dependabot")
            });
            assert!(
                !version.is_empty(),
                "{file}: `{action}`'s pin has an empty version comment",
            );
            checked += 1;
        }
    }
    // The four that exist today; a new third-party action raises this.
    assert_eq!(checked, 4, "expected 4 pinned third-party actions");
}

/// Executable half of the deny.toml ignore policy (decided 2026-07-14): every
/// advisory ignore entry carries a RustSec id, a non-empty reason, and — on
/// the same line, per the format the file documents — an
/// `# expires: YYYY-MM-DD` comment whose date has not yet passed. Once an
/// expiry passes this test fails, so the entry must be re-justified with a
/// new date or removed: an ignore cannot become permanent by inattention.
#[test]
fn advisory_ignores_expire() {
    let value: toml::Value = toml::from_str(DENY_TOML).expect("deny.toml parses as TOML");
    let ignore = value["advisories"]["ignore"]
        .as_array()
        .expect("advisories.ignore is an array");
    for entry in ignore {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .expect("ignore entry has a string id");
        assert!(id.starts_with("RUSTSEC-"), "id {id:?} is a RustSec id");
        let reason = entry
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("ignore entry {id} carries no reason"));
        assert!(
            !reason.trim().is_empty(),
            "ignore entry {id} has an empty reason"
        );
    }
    // One entry per line (the documented format), so each entry's line-end
    // expiry comment is attributable; comment lines (the format example)
    // don't count.
    let entry_lines: Vec<&str> = DENY_TOML
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && l.contains("RUSTSEC-"))
        .collect();
    assert_eq!(
        entry_lines.len(),
        ignore.len(),
        "each advisories.ignore entry sits on its own line with its expiry comment"
    );
    let today = chrono::Local::now().date_naive();
    for line in entry_lines {
        let expires = line.split("# expires: ").nth(1).unwrap_or_else(|| {
            panic!("ignore entry lacks an `# expires: YYYY-MM-DD` comment: {line}")
        });
        let date = chrono::NaiveDate::parse_from_str(expires.trim(), "%Y-%m-%d")
            .unwrap_or_else(|e| panic!("expiry {expires:?} is not YYYY-MM-DD ({e}): {line}"));
        assert!(
            date >= today,
            "advisory ignore expired on {date}: {line}\nre-justify it with a new expiry or remove it"
        );
    }
}

/// Docs-sync pin for the overview performance panel (REQUIREMENTS
/// 2026-07-25): the API docs the new endpoint's `(from, to]`/FX-attribution
/// conventions and its Known-limitations entry, the response-codes catalogue
/// covers its 422s, and the README surfaces the panel on the overview screen
/// (and no longer claims the graph lives on the snapshots screen).
#[test]
fn period_performance_panel_documented() {
    assert!(API_MD.contains("### Period performance"));
    assert!(API_MD.contains("POST /portfolio/period-performance"));
    assert!(API_MD.contains("half-open `(from, to]`"));
    assert!(
        API_MD.contains("a period-performance request whose `from` is not strictly before `to`")
    );
    let limits = known_limitations();
    assert!(limits.contains("Period-performance FX attribution is approximate"));
    assert!(limits.contains("capital_growth + fx_movement + income"));
    assert!(README_MD.contains("period performance"));
    assert!(README_MD.contains("capital growth"));
    assert!(README_MD.contains("date-range presets"));
}

/// Docs-sync pin for the overview panel's 2Y/3Y presets, remembered range,
/// and the per-holding "hide no-activity holdings" checkbox (REQUIREMENTS
/// 2026-07-26): the README's preset list includes 2Y/3Y and the remembered-
/// range/hide-inactive behaviour, and the API docs' Period performance
/// section explains that `holdings` returns every holding unfiltered and the
/// UI hides all-zero ones by default.
#[test]
fn overview_range_presets_and_activity_filter_documented() {
    assert!(README_MD.contains("1M/3M/6M/1Y/2Y/3Y/FY-to-date/all"));
    assert!(README_MD.contains("last-picked preset is remembered across reloads"));
    assert!(README_MD.contains("hide holdings with no activity in this period"));
    assert!(API_MD.contains(
        "a row for **every** holding with any history up to either endpoint, including one fully closed well before `from`"
    ));
    assert!(API_MD.contains("hide holdings with no activity in this period"));
}

/// Docs-sync pin for the top menu bar + overview-first home screen
/// (REQUIREMENTS 2026-07-25): the API docs name the menu bar, its four menus,
/// `#/` as the home route, and the new `/static/nav.js` module; the README's
/// Web UI bullet describes the menu bar, and the Portfolio overview bullet
/// names the home screen and its shortcut buttons.
#[test]
fn top_menu_bar_documented() {
    assert!(API_MD.contains("/static/nav.js"));
    assert!(API_MD.contains("top menu bar"));
    for menu in ["Activity", "Reports", "Reference Data", "Jobs"] {
        assert!(API_MD.contains(menu), "API.md should name the {menu} menu");
    }
    assert!(API_MD.contains("the app's home screen"));
    assert!(README_MD.contains("top menu bar"));
    assert!(README_MD.contains("New trade/income/sell/transfer shortcut buttons"));
}

/// Docs-sync pin for the uniform DELETE 404 contract (2026-07-29 Rust review):
/// the Response-codes table states that a `DELETE` of a missing row answers
/// with a plain-text reason (a `GET` still answers with an empty body), and
/// the Error-bodies paragraph counts deletes among the reasoned 404s. The
/// behaviour itself is pinned by
/// `entities::tests::deleting_a_missing_row_is_404_naming_what_was_missing`.
#[test]
fn delete_404_reason_documented() {
    assert!(API_MD.contains("A `GET` of a missing row answers with an empty body"));
    assert!(API_MD.contains("every `DELETE` of a missing row"));
    assert!(API_MD.contains("no AMMA statement with that id"));
    assert!(API_MD.contains("`404`-with-a-cause — which includes every `DELETE` of a row"));
}

/// Docs-sync pin for the delete-time guard on the three read-time corporate
/// actions (SCENARIOS A-06/A-20/A-21, 2026-08-14): the Corporate actions
/// section states the guard per type and the direction each one runs in, the
/// Response-codes `422` row lists both refusals, and the deliberately
/// *unguarded* `PUT` — the correction path the guard would otherwise close —
/// is a Known limitations entry rather than silent behaviour. The refusals
/// themselves are pinned by `entities::corporate_action::tests`.
#[test]
fn corporate_action_delete_guard_documented() {
    assert!(API_MD.contains("**Deleting an action that is already depended on.**"));
    assert!(API_MD.contains("**any trade dated on or after** the action's `date`"));
    assert!(API_MD.contains("**any parcel acquired on or before** the payment `date`"));
    assert!(API_MD.contains("deleting a ShareSplit or BonusIssue whose listing has a trade"));
    assert!(API_MD.contains("deleting a ReturnOfCapital whose listing has a parcel"));
    // The edit path stays open by decision, so it is documented as such —
    // narrowed (2026-08-15) by the write-time state check below, which the
    // limitation now points at as the bound on what an edit can leave behind.
    let limitations = known_limitations();
    assert!(limitations.contains(
        "**Editing a split, bonus issue, or return of capital in place restates prior figures**"
    ));
    assert!(limitations.contains("the residual exposure is restatement of valid figures only"));
    assert!(limitations.contains("There is no lodged/closed-year concept in the data model"));
}

/// Docs-sync pin for the return-of-capital record date (SCENARIOS B-09,
/// 2026-08-15): entitlement is fixed at the record date, and the docs say so
/// — including what the *absence* of a record date falls back to, which is
/// the over-reduction the field exists to correct. The behaviour itself is
/// pinned by `entities::corporate_action::tests` and the report tests.
#[test]
fn return_of_capital_record_date_documented() {
    assert!(API_MD.contains("entitlement is fixed earlier, at the **record date**"));
    assert!(API_MD.contains("**Leaving `record_date` out keeps the older, coarser rule**"));
    assert!(API_MD.contains("over-reduces a parcel bought inside the record-to-payment window"));
    // The schema documents the column, its CHECK, and the same fallback.
    assert!(SCHEMA_MD.contains("record_date       TEXT (date, nullable)  ReturnOfCapital only"));
    assert!(SCHEMA_MD.contains("never after `date`"));
    assert!(SCHEMA_MD.contains("NULL = not recorded, and the payment date decides instead"));
}

/// Docs-sync pin for the sale-side incidental-costs convention (SCENARIOS
/// B-17, 2026-08-15). Netting a Sell's brokerage off `proceeds` rather than
/// adding it to `cost_base` gives the identical capital gain, so nothing is
/// wrong — but the two *reported components* differ from the ATO's own
/// presentation, and a user reconciling against a worksheet finds two figures
/// that don't match and a gain that does. Documented, with the worked figures
/// and the reason the convention is what it is.
#[test]
fn sale_side_incidental_costs_convention_documented() {
    assert!(API_MD.contains("**Where a Sell's brokerage and GST land.**"));
    assert!(API_MD.contains("**netted off `proceeds`**"));
    assert!(API_MD.contains("**The capital gain is identical either way**"));
    // The worked example both presentations are shown through.
    assert!(API_MD.contains("`proceeds: 1189.055` / `cost_base: 1010.945`"));
    assert!(API_MD.contains("$1,200.00 / $1,021.89"));
    // Why: the parcel's cost base must read the same before and after sale.
    assert!(API_MD.contains("doesn't move the moment it is sold"));
    // And the ATO's own definition of the second element.
    assert!(API_MD.contains("docs/ato/cgt-cost-base.md"));
    assert!(
        include_str!("../docs/ato/cgt-cost-base.md").contains(
            "Second element: incidental costs of acquiring the CGT asset or that relate to the \
             CGT event"
        ),
        "the cited ATO mirror still carries the second-element definition"
    );
}

/// Docs-sync pin for rights acquired beyond the holding's own entitlement
/// (SCENARIOS B-20, 2026-08-15): `rights_cost` reads as though purchased
/// rights were fully supported, while both endpoints cap cumulative units at
/// the record-date entitlement and refuse past it. The cap is a safe refusal,
/// so the gap is documentation — the exercise section and Known limitations
/// now say what is and isn't recordable, and where the extra shares go.
#[test]
fn rights_beyond_the_entitlement_documented() {
    // The exercise section: purchased rights are in scope only within the
    // entitlement, with the refusal quoted.
    assert!(API_MD.contains("`rights_cost` covers rights **bought on-market**"));
    assert!(API_MD.contains(
        "the units exercised exceed the entitlement earned by the holding at the record date"
    ));
    // Known limitations: the scope cut and the entry route it leaves.
    let limitations = known_limitations();
    assert!(limitations.contains("**rights acquired beyond the holding's own entitlement**"));
    assert!(limitations.contains("supported up to the entitlement the holding earned"));
    assert!(limitations.contains("a purchase of extra rights has nowhere to be recorded"));
    assert!(limitations.contains("as an ordinary [Buy](#trades) at their full acquisition cost"));
}

/// Docs-sync pin for the rollover-assumed scope cut (SCENARIOS C-09,
/// 2026-08-15). `ScripForScrip` and `Demerger` model only the rollover case,
/// and recording one *is* the assertion that the rollover applies — nothing
/// checks eligibility. The behaviour is right for what it models and the
/// no-rollover variant fails safe (there is no operation to invoke; the user
/// enters the trades by hand), so the gap was that "not modelled" lived only
/// in the per-action prose and `docs/ato/demergers.md` — never in the Known
/// limitations list a reader scans for scope cuts, unlike the neighbouring
/// rights-issue entry. It matters because the two cases differ in the
/// *opposite* direction on the discount clock: with rollover the new
/// interests carry the original acquisition date, without it they start
/// their own at the event date.
#[test]
fn rollover_assumed_scope_cut_documented() {
    let limitations = known_limitations();
    assert!(limitations.contains("**Rollovers assume the rollover was chosen**"));
    // Recording the action is the assertion; nothing verifies eligibility.
    assert!(limitations.contains("the taxpayer's assertion that the rollover applies"));
    assert!(limitations.contains("nothing checks eligibility"));
    // The two directions of the discount clock, which is what C-09 turns on.
    assert!(limitations.contains("`deemed_acquisition_date`"));
    assert!(limitations.contains("run their own 12-month clock from it"));
    // The manual entry route each no-rollover case leaves.
    assert!(limitations.contains("enter a no-rollover exchange as a manual Sell plus Buy"));
    assert!(limitations.contains("dated the demerger date"));
    // The ATO mirror cited, still carrying the example the rule comes from.
    assert!(limitations.contains("docs/ato/demergers.md"));
    assert!(
        include_str!("../docs/ato/demergers.md")
            .contains("you calculate the 12 months from the date of demerger if you did not"),
        "the cited ATO mirror still carries the no-rollover discount-clock rule"
    );
}

/// Docs-sync pin for the closed-year decision (SCENARIOS A-15/A-21/A-25/A-35,
/// 2026-08-15): the restatement exposure is real but not a bug — there is no
/// lodged-year concept in the data model, and building one (a per-FY lodgement
/// marker plus a changed-since-lodgement flag over `row_history`) is a feature
/// deliberately not taken on. That decision is only honest if it is *stated*,
/// since a user reasonably assumes a prior year's numbers are settled — so
/// this is a documentation-only requirement, pinned here: the Known
/// limitations entry, the two properties that make it survivable (auditable
/// via row history, snapshots don't cover tax reports), the mitigation, the
/// A-40 exchange-holiday footnote — including its Q-05/Q-08 correction, that
/// the calendar is a live valuation input and a holiday write now stales the
/// snapshots it re-values — and the README's own scope-cuts summary.
#[test]
fn closed_year_restatement_documented() {
    let limitations = known_limitations();
    assert!(limitations.contains(
        "**A lodged financial year can be restated with nothing marking it** (2026-08-15)"
    ));
    assert!(limitations.contains("**no financial year is ever closed**"));
    // The two facts that bound the exposure: it is recoverable after the fact,
    // and the one stored-report mechanism that sounds like it would help
    // deliberately does not.
    assert!(limitations.contains("auditable after the fact"));
    assert!(limitations.contains(
        "[Report snapshots](#report-snapshots) do not help here either; they persist the three \
         price-dependent reports only, never a tax report"
    ));
    // The mitigation the user is left with, and the low-severity companion.
    assert!(limitations.contains("save the [annual tax report](#annual-tax-report) as a PDF"));
    assert!(limitations.contains("[`DELETE /exchange_holidays/:mic/:date`](#exchange-holidays)"));
    assert!(limitations.contains("a record field, not a tax figure"));
    // …but only on the trade side: the calendar itself is read live by
    // valuation, which the footnote used to deny (SCENARIOS Q-05/Q-08).
    assert!(limitations.contains("it was once described here as one, wrongly"));
    assert!(limitations.contains("marks those snapshots stale in the same transaction"));
    assert!(README_MD.contains("**no financial year is ever closed**"));
}

/// Docs-sync pin for the as-at-date convention shared by every holdings
/// report (2026-08-16, SCENARIOS E-14): an undated report is the position as
/// at *today*, so a corporate action or trade recorded ahead of its effective
/// date — the normal way terms are recorded — is not in force yet, and the
/// undated reports can't disagree with the dated ones run for today. The
/// behaviour itself is pinned by `domain::open_parcels::tests`.
#[test]
fn as_at_today_convention_documented() {
    assert!(API_MD.contains("### As-at date"));
    assert!(
        API_MD.contains(
            "a report that takes no date is as at **today** — never \"every fact on file\""
        )
    );
    assert!(API_MD.contains(
        "A [trade](#trades) or [corporate action](#corporate-actions) dated in the future is \
         recorded but not yet in force"
    ));
    // The two halves that make it one rule: trades are bounded too, and the
    // FY-keyed reports deliberately are not.
    assert!(
        API_MD.contains("A future-dated *trade* is bounded the same way rather than carved out")
    );
    assert!(
        API_MD.contains("The [realised](#realised-gains) and FY-keyed tax reports are not bounded")
    );
    // The undated reports name themselves as as-at-today where they are
    // documented, not only in the shared section.
    assert!(API_MD.contains("as at today (see [As-at date](#as-at-date))"));
}

/// Docs-sync pin for the AMIT/E4 mutual exclusion (2026-08-16, SCENARIOS
/// E-04): a return of capital is refused on a listing flagged `amit`, whose
/// cost-base movement is its AMMA statement's `cost_base_adjustment`. Both
/// ends say so — the corporate-actions write rules and the AMIT-adjustments
/// section — and the Response-codes `422` row lists the refusal. The
/// behaviour itself is pinned by `entities::corporate_action::tests`.
#[test]
fn amit_return_of_capital_refusal_documented() {
    assert!(API_MD.contains(
        "A `ReturnOfCapital` dated in a financial year the [listing](#listings) was an `amit` \
         is rejected with `422`"
    ));
    assert!(API_MD.contains("the two paths are **mutually exclusive**"));
    // The AMIT side names both doors, so a reader arriving from the AMMA
    // statement sees why neither other path is open to it.
    assert!(API_MD.contains("**This is an AMIT's only cost-base movement.**"));
    assert!(API_MD.contains(
        "a `tax_deferred_amount` on its [income](#income) rows and a `ReturnOfCapital` on the \
         listing are each refused `422`"
    ));
    // SCENARIOS F-23: the converted-fund case is dated, not absolute — the
    // refusal follows the payment's own financial year, so the pre-conversion
    // years stay enterable and editable, and the cost-base chain nets both
    // kinds against one balance.
    assert!(API_MD.contains("the refusal then follows the **payment's own financial year**"));
    assert!(API_MD.contains(
        "since a `ReturnOfCapital` dated in one of the listing's AMIT years is refused at write time"
    ));
    assert!(
        API_MD.contains("a `ReturnOfCapital` dated in a financial year its listing was an `amit`")
    );
}

/// Docs-sync pin for SCENARIOS P-08: an investment-expense deduction's
/// destination question. The wrong label was previously neither corrected nor
/// disclosed — `13X`/`13Y` appeared nowhere in the docs at all — so this pins
/// the corrected routing in every place a user could read it: the tax
/// summary's deduction paragraph and CSV label table, the annual tax report's
/// `income` bullet, the README feature, and the mirrored ATO reference that
/// carries the instruction text behind each label.
#[test]
fn investment_expense_deduction_destinations_documented() {
    const ATO_LABELS: &str = include_str!("../docs/ato/tax-return-labels-2026.md");
    const ATO_OVERVIEW: &str = include_str!("../docs/ato/OVERVIEW.md");
    // The rule, and each destination with its label.
    assert!(
        API_MD.contains("**The same deductions are also cut by the question each is claimed at**")
    );
    for line in [
        "| `deductions_trust_distributions` | `13Y` |",
        "| `deductions_foreign_income` | `20M` |",
        "| `deductions_foreign_debt` | `D15` |",
        "| `deductions_dividend_and_interest` | `D7 / D8` |",
    ] {
        assert!(API_MD.contains(line), "API.md documents {line}");
    }
    // What the routing reads, and the two cases it cannot decide.
    assert!(
        API_MD.contains(
            "The destination is derived from the **listing the expense is attributed to**"
        )
    );
    assert!(API_MD.contains("Two cases are **not decidable** from what is recorded"));
    assert!(API_MD.contains("**portfolio-wide expense**"));
    assert!(API_MD.contains("**AUD listing with no income recorded**"));
    // The two cuts are of one total — no double counting.
    assert!(API_MD.contains(
        "`deductions_total` is the sum of the six per-type lines *or* of the four destination \
         lines, never of both"
    ));
    // The annual tax report prints each row's destination.
    assert!(API_MD.contains("`TrustDistributions`/`13Y`, `ForeignIncome`/`20M`, `ForeignDebt`/`D15`, or `DividendAndInterest`/`D7 / D8`"));
    assert!(
        README_MD.contains(
            "The same total is also cut by **the question each deduction is claimed at**"
        )
    );
    // The ATO instruction text each label rests on, mirrored and indexed.
    assert!(ATO_LABELS.contains("## Where an investment-expense deduction goes"));
    assert!(
        ATO_LABELS.contains("Write at question 13 – label **Y** the total of other deductions")
    );
    assert!(ATO_LABELS.contains("**excluding any debt deductions**"));
    assert!(ATO_LABELS.contains("D15 (label J)"));
    assert!(ATO_OVERVIEW.contains("*Where an investment-expense deduction goes* section"));
}

/// Docs-sync pin for SCENARIOS F-23: the AMIT status is dated, so a fund that
/// converted part-way through a holding keeps its earlier years as ordinary
/// trust income. The Listings section states the column, its 1 July rule and
/// its reason; every reader that compares against it is named; the schema
/// records the column; and the README surfaces the feature.
#[test]
fn dated_amit_status_documented() {
    assert!(API_MD.contains("**A fund that *converted* to an AMIT.**"));
    assert!(API_MD.contains("says *from when*, for a MIT that elected into the regime"));
    assert!(API_MD.contains("AMIT status is *elected for an income year*"));
    assert!(API_MD.contains("Use the 1 July on which the fund's first AMIT financial year began"));
    // The readers that compare against it, so a reader of any one of them
    // finds the rule rather than assuming the flag is absolute.
    assert!(API_MD.contains("for the years the listing was an AMIT"));
    assert!(API_MD.contains("only the years the listing was an AMIT are asked about"));
    // SCENARIOS P-01/P-07: the two readers that had drifted to a flat `amit`
    // filter — the annual tax report's printed income rows and the franking
    // holding-period walk — are named beside the rest, and what the per-year
    // rule means for each is stated where that report documents itself.
    assert!(API_MD.contains("completeness section *and* its printed income rows"));
    assert!(API_MD.contains(
        "still holding-period tested and still count toward the year's A$5,000 small-shareholder \
         total"
    ));
    assert!(API_MD.contains(
        "so a converted fund's pre-conversion distributions print here, behind the tax-summary \
         total that counts them"
    ));
    assert!(API_MD.contains(
        "a converted fund's pre-conversion distributions are ordinary trust income, so they are \
         tested here and their credits count toward the year's A$5,000 total"
    ));
    assert!(SCHEMA_MD.contains("amit_from    TEXT (nullable)"));
    assert!(README_MD.contains("A fund that **converted** to an AMIT records the 1 July"));
}

/// Docs-sync pin for the write-time state check that bounds that open edit
/// path (2026-08-15): a corporate-action write is refused when the resulting
/// terms would leave a sale allocating more units than its parcel holds, so
/// the Corporate actions section states what is re-checked (and that both
/// listings are, on a move), and the Response-codes `422` row lists the
/// refusal. The behaviour itself is pinned by
/// `entities::corporate_action::tests`.
#[test]
fn corporate_action_write_state_check_documented() {
    assert!(API_MD.contains("**Writing terms that would over-consume a parcel.**"));
    assert!(API_MD.contains(
        "every write of a corporate action re-checks — in the write's own transaction — that \
         each parcel of the affected listing still covers the allocations drawn on it"
    ));
    assert!(API_MD.contains("**both** listings are re-checked"));
    assert!(API_MD.contains(
        "writing a corporate action whose terms would leave a sale allocating more units than \
         the parcel it draws on holds"
    ));
}

/// Docs-sync pin for serving the app under a reverse-proxy sub-path: the
/// deployment story lives in the README (including the nginx block, whose
/// two easy-to-get-wrong details — no trailing slash on `proxy_pass`, and
/// an upload limit above the 25 MB attachment cap — are the whole reason
/// the snippet is shipped rather than left to the reader), and the API doc
/// states that every documented path moves under the prefix.
#[test]
fn reverse_proxy_base_path_documented() {
    assert!(README_MD.contains("### Behind a reverse proxy"));
    assert!(README_MD.contains("`--base-path`"));
    assert!(README_MD.contains("location /share_tracker/ {"));
    assert!(README_MD.contains("proxy_pass http://127.0.0.1:3000;"));
    assert!(README_MD.contains("No trailing slash on proxy_pass"));
    assert!(README_MD.contains("client_max_body_size 25m;"));
    assert!(README_MD.contains("X-Forwarded-Proto"));
    // API.md: the prefix applies to every documented path, and the
    // trailing-slash redirect is a documented response code.
    assert!(API_MD.contains("**Base path.**"));
    assert!(API_MD.contains("`GET /listings` becomes `GET /share_tracker/listings`"));
    assert!(API_MD.contains("`307 Temporary Redirect`"));
}

/// Docs-sync pin for the optional `[auth]` shared-credential access control
/// (`infra::auth`): the README surfaces it as a Feature and documents how to
/// configure and generate credentials for it; the API doc has its own
/// section plus the `401`/`303` response codes and the two accepted
/// limitations (cookie revocation, login CSRF) it introduces.
#[test]
fn authentication_documented() {
    // README: the Features bullet, the rewritten no-auth notes, the
    // dedicated section and its config/CLI-helper examples.
    assert!(README_MD.contains("**Authentication (optional)**"));
    assert!(README_MD.contains("### Authentication"));
    assert!(README_MD.contains("unless [`[auth]`](#authentication) is configured"));
    assert!(README_MD.contains("share-tracker hash-password"));
    assert!(README_MD.contains("share-tracker gen-token"));
    assert!(README_MD.contains("[auth]"));
    assert!(README_MD.contains("password_hash"));
    // API.md: the preamble line, the dedicated section, the login/logout
    // endpoints, and the response-code/known-limitations entries.
    assert!(API_MD.contains("**Authentication.**"));
    assert!(API_MD.contains("## Authentication"));
    assert!(API_MD.contains("GET` | `/login`"));
    assert!(API_MD.contains("POST` | `/login`"));
    assert!(API_MD.contains("POST` | `/logout`"));
    assert!(API_MD.contains("`303 See Other`"));
    assert!(API_MD.contains("`401 Unauthorized`"));
    assert!(known_limitations().contains("session cookies aren't revocable"));
    assert!(known_limitations().contains("no CSRF token"));
}

/// Docs-sync pin for deletes blocked by an inbound foreign key (SCENARIOS
/// A-18, A-23, A-38, A-41). The behaviour itself is tested in
/// `entities::tests`; what needs pinning here is the documentation half —
/// the shared explainer, the two directions the same `422` can mean, and the
/// A-23 dead end (a listing that has ever carried a **manual** closing price
/// can never be deleted), which is a consequence of two documented rules and
/// was stated by neither.
#[test]
fn deletes_blocked_by_a_dependant_documented() {
    // The shared section, its wording, and both directions of the 422.
    assert!(API_MD.contains("## Deletes blocked by a dependant"));
    assert!(API_MD.contains("this listing is still referenced by closing prices (2)"));
    assert!(API_MD.contains("the request refers to a record that does not exist"));
    assert!(API_MD.contains("There is no cascade delete"));
    // The response-code table and the error-bodies note point at it.
    assert!(API_MD.contains(
        "any `DELETE` of a row another table still references — see \
         [Deletes blocked by a dependant](#deletes-blocked-by-a-dependant)"
    ));
    assert!(API_MD.contains("except a blocked delete, whose body names the dependants instead"));
    // The A-23 dead end, stated where a reader would look for it.
    assert!(API_MD.contains(
        "**A listing that has ever had a closing price [entered by hand](#closing-prices) can no \
         longer be deleted at all.**"
    ));
    // The two entity sections whose blocked delete needed explaining.
    assert!(API_MD.contains("There is no cascade: each adjustment is removed individually"));
    assert!(API_MD.contains("`DELETE` returns `422` while anything still references the exchange"));
}

/// Docs-sync pin for fractional entitlements (SCENARIOS E-11, E-36). The
/// behaviour — every ratio-driven action keeping the exact fraction its ratio
/// produces — is pinned by tests in the corporate-action and rollover modules;
/// what needed writing down is the *convention* (registry rounding and
/// cash-in-lieu are deliberately not modelled), stated for all four ratio
/// actions rather than only `ShareSplit` and `Demerger`, and the answer to the
/// question it leaves the reader with: what to do with the cash actually
/// received for a fraction.
#[test]
fn fractional_entitlements_documented() {
    // The shared section: the convention, its reason, and both registry
    // practices it declines to model.
    assert!(API_MD.contains("### Fractional entitlements"));
    assert!(API_MD.contains(
        "Every ratio-driven action — `ShareSplit`, `BonusIssue`, `ScripForScrip`, `Demerger` — \
         keeps the **exact fractional quantity** its ratio produces"
    ));
    assert!(API_MD.contains("sell the aggregated fractions on-market and pay **cash in lieu**"));
    assert!(API_MD.contains("would silently lose (or invent) part of a parcel"));
    // What to do with the cash: a CGT event on the disposed fraction, entered
    // as an ordinary Sell — not a rounding to be absorbed.
    assert!(API_MD.contains(
        "it is the disposal of that fraction and its own (small) CGT event, not a bookkeeping \
         rounding: enter it as an ordinary [Sell](#sells) of the fractional units"
    ));
    assert!(API_MD.contains("A registry that rounds the entitlement *up* to a whole unit instead"));
    // E-11 / E-36: the two bullets that stated it for neither, with the
    // worked figures a reader can check their own entry against.
    assert!(API_MD.contains("a 1-for-10 issue on 105 units gives 10.5 bonus units"));
    assert!(API_MD.contains(
        "a 1-for-3 exchange of 101 units gives 33.666666666666666666666666667 replacement units"
    ));
    // …and the two that did, now pointing at the shared section.
    assert_eq!(
        API_MD
            .matches("[Fractional entitlements](#fractional-entitlements)")
            .count(),
        4
    );
}

/// Docs-sync pin for the franking-credit ceiling (SCENARIOS G-25): a credit
/// with no dividend behind it, or above what a company could have attached,
/// used to be accepted and reported as a refundable offset. The rule, its two
/// scope limits (the rounding tolerance and the pre-2001 cut), the trust
/// exemption, and the ATO mirror it rests on are all load-bearing — a reader
/// hitting the 422 has to be able to find out why.
#[test]
fn franking_credit_ceiling_documented() {
    assert!(API_MD.contains("**Franking credits are bounded by the dividend behind them:**"));
    assert!(API_MD.contains("`franked_amount × 30/70`"));
    // The scope limits, both of which decide whether a row is checked at all.
    assert!(API_MD.contains("before **1 July 2001**"));
    assert!(API_MD.contains("**Trust rows are exempt entirely**"));
    // The 422 catalogue carries both breaches and the buy-back's.
    assert!(API_MD.contains("have no `franked_amount` behind them"));
    assert!(API_MD.contains("a buy-back participation whose dividend component would breach"));

    // The mirror the ceiling rests on: the formula, and the member-side
    // sentence that makes it a rejection rather than a warning.
    const ALLOCATING: &str = include_str!("../docs/ato/allocating-franking-credits.md");
    assert!(
        ALLOCATING.contains("allocating-franking-credits"),
        "source header"
    );
    assert!(ALLOCATING.contains("QC 47305"));
    assert!(
        ALLOCATING
            .contains("Amount of the frankable distribution × (1 ÷ Applicable gross-up rate).")
    );
    assert!(ALLOCATING.contains(
        "the recipient is only entitled to a franking credit equal to the maximum amount."
    ));
    assert!(include_str!("../docs/ato/OVERVIEW.md").contains("allocating-franking-credits.md"));
}

/// Docs-sync pin for the conduit-foreign-income entry convention (SCENARIOS
/// G-03). The field was previously excluded from every total with nothing
/// stating why, which is right only if the figure is a memo *within*
/// `unfranked_amount` — and wrong, silently understating the year's income, if
/// a user keys the statement's CFI line as an amount of its own. The
/// convention is now decided and stated, so these pin the statement itself in
/// the API doc and the schema, the write-time ceiling that enforces it, and the
/// ATO mirror that supports it (the index used to credit a mirror that says
/// nothing about CFI at all).
#[test]
fn conduit_foreign_income_entry_convention_documented() {
    // API.md: the convention, its direction, and the resident's treatment.
    assert!(API_MD.contains("**Conduit foreign income is a memo inside the unfranked amount:**"));
    assert!(API_MD.contains("within** the unfranked amount, never in addition to it"));
    assert!(API_MD.contains("non-assessable non-exempt only for a **foreign resident**"));
    // …the write-time ceiling that keeps the convention true of stored rows.
    assert!(
        API_MD
            .contains("an income `conduit_foreign_income` exceeding the row's `unfranked_amount`")
    );
    // …and the memo column the annual tax report prints it as.
    assert!(API_MD.contains("`conduit_foreign_income_aud`"));

    // SCHEMA.md: the column carries the same convention, not "excluded from
    // assessable income" (which read as a treatment rather than a memo).
    let column = SCHEMA_MD
        .split("├── conduit_foreign_income")
        .nth(1)
        .expect("SCHEMA.md documents the income.conduit_foreign_income column")
        .split('\n')
        .next()
        .expect("split always yields at least one part");
    assert!(column.contains("Memo"), "{column}");
    assert!(column.contains("within that amount"), "{column}");
    assert!(
        !column.contains("Excluded from assessable income"),
        "{column}"
    );

    // The ATO mirror the treatment rests on really says it, and the index
    // attributes it to that mirror rather than to `mytax-managed-funds.md`.
    const AMMA_NOTES: &str = include_str!("../docs/ato/amma-statement-guidance-notes.md");
    const ATO_OVERVIEW: &str = include_str!("../docs/ato/OVERVIEW.md");
    assert!(AMMA_NOTES.contains(
        "Include an unfranked dividend paid out of conduit foreign income in \
         **Dividends: unfranked amount declared to be CFI**, which forms part of the \
         non-primary production income."
    ));
    assert!(
        !include_str!("../docs/ato/mytax-managed-funds.md")
            .to_lowercase()
            .contains("conduit"),
        "the mirror the index once credited still says nothing about CFI — \
         if that changes, revisit the attribution rather than deleting this"
    );
    assert!(ATO_OVERVIEW.contains("Part B item 13U"));
}

/// Docs-sync pin for the interest-timing convention (SCENARIOS H-05). An
/// `interest_income` row carries one date, and the calculation that buckets it
/// into a financial year is right — but nothing said *which* date it wants, so
/// a term deposit crediting $500 on 30 June with the funds only reachable on
/// 2 July could be keyed either way, moving a whole year's interest. The
/// decision was to state the convention rather than model a second column
/// (availability is not a tax fact), so these pin the statement everywhere a
/// user meets the field — the API doc, the schema, the UI hint (in `web.rs`'s
/// `interest_income_date_credited_hint_present`) — against the ATO wording it
/// rests on.
#[test]
fn interest_credited_date_convention_documented() {
    const TIMING: &str = include_str!("../docs/ato/investment-income-timing.md");
    const ATO_OVERVIEW: &str = include_str!("../docs/ato/OVERVIEW.md");

    // The mirror really carries the rule (line-wrapped and blockquoted, so
    // compare on collapsed whitespace) and its provenance header.
    // The quote is line-wrapped and blockquoted in the mirror, so compare with
    // the `>` markers and the wrapping collapsed away.
    let flat = |s: &str| {
        s.replace('>', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    const RULE: &str = "You must declare interest income in the year it is credited, received or \
                        applied or dealt with in any way on your behalf or as you direct. For term \
                        deposits this usually means you should declare interest in the year the \
                        investment matures.";
    assert!(flat(TIMING).contains(&flat(RULE)), "{}", flat(RULE));
    assert!(TIMING.contains("QC 72101"));
    assert!(TIMING.contains("**Retrieved:** 2026-08-17"));
    // …and the index credits it, so the mirror is reachable from OVERVIEW.
    assert!(ATO_OVERVIEW.contains("investment-income-timing.md"));

    // API.md states the convention, its cite, and the worked case that made it
    // ambiguous — plus why there is deliberately no availability column.
    assert!(API_MD.contains("**`date_paid` is the date credited:**"));
    assert!(
        API_MD.contains("credited, received or applied or dealt with in any way on your behalf")
    );
    assert!(API_MD.contains("declare interest in the year the investment matures"));
    assert!(API_MD.contains("`docs/ato/investment-income-timing.md`"));
    assert!(API_MD.contains("withdrawable on 2 July is FY2026 interest, not FY2027"));
    assert!(API_MD.contains("no second date column"));

    // SCHEMA.md's column says the same thing, so the field is unambiguous from
    // the data model alone.
    let column = SCHEMA_MD
        .split("├── date_paid             DATE")
        .nth(1)
        .expect("SCHEMA.md documents the interest_income.date_paid column")
        .split('\n')
        .next()
        .expect("split always yields at least one part");
    assert!(column.contains("credited"), "{column}");
    assert!(column.contains("investment-income-timing.md"), "{column}");
    assert!(
        column.contains("Never the date the funds became reachable"),
        "{column}"
    );
}

/// Docs-sync pin for the multi-year expense convention (SCENARIOS H-08). One
/// `investment_expenses` row is one financial year and deducts its whole
/// amount there, which is wrong for the two ordinary share-investor expenses
/// the ATO spreads across years — a $2,000 loan establishment fee keyed once
/// claims five years' deduction at once and nothing refuses it. The decision
/// was to document the one-row-per-year workaround rather than model a service
/// period, so these pin the limitation, both cited rules, and the mirror they
/// rest on (the deductions mirror lists borrowing costs as claimable without
/// saying over what period).
#[test]
fn multi_year_expense_apportionment_documented() {
    const APPORTIONMENT: &str = include_str!("../docs/ato/expense-time-apportionment.md");
    const ATO_OVERVIEW: &str = include_str!("../docs/ato/OVERVIEW.md");
    let flat = |s: &str| {
        s.replace('>', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };

    // The mirror carries both rules verbatim, with both provenance headers.
    assert!(flat(APPORTIONMENT).contains(
        "If your expenses total more than $100, apportion them over 5 years or the loan term, \
         whichever is shorter. If your expenses are $100 or less, you can claim a deduction for \
         the full amount in the year you incur them."
    ));
    assert!(APPORTIONMENT.contains("QC 104069"));
    assert!(APPORTIONMENT.contains("QC 106556"));
    assert!(APPORTIONMENT.contains("**Retrieved:** 2026-08-17"));
    // …the day-count formula and the worked example behind it.
    assert!(flat(APPORTIONMENT).contains("**A multiplied by (B divided by C)**"));
    assert!(flat(APPORTIONMENT).contains("$1,250 × (182 ÷ 396) = $572"));
    assert!(flat(APPORTIONMENT).contains("$1,250 × (215 ÷ 396) = $678"));
    // …and it is reachable from the index.
    assert!(ATO_OVERVIEW.contains("expense-time-apportionment.md"));

    // The Known limitation names both rules, their sources, and the workaround.
    let limits = known_limitations();
    assert!(limits.contains("An expense covering more than one financial year is not apportioned"));
    assert!(limits.contains("5 years or the loan term, whichever is shorter"));
    assert!(limits.contains("QC 104069"));
    assert!(limits.contains("QC 106556"));
    assert!(limits.contains("one row per financial year"));
    assert!(limits.contains("`docs/ato/expense-time-apportionment.md`"));
    // …including that the 12-month-rule case is the one the model gets right.
    assert!(limits.contains("*inside* the 12-month rule is immediately deductible"));

    // The entity's own section says the same where the row is written, and
    // SCHEMA.md's date column carries it too.
    assert!(
        API_MD.contains(
            "**One row is one financial year — a multi-year expense is entered per year:**"
        )
    );
    assert!(README_MD.contains("expense-time-apportionment.md"));
    let column = SCHEMA_MD
        .split("├── date_incurred         DATE")
        .nth(1)
        .expect("SCHEMA.md documents the investment_expenses.date_incurred column")
        .split('\n')
        .next()
        .expect("split always yields at least one part");
    assert!(column.contains("One row is one year"), "{column}");
    assert!(column.contains("expense-time-apportionment.md"), "{column}");
}

/// Docs-sync pin for what the inherited `cost_base` figure must already be net
/// of (SCENARIOS K-02, K-09). It is one number typed off an estate's records,
/// and two ATO rules can make it wrong in a way no stored fact can check — the
/// indexation a pre-1999 acquirer may have been carrying, and the
/// apportionment a holding split between beneficiaries needs. Neither is
/// modellable here (the estate side and the other beneficiaries are both out of
/// scope), so the requirement is satisfied by documentation, and these pin it
/// on every surface plus the mirror it rests on.
#[test]
fn inherited_cost_base_entry_conventions_documented() {
    const INHERITED_COST_BASE: &str = include_str!("../docs/ato/inherited-assets-cost-base.md");
    const ATO_OVERVIEW: &str = include_str!("../docs/ato/OVERVIEW.md");
    let flat = |s: &str| {
        s.replace('>', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };

    // The mirror carries the indexation rule verbatim, and the LPR test the
    // field's own hint rests on — both re-fetched 2026-08-18, when the page
    // had gained the legal-costs section.
    assert!(flat(INHERITED_COST_BASE).contains(
        "If the deceased died on or after 21 September 1999, you can't use indexation. If the \
         deceased's cost base includes indexation, you must recalculate the first element of \
         your cost base to exclude it."
    ));
    assert!(
        flat(INHERITED_COST_BASE)
            .contains("You include the expenditure on the date the LPR incurred it.")
    );
    assert!(flat(INHERITED_COST_BASE).contains(
        "if a LPR incurs costs to confirm the validity of the deceased's will or defend a claim \
         for control of the estate, these costs form part of the cost base of the estate's assets"
    ));
    assert!(flat(INHERITED_COST_BASE).contains(
        "Any charges for Cassie's solicitor services prior to the deceased's death can't be \
         included in the cost base of the estate's assets."
    ));
    assert!(INHERITED_COST_BASE.contains("QC 66053, last updated 22 June 2026"));
    assert!(INHERITED_COST_BASE.contains("re-fetched and expanded 2026-08-18"));
    assert!(ATO_OVERVIEW.contains("inherited-assets-cost-base.md"));

    // The entity's own section states both conventions where the row is
    // written, with the rule cited.
    assert!(API_MD.contains("**What the `cost_base` figure must already be net of.**"));
    assert!(API_MD.contains(
        "**Indexation is recalculated out** where the death was **on or after 21 September 1999**"
    ));
    assert!(API_MD.contains("**It is apportioned with the units.**"));
    assert!(API_MD.contains("half the units carry half the deceased's cost base"));
    // …and the LPR test, so a pre-death solicitor's bill is not entered.
    assert!(API_MD.contains("anything the same solicitor billed before the death is out"));

    // The Known limitation says the same, so a reader who never opens the
    // Inheritances section still meets it.
    let limits = known_limitations();
    assert!(limits.contains("indexation recalculated out"));
    assert!(limits.contains("apportioned to your own share"));
    assert!(README_MD.contains("the cost base you enter is your own share of it"));
}

/// Docs-sync pin for the LPR-expenditure scope cut (SCENARIOS K-04). The
/// cost-base pipeline converts a parcel at one rate — its (deemed) acquisition
/// month's — while the LPR incurs their expenditure after the death, so on a
/// foreign parcel the fee would translate at a month that can predate it by
/// decades. Recording it correctly needs a second, separately translated
/// cost-base element the single-rate design does not have, so the pair is
/// refused at write time and the omission documented.
#[test]
fn lpr_expenditure_on_a_foreign_parcel_documented() {
    let limits = known_limitations();
    assert!(limits.contains("**LPR expenditure is only recordable on an AUD inheritance**"));
    // Named against the FX-timing limitation it follows from, with the size of
    // the error it would otherwise report.
    assert!(limits.contains("*Cost-base FX timing*"));
    assert!(limits.contains("US$1,000 fee incurred at 0.50 reported as A$500"));
    // …and that the ordinary case is untouched.
    assert!(limits.contains("Australian LPR fees on Australian holdings"));

    // The entity's section says it where the row is written, and the 422 list
    // carries the cause.
    assert!(API_MD.contains("**LPR expenditure is only recordable on an AUD inheritance** (`422`"));
    assert!(API_MD.contains("a non-zero LPR expenditure is recorded on a non-AUD inheritance"));
    assert!(README_MD.contains("AUD holdings only — see Known limitations"));

    // SCHEMA.md's column carries the restriction too, so the data model states
    // it without the API doc.
    let column = SCHEMA_MD
        .split("├── lpr_expenditure           TEXT (decimal)")
        .nth(1)
        .expect("SCHEMA.md documents the inheritances.lpr_expenditure column")
        .split('\n')
        .next()
        .expect("split always yields at least one part");
    assert!(
        column.contains("Only recordable where `currency` is AUD"),
        "{column}"
    );
}

/// Docs-sync pin for the pre-sale tools' as-at candidate read (SCENARIOS
/// O-14/O-15/O-16). Both the [parcel-selection optimiser] and the [pre-sale
/// what-if] used to read the parcels open *today*, whatever date the request
/// named — so a past-dated request offered parcels that did not exist yet
/// (which a real Sell refuses) and withheld parcels sold since. The behaviour
/// is now the as-at rule the rest of the reports follow, and the two sections
/// plus the As-at date section have to say so, because the unit basis of a
/// caller's `units` and `price` depends on it.
#[test]
fn presale_tools_read_candidates_as_at_the_request_date() {
    // The As-at date section names them and states the unit basis.
    let as_at = API_MD
        .split("### As-at date")
        .nth(1)
        .expect("docs/API.md has an As-at date section")
        .split("\n### ")
        .next()
        .expect("split always yields at least one part");
    assert!(as_at.contains("[parcel-selection optimiser](#parcel-selection-optimiser)"));
    assert!(as_at.contains("[pre-sale what-if](#pre-sale-what-if)"));
    assert!(as_at.contains("as at the request's `sale_date` / `date`"));
    assert!(as_at.contains("that date's** unit basis"));

    // Each endpoint's own section states the dated read and both directions.
    assert!(API_MD.contains(
        "The candidate parcels are the [open-parcels](#open-parcels) rows **as at `sale_date`**"
    ));
    assert!(API_MD.contains("A parcel acquired *after* `sale_date` is not a candidate"));
    assert!(API_MD.contains("a parcel sold *since* `sale_date` is, because it was open then"));
    assert!(
        API_MD.contains("drawn from the listing's [open parcels](#open-parcels) **as at `date`**")
    );
    assert!(API_MD.contains("open **as at `date`** with enough remaining units then"));
    assert!(API_MD.contains("beyond the quantity open as at `date`"));

    // The 422 catalogue carries the dated bound for both causes.
    assert!(
        API_MD.contains(
            "more units than the listing's open quantity **as at the request's sale date**"
        )
    );
    assert!(API_MD.contains("a parcel not open as at that date (including one acquired after it"));
}

/// Docs-sync pin for the price-collection lookback window (SCENARIOS Q-01).
/// The Jobs section stated a **7**-day window for `price-import` while the
/// Closing prices section, the README and the code all said 14 — and 14 is
/// not an arbitrary figure: it is *the same constant* the report-snapshot
/// catch-up window is defined as, because a date the snapshot job keeps
/// retrying but collection no longer refills could never unblock itself. So
/// every place that states the window is pinned to the constant itself, along
/// with the tie between the two windows that forces them to be one number.
#[test]
fn price_collection_lookback_window_documented_as_the_constant() {
    let days = crate::entities::closing_price::COLLECTION_LOOKBACK_DAYS;
    // One number, not two — so the docs are entitled to state one figure.
    assert_eq!(crate::reports::snapshot::CATCHUP_LOOKBACK_DAYS, days);
    let window = format!("last {days} calendar days");

    let section = |heading: &str| -> &'static str {
        API_MD
            .split(heading)
            .nth(1)
            .unwrap_or_else(|| panic!("docs/API.md has a {heading} section"))
            .split("\n## ")
            .next()
            .expect("split always yields at least one part")
    };

    // Closing prices: the window, and the reason it has to be that long.
    let closing_prices = section("## Closing prices");
    assert!(
        closing_prices.contains(&format!("**{window}**")),
        "the Closing prices section states the collection window as {days} calendar days"
    );
    assert!(closing_prices.contains(
        "deliberately the same length as the [report-snapshot](#report-snapshots) catch-up window"
    ));
    assert!(closing_prices.contains("could never unblock itself"));

    // Jobs: the same window for the same job, described a second time.
    let jobs = section("## Jobs");
    assert!(jobs.contains("`price-import`"));
    assert!(
        jobs.contains(&window),
        "the Jobs section states the `price-import` window as {days} calendar days"
    );
    assert!(
        jobs.contains(&format!("over its {days}-day window")),
        "the Jobs section states the `report-snapshot` window as {days} days"
    );

    // README's Features list states both jobs' windows.
    assert!(README_MD.contains(&format!("self-heals the {window}")));
    assert!(README_MD.contains(&format!("backfills missing dates over a {days}-day window")));

    // The Known limitations entry states them as the one bounded window.
    assert!(known_limitations().contains(&format!(
        "({days} calendar days, for prices and snapshots alike)"
    )));
}

/// Docs-sync pin for the rename UI (SCENARIOS R-01/R-05). The Web frontend
/// paragraph enumerates the UI's screens and actions, so the rename action
/// and the chain view it is paired with belong in it: the 422 the Listings
/// form raises names `POST /listings/:id/rename`, and this is where the docs
/// say that endpoint is reachable from.
#[test]
fn listing_rename_ui_documented() {
    let frontend = API_MD
        .split("## Web frontend")
        .nth(1)
        .expect("docs/API.md has a Web frontend section")
        .split("\n## ")
        .next()
        .expect("split always yields at least one part");
    // The action, and why it exists at all (the PUT refusal it answers).
    assert!(frontend.contains("a **Rename** action on listing rows (`POST /listings/:id/rename`"));
    assert!(
        frontend.contains("a `PUT` refuses on a listing with recorded trades, income, or prices")
    );
    // The chain view, its route, and the read behind it.
    assert!(frontend.contains("a **Rename history** view (`#/renames/<listing>`)"));
    assert!(frontend.contains("`GET /listings/:id/renames`"));
    // The undo, and the newest-only rule the API enforces.
    assert!(
        frontend.contains(
            "**Undo** on the newest entry only (`DELETE /listings/:id/renames/:rename_id`"
        )
    );
    assert!(frontend.contains("the chain unwinds last-in-first-out"));
    // The hash-route list carries the new route.
    assert!(frontend.contains("`#/renames/<listing>`, `#/r/<report>`"));
}

/// Docs-sync pin for SCENARIOS T-11: the run record now opens when a run
/// *starts*, and a backup is written under a staging name and renamed into
/// place only once it verifies. Both are operational promises an operator reads
/// before they ever read the code — what `GET /jobs` says about a run that
/// never finished, and what is safe to restore from the backup directory — so
/// each has to be stated where it is looked for.
#[test]
fn interrupted_runs_and_staged_backups_documented() {
    // API.md: the response shape's new field, the three run states, and the
    // meaning of a row left `running`.
    assert!(API_MD.contains(r#""last_finished_at", "last_status", "last_error""#));
    assert!(API_MD.contains(r#"`status` is one of `"running"`, `"ok"` or `"failed"`"#));
    assert!(API_MD.contains("appends a `job_runs` row **when it starts**"));
    assert!(
        API_MD.contains("which is what distinguishes an interrupted run from one that never began")
    );
    // API.md: the backup job's own paragraph — staging, the order, the bound.
    assert!(API_MD.contains("staging name** (`<name>.db.partial`)"));
    assert!(API_MD.contains("write, verify, rename, in that order"));
    assert!(API_MD.contains("bounded to the newest 3"));
    // SCHEMA.md: the columns the migration changed.
    assert!(SCHEMA_MD.contains("running | ok | failed (CHECK, 0042)"));
    assert!(SCHEMA_MD.contains("NULL while status = 'running' (0042)"));
    // README, Scheduled maintenance: the staging file, the startup sweep, and
    // the `.bad` bound that replaced "never touched".
    assert!(README_MD.contains("`<stem>-YYYY-MM-DD-HHMMSS.db.partial`"));
    assert!(README_MD.contains("Startup sweeps leftover `.partial` files of this database"));
    assert!(README_MD.contains("bounded to the **newest 3**"));
    assert!(
        !README_MD.contains("quarantined `.bad` files, and anything else are never touched"),
        "the old promise that `.bad` files are never pruned must not survive alongside the bound"
    );
}

/// Docs-sync pin for manual-only jobs (SCENARIOS T-09/schedule). The startup
/// "no schedule entry" WARN now fires only for a job that expects a schedule,
/// because the registry records the intent (`register_manual`); both documents
/// have to say where that intent lives, and `GET /jobs`'s shape has to carry
/// the new field, or the WARN's meaning is only knowable from the code.
#[test]
fn manual_only_jobs_documented() {
    // API.md: the new field, both of its values, and where the intent is set.
    assert!(API_MD.contains(r#"`{ "name", "trigger", "next_run_at", "last_started_at""#));
    assert!(API_MD.contains(r#"`"manual_only"`"#));
    assert!(API_MD.contains("registered with `register_manual`"));
    assert!(
        API_MD.contains("The two manual-only jobs are `price-rebase` and `settlement-recompute`")
    );
    // README: the WARN means a lost line, and the registry is where a
    // deliberately schedule-less job says so.
    assert!(README_MD.contains("that warning now means one thing only, that a line has been lost"));
    assert!(README_MD.contains("it is added with `register_manual` rather than `register`"));
    assert!(README_MD.contains(r#"`"trigger": "manual_only"`"#));
}

/// Docs-sync pin for the stored schedule and the overdue check
/// (SCENARIOS T-11/T-02/T-12). A job that has stopped running records nothing,
/// so no run history can show it: the signal is the scheduler's own next-run
/// instant, persisted. All three documents have to carry that — the endpoint
/// shape it reaches the UI through, the table it lives in, and what an operator
/// is told the alert means — or the only place the rule exists is the code.
#[test]
fn overdue_jobs_and_the_stored_schedule_documented() {
    // API.md: the two new health lists, the margin, and what each is *not*.
    assert!(API_MD.contains(r#""failed_jobs", "overdue_jobs", "stalled_jobs""#));
    assert!(API_MD.contains("- `overdue_jobs` — every scheduled entry whose stored next run"));
    assert!(API_MD.contains("a job that is not running at all records nothing"));
    assert!(API_MD.contains("a scheduler that has stopped **while the process is up**"));
    assert!(API_MD.contains("A **manual-only** job never appears"));
    assert!(API_MD.contains("- `stalled_jobs` — every job whose most recent run has been"));
    // API.md: the field the Jobs screen's "next run" column reads.
    assert!(
        API_MD.contains("`next_run_at` is when the **running scheduler** says the job is next due")
    );
    // SCHEMA.md: the table and the two columns the check turns on.
    assert!(SCHEMA_MD.contains("job_schedule "));
    assert!(SCHEMA_MD.contains("RFC 3339 **UTC** instant of the next scheduled run"));
    assert!(
        SCHEMA_MD.contains("a job whose `schedule.cron` line has been removed simply has no row")
    );
    // README, Scheduled maintenance: the stored instant, the 30-February case
    // that started it, and the two surfaces an operator reads.
    assert!(README_MD.contains("**and stored**, in `job_schedule`, one row per schedule entry"));
    assert!(README_MD.contains("`0 0 30 2 *`, 30 February"));
    assert!(README_MD.contains("the Jobs screen carries a **next run** column"));
}

/// Pins for the FreeBSD packaging + versioned-release pipeline (REQUIREMENTS
/// 2026-07-13). The release workflow and package skeleton are plain text CI
/// consumes, so these tests keep their load-bearing pieces from silently
/// drifting apart: the version flows Cargo.toml → build-pkg.sh → manifest,
/// every file the plist packages is staged, and the rc script drives the
/// config file the package installs.
mod freebsd_packaging {
    use super::README_MD;

    const RELEASE_YML: &str = include_str!("../.github/workflows/release.yml");
    const BUILD_SH: &str = include_str!("../pkg/freebsd/build-pkg.sh");
    const SMOKE_SH: &str = include_str!("../pkg/freebsd/smoke-test.sh");
    const MANIFEST: &str = include_str!("../pkg/freebsd/manifest.ucl");
    const PLIST: &str = include_str!("../pkg/freebsd/plist");
    const RC_SCRIPT: &str = include_str!("../pkg/freebsd/share_tracker");

    /// The version number has one source of truth: Cargo.toml. The build
    /// script reads it from there and substitutes the manifest's placeholder;
    /// the binary's --version comes from the same place (pinned in
    /// `infra::args`); the workflow reads it the same way for the tag.
    #[test]
    fn version_flows_from_cargo_toml_alone() {
        let extract = r#"sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml"#;
        assert!(BUILD_SH.contains(extract));
        assert!(SMOKE_SH.contains(extract));
        assert!(RELEASE_YML.contains(extract));
        assert!(MANIFEST.contains(r#"version: "__VERSION__""#));
        assert!(BUILD_SH.contains("sed \"s/__VERSION__/$VERSION/\""));
    }

    /// The workflow releases on push to main only when the version has no
    /// release yet, builds in a FreeBSD 15.1 VM (pkg ABI matches the
    /// deployment host), smoke-tests the installed package, and tags the
    /// exact commit that produced the pkg.
    #[test]
    fn release_workflow_shape() {
        assert!(RELEASE_YML.contains("branches: [main]"));
        assert!(RELEASE_YML.contains(r#"gh release view "v$version""#));
        assert!(RELEASE_YML.contains("vmactions/freebsd-vm"));
        assert!(RELEASE_YML.contains(r#"release: "15.1""#));
        // curl is the smoke test's POST client — fetch(1) can't POST and a
        // raw nc request half-closes early (hyper cancels the request).
        assert!(RELEASE_YML.contains("pkg install -y rust protobuf curl"));
        assert!(RELEASE_YML.contains("sh pkg/freebsd/build-pkg.sh"));
        assert!(RELEASE_YML.contains("sh pkg/freebsd/smoke-test.sh"));
        // The release tag points at the built commit, not a branch head.
        assert!(RELEASE_YML.contains(r#"--target "$GITHUB_SHA""#));
        assert!(RELEASE_YML.contains("share-tracker-*.pkg"));
        // Release notes come from the commits between tags, not the PR-based
        // --generate-notes (empty on a direct-to-main repo). The notes script
        // needs the previous tag, so the checkout must be full-history.
        assert!(RELEASE_YML.contains(r#"sh scripts/release-notes.sh "$VERSION""#));
        assert!(RELEASE_YML.contains("--notes-file notes.md"));
        assert!(RELEASE_YML.contains("fetch-depth: 0"));
        assert!(!RELEASE_YML.contains("--generate-notes"));
    }

    /// Executable check of `scripts/release-notes.sh` in a scratch git repo:
    /// the notes list exactly the commits after the previous tag (newest
    /// first, abbreviated SHA), link the tag-to-tag compare, exclude the
    /// release's own tag when re-run, and fall back to "Initial release."
    /// with the full history when no tag exists yet.
    #[test]
    fn release_notes_script_lists_commits_between_tags() {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/release-notes.sh");
        let dir = tempfile::tempdir().expect("tempdir");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        let notes = |version: &str| {
            let out = std::process::Command::new("sh")
                .args([script, version])
                .current_dir(dir.path())
                .output()
                .expect("script runs");
            assert!(out.status.success(), "release-notes.sh: {out:?}");
            String::from_utf8(out.stdout).expect("utf-8 notes")
        };

        git(&["init", "-q"]);
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "Ancient pre-tag work",
        ]);

        // No tag yet: first-release mode lists the whole history.
        let first = notes("0.1.0");
        assert!(first.contains("Initial release."));
        assert!(first.contains("- Ancient pre-tag work ("));

        git(&["tag", "v0.1.0"]);
        git(&["commit", "-q", "--allow-empty", "-m", "Add feature X"]);
        git(&["commit", "-q", "--allow-empty", "-m", "Fix feature X"]);

        let body = notes("0.2.0");
        assert!(body.contains("## Changes since v0.1.0"));
        assert!(body.contains("- Add feature X ("));
        assert!(body.contains("- Fix feature X ("));
        // Only the commits after the previous tag appear.
        assert!(!body.contains("Ancient pre-tag work"));
        // Compare link spans previous tag to this release's tag.
        assert!(body.contains("/compare/v0.1.0...v0.2.0"));

        // Re-run after this release's tag exists (e.g. a retried job): the
        // notes still diff against the previous tag, not the release itself.
        git(&["tag", "v0.2.0"]);
        assert_eq!(notes("0.2.0"), body);
    }

    /// Every path the plist packages is staged by the build script, and vice
    /// versa nothing is staged that the plist would silently drop.
    #[test]
    fn plist_matches_staged_files() {
        for (plist_entry, staged) in [
            ("bin/share-tracker", "$STAGE/usr/local/bin/"),
            ("etc/rc.d/share_tracker", "$STAGE/usr/local/etc/rc.d/"),
            ("etc/share-tracker.toml.sample", "share-tracker.toml.sample"),
            ("etc/share-tracker.cron.sample", "share-tracker.cron.sample"),
            (
                "etc/newsyslog.conf.d/share-tracker.conf.sample",
                "newsyslog.conf.d/share-tracker.conf.sample",
            ),
        ] {
            assert!(PLIST.contains(plist_entry), "plist lists {plist_entry}");
            assert!(BUILD_SH.contains(staged), "build-pkg.sh stages {staged}");
        }
        // Four install lines in the script, four plist entries — a new staged
        // file must appear in both.
        let installs = BUILD_SH
            .lines()
            .filter(|l| l.starts_with("install "))
            .count();
        let entries = PLIST.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(installs, entries);
    }

    /// The plist must stay free of ports keywords: `pkg create` resolves
    /// `@sample` & co. from the ports tree (`/usr/ports/Keywords/*.ucl`),
    /// which the release VM doesn't have — an unknown keyword made pkg create
    /// emit nothing while still "succeeding" (the first v0.2.0 run). The
    /// sample→live copy semantics live in the manifest's own scripts instead,
    /// and both scripts hard-fail on any missed step.
    #[test]
    fn sample_configs_activate_without_ports_keywords() {
        assert!(!PLIST.contains('@'), "plist uses no ports keywords");
        // Manifest scripts re-implement @sample: copy into place on first
        // install, remove on deinstall only while unmodified.
        assert!(MANIFEST.contains("post-install"));
        assert!(MANIFEST.contains(r#"cp -p "/usr/local/etc/$f.sample" "/usr/local/etc/$f""#));
        assert!(MANIFEST.contains("pre-deinstall"));
        assert!(MANIFEST.contains(r#"cmp -s "/usr/local/etc/$f.sample" "/usr/local/etc/$f""#));
        // Every shipped config goes through that activation loop.
        assert_eq!(
            MANIFEST
                .matches("for f in share-tracker.toml share-tracker.cron newsyslog.conf.d/share-tracker.conf")
                .count(),
            2,
            "install and deinstall loops both cover every sample config"
        );
        // The scripts enforce -eu in the body (`sh script.sh` drops shebang
        // flags) and build-pkg.sh refuses to "succeed" without the artifact.
        assert!(BUILD_SH.contains("\nset -eu\n"));
        assert!(SMOKE_SH.contains("\nset -eu\n"));
        assert!(BUILD_SH.contains(r#"[ -f "$OUT/share-tracker-$VERSION.pkg" ]"#));
        // The smoke test proves activation happened: without these, a failed
        // copy goes unnoticed because the server answers HTTP on defaults.
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/share-tracker.toml ]"));
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/share-tracker.cron ]"));
        assert!(SMOKE_SH.contains("[ -f /usr/local/etc/newsyslog.conf.d/share-tracker.conf ]"));
        assert!(SMOKE_SH.contains("pw usershow share_tracker"));
    }

    /// The rc script points the server at the config file the package
    /// installs (the activated copy of `share-tracker.toml.sample`, whose
    /// parseability is pinned in `infra::config`), and the smoke test proves
    /// the service pieces before anything is released.
    #[test]
    fn rc_script_and_smoke_test_drive_the_installed_config() {
        assert!(RC_SCRIPT.contains("rcvar=share_tracker_enable"));
        assert!(
            RC_SCRIPT.contains(": ${share_tracker_config:=\"/usr/local/etc/share-tracker.toml\"}")
        );
        assert!(RC_SCRIPT.contains("--config ${share_tracker_config}"));
        // daemon(8) supervision: restart on exit, log to the server's own
        // file, and reopen it (-H, instead of forwarding SIGHUP) when the
        // shipped newsyslog config rotates it — rotation must never restart
        // the server.
        assert!(RC_SCRIPT.contains("command=\"/usr/sbin/daemon\""));
        assert!(RC_SCRIPT.contains(": ${share_tracker_logfile:=\"/var/log/share-tracker.log\"}"));
        assert!(RC_SCRIPT.contains("-H -o ${share_tracker_logfile}"));
        const NEWSYSLOG: &str = include_str!("../pkg/freebsd/newsyslog.conf");
        assert!(NEWSYSLOG.contains("/var/log/share-tracker.log"));
        assert!(NEWSYSLOG.contains("/var/run/share_tracker/share_tracker.pid"));
        // share_tracker_user is rc.subr's ${name}_user convention: the whole
        // daemon(8) chain runs su(1)'d to the service user, so the pidfile
        // lives in a service-user-owned subdirectory the precmd (re)creates
        // each start — /var/run itself is root-only and cleared at boot. A
        // bare /var/run/<name>.pid fails "Permission denied" (shipped broken
        // in v0.4.0).
        assert!(RC_SCRIPT.contains("pidfile=\"/var/run/${name}/${name}.pid\""));
        assert!(RC_SCRIPT.contains("start_precmd=\"share_tracker_precmd\""));
        assert!(RC_SCRIPT.contains("install -d -o \"${share_tracker_user}\" \"/var/run/${name}\""));
        // The pidfile holds the daemon(8) supervisor's pid, so procname must
        // stay at its rc.subr default ($command = daemon): overriding it to
        // the server binary made check_pidfile reject the pid and status/stop
        // report "not running" while the service kept running.
        assert!(!RC_SCRIPT.contains("procname="));
        // daemon(8)'s argv carries the server binary explicitly (a ${procname}
        // spelling once expanded empty, so daemon parsed --config as its own
        // option) and must NOT carry -u: rc.subr's ${name}_user su already
        // dropped privileges, and an unprivileged daemon -u fails
        // setusercontext (initgroups EPERM), which -r turns into a respawn
        // loop that never starts the server (the v0.4.1 release-blocker).
        assert!(
            RC_SCRIPT.contains("-r /usr/local/bin/share-tracker --config ${share_tracker_config}")
        );
        assert!(!RC_SCRIPT.contains("-u ${share_tracker_user}"));
        // Install creates the service user the rc script runs as.
        assert!(MANIFEST.contains("pw useradd share_tracker"));
        assert!(RC_SCRIPT.contains(": ${share_tracker_user:=\"share_tracker\"}"));
        // Smoke test: installed version agrees, rc script loads, HTTP answers.
        assert!(SMOKE_SH.contains("grep -qx \"share-tracker $VERSION\""));
        assert!(SMOKE_SH.contains("/usr/local/etc/rc.d/share_tracker rcvar"));
        assert!(SMOKE_SH.contains("http://127.0.0.1:3999/reports/health"));
        // …and the service really starts through the rc script (a direct
        // binary run can't catch rc plumbing like the pidfile bug above).
        assert!(SMOKE_SH.contains("service share_tracker onestart"));
        assert!(SMOKE_SH.contains("http://127.0.0.1:3000/reports/health"));
        assert!(SMOKE_SH.contains("[ -s /var/run/share_tracker/share_tracker.pid ]"));
        // Stop must be bounded (an unbounded onestop once wedged the release
        // VM for 20 minutes), must genuinely succeed — no `|| true` — and
        // must take the server down with the supervisor.
        assert!(SMOKE_SH.contains("\ntimeout 30 service share_tracker onestop\n"));
        assert!(SMOKE_SH.contains("server survived onestop"));
        // Rotation is proven end-to-end: force a newsyslog rotation, then
        // fresh log lines must land in the new file (daemon -H reopened it)
        // without a new startup banner (the server was not restarted).
        assert!(SMOKE_SH.contains("newsyslog -F /var/log/share-tracker.log"));
        assert!(SMOKE_SH.contains("daemon -H reopen failed"));
        assert!(SMOKE_SH.contains("server restarted on rotation"));
        // The file gets plain text: logging::init switches ANSI off when
        // stdout is not a terminal, and the smoke test holds that end-to-end.
        assert!(SMOKE_SH.contains("ANSI escape codes in log file"));
        // On failure the smoke test surfaces the server's own output (it
        // goes to syslog under daemon -S, invisible in the CI log) by
        // re-running the service invocation with output to a file.
        assert!(SMOKE_SH.contains("-o /tmp/svc-diag.log"));
    }

    /// README documents installing the package, the configuration file, and
    /// how a release is cut.
    #[test]
    fn readme_documents_packaging_and_versioning() {
        assert!(README_MD.contains("## Installing on FreeBSD"));
        assert!(README_MD.contains("sysrc share_tracker_enable=YES"));
        assert!(README_MD.contains("`/var/log/share-tracker.log`"));
        assert!(README_MD.contains("newsyslog"));
        assert!(README_MD.contains("### Configuration file"));
        assert!(README_MD.contains("**CLI flag > config-file value > built-in default**"));
        assert!(README_MD.contains("## Releases and versioning"));
        assert!(README_MD.contains("**Cutting a release = bumping `version` in `Cargo.toml`**"));
    }
}
