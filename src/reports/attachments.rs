use crate::infra::http::ApiError;
use axum::{Json, Router, extract::State, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// One stored document, joined out to the activity it is attached to (see
/// `entities::attachment` — ownership is six nullable FKs with a DB CHECK
/// that exactly one is set). The per-owner attachments view
/// (`#/attachments/<owner_field>/<owner_id>`) is the only other place these
/// rows are visible; this report is the whole-portfolio document register —
/// what has been filed, against which activity, for which listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentIndexRow {
    pub id: i64,
    pub filename: String,
    /// The stored MIME type (`application/pdf`, `image/png`, `image/jpeg`,
    /// `text/plain`).
    pub content_type: String,
    pub byte_size: i64,
    /// Server-set upload timestamp (RFC 3339), the report's default sort key.
    pub uploaded_at: String,
    /// Human label for which activity this is attached to: `Trade`,
    /// `Income`, `AMMA statement`, `ESS statement`, `Interest income`, or
    /// `Corporate action`.
    pub owner_type: String,
    /// The owner id column name (`trade_id`, `income_id`, …) — the query key
    /// for `GET /attachments?<owner_field>=<owner_id>` and the UI's Record
    /// link back to the per-owner view.
    pub owner_field: String,
    pub owner_id: i64,
    /// A human summary of the owning row, e.g. "Buy on 2024-05-01" or "FY
    /// ending 2024-06-30" — enough to place the document without following
    /// the link.
    pub owner_description: String,
    /// The owning activity's listing, where it has one. Null only for an
    /// interest-income attachment (interest income has no listing).
    pub listing_id: Option<i64>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/attachments", get(report))
}

/// Every stored attachment joined out to its owning activity (all six owner
/// tables are nullable FKs on `attachments`, so a single LEFT JOIN reaches
/// whichever one is set) and, where the owner has one, its listing. Newest
/// upload first.
pub async fn db_attachments(pool: &SqlitePool) -> Result<Vec<AttachmentIndexRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT a.id, a.filename, a.content_type, a.byte_size, a.uploaded_at, \
                a.trade_id, a.income_id, a.amma_statement_id, a.ess_statement_id, \
                a.interest_income_id, a.corporate_action_id, \
                t.trade_type, t.date AS trade_date, t.listing_id AS trade_listing_id, \
                i.date_paid AS income_date_paid, i.listing_id AS income_listing_id, \
                m.tax_year_end_date, m.listing_id AS amma_listing_id, \
                e.taxing_point_date, e.listing_id AS ess_listing_id, \
                ii.date_paid AS interest_date_paid, ii.source AS interest_source, \
                ca.action_type, ca.date AS action_date, ca.listing_id AS action_listing_id \
         FROM attachments a \
         LEFT JOIN trades            t  ON t.id  = a.trade_id \
         LEFT JOIN income            i  ON i.id  = a.income_id \
         LEFT JOIN amma_statements   m  ON m.id  = a.amma_statement_id \
         LEFT JOIN ess_statements    e  ON e.id  = a.ess_statement_id \
         LEFT JOIN interest_income   ii ON ii.id = a.interest_income_id \
         LEFT JOIN corporate_actions ca ON ca.id = a.corporate_action_id \
         ORDER BY a.uploaded_at DESC, a.id DESC",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        // Exactly one owner FK is non-null (the DB CHECK on `attachments`
        // guarantees it), so the first match wins.
        let (owner_type, owner_field, owner_id, owner_description, listing_id): (
            &str,
            &str,
            i64,
            String,
            Option<i64>,
        ) = if let Some(trade_id) = row.try_get::<Option<i64>, _>("trade_id")? {
            let trade_type: String = row.try_get("trade_type")?;
            let date: String = row.try_get("trade_date")?;
            (
                "Trade",
                "trade_id",
                trade_id,
                format!("{trade_type} on {date}"),
                row.try_get("trade_listing_id")?,
            )
        } else if let Some(income_id) = row.try_get::<Option<i64>, _>("income_id")? {
            let date: String = row.try_get("income_date_paid")?;
            (
                "Income",
                "income_id",
                income_id,
                format!("Distribution paid {date}"),
                row.try_get("income_listing_id")?,
            )
        } else if let Some(amma_id) = row.try_get::<Option<i64>, _>("amma_statement_id")? {
            let fy_end: String = row.try_get("tax_year_end_date")?;
            (
                "AMMA statement",
                "amma_statement_id",
                amma_id,
                format!("FY ending {fy_end}"),
                row.try_get("amma_listing_id")?,
            )
        } else if let Some(ess_id) = row.try_get::<Option<i64>, _>("ess_statement_id")? {
            let taxing_point: String = row.try_get("taxing_point_date")?;
            (
                "ESS statement",
                "ess_statement_id",
                ess_id,
                format!("Taxing point {taxing_point}"),
                row.try_get("ess_listing_id")?,
            )
        } else if let Some(interest_id) = row.try_get::<Option<i64>, _>("interest_income_id")? {
            let date: String = row.try_get("interest_date_paid")?;
            let source: Option<String> = row.try_get("interest_source")?;
            let desc = match source {
                Some(s) => format!("{s} on {date}"),
                None => format!("Interest on {date}"),
            };
            (
                "Interest income",
                "interest_income_id",
                interest_id,
                desc,
                None,
            )
        } else if let Some(action_id) = row.try_get::<Option<i64>, _>("corporate_action_id")? {
            let action_type: String = row.try_get("action_type")?;
            let date: String = row.try_get("action_date")?;
            (
                "Corporate action",
                "corporate_action_id",
                action_id,
                format!("{action_type} on {date}"),
                row.try_get("action_listing_id")?,
            )
        } else {
            // The `attachments` CHECK constraint makes this unreachable in
            // practice; fail loudly rather than silently drop the row.
            return Err(sqlx::Error::RowNotFound);
        };

        out.push(AttachmentIndexRow {
            id: row.try_get("id")?,
            filename: row.try_get("filename")?,
            content_type: row.try_get("content_type")?,
            byte_size: row.try_get("byte_size")?,
            uploaded_at: row.try_get("uploaded_at")?,
            owner_type: owner_type.to_string(),
            owner_field: owner_field.to_string(),
            owner_id,
            owner_description,
            listing_id,
        });
    }
    Ok(out)
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<Vec<AttachmentIndexRow>>, ApiError> {
    db_attachments(&pool)
        .await
        .map(Json)
        .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{attachment, corporate_action, interest_income};
    use crate::test_support::{self, test_pool, ymd};
    use axum::http::StatusCode;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use rust_decimal::Decimal;
    use tower::ServiceExt;

    async fn attach(
        pool: &SqlitePool,
        filename: &str,
        uploaded_at: &str,
        owner: attachment::NewAttachment<'_>,
    ) -> i64 {
        attachment::db_insert(
            pool,
            &attachment::NewAttachment {
                filename: filename.to_string(),
                uploaded_at: uploaded_at.to_string(),
                ..owner
            },
        )
        .await
        .unwrap()
    }

    fn base_owner(content: &'static [u8]) -> attachment::NewAttachment<'static> {
        attachment::NewAttachment {
            trade_id: None,
            income_id: None,
            amma_statement_id: None,
            ess_statement_id: None,
            interest_income_id: None,
            corporate_action_id: None,
            filename: String::new(),
            content_type: attachment::ContentType::Pdf,
            checksum: "deadbeef".to_string(),
            uploaded_at: String::new(),
            content,
        }
    }

    /// A trade-owned attachment reports the trade type/date and the trade's
    /// listing.
    #[tokio::test]
    async fn db_trade_owned_attachment() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 5, 1))
            .insert(&pool)
            .await;
        attach(
            &pool,
            "conf.pdf",
            "2024-05-02T00:00:00Z",
            attachment::NewAttachment {
                trade_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;

        let rows = db_attachments(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.owner_type, "Trade");
        assert_eq!(r.owner_field, "trade_id");
        assert_eq!(r.owner_id, 1);
        assert_eq!(r.owner_description, "Buy on 2024-05-01");
        assert_eq!(r.listing_id, Some(1));
        assert_eq!(r.filename, "conf.pdf");
    }

    /// An income-owned attachment names the distribution and its listing.
    #[tokio::test]
    async fn db_income_owned_attachment() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::income(1, 1, ymd(2024, 8, 30))
            .insert(&pool)
            .await;
        attach(
            &pool,
            "dist.pdf",
            "2024-09-01T00:00:00Z",
            attachment::NewAttachment {
                income_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;

        let rows = db_attachments(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_type, "Income");
        assert_eq!(rows[0].owner_field, "income_id");
        assert_eq!(rows[0].owner_description, "Distribution paid 2024-08-30");
        assert_eq!(rows[0].listing_id, Some(1));
    }

    /// An AMMA-statement-owned attachment names the FY it covers.
    #[tokio::test]
    async fn db_amma_owned_attachment() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::amma(1, 1).insert(&pool).await;
        attach(
            &pool,
            "amma.pdf",
            "2024-09-01T00:00:00Z",
            attachment::NewAttachment {
                amma_statement_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;

        let rows = db_attachments(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_type, "AMMA statement");
        assert_eq!(rows[0].owner_field, "amma_statement_id");
        assert_eq!(rows[0].listing_id, Some(1));
    }

    /// An ESS-statement-owned attachment names its taxing point.
    #[tokio::test]
    async fn db_ess_owned_attachment() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::ess_statement(1, 1, ymd(2024, 3, 15))
            .insert(&pool)
            .await;
        attach(
            &pool,
            "ess.pdf",
            "2024-09-01T00:00:00Z",
            attachment::NewAttachment {
                ess_statement_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;

        let rows = db_attachments(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_type, "ESS statement");
        assert_eq!(rows[0].owner_description, "Taxing point 2024-03-15");
        assert_eq!(rows[0].listing_id, Some(1));
    }

    /// An interest-income-owned attachment has no listing — interest income
    /// isn't tied to one.
    #[tokio::test]
    async fn db_interest_income_owned_attachment_has_no_listing() {
        let pool = test_pool().await;
        interest_income::db_upsert(
            &pool,
            &interest_income::InterestIncome {
                id: 1,
                date_paid: ymd(2024, 5, 1),
                amount: Decimal::from(100),
                tfn_withholding_tax: Decimal::ZERO,
                foreign_source: false,
                foreign_tax_paid: Decimal::ZERO,
                currency: "AUD".to_string(),
                source: Some("ANZ savings".to_string()),
                holding_account_id: None,
            },
        )
        .await
        .unwrap();
        attach(
            &pool,
            "int.pdf",
            "2024-06-01T00:00:00Z",
            attachment::NewAttachment {
                interest_income_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;

        let rows = db_attachments(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_type, "Interest income");
        assert_eq!(rows[0].owner_description, "ANZ savings on 2024-05-01");
        assert_eq!(rows[0].listing_id, None);
    }

    /// A corporate-action-owned attachment names the action type and date.
    #[tokio::test]
    async fn db_corporate_action_owned_attachment() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 6, 30),
                kind: corporate_action::ActionKind::ReturnOfCapital {
                    amount_per_unit: "0.10".parse().unwrap(),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
        attach(
            &pool,
            "booklet.pdf",
            "2024-07-01T00:00:00Z",
            attachment::NewAttachment {
                corporate_action_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;

        let rows = db_attachments(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_type, "Corporate action");
        assert_eq!(rows[0].owner_description, "ReturnOfCapital on 2024-06-30");
        assert_eq!(rows[0].listing_id, Some(1));
    }

    /// Newest upload first.
    #[tokio::test]
    async fn db_orders_newest_upload_first() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1).insert(&pool).await;
        attach(
            &pool,
            "first.pdf",
            "2024-01-01T00:00:00Z",
            attachment::NewAttachment {
                trade_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;
        attach(
            &pool,
            "second.pdf",
            "2024-06-01T00:00:00Z",
            attachment::NewAttachment {
                trade_id: Some(1),
                ..base_owner(b"y")
            },
        )
        .await;

        let rows = db_attachments(&pool).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].filename, "second.pdf");
        assert_eq!(rows[1].filename, "first.pdf");
    }

    /// An empty database reports an empty list.
    #[tokio::test]
    async fn db_empty_when_no_attachments() {
        let pool = test_pool().await;
        assert!(db_attachments(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_get_attachments_report() {
        let pool = test_pool().await;
        test_support::listing(1).insert(&pool).await;
        test_support::buy(1, 1)
            .date(ymd(2024, 5, 1))
            .insert(&pool)
            .await;
        attach(
            &pool,
            "conf.pdf",
            "2024-05-02T00:00:00Z",
            attachment::NewAttachment {
                trade_id: Some(1),
                ..base_owner(b"x")
            },
        )
        .await;

        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/reports/attachments")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let rows: Vec<AttachmentIndexRow> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_type, "Trade");
        assert_eq!(rows[0].filename, "conf.pdf");
    }
}
