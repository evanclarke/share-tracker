//! Where an investment-expense deduction goes on the individual return.
//!
//! The deductible *amount* of an investment expense is one figure, but the
//! **question** it is claimed at depends on the income it was incurred
//! earning. `docs/ato/dividend-income-deductions.md`, under *Don't show at
//! this section*, keeps two whole classes of expense off D7/D8:
//!
//! > - expenses incurred earning **trust and partnership** distributions (go
//! >   to Partnerships or Trusts)
//! > - expenses incurred earning **foreign-source dividends** (go to Other
//! >   foreign income or Other deductions)
//!
//! and `docs/ato/tax-return-labels-2026.md` (*Where an investment-expense
//! deduction goes*) resolves each of those to its label on the 2026 form:
//!
//! - **13Y** — question 13's *other deductions relating to distributions*,
//!   non-primary production. The instruction is explicit that debt deductions
//!   (interest, borrowing costs) incurred deriving assessable trust income
//!   belong there too, so interest on money borrowed to buy units in a trust
//!   follows the trust rather than staying at D7/D8.
//! - **20M** — question 20's *other net foreign source income* is the foreign
//!   income **net of** the expenses of earning it (worksheet 1 rows r − s), so
//!   a foreign-income expense is subtracted there rather than claimed at its
//!   own deduction label.
//! - **D15** — the one exception carved out of 20M: *debt* deductions
//!   (interest and borrowing costs) are expressly excluded from the question
//!   20 worksheet and claimed at D15 (label J) instead.
//! - **D7 / D8** — the ordinary case: expenses of earning Australian interest
//!   (D7) and Australian dividend/distribution income (D8).
//!
//! This module is the single rule behind both readers — the [tax
//! summary](crate::reports::tax_summary)'s per-destination lines and the
//! [annual tax report](crate::reports::tax_report)'s printed deduction rows —
//! so the archived document and the CSV can never disagree about where a
//! figure goes (SCENARIOS P-08).

use crate::entities::investment_expense::ExpenseType;
use crate::entities::listing;
use crate::infra::decimal::row_dec;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;

/// The tax-return question an investment-expense deduction is claimed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DeductionDestination {
    /// Question 13 label Y — deductions relating to a trust (or partnership)
    /// distribution, non-primary production.
    TrustDistributions,
    /// Question 20 label M — netted against the year's other foreign source
    /// income rather than claimed at a deduction label of its own.
    ForeignIncome,
    /// Question D15 label J — a *debt* deduction (interest, borrowing costs)
    /// incurred earning foreign income, which question 20's worksheet
    /// expressly excludes.
    ForeignDebt,
    /// Questions D7 / D8 — the expenses of earning Australian interest and
    /// dividend income.
    DividendAndInterest,
}

impl DeductionDestination {
    /// The 2026-form label this destination reports at
    /// (`docs/ato/tax-return-labels-2026.md`; labels shift year to year).
    pub fn ato_label(self) -> &'static str {
        match self {
            Self::TrustDistributions => "13Y",
            Self::ForeignIncome => "20M",
            Self::ForeignDebt => "D15",
            Self::DividendAndInterest => "D7 / D8",
        }
    }
}

/// Is this expense kind a **debt deduction** — interest or borrowing costs?
/// Only [`ExpenseType::LoanInterest`] is: it is the one type the enum defines
/// as interest on borrowed money. A borrowing cost recorded as
/// [`ExpenseType::Other`] is not distinguishable from any other "other"
/// expense and follows the non-debt routing.
fn is_debt_deduction(expense_type: ExpenseType) -> bool {
    matches!(expense_type, ExpenseType::LoanInterest)
}

/// What a listing's recorded history says about the income an expense
/// attributed to it was incurred earning.
struct ListingFacts {
    amit: bool,
    amit_from: Option<NaiveDate>,
    /// The listing is quoted in something other than AUD.
    non_aud: bool,
    /// Some income row on this listing is a trust distribution.
    trust_income: bool,
    /// Some income row on this listing carries foreign-source income.
    foreign_income: bool,
    /// Any income at all is recorded against this listing.
    any_income: bool,
}

/// The per-listing facts the routing rule reads, loaded once per report.
pub struct DeductionRouting {
    listings: HashMap<i64, ListingFacts>,
}

impl DeductionRouting {
    /// Load the routing inputs on the caller's own connection, so a report
    /// resolves destinations from the same single-snapshot read transaction as
    /// the figures it is routing.
    pub async fn load(conn: &mut sqlx::SqliteConnection) -> Result<Self, sqlx::Error> {
        let mut listings: HashMap<i64, ListingFacts> =
            sqlx::query("SELECT id, amit, amit_from, currency FROM listings")
                .fetch_all(&mut *conn)
                .await?
                .iter()
                .map(|row| {
                    Ok::<_, sqlx::Error>((
                        row.try_get::<i64, _>("id")?,
                        ListingFacts {
                            amit: row.try_get("amit")?,
                            amit_from: row.try_get("amit_from")?,
                            non_aud: row.try_get::<String, _>("currency")? != "AUD",
                            trust_income: false,
                            foreign_income: false,
                            any_income: false,
                        },
                    ))
                })
                .collect::<Result<_, _>>()?;

        // The income side characterises a listing the flags cannot: a
        // non-AMIT trust is a trust because its distributions are recorded as
        // trust income (a property of the income row, not of the listing), and
        // a holding earns foreign-source income because a row says so — a
        // foreign currency alone doesn't make it so.
        let income_rows =
            sqlx::query("SELECT listing_id, trust_income, foreign_source_income FROM income")
                .fetch_all(&mut *conn)
                .await?;
        for row in &income_rows {
            let listing_id: i64 = row.try_get("listing_id")?;
            let Some(facts) = listings.get_mut(&listing_id) else {
                continue;
            };
            facts.any_income = true;
            facts.trust_income |= row.try_get::<bool, _>("trust_income")?;
            facts.foreign_income |= row_dec(row, "foreign_source_income")? > Decimal::ZERO;
        }

        Ok(Self { listings })
    }

    /// The question an expense of `expense_type`, attributed to `listing_id`
    /// and incurred in `tax_year`, is claimed at.
    ///
    /// Two cases are **not decidable** from what is recorded and take the
    /// D7/D8 default, which is where the ATO puts an ordinary share-investment
    /// expense:
    ///
    /// - a **portfolio-wide** expense (`listing_id` is `None` — an expense
    ///   attributed to a holding account, or to nothing at all): it relates to
    ///   the portfolio, which may span all three destinations, and nothing
    ///   records how to apportion it. Split it into one row per holding to
    ///   route it.
    /// - a listing with **no income recorded at all** and quoted in AUD: there
    ///   is nothing to say what kind of income the expense was earning. The
    ///   non-AUD case does get routed — a foreign-quoted holding with nothing
    ///   recorded yet is treated as earning foreign-source income — but that
    ///   is a fallback, and any recorded income overrides it.
    ///
    /// A holding whose distributions carry **both** trust and foreign
    /// components (an Australian fund with foreign income inside it, the
    /// ordinary case for a diversified ETF) routes wholly to 13Y: one expense
    /// row carries no component split, and question 13 is where the
    /// distribution itself is reported. Split the expense across two rows to
    /// apportion it.
    pub fn destination(
        &self,
        listing_id: Option<i64>,
        expense_type: ExpenseType,
        tax_year: i32,
    ) -> DeductionDestination {
        let Some(facts) = listing_id.and_then(|id| self.listings.get(&id)) else {
            return DeductionDestination::DividendAndInterest;
        };

        // A trust is a trust on both sides of an AMIT conversion. The shared
        // per-year rule says which *kind* it was in this year — an AMIT
        // attributing on an AMMA statement, or the ordinary trust it was
        // before its `amit_from` year — and question 13 reports both, so both
        // route to 13Y. Reading the flag per year rather than flatly is what
        // keeps a converted fund's pre-conversion years from being described
        // as AMIT years here (SCENARIOS F-23, P-01/P-07).
        let amit_year = listing::amit_in_tax_year(facts.amit, facts.amit_from, tax_year);
        let ordinary_trust_year = facts.amit && !amit_year;
        if amit_year || ordinary_trust_year || facts.trust_income {
            return DeductionDestination::TrustDistributions;
        }

        if facts.foreign_income || (!facts.any_income && facts.non_aud) {
            return if is_debt_deduction(expense_type) {
                DeductionDestination::ForeignDebt
            } else {
                DeductionDestination::ForeignIncome
            };
        }

        DeductionDestination::DividendAndInterest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{listing, test_pool};

    /// Every destination names the label it reports at, and no two share one
    /// (the CSV carries one label per column, so a collision would silently
    /// merge two questions).
    #[test]
    fn each_destination_has_its_own_label() {
        let all = [
            DeductionDestination::TrustDistributions,
            DeductionDestination::ForeignIncome,
            DeductionDestination::ForeignDebt,
            DeductionDestination::DividendAndInterest,
        ];
        let labels: Vec<&str> = all.iter().map(|d| d.ato_label()).collect();
        assert_eq!(labels, vec!["13Y", "20M", "D15", "D7 / D8"]);
    }

    /// An expense attributed to nothing, or to a listing that isn't there,
    /// takes the D7/D8 default rather than being dropped or guessed at. (A
    /// foreign key keeps the second case out of the database; the routing
    /// still answers rather than panicking.)
    #[tokio::test]
    async fn an_unattributed_expense_takes_the_default() {
        let pool = test_pool().await;
        listing(1).insert(&pool).await;
        let mut conn = pool.acquire().await.unwrap();
        let routing = DeductionRouting::load(&mut conn).await.unwrap();
        for listing_id in [None, Some(999)] {
            assert_eq!(
                routing.destination(listing_id, ExpenseType::ManagementFee, 2026),
                DeductionDestination::DividendAndInterest
            );
            // Not even a debt deduction moves off the default: with no holding
            // to characterise, there is no foreign income to net it against.
            assert_eq!(
                routing.destination(listing_id, ExpenseType::LoanInterest, 2026),
                DeductionDestination::DividendAndInterest
            );
        }
    }
}
