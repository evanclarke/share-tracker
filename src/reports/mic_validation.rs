use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// Validation status of a curated exchange's MIC against the ISO 10383 registry.
/// Non-blocking: writes to `exchanges` are never rejected — this report only
/// surfaces MICs worth a second look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeMicStatus {
    pub mic: String,
    pub exchange_name: String,
    /// `ok` (active in the registry), `expired` (present but EXPIRED), or
    /// `unknown` (no registry entry — a typo, or the registry isn't imported yet).
    pub registry_status: String,
    /// The raw ISO STATUS (`ACTIVE` | `UPDATED` | `EXPIRED`), or `None` when unknown.
    pub iso_status: Option<String>,
    pub expiry_date: Option<String>,
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/reports/exchange_mic_validation", get(report))
}

/// Classify each curated exchange's MIC against the registry. An exchange with no
/// matching registry row is `unknown`; an `EXPIRED` registry entry is `expired`;
/// anything else present (`ACTIVE`/`UPDATED`) is `ok`.
pub async fn db_validate(pool: &SqlitePool) -> Result<Vec<ExchangeMicStatus>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT e.mic AS mic, e.name AS exchange_name, \
                r.status AS iso_status, r.expiry_date AS expiry_date \
         FROM exchanges e \
         LEFT JOIN mic_registry r ON e.mic = r.mic \
         ORDER BY e.mic",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let iso_status: Option<String> = row.try_get("iso_status")?;
        let registry_status = match iso_status.as_deref() {
            None => "unknown",
            Some("EXPIRED") => "expired",
            Some(_) => "ok",
        }
        .to_string();
        out.push(ExchangeMicStatus {
            mic: row.try_get("mic")?,
            exchange_name: row.try_get("exchange_name")?,
            registry_status,
            iso_status,
            expiry_date: row.try_get("expiry_date")?,
        });
    }
    Ok(out)
}

async fn report(State(pool): State<SqlitePool>) -> Result<Json<Vec<ExchangeMicStatus>>, StatusCode> {
    db_validate(&pool).await.map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{exchange, mic_registry};
    use crate::infra::db;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn test_pool() -> SqlitePool {
        db::init(":memory:").await.unwrap()
    }

    fn mic(mic: &str, status: &str, expiry: Option<&str>) -> mic_registry::MicEntry {
        mic_registry::MicEntry {
            mic: mic.to_string(),
            operating_mic: mic.to_string(),
            name: format!("{mic} Exchange"),
            country_code: "AU".to_string(),
            city: None,
            status: status.to_string(),
            expiry_date: expiry.map(str::to_string),
        }
    }

    /// With an empty registry, every curated exchange is `unknown` (nothing to
    /// validate against yet).
    #[tokio::test]
    async fn unknown_when_registry_empty() {
        let pool = test_pool().await;
        let report = db_validate(&pool).await.unwrap();
        // Seed data has XASX and XNYS.
        assert!(report.iter().all(|s| s.registry_status == "unknown"));
        assert!(report.iter().any(|s| s.mic == "XASX"));
    }

    #[tokio::test]
    async fn classifies_ok_expired_and_unknown() {
        let pool = test_pool().await;
        // XASX active, XNYS expired in the registry. Both are seed exchanges.
        mic_registry::db_upsert(&pool, &mic("XASX", "ACTIVE", None)).await.unwrap();
        mic_registry::db_upsert(&pool, &mic("XNYS", "EXPIRED", Some("2024-06-30")))
            .await
            .unwrap();
        // A curated exchange whose MIC isn't in the registry at all.
        exchange::db_upsert(
            &pool,
            &exchange::Exchange {
                mic: "XBOG".to_string(),
                name: "Bogus".to_string(),
                country: "Nowhere".to_string(),
                currency: "AUD".to_string(),
                timezone: "UTC".to_string(),
                settlement_days: 2,
            },
        )
        .await
        .unwrap();

        let report = db_validate(&pool).await.unwrap();
        let by_mic = |m: &str| report.iter().find(|s| s.mic == m).unwrap().clone();

        let asx = by_mic("XASX");
        assert_eq!(asx.registry_status, "ok");
        assert_eq!(asx.iso_status, Some("ACTIVE".to_string()));

        let nyse = by_mic("XNYS");
        assert_eq!(nyse.registry_status, "expired");
        assert_eq!(nyse.expiry_date, Some("2024-06-30".to_string()));

        let bogus = by_mic("XBOG");
        assert_eq!(bogus.registry_status, "unknown");
        assert_eq!(bogus.iso_status, None);
    }

    /// The report validates curated *exchanges*; an exchange-less (Crypto)
    /// listing has no MIC to validate, so recording one adds no row.
    #[tokio::test]
    async fn crypto_listings_are_not_validated() {
        let pool = test_pool().await;
        let before = db_validate(&pool).await.unwrap().len();
        crate::entities::listing::db_upsert(
            &pool,
            &crate::entities::listing::Listing {
                id: 1,
                exchange_mic: None,
                ticker: "BTC".to_string(),
                name: "Bitcoin".to_string(),
                isin: None,
                security_type: crate::entities::listing::SecurityType::Crypto,
                currency: "AUD".to_string(),
                amit: false,
                preference: false,
            },
        )
        .await
        .unwrap();
        let report = db_validate(&pool).await.unwrap();
        assert_eq!(report.len(), before, "a Crypto listing adds nothing to validate");
    }

    #[tokio::test]
    async fn api_returns_ok() {
        let pool = test_pool().await;
        mic_registry::db_upsert(&pool, &mic("XASX", "ACTIVE", None)).await.unwrap();
        let resp = router()
            .with_state(pool)
            .oneshot(
                Request::builder()
                    .uri("/reports/exchange_mic_validation")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let report: Vec<ExchangeMicStatus> = serde_json::from_slice(&bytes).unwrap();
        assert!(report.iter().any(|s| s.mic == "XASX" && s.registry_status == "ok"));
    }
}
