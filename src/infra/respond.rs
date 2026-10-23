//! The success types a handler returns, each carrying its status — and, for a
//! non-JSON payload, its media type — in the type itself.
//!
//! Every handler is annotated `#[utoipa::path]`, and utoipa's
//! `auto_into_responses` reads the operation's success responses off the
//! handler's **return type** through [`utoipa::IntoResponses`]: so a handler
//! cannot answer `201` while the document says `200`, because the one type is
//! both what axum turns into the response and what the document is generated
//! from. That is why no handler returns a bare `StatusCode`, a
//! `(StatusCode, Json<T>)` tuple or a hand-built `Response` — none of those can
//! say which status they answer — and why `ApiError` documents nothing here:
//! the non-success responses depend on the route (a body, a path parameter, an
//! upstream feed), and `api_spec` adds them per operation.
//!
//! A JSON body is documented as a `$ref` to its component schema through
//! [`JsonBody`], implemented for every [`Component`] (a type `api_spec`
//! registers in the document — the `components!` list there is what implements
//! it, so a response type missing from the document's schemas does not
//! compile) and for a `Vec` of one.

use crate::infra::db::WriteMode;
use crate::infra::extract::Json;
use crate::infra::http::{ApiError, Upsert, UpsertResponse};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use utoipa::IntoResponses;
use utoipa::openapi::schema::{AdditionalProperties, ArrayBuilder, ObjectBuilder, Type};
use utoipa::openapi::{
    ContentBuilder, Ref, RefOr, ResponseBuilder, ResponsesBuilder, Schema,
    response::Response as DocResponse,
};

/// A type the document registers as a named component schema. Implemented by
/// `api_spec`'s `components!` list and nowhere else, so every type a handler
/// answers is one the document's `components.schemas` carries.
pub trait Component: utoipa::ToSchema {}

/// How a JSON payload is described: a `$ref` to its component, or an array of
/// them.
pub trait JsonBody {
    fn schema() -> RefOr<Schema>;
}

impl<T: Component> JsonBody for T {
    fn schema() -> RefOr<Schema> {
        RefOr::Ref(Ref::from_schema_name(T::name()))
    }
}

impl<T: Component> JsonBody for Vec<T> {
    fn schema() -> RefOr<Schema> {
        RefOr::T(Schema::Array(
            ArrayBuilder::new().items(<T as JsonBody>::schema()).build(),
        ))
    }
}

/// `GET /reports/tax_report/years`: a bare array of income years.
impl JsonBody for Vec<i32> {
    fn schema() -> RefOr<Schema> {
        RefOr::T(Schema::Array(
            ArrayBuilder::new()
                .items(ObjectBuilder::new().schema_type(Type::Integer))
                .build(),
        ))
    }
}

/// A JSON value no single struct models: an object allowing any property.
impl JsonBody for serde_json::Value {
    fn schema() -> RefOr<Schema> {
        RefOr::T(Schema::Object(
            ObjectBuilder::new()
                .schema_type(Type::Object)
                .additional_properties(Some(AdditionalProperties::FreeForm(true)))
                .build(),
        ))
    }
}

/// One success response: `description`, with an `application/json` body of
/// `T` when there is one.
fn json_response<T: JsonBody>(description: &str) -> DocResponse {
    ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            ContentBuilder::new().schema(Some(T::schema())).build(),
        )
        .build()
}

/// One bodiless success response.
fn empty_response(description: &str) -> DocResponse {
    ResponseBuilder::new().description(description).build()
}

fn responses(
    entries: impl IntoIterator<Item = (u16, DocResponse)>,
) -> BTreeMap<String, RefOr<DocResponse>> {
    let mut builder = ResponsesBuilder::new();
    for (status, response) in entries {
        builder = builder.response(status.to_string(), response);
    }
    builder.build().into()
}

/// `200 OK` with the JSON body.
impl<T: JsonBody> IntoResponses for Json<T> {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        responses([(200, json_response::<T>("OK"))])
    }
}

/// The non-success responses are the route's, not the error type's: `api_spec`
/// adds them per operation (a body can be refused `422`, a path parameter can
/// name a missing row `404`, …).
impl IntoResponses for ApiError {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        BTreeMap::new()
    }
}

/// A `PUT` upsert's two outcomes: `201 Created` with the created row, or
/// `204 No Content` when it replaced one.
impl<T: JsonBody> IntoResponses for UpsertResponse<T> {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        responses([
            (201, json_response::<T>("Created")),
            (204, empty_response("No Content")),
        ])
    }
}

/// `201 Created` with the created resource as its JSON body.
pub struct Created<T>(pub T);

impl<T: serde::Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, Json(self.0)).into_response()
    }
}

impl<T: JsonBody> IntoResponses for Created<T> {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        responses([(201, json_response::<T>("Created"))])
    }
}

/// A previewable computed write's answer: `201 Created` with what it created,
/// or — for a `?dry_run=true` preview, which created nothing — `200 OK` with
/// the same body (`infra::http::DryRunQuery`).
pub struct Written<T> {
    /// `201` when the call created what `body` describes, `200` when it did
    /// not — fixed by the constructor, from a [`WriteMode`] or an [`Upsert`].
    status: StatusCode,
    body: T,
}

impl<T> Written<T> {
    /// The answer of a write whose transaction ended in `mode`: `201` for a
    /// committed write, `200` for a [`WriteMode::DryRun`] preview.
    pub fn new(mode: WriteMode, body: T) -> Self {
        let status = match mode {
            WriteMode::Commit => StatusCode::CREATED,
            WriteMode::DryRun => StatusCode::OK,
        };
        Self { status, body }
    }

    /// The answer of a `POST` that stores on a natural key and so either
    /// creates the row (`201`) or replaces the one stored (`200` — the row
    /// keeps its id, so it is the same resource, not a created one).
    pub fn upserted(outcome: Upsert, body: T) -> Self {
        let status = match outcome {
            Upsert::Created => StatusCode::CREATED,
            Upsert::Replaced => StatusCode::OK,
        };
        Self { status, body }
    }
}

impl<T: serde::Serialize> IntoResponse for Written<T> {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl<T: JsonBody> IntoResponses for Written<T> {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        responses([
            (200, json_response::<T>("OK")),
            (201, json_response::<T>("Created")),
        ])
    }
}

/// `204 No Content`.
pub struct NoContent;

impl IntoResponse for NoContent {
    fn into_response(self) -> Response {
        StatusCode::NO_CONTENT.into_response()
    }
}

impl IntoResponses for NoContent {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        responses([(204, empty_response("No Content"))])
    }
}

/// `303 See Other`: the `Location` is the whole answer (the login and logout
/// redirects). The response is built by the handler; the type says what it is.
pub struct SeeOther(pub Response);

impl IntoResponse for SeeOther {
    fn into_response(self) -> Response {
        self.0
    }
}

impl IntoResponses for SeeOther {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        responses([(303, empty_response("See Other"))])
    }
}

/// The media type of a non-JSON [`Payload`].
pub trait MediaType {
    /// As the document records it (no parameters).
    const MEDIA_TYPE: &'static str;
}

/// A `200 OK` whose body is not JSON — a page, a stylesheet, a CSV export, a
/// stored file. The handler builds the response (its own headers included);
/// the type parameter says what it is, recorded as a media type with no
/// schema, since the bytes are not JSON-Schema-shaped.
pub struct Payload<M: MediaType>(Response, PhantomData<M>);

impl<M: MediaType> Payload<M> {
    /// A payload whose response the handler built itself.
    pub fn from_response(response: Response) -> Self {
        Self(response, PhantomData)
    }

    /// A payload of `body`, served with `content_type` (the media type plus
    /// any parameters, e.g. `text/csv; charset=utf-8`).
    pub fn new(content_type: &'static str, body: impl IntoResponse) -> Self {
        Self::from_response(([(header::CONTENT_TYPE, content_type)], body).into_response())
    }
}

impl<M: MediaType> IntoResponse for Payload<M> {
    fn into_response(self) -> Response {
        self.0
    }
}

impl<M: MediaType> IntoResponses for Payload<M> {
    fn responses() -> BTreeMap<String, RefOr<DocResponse>> {
        responses([(
            200,
            ResponseBuilder::new()
                .description("OK")
                .content(M::MEDIA_TYPE, ContentBuilder::new().build())
                .build(),
        )])
    }
}

macro_rules! media_types {
    ($($(#[$doc:meta])* $name:ident = $media:literal;)*) => {
        $(
            $(#[$doc])*
            pub enum $name {}
            impl MediaType for $name {
                const MEDIA_TYPE: &'static str = $media;
            }
        )*
    };
}

media_types! {
    /// An HTML page (the web frontend's shell, the sign-in page).
    Html = "text/html";
    /// A stylesheet.
    Css = "text/css";
    /// A CSV export.
    Csv = "text/csv";
    /// Plain text (the route index).
    PlainText = "text/plain";
    /// A stored file served back as it was uploaded.
    OctetStream = "application/octet-stream";
    /// A frontend ES module.
    JavaScript = "text/javascript";
}
