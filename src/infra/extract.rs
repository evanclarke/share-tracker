//! The `Query` and `Json` extractors every handler takes, wrapping axum's own
//! so a request the decoder cannot read is refused in **one** shape whichever
//! route it reached.
//!
//! Before this module, axum answered its extractors' rejections itself, in
//! framework wording (`Failed to deserialize query string: …`, `Failed to
//! deserialize the JSON body into the target type: …`), while `POST
//! /jobs/{name}` hand-mapped its own query rejection to a `422` in different
//! words — so the same misspelt parameter was a `400` on one route and a `422`
//! on the next. Here the status is decided once and the body is one sentence
//! shape, `cannot read the <part>: <serde's own reason>`, the reason carrying
//! the failing field's path (`sufix: unknown field `sufix`, expected
//! `suffix``) so a toast with room for one line says what to fix:
//!
//! - **query string** — always `400`. The query is part of the URL, and a URL
//!   the route cannot read is a malformed request whatever was wrong with it:
//!   an unrecognised parameter, a missing required one, an unparseable value.
//! - **request body** — the status axum's `Json` assigns, which already
//!   distinguishes the cases a client acts on differently: `422` for JSON that
//!   parsed but does not fit the body type (an unknown field, a wrong type, a
//!   money figure sent as a JSON number), `400` for text that is not JSON,
//!   `415` for a body without `Content-Type: application/json`, `413` for one
//!   past the length limit.
//!
//! `infra::extract::tests::no_handler_takes_axums_own_query_or_json` keeps
//! every route on these: a module that imports axum's `Json` or `Query`
//! directly fails it.
use axum::extract::{FromRequest, FromRequestParts, OptionalFromRequest, Request};
use axum::http::{StatusCode, request::Parts};
use axum::response::{IntoResponse, Response};
use serde::{Serialize, de::DeserializeOwned};

/// A query string decoded into `T`, refused `400` naming why when it cannot be.
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

/// A JSON request body decoded into `T` (refused with the status table in the
/// module docs), and the JSON response wrapper — the same type both ways, as
/// axum's is, so a handler names one `Json`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

/// The one body shape: `cannot read the <part>: <reason>`. The reason is the
/// rejection's *source* — serde's message, path included — rather than its
/// `Display`, which prefixes axum's own wording; a rejection with no source
/// (the missing content type) has only its body text to give.
fn rejected(status: StatusCode, part: &str, rejection: &dyn std::error::Error) -> Response {
    let reason = rejection
        .source()
        .map(|source| source.to_string())
        .unwrap_or_else(|| rejection.to_string());
    (status, format!("cannot read the {part}: {reason}")).into_response()
}

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|axum::extract::Query(value)| Query(value))
            .map_err(|e| rejected(StatusCode::BAD_REQUEST, "query string", &e))
    }
}

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        <axum::Json<T> as FromRequest<S>>::from_request(req, state)
            .await
            .map(|axum::Json(value)| Json(value))
            .map_err(|e| rejected(e.status(), "request body", &e))
    }
}

/// `Option<Json<T>>`: no body (no JSON content type) is `None`, exactly as
/// axum's; a body that is there but unreadable is refused like [`Json`]'s.
impl<T, S> OptionalFromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Option<Self>, Self::Rejection> {
        <axum::Json<T> as OptionalFromRequest<S>>::from_request(req, state)
            .await
            .map(|body| body.map(|axum::Json(value)| Json(value)))
            .map_err(|e| rejected(e.status(), "request body", &e))
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, code_only, test_pool};

    /// Every module under `src` reaches `Json`/`Query` through this module,
    /// never axum's — one import of axum's and that route answers its
    /// rejections in framework wording and its own status again.
    #[test]
    fn no_handler_takes_axums_own_query_or_json() {
        fn walk(dir: &std::path::Path, offenders: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, offenders);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") || path.ends_with("infra/extract.rs")
                {
                    continue;
                }
                let code = code_only(&std::fs::read_to_string(&path).unwrap());
                // Every `use axum…;` tree, flattened onto one line.
                let mut rest = code.as_str();
                while let Some(start) = rest.find("use axum") {
                    let tree = &rest[start..];
                    let end = tree.find(';').unwrap_or(tree.len());
                    let tree = &tree[..end];
                    let names_one = tree
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .any(|word| word == "Json" || word == "Query");
                    if names_one {
                        offenders.push(format!("{}: {tree}", path.display()));
                    }
                    rest = &rest[start + end..];
                }
                for qualified in ["axum::Json", "axum::extract::Query"] {
                    if code.contains(qualified) {
                        offenders.push(format!("{}: {qualified}", path.display()));
                    }
                }
            }
        }
        let mut offenders = Vec::new();
        walk(std::path::Path::new("src"), &mut offenders);
        assert!(
            offenders.is_empty(),
            "use crate::infra::extract::{{Json, Query}} instead of axum's: {offenders:#?}"
        );
    }

    /// The one shape, on the real routes: a query rejection is `400` wherever
    /// it lands — a generic list, a report read, and the job trigger that used
    /// to answer `422` — and a body rejection keeps its status but loses
    /// axum's wording.
    #[tokio::test]
    async fn every_rejection_reads_cannot_read_the_part() {
        let pool = test_pool().await;
        let client = ApiClient::full(&pool);

        for path in [
            "/closing_prices?listng_id=1",
            "/portfolio/activity?listingid=1",
            "/jobs/backup?sufix=x",
        ] {
            let resp = if path.starts_with("/jobs") {
                client.post_empty(path).await
            } else {
                client.get(path).await
            };
            assert_eq!(
                resp.status,
                StatusCode::BAD_REQUEST,
                "{path}: {}",
                resp.text()
            );
            let body = resp.text();
            assert!(
                body.starts_with("cannot read the query string: ")
                    && body.contains("unknown field"),
                "{path}: {body}"
            );
        }

        // An unknown body field: 422, the field named, axum's prefix gone.
        let resp = client
            .post_bytes("/listings", Some("application/json"), r#"{"tikcer":"X"}"#)
            .await;
        let body = resp.text();
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body.starts_with("cannot read the request body: ") && body.contains("tikcer"),
            "{body}"
        );

        // Not JSON at all: 400, same shape.
        let resp = client
            .post_bytes("/listings", Some("application/json"), "{")
            .await;
        let body = resp.text();
        assert_eq!(resp.status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.starts_with("cannot read the request body: "), "{body}");

        // No JSON content type: 415, same shape.
        let resp = client.post_bytes("/listings", None, "{}").await;
        let body = resp.text();
        assert_eq!(resp.status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{body}");
        assert!(body.starts_with("cannot read the request body: "), "{body}");
        assert!(body.contains("Content-Type: application/json"), "{body}");

        // An optional body that is present but unreadable is refused the same
        // way rather than read as absent.
        let resp = client
            .post_bytes(
                "/report_snapshots/generate",
                Some("application/json"),
                r#"{"nope":1}"#,
            )
            .await;
        let body = resp.text();
        assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body.starts_with("cannot read the request body: "), "{body}");
    }
}
