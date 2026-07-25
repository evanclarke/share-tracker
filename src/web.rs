//! Serves the embedded single-page web frontend.
//!
//! The frontend is plain HTML/CSS/JS with no build step — the assets are
//! compiled into the binary with `include_str!` (the same approach as
//! `schedule.cron`), so the server has no runtime filesystem dependency and the
//! routes are testable with `oneshot`. The JS ships as native ES modules
//! (`<script type="module">` in the shell): `app.js` is the entry point (the
//! generic rendering engine and router) importing `config.js` (the
//! ENTITIES/REPORTS/ACTIONS configuration), `forms.js` (field constructors and
//! form wiring), and `util.js` (shared utilities) — each import specifier must
//! have a matching `/static/...` route here. The app is a hash-routed SPA that
//! talks to the existing JSON API, so `/` is the only HTML entry point (deep
//! links use `#/...` fragments, which never reach the server) and no SPA
//! fallback route is needed.
//!
//! The pure JS helpers are unit-tested by `src/web/*.test.js`, executed with
//! `node --test 'src/web/*.test.js'` (Node 22+, no build step —
//! `src/web/package.json` marks the tree as ES modules so Node parses the
//! files exactly as the browser does). Test files are never servable: this
//! `JS_MODULES` allowlist is the only route table, and the
//! `js_test_files_are_not_served_and_every_module_is` test pins that no
//! `*.test.js` file is listed on it (and that every non-test module is).
use axum::{
    Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use sqlx::SqlitePool;

const INDEX_HTML: &str = include_str!("web/index.html");
const STYLE_CSS: &str = include_str!("web/style.css");

/// The ES modules making up the app, as (route, source) pairs: the `app.js`
/// entry point plus everything it (transitively) imports. A new module is
/// served by adding a pair here.
const JS_MODULES: [(&str, &str); 5] = [
    ("/static/app.js", include_str!("web/app.js")),
    ("/static/chart.js", include_str!("web/chart.js")),
    ("/static/config.js", include_str!("web/config.js")),
    ("/static/forms.js", include_str!("web/forms.js")),
    ("/static/util.js", include_str!("web/util.js")),
];

/// Routes serving the frontend shell and its static assets. Returns a
/// `Router<SqlitePool>` purely so it merges with the entity/report routers; the
/// handlers are stateless.
pub fn router() -> Router<SqlitePool> {
    let mut router = Router::new()
        .route("/", get(index))
        .route("/static/style.css", get(style_css));
    for (path, source) in JS_MODULES {
        router = router.route(path, get(move || async move { js_asset(source) }));
    }
    router
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

fn js_asset(body: &'static str) -> Response {
    asset("text/javascript; charset=utf-8", body)
}

async fn index() -> Response {
    asset("text/html; charset=utf-8", INDEX_HTML)
}

async fn style_css() -> Response {
    asset("text/css; charset=utf-8", STYLE_CSS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get(uri: &str) -> Response {
        router()
            .with_state(SqlitePool::connect(":memory:").await.unwrap())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_string(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn index_is_served_as_html() {
        let resp = get("/").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let body = body_string(resp).await;
        // The shell loads the SPA assets and provides the mount point + nav.
        // The JS is native ES modules, so the entry script must load as one.
        assert!(body.contains("<script type=\"module\" src=\"/static/app.js\">"));
        assert!(body.contains("/static/style.css"));
        assert!(body.contains("id=\"app\""));
        assert!(body.contains("id=\"nav\""));
    }

    #[tokio::test]
    async fn es_modules_are_served_as_javascript() {
        for (path, source) in JS_MODULES {
            let resp = get(path).await;
            assert_eq!(resp.status(), StatusCode::OK, "{path}");
            assert_eq!(
                resp.headers().get(header::CONTENT_TYPE).unwrap(),
                "text/javascript; charset=utf-8",
                "{path}"
            );
            assert_eq!(body_string(resp).await, source, "{path}");
        }
    }

    // Every `./x.js` import specifier in the served modules must resolve to a
    // served `/static/x.js` route — a module added to the graph but not to
    // JS_MODULES would 404 at runtime and break the whole app.
    #[tokio::test]
    async fn every_module_import_is_served() {
        for (path, source) in JS_MODULES {
            for line in source.lines() {
                let Some((_, spec)) = line.split_once(" from './") else {
                    continue;
                };
                let module = spec.trim_end_matches(';').trim_end_matches('\'');
                let route = format!("/static/{module}");
                assert!(
                    JS_MODULES.iter().any(|(p, _)| *p == route),
                    "{path} imports {module}, but {route} is not served"
                );
            }
        }
    }

    // The recorded decision on how JS test files stay out of the served
    // bundle: `JS_MODULES` is an explicit allowlist (nothing under `src/web/`
    // is served unless listed), unit tests live beside the modules as
    // `src/web/*.test.js` (run by `node --test`, never by the server), and
    // this test keeps the two sets partitioned — a test file on the route
    // table, or a new module missing from it, fails here.
    #[test]
    fn js_test_files_are_not_served_and_every_module_is() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web");
        for entry in std::fs::read_dir(dir).expect("src/web exists") {
            let name = entry.expect("readable dir entry").file_name();
            let name = name.to_str().expect("utf-8 filename");
            if !name.ends_with(".js") {
                continue;
            }
            let route = format!("/static/{name}");
            let served = JS_MODULES.iter().any(|(p, _)| *p == route);
            if name.ends_with(".test.js") {
                assert!(!served, "{name} is a test file and must not be served");
            } else {
                assert!(
                    served,
                    "{name} is not in JS_MODULES — add its (route, include_str!) pair"
                );
            }
        }
    }

    #[tokio::test]
    async fn style_css_is_served_as_css() {
        let resp = get("/static/style.css").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/css; charset=utf-8"
        );
    }

    // Each UI item maps to a view registered in the served app bundle — the
    // concatenation of every served ES module. Without a browser harness these
    // assert the view (and the API endpoint it drives) is present in the
    // shipped JS — the honest, testable limit of an embedded SPA.
    async fn app_js_body() -> String {
        let mut bundle = String::new();
        for (path, _) in JS_MODULES {
            bundle.push_str(&body_string(get(path).await).await);
        }
        bundle
    }

    #[tokio::test]
    async fn exchange_management_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/exchanges"));
        // The session close time (gates same-day closing-price collection) is
        // editable alongside the other exchange fields.
        assert!(js.contains("close_time"));
    }

    #[tokio::test]
    async fn closing_prices_ui_present() {
        let js = app_js_body().await;
        // The price-history screen lists stored prices (incl. errored rows)
        // through the shared filterableTable…
        assert!(js.contains("viewClosingPrices"));
        assert!(js.contains("/closing_prices"));
        // …with the per-row re-fetch action and the backfill form driving the
        // two on-demand endpoints.
        assert!(js.contains("/closing_prices/fetch"));
        assert!(js.contains("/closing_prices/backfill"));
        // The price-import job is described in the Jobs view.
        assert!(js.contains("price-import"));
    }

    #[tokio::test]
    async fn report_snapshots_ui_present() {
        let js = app_js_body().await;
        // The snapshot list + per-snapshot detail views drive the snapshot
        // endpoints (list, detail, on-demand generate/regenerate)…
        assert!(js.contains("viewSnapshots"));
        assert!(js.contains("viewSnapshotDetail"));
        assert!(js.contains("/report_snapshots"));
        assert!(js.contains("/report_snapshots/generate"));
        assert!(js.contains("#/r/snapshots"));
        // …with stale/provisional snapshots badged and regenerable per row.
        assert!(js.contains("m.stale ? 'stale' : (m.provisional ? 'provisional' : 'ok')"));
        assert!(js.contains("Regenerate"));
        // The bulk repair buttons drive the two regeneration endpoints, and
        // the detail view explains a provisional snapshot.
        assert!(js.contains("/report_snapshots/regenerate_all"));
        assert!(js.contains("/report_snapshots/regenerate_provisional"));
        assert!(js.contains("Regenerate all"));
        assert!(js.contains("Regenerate provisional"));
        assert!(js.contains("snap.provisional"));
        // Regenerate-all takes a date range, prefilled from the API's
        // default-range endpoint.
        assert!(js.contains("/report_snapshots/regenerate_range"));
        assert!(js.contains("rangeFromInp"));
        assert!(js.contains("rangeToInp"));
        // The time-series graph is built as inline SVG (no build step, no
        // chart library) from the series endpoint: market value and
        // unrealised gain over the stored snapshot dates.
        assert!(js.contains("/report_snapshots/series"));
        assert!(js.contains("seriesChart"));
        assert!(js.contains("createElementNS"));
        assert!(js.contains("polyline"));
        assert!(js.contains("market_value"));
        assert!(js.contains("unrealised_gain"));
        // The report-snapshot job is described in the Jobs view.
        assert!(js.contains("report-snapshot"));
        // The graph marks provisional points distinctly from stale ones.
        assert!(js.contains("p.provisional ? ' provisional' : ''"));
        // The chart styles ship in the bundle too.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".series-chart"));
        assert!(css.contains(".badge.stale"));
        assert!(css.contains(".badge.provisional"));
        assert!(css.contains("circle.provisional"));
    }

    #[tokio::test]
    async fn cgt_decision_support_ui_present() {
        let js = app_js_body().await;
        // The parcel-optimiser screen drives the optimiser endpoint…
        assert!(js.contains("'parcel-optimiser'"));
        assert!(js.contains("/portfolio/parcel-optimiser"));
        // …and the pre-sale what-if screen the dry-run endpoint, with the
        // strategy picker naming each optimiser strategy.
        assert!(js.contains("'net-capital-gain-what-if'"));
        assert!(js.contains("/portfolio/net-capital-gain/what-if"));
        for strategy in ["'fifo'", "'min_gain'", "'max_discount'", "'harvest_losses'"] {
            assert!(js.contains(strategy), "missing strategy {strategy}");
        }
        // Both render through the generic report runner's params form and
        // per-key result tables (no bespoke views).
        assert!(js.contains("report.params"));
        assert!(js.contains("report.tables"));
    }

    #[tokio::test]
    async fn row_history_ui_present() {
        let js = app_js_body().await;
        // The audit-trail screen is one generic params-report entry driving
        // the row-history endpoint (no bespoke view)…
        assert!(js.contains("'row-history'"));
        assert!(js.contains("/reports/row_history"));
        // …whose table picker names every audited table — checked against
        // the Rust const the endpoint validates with, so a table added to
        // the trigger set cannot be forgotten here (an extra/mistyped UI
        // option is caught by the endpoint's own 422).
        for table in crate::reports::row_history::AUDITED_TABLES {
            assert!(
                js.contains(&format!("'{table}'")),
                "table picker missing {table}"
            );
        }
    }

    #[tokio::test]
    async fn timestamps_render_local_with_utc_tooltip() {
        let js = app_js_body().await;
        // RFC 3339 UTC server timestamps (fetched_at, generated_at,
        // uploaded_at, job last-run) are detected by the shared cell renderer…
        assert!(js.contains("isTimestamp"));
        // …displayed in the user's timezone…
        assert!(js.contains("fmtLocalTimestamp"));
        // …with the UTC instant on the cell's hover tooltip (title attr).
        assert!(js.contains("utcTooltip"));
        assert!(js.contains("title: utcTooltip(v)"));
        // The snapshot-detail "Generated …" line gets the same treatment.
        assert!(js.contains("title: utcTooltip(snap.generated_at)"));
    }

    #[tokio::test]
    async fn currency_amounts_round_in_tables_rates_keep_precision() {
        let js = app_js_body().await;
        // Display-only rounding lives in the web layer's formatter, fed by a
        // per-column kind keyed by column name (COLUMN_KINDS) so every table —
        // entity lists, the bespoke Sells/Transfers lists, and the report
        // tables — inherits the rule with no bespoke code.
        assert!(js.contains("COLUMN_KINDS"));
        assert!(js.contains("function columnKinds("));
        assert!(js.contains("const kinds = columnKinds(cols)"));
        // Monetary amounts round to 2 dp (half away from zero) with thousands
        // grouping, via exact BigInt decimal-string arithmetic — never
        // parseFloat on money.
        assert!(js.contains("function roundDecimalStr("));
        assert!(js.contains("function groupThousands("));
        assert!(js.contains("roundDecimalStr(value, 2)"));
        // Representative columns classified: a money amount, a per-unit rate
        // (kept at entered precision), a derived per-unit figure (≥4 dp), and a
        // quantity (kept at entered precision).
        assert!(js.contains("'total_cost_base'"));
        assert!(js.contains("'average_price'"));
        assert!(js.contains("'avg_cost_base_per_unit'"));
        assert!(js.contains("'quantity'"));
        // Rates/quantities keep their precision; derived per-unit figures show
        // at least 4 dp.
        assert!(js.contains("function padMinDp("));
        assert!(js.contains("kind === 'rate4'"));
        // The shared cell renderer applies the kinds and, when money rounding
        // drops precision, keeps the full value on the hover tooltip.
        assert!(js.contains("numericDisplay(row[c], kinds[c])"));
        assert!(js.contains("nd.tip"));
    }

    #[tokio::test]
    async fn listing_management_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/listings"));
        // The preference-share flag (90-day franking holding period) is editable.
        assert!(js.contains("preference"));
        // The Crypto security type is selectable, with the exchange optional
        // (blank for Crypto) — and crypto-aware labels never print "null".
        assert!(js.contains("'Crypto'"));
        assert!(js.contains("l.exchange_mic || 'Crypto'"));
    }

    #[tokio::test]
    async fn trade_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/trades"));
    }

    #[tokio::test]
    async fn fk_columns_render_names_not_ids() {
        let js = app_js_body().await;
        // Foreign-key id columns resolve to the referenced row's natural name
        // by *column name* (FK_COLUMN_SOURCES → fkLabelMaps), the one path
        // shared by the generic entity lists and the report tables…
        assert!(js.contains("fkLabelMaps"));
        assert!(js.contains("TABLE_LABEL_SOURCES"));
        assert!(js.contains("FK_COLUMN_SOURCES"));
        assert!(js.contains("columnLabelMaps"));
        // …a listing shows as MIC:TICKER (Crypto listings have no MIC)…
        assert!(js.contains("(l.exchange_mic || 'Crypto') + ':' + l.ticker"));
        // …a holding account shows by its name…
        assert!(js.contains("return a.name;"));
        // …and a trade (an id alone is meaningless) shows side/quantity/
        // listing/date via the shared describeTrade.
        assert!(js.contains("function describeTrade("));
        assert!(js.contains(
            "t.trade_type + ' ' + t.quantity + ' ' + listingName(t.listing_id) + ' on ' + t.date"
        ));
        // The trade/amma id columns are mapped by name — so income's
        // reinvestment_trade_id, the parcel-allocation sale/purchase trade ids,
        // and the amit-adjustment statement/trade ids all render names with no
        // per-entity field config.
        assert!(js.contains("reinvestment_trade_id: 'trades'"));
        assert!(js.contains("sale_trade_id: 'trades', purchase_trade_id: 'trades'"));
        assert!(js.contains("amma_statement_id: 'amma'"));
        // The generic entity list and the report tables both go through it.
        assert!(js.contains("const labels = await columnLabelMaps(cols);"));
        assert!(js.contains("labels: await columnLabelMaps(cols)"));
        // The raw id stays reachable on the cell tooltip.
        assert!(js.contains("'id ' + cellText(v)"));
        // The post-record action pages (Reinvest / Exercise / Participate /
        // Exchange / Demerge) name the listings in their titles and
        // descriptions; the scrip/demerger targets use the resolver.
        assert!(js.contains("action.title(id, owner, listingName)"));
        assert!(js.contains("action.desc(owner, listingName)"));
        assert!(!js.contains("'Creates a DRP trade for listing '"));
        assert!(js.contains("listing(a.scrip_listing_id)"));
        assert!(js.contains("listing(a.demerger_listing_id)"));
        // The allocation-editor parcel options and the AMMA-statement options
        // name the listing too (via the shared resolver).
        assert!(js.contains("listingNamer"));
        assert!(js.contains("listing(t.listing_id)"));
        assert!(js.contains("listing(a.listing_id)"));
        // …and the old raw-id label wording is gone (the "listing N" string
        // survives only as the unknown-id fallback).
        assert!(!js.contains("(listing '"));
        assert!(!js.contains(": listing '"));
    }

    #[tokio::test]
    async fn toasts_and_attachments_name_what_was_created_not_just_an_id() {
        let js = app_js_body().await;
        // Toasts that used to report only a created row's id now name what was
        // created (ticker/quantity/date via describeTrade), with the id as
        // secondary detail — across the action pages and the income form's
        // chained DRP reinvest.
        assert!(js.contains("'Reinvested into ' + describeTrade(trade, listing)"));
        assert!(js.contains("'Exercised into ' + describeTrade(trade, listing)"));
        assert!(js.contains("'Sold into the buy-back: ' + describeTrade(t, listing)"));
        assert!(js.contains("'Saved and reinvested into ' + describeTrade(trade, listingName)"));
        // The buy-back's dividend income and the scrip/demerger closing sells
        // are named by their listing too, not a bare id.
        assert!(js.contains("dividend income for ' + listing(r.income.listing_id)"));
        assert!(js.contains("'Exchanged ' + listing(a.listing_id) + ' into '"));
        assert!(js.contains("'Demerged ' + listing(a.listing_id) + ' into '"));
        // The transfer toast names the listing and both accounts (the
        // transfer-out sell id is only secondary detail).
        assert!(
            js.contains("'Transferred ' + n + ' parcel(s) of ' + listingName(body.listing_id)")
        );
        // The bare "trade #N" toast/heading wording is gone.
        assert!(!js.contains("'Reinvested into trade #'"));
        assert!(!js.contains("'Exercised into trade #'"));
        // The attachments view names the owning activity, not "trade #5".
        assert!(js.contains("const ATTACH_OWNER ="));
        assert!(js.contains("describeTrade(o, listing)"));
        assert!(!js.contains("ATTACH_OWNER_LABEL"));
    }

    #[tokio::test]
    async fn gst_inclusive_brokerage_and_statement_total_ui_present() {
        let js = app_js_body().await;
        // The GST-included checkbox ships on both the trades (Buy/DRP) and
        // Sell forms, driven by the shared wiring helper that hides the GST
        // field and relabels brokerage when ticked…
        assert!(js.contains("brokerage_includes_gst"));
        assert!(js.contains("Brokerage includes GST"));
        assert!(js.contains("wireGstBrokerage"));
        assert!(js.contains("Brokerage (GST-inclusive)"));
        // …and since the API reads a flagged trade's `brokerage` back as the
        // one GST-inclusive amount (the lossless round-trip contract), the
        // form fills the field straight from the row — the old client-side
        // recombination of the split pair is gone (it would double-count the
        // GST on top of the server's re-presentation).
        assert!(!js.contains("addDecimalStrings(existing.brokerage"));
        // The statement-total cross-check field ships on both forms and shows
        // as a column in the trades and Sells lists.
        assert!(js.contains("statement_total"));
        assert!(js.contains("Statement total"));
        assert!(
            js.contains("'statement_total', 'fx_rate'"),
            "trades list column"
        );
        assert!(
            js.contains("'statement_total', 'holding_account_id'"),
            "sells list column"
        );
    }

    /// The deliberate spot-rate override (QC 18020) ships on both the trades
    /// (Buy/DRP) and Sell forms, shows as a trades-list column, and is
    /// display-classified as a rate (full precision, never cent-rounded).
    #[tokio::test]
    async fn spot_fx_rate_override_ui_present() {
        let js = app_js_body().await;
        // Two field declarations: the trades config entry and SELL_FIELDS.
        assert_eq!(js.matches("dec('spot_fx_rate'").count(), 2);
        assert!(js.contains("Spot FX rate override"));
        assert!(js.contains("wins over the monthly RBA rate"));
        // Trades list column, classified as a rate in COLUMN_KINDS.
        assert!(js.contains("'fx_rate', 'spot_fx_rate', 'holding_account_id'"));
        assert!(js.contains("'fx_rate', 'spot_fx_rate', 'amount_per_security'"));
    }

    #[tokio::test]
    async fn income_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/income"));
        // Reinvest-distribution action drives POST /income/:id/reinvest.
        assert!(js.contains("/reinvest"));
        // A reinvested row offers Undo reinvest instead, driving
        // DELETE /income/:id/reinvest through the generic `del` row action
        // (confirm + DELETE + list refresh).
        assert!(js.contains("Undo reinvest"));
        assert!(js.contains("del: '/income/' + row.id + '/reinvest'"));
        assert!(js.contains("await api('DELETE', a.del)"));
    }

    #[tokio::test]
    async fn income_simple_entry_ui_present() {
        let js = app_js_body().await;
        // The income form opens simple-first: a payment amount + franking
        // selector mapped onto the component body at submit, with the full
        // tax-component field set behind an advanced toggle.
        assert!(js.contains("wireIncomeEntry"));
        assert!(js.contains("simple_amount"));
        assert!(js.contains("simple_franking"));
        assert!(js.contains("Fully franked (30%)"));
        assert!(js.contains("Trust distribution"));
        assert!(js.contains("Show advanced fields"));
        assert!(js.contains("INCOME_ADVANCED_FIELDS"));
        // Fully franked auto-computes the credit at amount × 30/70 with exact
        // BigInt decimal arithmetic (money — no float drift).
        assert!(js.contains("frankingCreditFor"));
        assert!(js.contains("* 3n"));
        // The per-share cross-check pair ships with a live computed-product
        // hint, driving the server-side 422 validation.
        assert!(js.contains("amount_per_security"));
        assert!(js.contains("securities_held"));
        assert!(js.contains("mulToCents"));
        // The DRP tick chains the existing reinvest POST after the save and
        // keeps the income on a reinvest failure.
        assert!(js.contains("Reinvested under DRP"));
        assert!(js.contains("'/income/' + id + '/reinvest'"));
        assert!(js.contains("reinvestment_price"));
        assert!(js.contains("Retry from the row’s Reinvest action"));
        // A fractional broker plan enters the statement's exact units; blank
        // keeps the whole-share default (the field is omitted from the body).
        assert!(js.contains("drp_units"));
        assert!(js.contains("Units allotted (fractional plans)"));
        assert!(js.contains("body.units = drpUnitsInput.value.trim()"));
        // The generic form honours the wireForm hook's submit extensions.
        assert!(js.contains("transformBody"));
        assert!(js.contains("afterSave"));
    }

    #[tokio::test]
    async fn income_entitlement_date_ui_present() {
        let js = app_js_body().await;
        // Trust present-entitlement timing (docs/ato/trust-income-timing.md):
        // the field exists, selecting Trust reveals it in simple mode
        // prefilled with the pay date, and switching away clears it so the
        // server's trust-only 422 can't be tripped by a leftover value.
        assert!(js.contains("entitlement_date"));
        assert!(js.contains("Entitlement date"));
        assert!(js.contains("applyEntitlement"));
        assert!(js.contains("presently entitled"));
        assert!(
            js.contains("if (mode !== 'Trust') { body.entitlement_date = null; body.tax_deferred_amount = null; }")
        );
    }

    #[tokio::test]
    async fn income_tax_deferred_e4_ui_present() {
        let js = app_js_body().await;
        // The tax-deferred amount joins the advanced income fields (CGT event
        // E4, docs/ato/cgt-non-assessable-payments.md) and is cleared when
        // simple mode switches away from Trust (asserted above with the
        // entitlement date); the cross-check report screen drives
        // GET /reports/e4_cross_check.
        assert!(js.contains("tax_deferred_amount"));
        assert!(js.contains("Tax-deferred amount"));
        assert!(js.contains("/reports/e4_cross_check"));
        assert!(js.contains("Tax-Deferred E4 Cross-Check"));
    }

    #[tokio::test]
    async fn amit_cash_cross_check_ui_present() {
        let js = app_js_body().await;
        // The cross-check report screen drives GET /reports/amit_cash_cross_check
        // (AMIT cash rows are cash-only — the AMMA is the assessable record —
        // so a missing AMMA must be visible, not silent), and its AUD cash
        // column is classified money with an explicit (AUD) heading.
        assert!(js.contains("/reports/amit_cash_cross_check"));
        assert!(js.contains("AMIT Cash Cross-Check"));
        assert!(js.contains("cash_total_aud"));
        assert!(js.contains("Cash total (AUD)"));
    }

    #[tokio::test]
    async fn interest_income_ui_present() {
        let js = app_js_body().await;
        // The Interest Income CRUD screen drives the /interest_income API,
        // with the gross amount (incl. TFN withheld) and the withheld amount.
        assert!(js.contains("/interest_income"));
        assert!(js.contains("Interest Income"));
        assert!(js.contains("Gross amount"));
        // The foreign-source classification (REQUIREMENTS 2026-07-13): the
        // form carries the flag and the foreign tax withheld field.
        assert!(js.contains("foreign_source"));
        assert!(js.contains("Foreign tax paid"));
        // The tax-summary interest columns (Australian 10L and foreign 20E)
        // are classified as money so they format automatically when the
        // report response carries them.
        assert!(js.contains("interest_income"));
        assert!(js.contains("foreign_interest_income"));
    }

    #[tokio::test]
    async fn investment_expenses_ui_present() {
        let js = app_js_body().await;
        // The Investment Expenses CRUD screen drives the /investment_expenses API,
        // with the deductible-expense type selector and the deductible amount.
        assert!(js.contains("/investment_expenses"));
        assert!(js.contains("Investment Expenses"));
        assert!(js.contains("expense_type"));
        assert!(js.contains("LoanInterest"));
        assert!(js.contains("ManagementFee"));
        assert!(js.contains("Deductible amount"));
        // The new tax-summary deduction/net columns are classified as money so
        // they format automatically when the report response carries them.
        assert!(js.contains("gross_assessable_investment_income"));
        assert!(js.contains("deductions_total"));
        assert!(js.contains("net_assessable_investment_income"));
    }

    #[tokio::test]
    async fn amma_statement_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/amma_statements"));
    }

    #[tokio::test]
    async fn ess_statement_ui_present() {
        let js = app_js_body().await;
        // The ESS statement CRUD screen and its discount labels.
        assert!(js.contains("/ess_statements"));
        assert!(js.contains("ESS Statements"));
        assert!(js.contains("taxed_upfront_eligible"));
        assert!(js.contains("deferral_discount"));
        assert!(js.contains("market_value_per_share"));
        // The statement-AUD override fields (the employer's stated AUD figures,
        // reported verbatim by the tax summary when present).
        assert!(js.contains("aud_deferral_discount"));
        assert!(js.contains("aud_taxed_upfront_eligible"));
        assert!(js.contains("Statement AUD: deferral discount (F)"));
        // The statement-AUD deferral figure (label F) is also a list column,
        // money-formatted, with a header the humaniser can't produce.
        assert!(js.contains("'deferral_discount', 'aud_deferral_discount'"));
        assert!(js.contains("Statement AUD deferral (F)"));
        // The Vest action (creates the cost-base-reset Buy) is reachable from a
        // statement row and posts to the vest endpoint.
        assert!(js.contains("#/ess-vest/"));
        assert!(js.contains("/ess_statements/' + id + '/vest"));
        // …but only while unvested: a vested row (vest_trade_id set) offers no
        // Vest action, and the vest_trade_id column names the linked Buy.
        assert!(js.contains("row.vest_trade_id == null"));
        assert!(js.contains("vest_trade_id: 'trades'"));
        // The new tax-summary ESS columns are classified as money so they format.
        assert!(js.contains("ess_discount_assessable"));
    }

    #[tokio::test]
    async fn inheritance_ui_present() {
        let js = app_js_body().await;
        // The inheritance CRUD screen drives /inheritances.
        assert!(js.contains("/inheritances"));
        assert!(js.contains("Inheritances"));
        // The form shows only the chosen cost-base rule's fields: the
        // deceased's acquisition date belongs to the post-CGT rule alone, and
        // the per-rule labels rename the cost-base figure.
        assert!(js.contains("cost_base_rule"));
        assert!(js.contains("DeceasedCostBase"));
        assert!(js.contains("MarketValueAtDeath"));
        assert!(js.contains("deceased_acquisition_date"));
        assert!(js.contains("Deceased’s cost base at death"));
        assert!(js.contains("Market value at death"));
        // The LPR expenditure pair, classified as money so it formats.
        assert!(js.contains("lpr_expenditure"));
        assert!(js.contains("lpr_expenditure_date"));
        assert!(js.contains("'lpr_expenditure'"));
    }

    #[tokio::test]
    async fn parcel_allocation_ui_present() {
        let js = app_js_body().await;
        // Allocations are entered as part of a Sell (PUT /sells/:id).
        assert!(js.contains("/sells"));
        assert!(js.contains("allocations"));
        // The allocation rows are built by the shared allocationEditor helper,
        // driven by the Sell form, the Transfer form (twice: the parcels to
        // move and the optional crypto network-fee parcels), and the buy-back
        // Participate action: one definition plus four call sites.
        assert!(js.contains("function allocationEditor"));
        assert_eq!(js.matches("allocationEditor(").count(), 5);
    }

    #[tokio::test]
    async fn drp_enrolment_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/drp_enrolments"));
        // The period model: enrol/unenrol/re-enrol via dated periods.
        assert!(js.contains("enrolment_date"));
        assert!(js.contains("unenrolment_date"));
    }

    #[tokio::test]
    async fn holding_account_ui_present() {
        let js = app_js_body().await;
        // The Holding Accounts entity view drives the /holding_accounts CRUD
        // API, and trades/income/AMMA/DRP enrolments (and the Sell and
        // exercise/participate forms) select an account.
        assert!(js.contains("/holding_accounts"));
        assert!(js.contains("holding_account_id"));
        assert!(js.contains("holdingAccounts"));
    }

    #[tokio::test]
    async fn transfers_ui_present() {
        let js = app_js_body().await;
        // The Transfers view lists/creates/deletes holding-account transfers
        // via the /transfers API, with from/to accounts and per-parcel
        // quantities.
        assert!(js.contains("/transfers"));
        assert!(js.contains("from_account_id"));
        assert!(js.contains("to_account_id"));
        assert!(js.contains("viewTransferForm"));
        // The optional crypto network fee: a fee-parcel allocation editor plus
        // the per-unit market value driving the fee's CGT disposal.
        assert!(js.contains("fee_allocations"));
        assert!(js.contains("fee_market_price"));
    }

    #[tokio::test]
    async fn trade_origin_labelling_present() {
        let js = app_js_body().await;
        // The Trades and Sells lists carry a derived Origin column labelling
        // the operation that created each row from its provenance links
        // (tradeOrigin in util.js, wired via the trades config's deriveRow and
        // the Sells view), so a rollover Buy's cost-base-carrying brokerage
        // figure — e.g. a transfer-in of a whole ETH parcel — never reads as
        // a real fee.
        assert!(js.contains("function tradeOrigin"));
        assert!(js.contains("deriveRow: function (row) { row.origin = tradeOrigin(row); }"));
        assert!(js.contains("entity.deriveRow"));
        assert!(js.contains("t.origin = tradeOrigin(t)"));
        assert!(js.contains("brokerage = carried cost base, not a fee"));
    }

    #[tokio::test]
    async fn rights_sales_ui_present() {
        let js = app_js_body().await;
        // The Rights Sales view lists the disposals recorded by the Sell
        // rights action via GET /rights_sales. Rows are operation-created and
        // immutable, so the entity is delete-only: Delete stays as the undo
        // path (DELETE /rights_sales/:id frees the entitlement) but there is
        // no New or Edit form.
        assert!(js.contains("'/rights_sales'"));
        assert!(js.contains("deleteOnly: true"));
        assert!(js.contains("entity.deleteOnly"));
    }

    #[tokio::test]
    async fn portfolio_overview_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/overview"));
    }

    #[tokio::test]
    async fn open_parcels_report_ui_present() {
        let js = app_js_body().await;
        // The Open Parcels report view drives GET /portfolio/open-parcels and
        // renders through the shared filterable table like every report.
        assert!(js.contains("/portfolio/open-parcels"));
    }

    #[tokio::test]
    async fn settlement_coverage_report_ui_present() {
        let js = app_js_body().await;
        // The Settlement Holiday Coverage report view drives
        // GET /reports/settlement_holiday_coverage and badges its status field.
        assert!(js.contains("/reports/settlement_holiday_coverage"));
        assert!(js.contains("coverage_status"));
    }

    #[tokio::test]
    async fn wash_sales_report_ui_present() {
        let js = app_js_body().await;
        // The Wash Sales report view drives POST /reports/wash_sales with a
        // configurable window field (blank = the 30-day default).
        assert!(js.contains("'wash-sales'"));
        assert!(js.contains("/reports/wash_sales"));
        assert!(js.contains("window_days"));
    }

    #[tokio::test]
    async fn franking_at_risk_ui_present() {
        let js = app_js_body().await;
        // The Franking At-Risk view drives GET /reports/franking_at_risk and
        // badges its status field; the what-if view drives the
        // contemplated-sale endpoint with listing/date/units params.
        assert!(js.contains("'franking-at-risk'"));
        assert!(js.contains("/reports/franking_at_risk"));
        assert!(js.contains("'franking-what-if'"));
        assert!(js.contains("/reports/franking_at_risk/what-if"));
        // Surfaced in the Sell flow: the Sells list and the Sell form link to
        // the foresight reports.
        assert!(js.contains("function sellForesightLinks()"));
        assert!(js.contains("#/r/franking-what-if"));
        assert!(js.contains("#/r/wash-sales"));
    }

    #[tokio::test]
    async fn gains_report_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/unrealised-gains"));
        assert!(js.contains("/portfolio/realised-gains"));
        assert!(js.contains("/portfolio/net-capital-gain"));
    }

    #[tokio::test]
    async fn expandable_parcel_detail_ui_present() {
        let js = app_js_body().await;
        // filterableTable's generic expand-to-a-child-table option, and the
        // Expand all / Collapse all control every expandable table gets.
        assert!(js.contains("opts.expand"));
        assert!(js.contains("Expand all"));
        assert!(js.contains("Collapse all"));
        // The Realised Gains report expands each disposal to its `parcels`
        // breakdown; the Net Capital Gain report expands each year to its
        // `disposals`, each of which nests its own `parcels` in turn.
        assert!(js.contains("'parcels'"));
        assert!(js.contains("'disposals'"));
        // The parcel optimiser / pre-sale what-if fold their sibling
        // `allocations` table into the same inline expansion, matched back to
        // the parent row by `matchOn`.
        assert!(js.contains("matchOn"));
        assert!(js.contains("'strategy'"));
        // Both drilldown-bearing reports are still driven by their existing
        // API paths — the feature is additive, not a new endpoint.
        assert!(js.contains("/portfolio/realised-gains"));
        assert!(js.contains("/portfolio/net-capital-gain"));
        // The toggle/detail-row/expand-all styling ships in the bundle too.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".expand-toggle"));
        assert!(css.contains(".expand-all-bar"));
        assert!(css.contains("td.detail-cell"));
    }

    #[tokio::test]
    async fn tax_summary_ui_present() {
        let js = app_js_body().await;
        assert!(js.contains("/portfolio/tax-summary"));
    }

    #[tokio::test]
    async fn listing_activity_report_ui_present() {
        let js = app_js_body().await;
        // The Listing Activity report drives POST /portfolio/activity through
        // the generic params-form + titled-tables report machinery: the
        // chronological ledger, then the final holding summary.
        assert!(js.contains("/portfolio/activity"));
        assert!(js.contains("Listing Activity"));
        assert!(js.contains("'events'"));
        assert!(js.contains("'holdings'"));
        // The ledger's numeric columns are classified/labelled for display.
        assert!(js.contains("'amount_aud'"));
        assert!(js.contains("'units_after'"));
        assert!(js.contains("Amount (AUD)"));
    }

    #[tokio::test]
    async fn performance_report_ui_present() {
        let js = app_js_body().await;
        // The Performance report view drives POST /portfolio/performance with
        // the shared price + as-of-date form.
        assert!(js.contains("/portfolio/performance"));
    }

    #[tokio::test]
    async fn live_valuation_ui_present() {
        let js = app_js_body().await;
        // The price-dependent report views value live by default: the shared
        // POST-report runner sends `live: true`, runs on first load (no manual
        // price entry needed), and treats the price form as overrides.
        assert!(js.contains("live: true"));
        assert!(js.contains("function buildBody()"));
        assert!(js.contains("Manual Price Overrides"));
        assert!(js.contains("Leave blank to value from the live price source"));
        // The per-row as-of times roll up into a freshness "as at …" line, with
        // a count of holdings the live fetch could not value.
        assert!(js.contains("function asAtSummary("));
        assert!(js.contains("Live prices as at "));
        assert!(js.contains("price_as_of"));
        assert!(js.contains("price_unavailable"));
        assert!(js.contains("had no live price"));
        // …and a count of holdings valued at a provisional (fallback-month)
        // FX rate, so a flagged valuation is never silently presented.
        assert!(js.contains("fx_provisional"));
        assert!(js.contains("valued at a provisional FX rate"));
        // The "as at" line styling ships in the bundle.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".hint.as-at"));
    }

    #[tokio::test]
    async fn report_export_ui_present() {
        let js = app_js_body().await;
        // The tax-summary and net-capital-gain report views carry an Export CSV
        // action linking to the report's `<api>/export` download endpoint.
        assert!(js.contains("export: true"));
        assert!(js.contains("'/export'"));
        assert!(js.contains("Export CSV"));
    }

    #[tokio::test]
    async fn attachments_ui_present() {
        let js = app_js_body().await;
        // The attachments view lists/uploads/downloads via the /attachments API,
        // reached from the Trade/Income/AMMA/ESS-statement/Interest-income row
        // "Attachments" action.
        assert!(js.contains("viewAttachments"));
        assert!(js.contains("/attachments"));
        assert!(js.contains("attachOwner"));
        // Every attachable owner is wired: the entity config carries its owner
        // field and the view can name the owning row.
        for owner in [
            "'trade_id'",
            "'income_id'",
            "'amma_statement_id'",
            "'ess_statement_id'",
            "'interest_income_id'",
        ] {
            assert!(js.contains(&format!("attachOwner: {owner}")), "{owner}");
        }
        assert!(js.contains("ess_statement_id: { noun: 'ESS statement'"));
        assert!(js.contains("interest_income_id: { noun: 'interest income'"));
        // Plain-text records are attachable; the file picker offers .txt.
        assert!(js.contains(".pdf,.png,.jpg,.jpeg,.txt"));
        // The checksum is stored integrity metadata, not a user-facing column.
        assert!(!js.contains("'checksum'"));
    }

    #[tokio::test]
    async fn attachments_trade_view_lists_linked_source_documents() {
        let js = app_js_body().await;
        // A trade's attachments view asks the server to traverse the
        // provenance link (DRP funding distribution / buy-back income row /
        // ESS vest statement) so the source record's paperwork shows up…
        assert!(js.contains("include_linked=true"));
        // …only on trade views — the flag is gated on the owner field.
        assert!(js.contains("ownerField === 'trade_id'"));
        // Linked rows are labelled with their owning record and offer a link
        // to that record's own Attachments view instead of Delete (delete
        // stays with the owner).
        assert!(js.contains("linkedOwner"));
        assert!(js.contains("(linked)"));
        assert!(js.contains("Owner's attachments"));
    }

    #[tokio::test]
    async fn jobs_ui_present() {
        let js = app_js_body().await;
        // The maintenance view lists and triggers jobs via the /jobs endpoints.
        assert!(js.contains("/jobs"));
        // It also surfaces each job's last run (success/error) from the GET /jobs
        // fields, rendered through the shared filterable table.
        assert!(js.contains("last_finished_at"));
        assert!(js.contains("last_success"));
        assert!(js.contains("last_error"));
        // Each job row expands to its stored run history (GET /jobs `runs`),
        // so a flapping job's intermittent failures are diagnosable in the UI.
        assert!(js.contains("j.runs"));
        assert!(js.contains("r.success ? 'ok' : 'failed'"));
    }

    #[tokio::test]
    async fn health_banner_ui_present() {
        let js = app_js_body().await;
        // The cross-view banner is driven by the health/freshness endpoint…
        assert!(js.contains("refreshHealthBanner"));
        assert!(js.contains("/reports/health"));
        // …surfacing stale prices, stale FX, and failed jobs, linking to the
        // Jobs page…
        assert!(js.contains("prices_stale"));
        assert!(js.contains("fx_stale"));
        assert!(js.contains("failed_jobs"));
        assert!(js.contains("'#/jobs'"));
        // …and refreshes on every route render so it appears on the main views.
        assert!(js.contains("refreshHealthBanner(); // deliberately not awaited"));
        // The strip's host element ships in the page shell with its styles.
        let index = body_string(get("/").await).await;
        assert!(index.contains("id=\"health-banner\""));
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains("#health-banner"));
    }

    #[tokio::test]
    async fn cgt_settings_ui_present() {
        let js = app_js_body().await;
        // The CGT Settings view edits the opening carried-forward capital loss
        // consumed by the net-capital-gain report.
        assert!(js.contains("/cgt_settings"));
        assert!(js.contains("opening_capital_loss"));
    }

    #[tokio::test]
    async fn corporate_actions_ui_present() {
        let js = app_js_body().await;
        // The Corporate Actions view records return-of-capital payments (CGT
        // event G1), share splits/consolidations (TD 2000/10), non-assessable
        // bonus issues, rights issues, and off-market buy-backs against a
        // listing.
        assert!(js.contains("/corporate_actions"));
        assert!(js.contains("ReturnOfCapital"));
        assert!(js.contains("amount_per_unit"));
        assert!(js.contains("ShareSplit"));
        assert!(js.contains("split_new_units"));
        assert!(js.contains("split_old_units"));
        assert!(js.contains("BonusIssue"));
        assert!(js.contains("bonus_units"));
        assert!(js.contains("bonus_held_units"));
        assert!(js.contains("RightsIssue"));
        assert!(js.contains("rights_units"));
        assert!(js.contains("rights_held_units"));
        assert!(js.contains("exercise_price"));
        // A RightsIssue row's Exercise action drives the exercise endpoint
        // (a config-driven ACTIONS entry rendered by viewAction).
        assert!(js.contains("#/exercise/"));
        assert!(js.contains("/exercise"));
        assert!(js.contains("rights_cost"));
        // ...and its Sell rights action drives the sell-rights endpoint, with
        // the anchoring-parcel allocation editor posting `units` rows.
        assert!(js.contains("#/sell-rights/"));
        assert!(js.contains("/sell_rights"));
        assert!(js.contains("proceeds_per_right"));
        assert!(js.contains("qtyField: 'units'"));
        assert!(js.contains("BuyBack"));
        assert!(js.contains("buyback_price"));
        assert!(js.contains("buyback_dividend"));
        assert!(js.contains("buyback_franking_credit"));
        assert!(js.contains("buyback_market_value"));
        // A BuyBack row's Participate action drives the participate endpoint.
        assert!(js.contains("#/participate/"));
        assert!(js.contains("/participate"));
        assert!(js.contains("ScripForScrip"));
        assert!(js.contains("scrip_listing_id"));
        assert!(js.contains("scrip_new_units"));
        assert!(js.contains("scrip_old_units"));
        // The optional cash component (partial rollover, Example 27).
        assert!(js.contains("scrip_cash_per_unit"));
        assert!(js.contains("scrip_market_value"));
        assert!(js.contains("scrip_cash_currency"));
        // A ScripForScrip row's Exchange action drives the exchange endpoint.
        assert!(js.contains("#/scrip-exchange/"));
        assert!(js.contains("/exchange"));
        assert!(js.contains("Demerger"));
        assert!(js.contains("demerger_listing_id"));
        assert!(js.contains("demerger_new_units"));
        assert!(js.contains("demerger_held_units"));
        assert!(js.contains("demerger_cost_base_pct"));
        // A Demerger row's Demerge action drives the demerge endpoint.
        assert!(js.contains("#/demerge/"));
        assert!(js.contains("/demerge"));
        // Worthless / delisted shares (CGT events G3 and C2): the type, its
        // event discriminator, and the Recognise action driving the endpoint.
        assert!(js.contains("WorthlessShares"));
        assert!(js.contains("worthless_event"));
        assert!(js.contains("G3Declaration"));
        assert!(js.contains("C2Cancellation"));
        assert!(js.contains("#/recognise/"));
        assert!(js.contains("/recognise"));
    }

    #[tokio::test]
    async fn post_actions_are_config_driven() {
        let js = app_js_body().await;
        // The five post-record action forms (DRP reinvest, rights exercise,
        // buy-back participate, scrip exchange, demerge) are entries in the
        // ACTIONS config rendered by the one generic viewAction, mirroring how
        // ENTITIES drives viewEntityForm.
        assert!(js.contains("const ACTIONS"));
        assert!(js.contains("function viewAction"));
        for slug in [
            "'reinvest'",
            "'exercise'",
            "'participate'",
            "'scrip-exchange'",
            "'demerge'",
            "'recognise'",
            "'sell-rights'",
        ] {
            assert!(js.contains(slug), "missing action slug {slug}");
        }
        // Each action's POST endpoint ships in the bundle.
        for endpoint in [
            "/reinvest'",
            "/exercise'",
            "/participate'",
            "/exchange'",
            "/demerge'",
            "/recognise'",
            "/sell_rights'",
        ] {
            assert!(js.contains(endpoint), "missing action endpoint {endpoint}");
        }
        // The reinvest action form takes a fractional plan's stated units
        // (optional — blank keeps the whole-share default).
        assert!(js.contains("dec('units', 'Units allotted (fractional plans)'"));
    }

    #[tokio::test]
    async fn corporate_action_form_is_split_by_type() {
        let js = app_js_body().await;
        // The corporate-actions form shows only the chosen action_type's
        // fields: a per-type field-group map (plus a per-type description) on
        // the entity config, re-rendered on type change by the generic entity
        // form. Unchosen types' fields submit as null, as their blank inputs
        // used to.
        assert!(js.contains("typeField: 'action_type'"));
        assert!(js.contains("fieldGroups"));
        assert!(js.contains("typeDescs"));
        // The common date field's label is scoped per type too.
        assert!(js.contains("typeLabels"));
        assert!(js.contains("Payment date"));
        assert!(js.contains("Demerger date"));
        // Values typed into a group survive flipping the type away and back...
        assert!(js.contains("const draft"));
        // ...a stale async render can't clobber a newer selection...
        assert!(js.contains("renderSeq"));
        // ...and editing a row to a different type warns that the saved
        // type's fields clear on save.
        assert!(js.contains("clears the saved"));
        for group in [
            "ReturnOfCapital: [",
            "ShareSplit: [",
            "BonusIssue: [",
            "RightsIssue: [",
            "BuyBack: [",
            "ScripForScrip: [",
            "Demerger: [",
            "WorthlessShares: [",
        ] {
            assert!(js.contains(group), "missing field group {group}");
        }
    }

    #[tokio::test]
    async fn tables_are_filterable_and_sortable() {
        let js = app_js_body().await;
        // Every entity list and report table renders through the shared
        // filterableTable, which adds a per-column filter row (inputs AND
        // together) and click-to-sort column headers. Assert those controls
        // ship in the bundle.
        assert!(js.contains("filterableTable"));
        assert!(js.contains("table-filter"));
        assert!(js.contains("filter-row"));
        assert!(js.contains("sortable"));
    }

    #[tokio::test]
    async fn tables_are_paginated() {
        let js = app_js_body().await;
        // Pagination lives in the shared filterableTable, so every entity list
        // and report table inherits it: a 50-row default page with a prev/next
        // pager + "showing m–n of total" count, only one page in the DOM at once.
        assert!(js.contains("const PAGE_SIZE = 50"));
        assert!(js.contains("function updatePager("));
        assert!(js.contains("showing "));
        assert!(js.contains("‹ Prev"));
        assert!(js.contains("Next ›"));
        // Filtering/sorting build the whole result set (visibleRows); only the
        // current page's slice is put in the DOM.
        assert!(js.contains("vr.slice(start, start + PAGE_SIZE)"));
        assert!(js.contains("pageRows.forEach"));
        // A changed filter re-pages from the first page; the pager hides when
        // the filtered total fits one page.
        assert!(js.contains("page = 0; // a changed filter re-pages"));
        assert!(js.contains("if (total <= PAGE_SIZE) { pager.hidden = true"));
        // The pager styling ships in the bundle too.
        let css = body_string(get("/static/style.css").await).await;
        assert!(css.contains(".pager"));
        assert!(css.contains(".pager-info"));
    }

    #[tokio::test]
    async fn column_headings_are_human_friendly() {
        let js = app_js_body().await;
        // Every table column header and filter placeholder reads through the
        // shared columnLabel — config-driven overrides over a default humaniser,
        // keyed by column name like COLUMN_KINDS — so the chrome around the data
        // never shows a raw database/JSON field name.
        assert!(js.contains("function humanizeLabel("));
        assert!(js.contains("const COLUMN_LABELS"));
        assert!(js.contains("function columnLabel("));
        // The shared table renderer uses the friendly label for both the header
        // cell and the per-column filter placeholder; the raw column name still
        // drives sorting/filtering, so it must not leak into the header text.
        assert!(js.contains("[columnLabel(c), indicator]"));
        assert!(js.contains("'Filter ' + columnLabel(c)"));
        assert!(!js.contains(", [c, indicator]"));
        assert!(!js.contains("'Filter ' + c +"));
        // The default humaniser drops a trailing "_id" (the cell already shows
        // the referenced row's name) and keeps known acronyms in canonical
        // casing rather than title-casing them to "Aud"/"Fx"/"Drp".
        assert!(js.contains("/_id$/"));
        assert!(js.contains("const LABEL_ACRONYMS"));
        assert!(js.contains("aud: 'AUD'"));
        assert!(js.contains("fx: 'FX'"));
        assert!(js.contains("fito: 'FITO'"));
        // Explicit overrides for the headers the requirement calls out, plus a
        // unit qualifier on an always-AUD report aggregate.
        assert!(js.contains("exchange_mic: 'Exchange'"));
        assert!(js.contains("holding_account_id: 'Account'"));
        assert!(js.contains("'Market value (AUD)'"));
    }
}
