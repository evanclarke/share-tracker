//! Every list route: classified for filtering, in the documented ordering
//! table, refusing an unknown parameter, and binding a filter value.

use super::*;

/// The value each filter is driven with — the shape the parameter takes,
/// not a row that has to exist (every route answers an empty `200` for a
/// filter that matches nothing).
///
/// `route` is passed so a parameter whose valid values are route-specific
/// can be classified per route and **fail closed**: `status` is driven at
/// `/closing_prices` only, and a future list with its own `status` value set
/// panics here until its sample is added rather than being handed `ok` and
/// silently left untested.
fn sample_filter_value(route: &str, param: &str) -> &'static str {
    match param {
        "from" | "to" => "2024-01-02",
        "exchange_mic" => "XASX",
        "security_type" => "Share",
        "include_linked" => "false",
        "status" => {
            assert_eq!(
                route, "/closing_prices",
                "a `status` filter on {route} needs its own sample value classified here"
            );
            "ok"
        }
        _ => "1",
    }
}

/// Every list `entities::router` serves — a `GET` answering a JSON array —
/// with the query parameters its `Query<T>` type declares, read off the
/// operations utoipa generated from the handlers themselves.
fn documented_lists() -> BTreeMap<String, Vec<String>> {
    let doc = serde_json::to_value(router().into_openapi()).expect("serialises");
    let mut lists = BTreeMap::new();
    for (path, item) in doc["paths"].as_object().expect("paths") {
        let get = &item["get"];
        if get["responses"]["200"]["content"]["application/json"]["schema"]["type"] != "array" {
            continue;
        }
        let mut params: Vec<String> = get["parameters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["in"] == "query")
            .filter_map(|p| p["name"].as_str().map(str::to_owned))
            .collect();
        params.sort();
        lists.insert(path.clone(), params);
    }
    lists
}

/// Every entity list route is classified in [`LIST_ROUTES`] with exactly
/// the query parameters the generated document says it takes — so a new
/// entity list cannot quietly ship without the filters its columns offer,
/// or without a stated reason it takes none.
#[test]
fn every_list_route_is_classified_for_filtering() {
    let documented = documented_lists();
    assert!(
        documented.len() >= 20,
        "the document carries only {} entity lists",
        documented.len()
    );
    let classified: BTreeMap<String, Vec<String>> = LIST_ROUTES
        .iter()
        .map(|r| {
            let mut filters: Vec<String> = r.filters.iter().map(|f| f.to_string()).collect();
            filters.sort();
            (r.path.to_owned(), filters)
        })
        .collect();
    assert_eq!(
        classified, documented,
        "every entity list must be classified in LIST_ROUTES with the query parameters its \
             Query type declares (and vice versa)"
    );

    let mut seen: Vec<&str> = LIST_ROUTES.iter().map(|r| r.path).collect();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(before, seen.len(), "LIST_ROUTES names a path twice");

    for route in LIST_ROUTES {
        if route.filters.is_empty() {
            assert!(
                !route.reason.is_empty(),
                "{} takes no filter and must say why in its reason",
                route.path
            );
        } else {
            assert!(
                route.reason.is_empty(),
                "{} is filtered; drop its reason",
                route.path
            );
        }
    }
}

/// Every entity list route the classification table knows is named in
/// **exactly one** row of `docs/API.md`'s ordering table.
///
/// The ordering pin in `doc_checks` asserts each row's endpoints were typed
/// in as they are; nothing there reads `LIST_ROUTES`, so a new entity list
/// shipped without any ordering row failed nothing — which is how
/// `/drp_enrolments` came to be filed under a key it does not have. This
/// closes the completeness half from the table's own source.
#[test]
fn every_classified_list_route_is_in_the_documented_ordering_table() {
    let md = include_str!("../../../docs/API.md");
    let section = md
        .split("## Reading a list")
        .nth(1)
        .expect("docs/API.md has a Reading a list section")
        .split("\n## ")
        .next()
        .expect("split always yields at least one part");
    let rows: Vec<&str> = section
        .lines()
        .filter(|line| line.starts_with("| ascending") || line.starts_with("| **descending**"))
        .collect();

    for route in LIST_ROUTES {
        // A path-narrowed variant (`/exchange_holidays/{mic}`) is documented
        // with its parent collection and has no row of its own.
        if route.path.contains('{') {
            continue;
        }
        let needle = format!("`{}`", route.path);
        let count = rows.iter().filter(|row| row.contains(&needle)).count();
        assert_eq!(
            count, 1,
            "`{}` must appear in exactly one ordering-table row (found {count}): {rows:?}",
            route.path
        );
    }
    // The non-entity list surfaces the table also names, so a row dropped
    // for one of those fails here too.
    for path in ["/report_snapshots", "/jobs", "/reports/row_history"] {
        let needle = format!("`{path}`");
        let count = rows.iter().filter(|row| row.contains(&needle)).count();
        assert_eq!(
            count, 1,
            "`{path}` must appear in exactly one ordering-table row (found {count})"
        );
    }
}

/// Every classified list route that decodes a query string refuses an
/// unrecognised parameter with a `400` naming it — the shared generic
/// lists (filtered and `NoFilter` alike), the trades list, and every
/// hand-written list, the path-narrowed ones included. An unfiltered `NoFilter` route is the
/// point of the last case: the empty braced struct does decode the empty
/// query string a bare `GET` sends, while any parameter at all is the
/// unknown-field rejection.
#[tokio::test]
async fn every_list_route_refuses_an_unknown_parameter() {
    let pool = test_pool().await;
    let client = ApiClient::full(&pool);
    let mut checked = 0;
    for route in LIST_ROUTES {
        // A path-narrowed list is driven at a sample key: the seeded
        // `XASX` calendar, and listing 1 (absent — an empty chain is
        // still a `200`).
        let path = route.path.replace("{mic}", "XASX").replace("{id}", "1");
        let path = path.as_str();
        let resp = client.get(path).await;
        assert_eq!(
            resp.status,
            StatusCode::OK,
            "GET {path} must list without a query string"
        );
        for filter in route.filters {
            let query = format!(
                "{}?{}={}",
                path,
                filter,
                sample_filter_value(route.path, filter)
            );
            let resp = client.get(&query).await;
            assert_eq!(resp.status, StatusCode::OK, "GET {query}");
        }
        let query = format!("{path}?zzz_unrecognised=1");
        let resp = client.get(&query).await;
        let (status, body) = resp.status_and_body();
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "GET {query} must be refused, not ignored: {body}"
        );
        assert!(
            body.contains("zzz_unrecognised"),
            "GET {query} must name the parameter: {body}"
        );
        checked += 1;
    }
    assert_eq!(
        checked, 29,
        "every query-decoding list route must be driven here"
    );
}

/// The behavioural half of the bind-not-interpolate rule
/// (`infra::http::FilterClauses` is the structural half — a filter is
/// handed a type with no raw push, so it *cannot* reach the SQL text).
///
/// `exchange_mic` is the only free-text filter in the tree, so it is the
/// one that can carry SQL: a value closing the quote and opening an `OR`
/// would list every row if it were interpolated, and a `;`-separated
/// statement would run. Bound, each is just a string no `exchange_mic`
/// equals — the *empty* list is the assertion, not a 200.
#[tokio::test]
async fn a_filter_value_is_bound_not_interpolated() {
    // Percent-encode everything but the alphanumerics, so the payload
    // reaches the handler as written rather than as a query-string split.
    fn encoded(value: &str) -> String {
        value
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }

    use crate::test_support::listing;

    let pool = test_pool().await;
    listing(1).mic("XASX").insert(&pool).await;
    listing(2).mic("XNYS").currency("USD").insert(&pool).await;
    let client = ApiClient::full(&pool);

    for payload in [
        "' OR 1=1 --",
        "XASX' OR '1'='1",
        "XASX'; DROP TABLE listings; --",
        "XASX",
    ] {
        let query = format!("/listings?exchange_mic={}", encoded(payload));
        let rows: Vec<serde_json::Value> = client.get_json(&query).await;
        let expected = if payload == "XASX" { 1 } else { 0 };
        assert_eq!(
            rows.len(),
            expected,
            "GET {query} must match the literal value, not interpret it"
        );
    }

    // …and the table the `DROP` named is still there, with both rows.
    let all: Vec<serde_json::Value> = client.get_json("/listings").await;
    assert_eq!(all.len(), 2, "the unfiltered list must be untouched");
}
