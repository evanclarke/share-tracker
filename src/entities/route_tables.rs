//! The classification tables the cross-entity tests (and `api_spec`'s and
//! `doc_checks`' pins) read: every `PUT` route by the outcome it reports, and
//! every list route by the query parameters it accepts. Test-only — the routes
//! themselves are each entity's own `router`.

/// The success outcome a `PUT` route reports.
///
/// The convention is `201 Created` with the created row on a fresh id and
/// `204 No Content` on a replace; the two exceptions are the only routes
/// that can answer one status, each with the reason it must.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PutOutcome {
    /// `201` with the created row on a create, `204` on a replace.
    CreateThenReplace,
    /// `PUT /transfers/{id}`: create-only, so always `201` with the
    /// executed group — a bare `204` would hide the created Sell/Buy ids.
    AlwaysCreatedGroup,
    /// `PUT /rba_fx_rates/{id}`: a correction of an existing row, so
    /// always `204`; it can never create one (that is the import route).
    NeverCreates,
}

/// Every `PUT` route `entities::router()` serves, classified by outcome.
///
/// A new `PUT` route fails `every_put_route_reports_create_then_replace`
/// until it is added here, so the create-vs-replace signal cannot be left
/// out by omission. `api_spec::tests::every_put_route_documents_its_outcome`
/// **reads this list** (rather than re-declaring its own copy) to check the
/// generated OpenAPI document records each route's statuses.
pub(crate) const PUT_ROUTES: &[(&str, PutOutcome)] = &[
    ("/exchanges/{mic}", PutOutcome::CreateThenReplace),
    (
        "/exchange_holidays/{mic}/{date}",
        PutOutcome::CreateThenReplace,
    ),
    ("/listings/{id}", PutOutcome::CreateThenReplace),
    ("/holding_accounts/{id}", PutOutcome::CreateThenReplace),
    ("/trades/{id}", PutOutcome::CreateThenReplace),
    ("/income/{id}", PutOutcome::CreateThenReplace),
    ("/interest_income/{id}", PutOutcome::CreateThenReplace),
    ("/investment_expenses/{id}", PutOutcome::CreateThenReplace),
    ("/amma_statements/{id}", PutOutcome::CreateThenReplace),
    ("/amit_adjustments/{id}", PutOutcome::CreateThenReplace),
    ("/drp_enrolments/{id}", PutOutcome::CreateThenReplace),
    ("/cgt_settings/{id}", PutOutcome::CreateThenReplace),
    (
        "/tax_year_settings/{tax_year}",
        PutOutcome::CreateThenReplace,
    ),
    ("/corporate_actions/{id}", PutOutcome::CreateThenReplace),
    ("/ess_statements/{id}", PutOutcome::CreateThenReplace),
    ("/inheritances/{id}", PutOutcome::CreateThenReplace),
    ("/sells/{id}", PutOutcome::CreateThenReplace),
    (
        "/closing_prices/{listing_id}/{price_date}",
        PutOutcome::CreateThenReplace,
    ),
    ("/transfers/{id}", PutOutcome::AlwaysCreatedGroup),
    ("/rba_fx_rates/{id}", PutOutcome::NeverCreates),
];

/// One entity list route.
pub(crate) struct ListRoute {
    pub(crate) path: &'static str,
    /// The query parameters it accepts; `&[]` for none, which then
    /// requires a `reason`.
    pub(crate) filters: &'static [&'static str],
    /// Why an unfiltered list takes none — empty for a filtered one.
    pub(crate) reason: &'static str,
}

/// Every list route `entities::router` serves, classified. The filtered
/// ones carry the parameters this item added (or the pre-existing ones the
/// hand-written lists already took); the unfiltered ones say why.
///
/// `every_list_route_is_classified_for_filtering` reads every list the
/// generated document carries for `entities::router` and fails on one missing
/// here, or whose documented query parameters are not these `filters`, so a
/// new entity list cannot ship without the filters its columns offer (or a
/// stated reason it takes none), and
/// `every_list_route_refuses_an_unknown_parameter` drives each classified
/// route so the refusal is real rather than documented.
pub(crate) const LIST_ROUTES: &[ListRoute] = &[
    // Filtered: the workhorse entity lists.
    ListRoute {
        path: "/listings",
        filters: &["exchange_mic", "security_type"],
        reason: "",
    },
    ListRoute {
        path: "/trades",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/income",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/interest_income",
        filters: &["holding_account_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/investment_expenses",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/amma_statements",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/ess_statements",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/inheritances",
        filters: &["listing_id", "holding_account_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/corporate_actions",
        filters: &["listing_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/transfers",
        filters: &["listing_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/distribution_events",
        filters: &["listing_id", "from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/drp_enrolments",
        filters: &["listing_id", "holding_account_id"],
        reason: "",
    },
    ListRoute {
        path: "/amit_adjustments",
        filters: &["amma_statement_id", "trade_id"],
        reason: "",
    },
    ListRoute {
        path: "/parcel_allocations",
        filters: &["sale_trade_id", "purchase_trade_id"],
        reason: "",
    },
    // Unfiltered entity lists: no filter this item's columns offer, or a
    // table only ever read whole.
    ListRoute {
        path: "/exchanges",
        filters: &[],
        reason: "Reference table: a handful of rows, read whole by every picker.",
    },
    ListRoute {
        path: "/currencies",
        filters: &[],
        reason: "Import-fed ISO reference table, read whole (a code-keyed GET-one exists).",
    },
    ListRoute {
        path: "/mic_registry",
        filters: &[],
        reason: "Import-fed ISO reference table, read whole (a MIC-keyed GET-one exists).",
    },
    ListRoute {
        path: "/rba_fx_rates",
        filters: &[],
        reason: "Import-fed reference table, read whole; its rows are corrected one at a time by id.",
    },
    ListRoute {
        path: "/holding_accounts",
        filters: &[],
        reason: "A handful of rows, read whole.",
    },
    ListRoute {
        path: "/cgt_settings",
        filters: &[],
        reason: "Singleton settings row.",
    },
    ListRoute {
        path: "/tax_year_settings",
        filters: &[],
        reason: "One row per financial year, read whole; the keyed GET-one is the narrowed read.",
    },
    // Hand-written lists outside the workhorse entity lists.
    ListRoute {
        path: "/closing_prices",
        filters: &["listing_id", "from", "to", "status"],
        reason: "",
    },
    ListRoute {
        path: "/attachments",
        filters: &[
            "trade_id",
            "income_id",
            "amma_statement_id",
            "ess_statement_id",
            "interest_income_id",
            "corporate_action_id",
            "include_linked",
        ],
        reason: "",
    },
    ListRoute {
        path: "/rights_sales",
        filters: &[
            "listing_id",
            "holding_account_id",
            "rights_action_id",
            "from",
            "to",
        ],
        reason: "",
    },
    ListRoute {
        path: "/exchange_holidays",
        filters: &["from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/exchange_holidays/{mic}",
        filters: &["from", "to"],
        reason: "",
    },
    ListRoute {
        path: "/listings/{id}/renames",
        filters: &[],
        reason: "Path-narrowed to one listing's rename chain, a handful of rows read whole.",
    },
];

/// The filtered entity list routes as `(path, query parameters)`, read by
/// `api_spec` and `doc_checks` so the per-route filter documentation is derived
/// from the one classification table rather than transcribed beside it.
pub(crate) fn filtered_list_routes() -> Vec<(&'static str, &'static [&'static str])> {
    LIST_ROUTES
        .iter()
        .filter(|route| !route.filters.is_empty())
        .map(|route| (route.path, route.filters))
        .collect()
}
