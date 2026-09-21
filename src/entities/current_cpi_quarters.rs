//! The current ABS quarterly CPI series (table `current_cpi_quarters`, 0052)
//! and the import that keeps it current.
//!
//! From 1 July 2027 an Australian-resident individual or trust indexes the
//! elements of a cost base — except the third — by the movement in the CPI
//! (new s 110-36(1A), Subdivision 960-M; EM 1.33–1.74). New s 960-275(1B)/(1C)
//! make the indexation factor the index number for the quarter of the CGT
//! **event** over that for the quarter the **expenditure** (or, for the first
//! element of a share or unit, the amount) was incurred, worked out to 3
//! decimal places by s 960-275(5). `domain::cgt_indexation` is the arithmetic;
//! this module is where the quarterly series it reads comes from.
//!
//! **Why a new table rather than the frozen `cpi_quarters` (0046).** That table
//! is the *frozen* ATO series for costs incurred by 21 September 1999 — its
//! CHECK refuses a later quarter precisely so the old method cannot read one —
//! and the new factor's range begins at the quarter ending 30 September 2027
//! (EM 1.72). Widening it would mean loosening the guarantee its only reader
//! depends on to store rows for a reader that must never see them; see the
//! migration for the full reasoning.
//!
//! **The feed.** The RBA's statistical table G1 *Consumer Price Inflation*
//! (`g1-data.csv`), series `GCPIAG` — "Consumer price index; All groups" —
//! which reproduces the ABS All Groups CPI (its own Source row reads "ABS /
//! RBA") at the stable CSV location the F11 FX import already reads. Only the
//! **ratio** of two quarters' index numbers is ever used, so the RBA's own
//! index reference base is immaterial: a re-based series still gives the same
//! factor, provided both quarters come from the same publication — which is
//! why every row is upserted on every run rather than inserted once.
//!
//! The module is deliberately routeless: the table is feed-fed reference data
//! the UI never edits, and `domain::cgt_indexation` reads it directly. Only the
//! scheduled job ([`run_import`]) writes to it.

use crate::infra::db::write_tx;
use crate::infra::decimal::Money;
use crate::infra::fetch::{FeedFetcher, LiveFeedFetcher, fetch_feed};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::SqlitePool;

/// RBA statistical table G1, *Consumer Price Inflation*, as CSV. Same host and
/// shape as the F11 exchange-rate feed (`entities::rba_fx_rate`).
const CPI_URL: &str = "https://www.rba.gov.au/statistics/tables/csv/g1-data.csv";

/// The RBA's series id for the All groups CPI **index numbers** — the first
/// column of the table. The percentage-change columns beside it are not index
/// numbers and cannot be fed into a factor.
const CPI_SERIES_ID: &str = "GCPIAG";

/// The earliest quarter the new indexation factor can name: the quarter
/// starting 1 July 2027, ending 30 September 2027 (EM 1.72 — the denominator
/// for expenditure deemed incurred on reacquisition). The table's CHECK
/// refuses anything earlier, and the import skips it rather than failing: the
/// published series legitimately carries a century of pre-reform quarters this
/// table has no use for.
pub const FIRST_QUARTER_END: NaiveDate = match NaiveDate::from_ymd_opt(2027, 9, 30) {
    Some(d) => d,
    None => unreachable!(),
};

/// One quarter's All groups CPI, verbatim as published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpiQuarter {
    pub quarter_end: NaiveDate,
    pub cpi: Decimal,
}

#[derive(thiserror::Error, Debug)]
pub enum ImportError {
    /// Could not retrieve the published table (network / HTTP error).
    #[error("could not fetch the CPI feed: {0}")]
    Fetch(String),
    /// The feed was not the expected RBA G1 shape (no series-id row, a bad
    /// index number, a date that is not a quarter end).
    #[error("the CPI feed is malformed: {0}")]
    Parse(String),
    #[error("CPI import write failed: {0}")]
    Db(#[from] sqlx::Error),
}

/// Outcome of an import run: how many quarterly rows were written (inserted or
/// updated) — zero until the ABS publishes the quarter ending 30 September
/// 2027, which is the legitimate state of the feed before the reform's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: usize,
}

/// Parse the RBA G1 CSV into the quarters the new factor can use.
///
/// The file carries a BOM, a `Series ID` row naming each column, several other
/// metadata rows (whose descriptions contain commas inside quotes — none of
/// them is read), then quarterly data rows keyed `DD/MM/YYYY`. The All groups
/// CPI column is located by its series id, never by position, so an inserted
/// column cannot silently shift the import onto a percentage-change series.
/// Rows before [`FIRST_QUARTER_END`] and rows whose value is not yet published
/// are skipped; a malformed figure or a non-quarter-end date fails loudly.
pub fn parse_quarters(content: &str) -> Result<Vec<CpiQuarter>, ImportError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);

    let mut series_column: Option<usize> = None;
    let mut out = Vec::new();

    for line in content.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();

        if fields[0].eq_ignore_ascii_case("Series ID") {
            series_column = fields
                .iter()
                .position(|f| f.eq_ignore_ascii_case(CPI_SERIES_ID));
            if series_column.is_none() {
                return Err(ImportError::Parse(format!(
                    "the feed has no {CPI_SERIES_ID} column"
                )));
            }
            continue;
        }

        // Data rows are keyed by a quarterly date; every other metadata row
        // (Title, Description, Units, …) fails this parse and is skipped.
        let Ok(date) = NaiveDate::parse_from_str(fields[0], "%d/%m/%Y") else {
            continue;
        };
        let column = series_column.ok_or_else(|| {
            ImportError::Parse("data row encountered before the Series ID header row".into())
        })?;
        let Some(value) = fields.get(column) else {
            continue;
        };
        if value.is_empty() {
            // A future quarter the ABS has not published yet.
            continue;
        }
        // The published table is quarterly, so a row that is not a quarter end
        // is a shape change, not a figure to store: normalising it would
        // silently put a month's index number behind a quarter's date.
        if crate::domain::indexation::quarter_end_for(date) != date {
            return Err(ImportError::Parse(format!("{date} is not a quarter end")));
        }
        if date < FIRST_QUARTER_END {
            continue;
        }
        let cpi: Decimal = value
            .parse()
            .map_err(|e| ImportError::Parse(format!("invalid CPI {value:?} for {date}: {e}")))?;
        if cpi <= Decimal::ZERO {
            return Err(ImportError::Parse(format!(
                "non-positive CPI {value:?} for {date}"
            )));
        }
        out.push(CpiQuarter {
            quarter_end: date,
            cpi,
        });
    }

    if series_column.is_none() {
        return Err(ImportError::Parse(
            "no `Series ID` header row found in feed".into(),
        ));
    }
    out.sort_by_key(|q| q.quarter_end);
    Ok(out)
}

/// Parse the given feed content and upsert every usable quarter in one
/// transaction, so a re-based series replaces the whole range together and no
/// stored factor is ever a ratio of two reference bases. Shared by the
/// scheduled task and the manual import path.
pub async fn import_from_content(
    pool: &SqlitePool,
    content: &str,
) -> Result<ImportSummary, ImportError> {
    let quarters = parse_quarters(content)?;
    let mut tx = write_tx(pool).await?;
    for quarter in &quarters {
        sqlx::query(
            "INSERT INTO current_cpi_quarters (quarter_end, cpi) VALUES (?, ?) \
             ON CONFLICT(quarter_end) DO UPDATE SET cpi = excluded.cpi",
        )
        .bind(quarter.quarter_end)
        .bind(Money(quarter.cpi))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(ImportSummary {
        imported: quarters.len(),
    })
}

/// Fetch the published table and import it.
pub async fn run_import(pool: &SqlitePool) -> Result<ImportSummary, ImportError> {
    run_import_with(pool, &LiveFeedFetcher).await
}

/// [`run_import`] through a caller-supplied transport, so the stalled and
/// oversized paths are testable with no socket in play.
async fn run_import_with(
    pool: &SqlitePool,
    fetcher: &dyn FeedFetcher,
) -> Result<ImportSummary, ImportError> {
    let content = fetch_feed(fetcher, CPI_URL, None)
        .await
        .map_err(|e| ImportError::Fetch(e.to_string()))?;
    import_from_content(pool, &content).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dec, test_pool, ymd};

    /// A minimal RBA G1 shape: the BOM, the `Series ID` row that locates the
    /// index column, a description row whose quoted text contains commas (which
    /// must not shift the column), and quarterly data rows — each with the
    /// percentage-change columns present so a positional read would pick the
    /// wrong figure.
    const FEED: &str = "\u{feff}G1 CONSUMER PRICE INFLATION\n\
        Title,Consumer price index,Year-ended inflation\n\
        Description,\"Consumer price index; All groups\",\"Consumer price index; All groups; \
        Year-ended change (in per cent)\"\n\
        Frequency,Quarterly,Quarterly\n\
        Series ID,GCPIAG,GCPIAGYP\n\
        30/06/2027,110.00,3.9\n\
        30/09/2027,110.85,3.2\n\
        31/12/2027,111.40,2.8\n\
        31/03/2028,112.10,2.5\n";

    /// Only the quarters from the reform's first possible denominator onward
    /// are kept, and the index column — not the percentage-change column beside
    /// it — is the one read.
    #[test]
    fn the_index_column_is_read_from_the_reform_quarter_onward() {
        let quarters = parse_quarters(FEED).unwrap();
        assert_eq!(
            quarters,
            vec![
                CpiQuarter {
                    quarter_end: ymd(2027, 9, 30),
                    cpi: dec("110.85"),
                },
                CpiQuarter {
                    quarter_end: ymd(2027, 12, 31),
                    cpi: dec("111.40"),
                },
                CpiQuarter {
                    quarter_end: ymd(2028, 3, 31),
                    cpi: dec("112.10"),
                },
            ]
        );
    }

    /// The pre-reform quarter in the same published series is skipped, not
    /// stored and not an error: the table's CHECK forbids it and the feed
    /// legitimately carries a century of them.
    #[test]
    fn pre_reform_quarters_are_skipped() {
        let quarters = parse_quarters(FEED).unwrap();
        assert!(quarters.iter().all(|q| q.quarter_end >= FIRST_QUARTER_END));
    }

    /// A feed whose index column has not been published yet parses to no rows —
    /// the ordinary state of the feed before October 2027 — while a feed that
    /// has lost its series-id row fails loudly rather than importing nothing
    /// quietly.
    #[test]
    fn an_unpublished_future_quarter_is_empty_but_a_missing_header_is_an_error() {
        let empty = "Series ID,GCPIAG,GCPIAGYP\n30/09/2027,\n31/12/2027,\n";
        assert!(parse_quarters(empty).unwrap().is_empty());

        let no_header = "30/09/2027,110.85\n";
        assert!(matches!(
            parse_quarters(no_header),
            Err(ImportError::Parse(_))
        ));
    }

    /// A data row whose date is not a quarter end is refused rather than
    /// normalised onto a quarter it does not belong to.
    #[test]
    fn a_non_quarter_end_row_is_refused() {
        let feed = "Series ID,GCPIAG\n30/11/2027,110.85\n";
        assert!(matches!(parse_quarters(feed), Err(ImportError::Parse(_))));
    }

    /// A malformed index number fails the import rather than being dropped —
    /// the same rule the FX import observes, for the same reason: a missing
    /// quarter must surface at import time, not as an unindexable disposal.
    #[test]
    fn a_malformed_index_number_fails_loudly() {
        let feed = "Series ID,GCPIAG\n30/09/2027,1x0.85\n";
        assert!(matches!(parse_quarters(feed), Err(ImportError::Parse(_))));
    }

    /// The import upserts in one transaction and is idempotent; a second run
    /// with a re-based (all rows changed) publication replaces the whole range.
    #[tokio::test]
    async fn the_import_upserts_the_whole_range_together() {
        let pool = test_pool().await;
        let first = import_from_content(&pool, FEED).await.unwrap();
        assert_eq!(first.imported, 3);
        let again = import_from_content(&pool, FEED).await.unwrap();
        assert_eq!(again.imported, 3);

        let rebased = FEED.replace("110.85", "221.70").replace("111.40", "222.80");
        import_from_content(&pool, &rebased).await.unwrap();
        let stored: Vec<(String, String)> = sqlx::query_as(
            "SELECT quarter_end, cpi FROM current_cpi_quarters ORDER BY quarter_end",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            stored,
            vec![
                ("2027-09-30".to_string(), "221.70".to_string()),
                ("2027-12-31".to_string(), "222.80".to_string()),
                ("2028-03-31".to_string(), "112.10".to_string()),
            ]
        );
    }
}
