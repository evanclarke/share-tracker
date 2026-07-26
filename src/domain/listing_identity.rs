//! As-at ticker resolution across a rename (`entities::listing_rename`,
//! `listing_renames` table): archived documents show the ticker as it stood
//! at each row's own date, so a printed document keeps reading the way the
//! broker statement did — see `docs/API.md`'s "Ticker or name changes".
//!
//! This is deliberately used by only two callers: the Annual Tax Report
//! (`reports::tax_report`, whose disposal/income rows are archived and
//! reconciled against source statements) and the listing activity ledger
//! (`reports::activity`, a chronological record of one listing's history).
//! Every other report shows the *current* ticker — a rename is the same
//! security, which is the ATO-correct view — with the full chain
//! discoverable via `GET /listings/:id/renames` and the Row History screen.

use chrono::NaiveDate;
use std::collections::HashMap;

struct Rename {
    effective_date: NaiveDate,
    old_ticker: String,
    new_ticker: String,
}

/// Every listing's rename chain, pre-loaded once per report — mirrors
/// `infra::fx::FxRates::load`, so per-row resolution costs no further DB
/// round-trip. Chains are stored ascending by `effective_date`.
pub struct RenameHistory {
    by_listing: HashMap<i64, Vec<Rename>>,
}

impl RenameHistory {
    pub async fn load(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let rows: Vec<(i64, NaiveDate, String, String)> = sqlx::query_as(
            "SELECT listing_id, effective_date, old_ticker, new_ticker \
             FROM listing_renames ORDER BY listing_id, effective_date ASC",
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut by_listing: HashMap<i64, Vec<Rename>> = HashMap::new();
        for (listing_id, effective_date, old_ticker, new_ticker) in rows {
            by_listing.entry(listing_id).or_default().push(Rename {
                effective_date,
                old_ticker,
                new_ticker,
            });
        }
        Ok(Self { by_listing })
    }

    /// The ticker in effect for `listing_id` on `date`: the latest rename
    /// whose `effective_date <= date` gives its `new_ticker`; before any
    /// recorded rename, the earliest one's `old_ticker`; with no rename at
    /// all, `current_ticker` (the caller's own `listings.ticker` read — this
    /// resolver holds only the rename chain, not a listings snapshot).
    pub fn ticker_as_at(&self, listing_id: i64, date: NaiveDate, current_ticker: &str) -> String {
        let Some(chain) = self.by_listing.get(&listing_id) else {
            return current_ticker.to_string();
        };
        match chain.iter().rev().find(|r| r.effective_date <= date) {
            Some(r) => r.new_ticker.clone(),
            None => chain[0].old_ticker.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{listing, test_pool, ymd};

    async fn insert_rename(
        pool: &sqlx::SqlitePool,
        listing_id: i64,
        effective_date: NaiveDate,
        old_ticker: &str,
        new_ticker: &str,
    ) {
        sqlx::query(
            "INSERT INTO listing_renames (listing_id, effective_date, old_ticker, new_ticker) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(listing_id)
        .bind(effective_date)
        .bind(old_ticker)
        .bind(new_ticker)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn no_rename_falls_back_to_the_current_ticker() {
        let pool = test_pool().await;
        listing(1).ticker("VAS").insert(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();
        assert_eq!(
            history.ticker_as_at(1, ymd(2024, 1, 1), "VAS"),
            "VAS".to_string()
        );
    }

    #[tokio::test]
    async fn resolves_before_at_and_after_a_single_rename() {
        let pool = test_pool().await;
        listing(1).ticker("LAR").insert(&pool).await;
        insert_rename(&pool, 1, ymd(2024, 6, 1), "LAAC", "LAR").await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();

        // Strictly before the rename: the old ticker.
        assert_eq!(history.ticker_as_at(1, ymd(2024, 1, 1), "LAR"), "LAAC");
        // On the effective date itself: the new ticker (in effect from then).
        assert_eq!(history.ticker_as_at(1, ymd(2024, 6, 1), "LAR"), "LAR");
        // After: still the new ticker.
        assert_eq!(history.ticker_as_at(1, ymd(2025, 1, 1), "LAR"), "LAR");
    }

    #[tokio::test]
    async fn resolves_across_a_chain_of_multiple_renames() {
        let pool = test_pool().await;
        listing(1).ticker("C").insert(&pool).await;
        insert_rename(&pool, 1, ymd(2023, 1, 1), "A", "B").await;
        insert_rename(&pool, 1, ymd(2024, 1, 1), "B", "C").await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();

        assert_eq!(history.ticker_as_at(1, ymd(2022, 6, 1), "C"), "A");
        assert_eq!(history.ticker_as_at(1, ymd(2023, 6, 1), "C"), "B");
        assert_eq!(history.ticker_as_at(1, ymd(2024, 6, 1), "C"), "C");
    }

    #[tokio::test]
    async fn a_different_listings_rename_does_not_leak_across() {
        let pool = test_pool().await;
        listing(1).ticker("X").insert(&pool).await;
        listing(2).ticker("Z").insert(&pool).await;
        insert_rename(&pool, 2, ymd(2024, 1, 1), "Y", "Z").await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();
        assert_eq!(history.ticker_as_at(1, ymd(2025, 1, 1), "X"), "X");
    }
}
