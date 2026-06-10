//! Document attachments for an activity (Trade, Income, or AMMA Statement).
//!
//! A user attaches a supporting file — a trade confirmation / contract note PDF,
//! a dividend statement, an AMMA statement scan — to exactly one activity. The
//! bytes are stored in the database as a BLOB (the `content` column) so the
//! existing weekly DB backup captures the documents too; there is no separate
//! file store to back up.
//!
//! Ownership is three nullable FK columns (`trade_id` / `income_id` /
//! `amma_statement_id`) with a DB CHECK that exactly one is set, rather than a
//! polymorphic `type` + `id` pair — that way referential integrity to the owning
//! activity is enforced by a real foreign key, and `ON DELETE CASCADE` removes an
//! activity's attachments when it is deleted (no orphaned blobs).
//!
//! The payload is binary, so this module does not follow the JSON-CRUD
//! convention used by the other entities:
//! - `POST /attachments` takes `multipart/form-data` (the file plus the target
//!   owner field) and returns `201` with the stored metadata.
//! - `GET /attachments` and `GET /attachments/{id}` return metadata only (never
//!   the blob); the list filters by owner (`?trade_id=` etc.).
//! - `GET /attachments/{id}/content` streams the raw bytes with the stored
//!   content type and a download filename.
//! - `DELETE /attachments/{id}` removes one attachment.

use crate::infra::http::ApiError;
use std::fmt::Write as _;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

/// Per-file upload ceiling (25 MB). Enforced explicitly so an oversized upload is
/// rejected with `413`; the route's body limit is set a little higher so this
/// check (on the file's own bytes, not the whole multipart envelope) is the
/// authority.
pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// The accepted attachment content types. Stored as TEXT and constrained by a DB
/// CHECK to the same allowlist; an unsupported type is rejected with `422`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum ContentType {
    #[sqlx(rename = "application/pdf")]
    #[serde(rename = "application/pdf")]
    Pdf,
    #[sqlx(rename = "image/png")]
    #[serde(rename = "image/png")]
    Png,
    #[sqlx(rename = "image/jpeg")]
    #[serde(rename = "image/jpeg")]
    Jpeg,
}

impl ContentType {
    /// Parse a declared MIME type (allowlist only); unknown types yield `None`.
    pub fn from_mime(mime: &str) -> Option<ContentType> {
        // Strip any parameters (e.g. "image/jpeg; charset=binary") before matching.
        match mime.split(';').next().unwrap_or(mime).trim() {
            "application/pdf" => Some(ContentType::Pdf),
            "image/png" => Some(ContentType::Png),
            "image/jpeg" => Some(ContentType::Jpeg),
            _ => None,
        }
    }

    /// The canonical MIME string, used for the download `Content-Type` header.
    pub fn as_mime(self) -> &'static str {
        match self {
            ContentType::Pdf => "application/pdf",
            ContentType::Png => "image/png",
            ContentType::Jpeg => "image/jpeg",
        }
    }
}

/// Attachment metadata. The `content` blob is deliberately absent here — it is
/// loaded only by the download path — so list/get stay light.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Attachment {
    pub id: i64,
    pub trade_id: Option<i64>,
    pub income_id: Option<i64>,
    pub amma_statement_id: Option<i64>,
    pub filename: String,
    pub content_type: ContentType,
    pub byte_size: i64,
    pub checksum: String,
    pub uploaded_at: String,
}

/// The owner + payload for a new attachment (the row to insert).
pub struct NewAttachment<'a> {
    pub trade_id: Option<i64>,
    pub income_id: Option<i64>,
    pub amma_statement_id: Option<i64>,
    pub filename: String,
    pub content_type: ContentType,
    pub checksum: String,
    pub uploaded_at: String,
    pub content: &'a [u8],
}

/// Owner filter for the list endpoint (`?trade_id=`/`?income_id=`/`?amma_statement_id=`).
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub trade_id: Option<i64>,
    pub income_id: Option<i64>,
    pub amma_statement_id: Option<i64>,
}

const METADATA_COLS: &str = "id, trade_id, income_id, amma_statement_id, filename, content_type, byte_size, checksum, uploaded_at";

pub fn router() -> Router<SqlitePool> {
    Router::new()
        .route("/attachments", get(list).post(upload))
        .route("/attachments/{id}", get(get_one).delete(delete))
        .route("/attachments/{id}/content", get(download))
        // Raise the body limit above the per-file ceiling so the explicit
        // size check (below) is what rejects an oversized file with 413,
        // rather than axum's default 2 MB limit silently rejecting everything.
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 1024 * 1024))
}

/// SHA-256 of the content as a lowercase hex string.
pub fn checksum_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    let mut out = String::with_capacity(64);
    for byte in digest {
        write!(out, "{byte:02x}").expect("writing to a String never fails");
    }
    out
}

pub async fn db_list(pool: &SqlitePool, q: &ListQuery) -> Result<Vec<Attachment>, sqlx::Error> {
    // Each present owner filter is ANDed in; the column names come from this
    // module's fixed set, never user input, so they are safe to push as raw SQL.
    let conds: Vec<(&str, i64)> = [
        ("trade_id", q.trade_id),
        ("income_id", q.income_id),
        ("amma_statement_id", q.amma_statement_id),
    ]
    .into_iter()
    .filter_map(|(col, val)| val.map(|v| (col, v)))
    .collect();

    let mut qb: QueryBuilder<Sqlite> =
        QueryBuilder::new(format!("SELECT {METADATA_COLS} FROM attachments"));
    for (i, (col, val)) in conds.iter().enumerate() {
        qb.push(if i == 0 { " WHERE " } else { " AND " });
        qb.push(*col).push(" = ").push_bind(*val);
    }
    qb.push(" ORDER BY id");
    qb.build_query_as::<Attachment>().fetch_all(pool).await
}

pub async fn db_get(pool: &SqlitePool, id: i64) -> Result<Option<Attachment>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {METADATA_COLS} FROM attachments WHERE id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Load the content for download: its content type, filename, and raw bytes.
pub async fn db_get_content(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<(ContentType, String, Vec<u8>)>, sqlx::Error> {
    sqlx::query_as("SELECT content_type, filename, content FROM attachments WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn db_insert(pool: &SqlitePool, att: &NewAttachment<'_>) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO attachments \
         (trade_id, income_id, amma_statement_id, filename, content_type, byte_size, checksum, uploaded_at, content) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(att.trade_id)
    .bind(att.income_id)
    .bind(att.amma_statement_id)
    .bind(&att.filename)
    .bind(att.content_type)
    .bind(att.content.len() as i64)
    .bind(&att.checksum)
    .bind(&att.uploaded_at)
    .bind(att.content)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn db_delete(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM attachments WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

async fn list(
    State(pool): State<SqlitePool>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Attachment>>, ApiError> {
    db_list(&pool, &q).await.map(Json).map_err(ApiError::from)
}

async fn get_one(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Attachment>, ApiError> {
    db_get(&pool, id)
        .await
        .map_err(ApiError::from)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

async fn upload(
    State(pool): State<SqlitePool>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let bad_request = |msg: &str| ApiError::bad_request(msg);
    let mut trade_id: Option<i64> = None;
    let mut income_id: Option<i64> = None;
    let mut amma_statement_id: Option<i64> = None;
    let mut file: Option<(String, ContentType, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| bad_request("the multipart upload is malformed"))?
    {
        // Take an owned name so the field can be consumed in the arm below.
        let name = field.name().map(str::to_string);
        match name.as_deref() {
            Some("trade_id") => trade_id = read_owner_field(field).await?,
            Some("income_id") => income_id = read_owner_field(field).await?,
            Some("amma_statement_id") => amma_statement_id = read_owner_field(field).await?,
            Some("file") => {
                let filename = field.file_name().unwrap_or("attachment").to_string();
                let declared = field.content_type().map(str::to_string);
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| bad_request("the uploaded file could not be read"))?;
                let content_type = declared
                    .as_deref()
                    .and_then(ContentType::from_mime)
                    .ok_or_else(|| {
                        ApiError::unprocessable(
                            "the file's content type is not a supported attachment type",
                        )
                    })?;
                file = Some((filename, content_type, bytes.to_vec()));
            }
            // Ignore unknown fields, but drain the body to keep the stream sane.
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    // Exactly one owner must be set.
    let owners =
        trade_id.is_some() as u8 + income_id.is_some() as u8 + amma_statement_id.is_some() as u8;
    if owners != 1 {
        return Err(ApiError::unprocessable(
            "attach to exactly one of a trade, income, or AMMA statement",
        ));
    }

    let (filename, content_type, content) =
        file.ok_or_else(|| ApiError::unprocessable("no file was included in the upload"))?;
    if content.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::PayloadTooLarge(format!(
            "the file exceeds the {MAX_UPLOAD_BYTES}-byte upload limit"
        )));
    }

    let new = NewAttachment {
        trade_id,
        income_id,
        amma_statement_id,
        filename,
        content_type,
        checksum: checksum_hex(&content),
        uploaded_at: chrono::Utc::now().to_rfc3339(),
        content: &content,
    };

    // A bad owner id violates the FK to the activity table → client error (422).
    let id = db_insert(&pool, &new).await.map_err(ApiError::from)?;
    let stored = db_get(&pool, id)
        .await?
        .ok_or_else(|| ApiError::internal("the stored attachment row vanished".to_string()))?;
    Ok((StatusCode::CREATED, Json(stored)).into_response())
}

async fn read_owner_field(
    field: axum::extract::multipart::Field<'_>,
) -> Result<Option<i64>, ApiError> {
    let text = field
        .text()
        .await
        .map_err(|_| ApiError::bad_request("an owner field could not be read"))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse::<i64>()
        .map(Some)
        .map_err(|_| ApiError::unprocessable("an owner id is not a valid integer"))
}

async fn download(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let (content_type, filename, bytes) = db_get_content(&pool, id)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;
    // Strip any quotes from the filename so it can't break out of the header.
    let safe_name = filename.replace(['"', '\\'], "");
    Response::builder()
        .header(header::CONTENT_TYPE, content_type.as_mime())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{safe_name}\""),
        )
        .body(Body::from(bytes))
        .map_err(ApiError::internal)
}

async fn delete(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    db_delete(&pool, id)
        .await
        .map(|found| {
            if found {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::NOT_FOUND
            }
        })
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const BOUNDARY: &str = "X-TEST-BOUNDARY";

    /// Seed a listing (id 1) so trades/income can reference it.
    async fn insert_listing(pool: &SqlitePool) {
        sqlx::query(
            "INSERT OR IGNORE INTO listings (id, exchange_mic, ticker, name, security_type, currency, amit) \
             VALUES (1, 'XASX', 'AAA', 'Test Co', 'Share', 'AUD', 0)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_trade(pool: &SqlitePool) -> i64 {
        insert_listing(pool).await;
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO trades (trade_type, date, settlement_date, listing_id, average_price, quantity, currency, brokerage_currency) \
             VALUES ('Buy', '2024-01-01', '2024-01-03', 1, '1', '1', 'AUD', 'AUD') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        id
    }

    async fn insert_income(pool: &SqlitePool) -> i64 {
        insert_listing(pool).await;
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO income (listing_id, date_paid) VALUES (1, '2024-02-01') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        id
    }

    fn push_field(body: &mut Vec<u8>, name: &str, value: &str) {
        body.extend_from_slice(
            format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }

    fn push_file(body: &mut Vec<u8>, filename: &str, content_type: &str, content: &[u8]) {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }

    fn finish(body: &mut Vec<u8>) {
        body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    }

    async fn post_multipart(pool: SqlitePool, body: Vec<u8>) -> Response {
        router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/attachments")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn db_insert_and_get_content_round_trips() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let content = b"%PDF-1.4 hello";
        let id = db_insert(
            &pool,
            &NewAttachment {
                trade_id: Some(trade_id),
                income_id: None,
                amma_statement_id: None,
                filename: "conf.pdf".to_string(),
                content_type: ContentType::Pdf,
                checksum: checksum_hex(content),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                content,
            },
        )
        .await
        .unwrap();

        let meta = db_get(&pool, id).await.unwrap().unwrap();
        assert_eq!(meta.trade_id, Some(trade_id));
        assert_eq!(meta.byte_size, content.len() as i64);
        assert_eq!(meta.content_type, ContentType::Pdf);

        let (ct, name, bytes) = db_get_content(&pool, id).await.unwrap().unwrap();
        assert_eq!(ct, ContentType::Pdf);
        assert_eq!(name, "conf.pdf");
        assert_eq!(bytes, content);
    }

    #[tokio::test]
    async fn api_upload_returns_201_with_metadata_and_stores_content() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let content = b"%PDF-1.4 trade confirmation";

        let mut body = Vec::new();
        push_field(&mut body, "trade_id", &trade_id.to_string());
        push_file(&mut body, "conf.pdf", "application/pdf", content);
        finish(&mut body);

        let resp = post_multipart(pool.clone(), body).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let meta: Attachment = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(meta.trade_id, Some(trade_id));
        assert_eq!(meta.filename, "conf.pdf");
        assert_eq!(meta.content_type, ContentType::Pdf);
        assert_eq!(meta.byte_size, content.len() as i64);
        // Checksum is the SHA-256 of the content, computed server-side.
        assert_eq!(meta.checksum, checksum_hex(content));

        // The content round-trips byte-for-byte with the stored content type.
        let dl = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri(format!("/attachments/{}/content", meta.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dl.status(), StatusCode::OK);
        assert_eq!(
            dl.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/pdf"
        );
        assert!(
            dl.headers()
                .get(header::CONTENT_DISPOSITION)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("conf.pdf")
        );
        let dl_bytes = dl.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&dl_bytes[..], content);
    }

    #[tokio::test]
    async fn api_upload_rejects_unsupported_content_type() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let mut body = Vec::new();
        push_field(&mut body, "trade_id", &trade_id.to_string());
        push_file(&mut body, "notes.txt", "text/plain", b"hello");
        finish(&mut body);

        let resp = post_multipart(pool, body).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_upload_rejects_missing_owner() {
        let pool = test_pool().await;
        let mut body = Vec::new();
        push_file(&mut body, "conf.pdf", "application/pdf", b"%PDF");
        finish(&mut body);

        let resp = post_multipart(pool, body).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_upload_rejects_two_owners() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let income_id = insert_income(&pool).await;
        let mut body = Vec::new();
        push_field(&mut body, "trade_id", &trade_id.to_string());
        push_field(&mut body, "income_id", &income_id.to_string());
        push_file(&mut body, "conf.pdf", "application/pdf", b"%PDF");
        finish(&mut body);

        let resp = post_multipart(pool, body).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_upload_unknown_owner_is_422() {
        let pool = test_pool().await;
        // No trade 999 exists → FK violation surfaces as 422, not 500.
        let mut body = Vec::new();
        push_field(&mut body, "trade_id", "999");
        push_file(&mut body, "conf.pdf", "application/pdf", b"%PDF");
        finish(&mut body);

        let resp = post_multipart(pool, body).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn api_upload_oversized_is_413() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let content = vec![0u8; MAX_UPLOAD_BYTES + 1];
        let mut body = Vec::new();
        push_field(&mut body, "trade_id", &trade_id.to_string());
        push_file(&mut body, "big.pdf", "application/pdf", &content);
        finish(&mut body);

        let resp = post_multipart(pool, body).await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn api_list_returns_metadata_only_and_filters_by_owner() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let income_id = insert_income(&pool).await;
        for (t, i, name) in [
            (Some(trade_id), None, "t.pdf"),
            (None, Some(income_id), "i.pdf"),
        ] {
            db_insert(
                &pool,
                &NewAttachment {
                    trade_id: t,
                    income_id: i,
                    amma_statement_id: None,
                    filename: name.to_string(),
                    content_type: ContentType::Pdf,
                    checksum: checksum_hex(b"x"),
                    uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                    content: b"x",
                },
            )
            .await
            .unwrap();
        }

        // Filter to the trade's attachments only.
        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/attachments?trade_id={trade_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        // The JSON carries metadata fields but never a "content" key.
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            !text.contains("\"content\""),
            "list must not include the blob"
        );
        let items: Vec<Attachment> = serde_json::from_str(&text).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].trade_id, Some(trade_id));
    }

    #[tokio::test]
    async fn api_delete_and_missing() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let id = db_insert(
            &pool,
            &NewAttachment {
                trade_id: Some(trade_id),
                income_id: None,
                amma_statement_id: None,
                filename: "x.pdf".to_string(),
                content_type: ContentType::Pdf,
                checksum: checksum_hex(b"x"),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                content: b"x",
            },
        )
        .await
        .unwrap();

        let resp = router()
            .with_state(pool.clone())
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/attachments/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Deleting again → 404.
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/attachments/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn deleting_owner_activity_cascades_to_attachments() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        db_insert(
            &pool,
            &NewAttachment {
                trade_id: Some(trade_id),
                income_id: None,
                amma_statement_id: None,
                filename: "x.pdf".to_string(),
                content_type: ContentType::Pdf,
                checksum: checksum_hex(b"x"),
                uploaded_at: "2024-01-01T00:00:00Z".to_string(),
                content: b"x",
            },
        )
        .await
        .unwrap();

        sqlx::query("DELETE FROM trades WHERE id = ?")
            .bind(trade_id)
            .execute(&pool)
            .await
            .unwrap();

        let remaining = db_list(
            &pool,
            &ListQuery {
                trade_id: Some(trade_id),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            remaining.is_empty(),
            "ON DELETE CASCADE should remove the attachment"
        );
    }

    #[tokio::test]
    async fn db_exactly_one_owner_check_enforced() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let income_id = insert_income(&pool).await;
        // Two owners set → CHECK violation.
        let err = sqlx::query(
            "INSERT INTO attachments (trade_id, income_id, filename, content_type, byte_size, checksum, uploaded_at, content) \
             VALUES (?, ?, 'x.pdf', 'application/pdf', 1, 'abc', '2024-01-01T00:00:00Z', X'00')",
        )
        .bind(trade_id)
        .bind(income_id)
        .execute(&pool)
        .await;
        assert!(
            err.is_err(),
            "exactly-one-owner CHECK should reject two owners"
        );

        // Zero owners set → CHECK violation.
        let err = sqlx::query(
            "INSERT INTO attachments (filename, content_type, byte_size, checksum, uploaded_at, content) \
             VALUES ('x.pdf', 'application/pdf', 1, 'abc', '2024-01-01T00:00:00Z', X'00')",
        )
        .execute(&pool)
        .await;
        assert!(
            err.is_err(),
            "exactly-one-owner CHECK should reject zero owners"
        );
    }

    #[tokio::test]
    async fn db_content_type_enum_constraint_enforced() {
        let pool = test_pool().await;
        let trade_id = insert_trade(&pool).await;
        let err = sqlx::query(
            "INSERT INTO attachments (trade_id, filename, content_type, byte_size, checksum, uploaded_at, content) \
             VALUES (?, 'x.txt', 'text/plain', 1, 'abc', '2024-01-01T00:00:00Z', X'00')",
        )
        .bind(trade_id)
        .execute(&pool)
        .await;
        assert!(
            err.is_err(),
            "content_type CHECK should reject a type outside the allowlist"
        );
    }
}
