//! As-at listing-identity resolution across a rename
//! (`entities::listing_rename`, `listing_renames` table): the ticker — and,
//! for an exchange-code change, the MIC — as they stood at a given date, so
//! archived documents keep reading the way the broker statement did and a
//! historical price fetch asks the provider for the symbol that was actually
//! quoted then. See `docs/API.md`'s "Ticker or name changes".
//!
//! Three callers, for two different reasons. Presentation: the Annual Tax
//! Report (`reports::tax_report`, whose disposal/income rows are archived and
//! reconciled against source statements) labels every row with
//! [`RenameHistory::ticker_as_at`], and the listing activity ledger
//! (`reports::activity`, a chronological record of one listing's history)
//! uses it for the *other* listing a row names — the counterpart of a
//! scrip-for-scrip takeover or a demerger — at the action's own date; the
//! ledger has no per-row ticker column of its own, since every row belongs to
//! the one listing it is a history of. Correctness: closing-price collection
//! (`entities::closing_price`) builds its `Market` identity timeline from
//! [`RenameHistory::identities`], so the provider symbol and the exchange
//! calendar for a historical date both resolve to the identity in force on
//! that date rather than today's.
//!
//! Every *other* report still shows the current ticker — a rename is the same
//! security, which is the ATO-correct view — with the full chain discoverable
//! via `GET /listings/:id/renames` and the Row History screen.

use chrono::NaiveDate;
use std::collections::HashMap;

struct Rename {
    effective_date: NaiveDate,
    old_ticker: String,
    new_ticker: String,
    old_exchange_mic: Option<String>,
    new_exchange_mic: Option<String>,
}

/// One listing identity and the date it took effect: what the security was
/// called, and where it traded, over one span of its history.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    /// First date this identity was in effect; `None` for the earliest span,
    /// which reaches back indefinitely (the listing's history before its
    /// first recorded rename).
    pub from: Option<NaiveDate>,
    pub ticker: String,
    /// `None` exactly for an exchange-less (Crypto) span, as on `listings`.
    pub exchange_mic: Option<String>,
}

impl Identity {
    fn in_effect_at(&self, date: NaiveDate) -> bool {
        self.from.is_none_or(|from| from <= date)
    }
}

/// Every listing's rename chain, pre-loaded once per report — mirrors
/// `infra::fx::FxRates::load`, so per-row resolution costs no further DB
/// round-trip. Chains are stored ascending by `effective_date`.
pub struct RenameHistory {
    by_listing: HashMap<i64, Vec<Rename>>,
}

/// One `listing_renames` row as selected by [`RenameHistory::load`]:
/// (listing_id, effective_date, old_ticker, new_ticker, old_mic, new_mic).
type RenameRow = (
    i64,
    NaiveDate,
    String,
    String,
    Option<String>,
    Option<String>,
);

impl RenameHistory {
    pub async fn load(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let rows: Vec<RenameRow> = sqlx::query_as(
            "SELECT listing_id, effective_date, old_ticker, new_ticker, \
                        old_exchange_mic, new_exchange_mic \
                 FROM listing_renames ORDER BY listing_id, effective_date ASC",
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut by_listing: HashMap<i64, Vec<Rename>> = HashMap::new();
        for (
            listing_id,
            effective_date,
            old_ticker,
            new_ticker,
            old_exchange_mic,
            new_exchange_mic,
        ) in rows
        {
            by_listing.entry(listing_id).or_default().push(Rename {
                effective_date,
                old_ticker,
                new_ticker,
                old_exchange_mic,
                new_exchange_mic,
            });
        }
        Ok(Self { by_listing })
    }

    /// The listing's identities over its whole history, ascending and
    /// contiguous: the span before its first recorded rename (that rename's
    /// `old_*`, `from: None`), then one span per rename starting on its
    /// `effective_date`. The final span always carries `current` — `listings`
    /// is the source of truth for the identity in effect now, and a rename
    /// deliberately doesn't re-derive it. With no rename at all the result is
    /// the single open-ended `current` span.
    pub fn identities(&self, listing_id: i64, current: Identity) -> Vec<Identity> {
        let Some(chain) = self.by_listing.get(&listing_id) else {
            return vec![Identity {
                from: None,
                ..current
            }];
        };
        let mut spans = Vec::with_capacity(chain.len() + 1);
        spans.push(Identity {
            from: None,
            ticker: chain[0].old_ticker.clone(),
            exchange_mic: chain[0].old_exchange_mic.clone(),
        });
        for r in chain {
            spans.push(Identity {
                from: Some(r.effective_date),
                ticker: r.new_ticker.clone(),
                exchange_mic: r.new_exchange_mic.clone(),
            });
        }
        let last = spans.last_mut().expect("pushed at least one span");
        last.ticker = current.ticker;
        last.exchange_mic = current.exchange_mic;
        spans
    }

    /// The identity in effect for `listing_id` on `date` — the latest span
    /// from [`identities`](Self::identities) that had started by then.
    pub fn identity_as_at(&self, listing_id: i64, date: NaiveDate, current: Identity) -> Identity {
        let spans = self.identities(listing_id, current);
        spans
            .into_iter()
            .rev()
            .find(|s| s.in_effect_at(date))
            .expect("the earliest span is open-ended, so one always matches")
    }

    /// The ticker in effect for `listing_id` on `date`. `current_ticker` is
    /// the caller's own `listings.ticker` read — this resolver holds only the
    /// rename chain, not a listings snapshot. Exchange changes don't affect
    /// the answer, so the current MIC is immaterial here.
    pub fn ticker_as_at(&self, listing_id: i64, date: NaiveDate, current_ticker: &str) -> String {
        self.identity_as_at(
            listing_id,
            date,
            Identity {
                from: None,
                ticker: current_ticker.to_string(),
                exchange_mic: None,
            },
        )
        .ticker
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

    async fn insert_exchange_rename(
        pool: &sqlx::SqlitePool,
        listing_id: i64,
        effective_date: NaiveDate,
        old: (&str, &str),
        new: (&str, &str),
    ) {
        sqlx::query(
            "INSERT INTO listing_renames \
                 (listing_id, effective_date, old_ticker, new_ticker, \
                  old_exchange_mic, new_exchange_mic) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(listing_id)
        .bind(effective_date)
        .bind(old.0)
        .bind(new.0)
        .bind(old.1)
        .bind(new.1)
        .execute(pool)
        .await
        .unwrap();
    }

    fn ident(ticker: &str, mic: Option<&str>) -> Identity {
        Identity {
            from: None,
            ticker: ticker.to_string(),
            exchange_mic: mic.map(str::to_string),
        }
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

    #[tokio::test]
    async fn identities_of_an_unrenamed_listing_are_one_open_ended_span() {
        let pool = test_pool().await;
        listing(1).ticker("VAS").insert(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();
        assert_eq!(
            history.identities(1, ident("VAS", Some("XASX"))),
            vec![ident("VAS", Some("XASX"))]
        );
    }

    #[tokio::test]
    async fn identities_span_a_chain_contiguously_and_end_on_the_current_row() {
        let pool = test_pool().await;
        listing(1).ticker("C").insert(&pool).await;
        insert_rename(&pool, 1, ymd(2023, 1, 1), "A", "B").await;
        insert_rename(&pool, 1, ymd(2024, 1, 1), "B", "C").await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();

        let spans = history.identities(1, ident("C", Some("XASX")));
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].from, None);
        assert_eq!(spans[0].ticker, "A");
        assert_eq!(spans[1].from, Some(ymd(2023, 1, 1)));
        assert_eq!(spans[1].ticker, "B");
        // The final span always carries the listing's current identity —
        // `listings` is the source of truth for what is in effect now.
        assert_eq!(spans[2].from, Some(ymd(2024, 1, 1)));
        assert_eq!(spans[2].ticker, "C");
        assert_eq!(spans[2].exchange_mic.as_deref(), Some("XASX"));
    }

    #[tokio::test]
    async fn identity_resolves_the_exchange_before_at_and_after_a_move() {
        let pool = test_pool().await;
        listing(1).ticker("LAR").insert(&pool).await;
        insert_exchange_rename(&pool, 1, ymd(2024, 6, 1), ("LAAC", "XASX"), ("LAR", "XNYS")).await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();
        let current = ident("LAR", Some("XNYS"));

        let before = history.identity_as_at(1, ymd(2024, 5, 31), current.clone());
        assert_eq!(before.ticker, "LAAC");
        assert_eq!(before.exchange_mic.as_deref(), Some("XASX"));
        // On the effective date itself the new identity is already in force.
        let on = history.identity_as_at(1, ymd(2024, 6, 1), current.clone());
        assert_eq!(on.ticker, "LAR");
        assert_eq!(on.exchange_mic.as_deref(), Some("XNYS"));
        let after = history.identity_as_at(1, ymd(2025, 1, 1), current);
        assert_eq!(after.ticker, "LAR");
        assert_eq!(after.exchange_mic.as_deref(), Some("XNYS"));
    }

    #[tokio::test]
    async fn identity_of_an_unrenamed_listing_is_the_current_one_at_any_date() {
        let pool = test_pool().await;
        listing(1).ticker("BTC").insert(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let history = RenameHistory::load(&mut conn).await.unwrap();
        let resolved = history.identity_as_at(1, ymd(2019, 1, 1), ident("BTC", None));
        assert_eq!(resolved.ticker, "BTC");
        assert_eq!(resolved.exchange_mic, None);
    }
}
