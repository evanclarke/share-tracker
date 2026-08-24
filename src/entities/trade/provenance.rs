//! What wrote a `trades` row — the one place that answers "was this trade
//! entered by hand, or constructed by an operation?".
//!
//! Nearly every derived write path stamps the row it inserts with a link back
//! to the fact behind it: an ESS vest, an inherited parcel, a rights exercise,
//! a buy-back participation, a scrip-for-scrip exchange, a demerger, a
//! holding-account transfer, a worthless-shares recognise. Two kinds of check
//! need to ask about that set as a whole rather than about one link at a time
//! — the health report's `non_trading_day_trades` (which names the write path
//! in plain English) and its `nil_proceeds_disposals` (which excludes the
//! mechanically constructed Sells) — and both were transcribing the column
//! list, which is exactly the hand-maintained list this project keeps
//! replacing with a rule.
//!
//! So the list lives here once, and
//! [`tests::every_trades_foreign_key_is_classified`] turns it into a rule: it
//! reads the live schema's foreign keys on `trades` and requires every one to
//! be classified — either here as a provenance link, or in `NOT_PROVENANCE`
//! with the reason it is not one. A future operation that adds a column is an
//! offender until it is classified, so no caller can silently miss it.
//!
//! The per-variant *refusals* in `entities::sell` stay as they are: each needs
//! its own error message ("this Sell belongs to a demerger"), which is a
//! different question from "was this constructed?".

/// One provenance link a `trades` row may carry: a nullable column pointing at
/// the record whose operation wrote the trade.
pub(crate) struct TradeProvenance {
    /// The `trades` column. Non-NULL means the operation wrote this row.
    pub column: &'static str,
    /// The write path that sets it, in plain English and article-first (`an
    /// ESS vest`), for a sentence reading "… (a demerger)".
    pub source: &'static str,
}

/// Every provenance link on `trades`, in the order a `CASE` should test them
/// (a row only ever carries one, so the order is presentational).
pub(crate) const TRADE_PROVENANCE: &[TradeProvenance] = &[
    TradeProvenance {
        column: "ess_statement_id",
        source: "an ESS vest",
    },
    TradeProvenance {
        column: "inheritance_id",
        source: "an inherited parcel",
    },
    TradeProvenance {
        column: "rights_action_id",
        source: "a rights exercise",
    },
    TradeProvenance {
        column: "buyback_action_id",
        source: "a buy-back participation",
    },
    TradeProvenance {
        column: "scrip_action_id",
        source: "a scrip-for-scrip exchange",
    },
    TradeProvenance {
        column: "demerger_action_id",
        source: "a demerger",
    },
    TradeProvenance {
        column: "transfer_id",
        source: "a holding-account transfer",
    },
    TradeProvenance {
        column: "worthless_action_id",
        source: "a worthless-shares recognise",
    },
];

/// The label for a trade nothing constructed.
const ENTERED_DIRECTLY: &str = "entered directly";

/// SQL fragment: the one provenance link that is **not** a column on `trades`.
///
/// A holding-account transfer of crypto disposes of the network fee as its own
/// Sell, and that Sell is linked from the *transfer* (`transfers`
/// `.fee_sale_trade_id`) rather than carrying `transfer_id` itself — precisely
/// so it stays in the gains reports, where the transfer-out Sell does not. It
/// is no less mechanically constructed for that (`entities::sell` guards it
/// separately for the same reason), so every caller here has to ask the extra
/// question.
fn fee_sale_sql(alias: &str) -> String {
    format!("EXISTS(SELECT 1 FROM transfers WHERE fee_sale_trade_id = {alias}.id)")
}

/// SQL predicate: true for a trade row (aliased `alias`) that an operation
/// constructed, false for one entered by hand through `PUT /trades/:id` or
/// `PUT /sells/:id`.
///
/// A DRP reinvestment is deliberately **not** here: `trade_type = 'DRP'` says
/// which *kind* of parcel a row is, not which path wrote it, and a caller that
/// wants it says so itself. (It is a Buy either way, so no disposal check ever
/// meets one.)
pub(crate) fn operation_written_sql(alias: &str) -> String {
    let mut clauses: Vec<String> = TRADE_PROVENANCE
        .iter()
        .map(|p| format!("{alias}.{} IS NOT NULL", p.column))
        .collect();
    clauses.push(fee_sale_sql(alias));
    format!("({})", clauses.join(" OR "))
}

/// SQL expression naming the write path behind a trade row (aliased `alias`)
/// in plain English — `an ESS vest`, `a demerger`, `entered directly` — for a
/// health alert that has to say where a row came from before anyone can decide
/// what to correct.
///
/// The DRP arm is a *label*, not a provenance link (see
/// [`operation_written_sql`]): a reinvestment Buy carries no column of its own,
/// and "a DRP reinvestment" is more useful than "entered directly" for a row
/// the reinvest operation wrote.
pub(crate) fn source_case_sql(alias: &str) -> String {
    let mut arms: Vec<String> = TRADE_PROVENANCE
        .iter()
        .map(|p| format!("WHEN {alias}.{} IS NOT NULL THEN '{}'", p.column, p.source))
        .collect();
    arms.push(format!(
        "WHEN {} THEN 'a holding-account transfer'",
        fee_sale_sql(alias)
    ));
    arms.push(format!(
        "WHEN {alias}.trade_type = 'DRP' THEN 'a DRP reinvestment'"
    ));
    format!("CASE {} ELSE '{ENTERED_DIRECTLY}' END", arms.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;

    /// The foreign keys on `trades` that do **not** record which path wrote the
    /// row, each with the reason — the other half of the classification
    /// [`every_trades_foreign_key_is_classified`] enforces.
    const NOT_PROVENANCE: &[(&str, &str)] = &[
        ("listing_id", "what was traded, on every trade alike"),
        ("currency", "the money the trade is denominated in"),
        (
            "brokerage_currency",
            "the money the brokerage is charged in",
        ),
        (
            "holding_account_id",
            "where the parcel is held; every trade has one, however it was written",
        ),
    ];

    /// One `PRAGMA foreign_key_list` row, reduced to the referencing column.
    #[derive(sqlx::FromRow)]
    struct ForeignKeyColumn {
        from: String,
    }

    /// Every foreign key on `trades` is classified: a provenance link here, or
    /// a column `NOT_PROVENANCE` says why it is not one.
    ///
    /// This is what makes the list above a rule rather than a transcription. A
    /// new operation stamping its trades with a new column fails this test
    /// until it is classified, and both callers of the list — the health
    /// report's write-path label and its nil-proceeds exclusion — pick the new
    /// column up with no edit of their own.
    #[tokio::test]
    async fn every_trades_foreign_key_is_classified() {
        let pool = test_pool().await;
        let keys: Vec<ForeignKeyColumn> = sqlx::query_as("PRAGMA foreign_key_list('trades')")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(!keys.is_empty(), "trades has foreign keys");
        for key in keys {
            let classified = TRADE_PROVENANCE.iter().any(|p| p.column == key.from)
                || NOT_PROVENANCE.iter().any(|(c, _)| *c == key.from);
            assert!(
                classified,
                "trades.{} is a foreign key that neither TRADE_PROVENANCE nor NOT_PROVENANCE \
                 classifies — if an operation stamps it, add it to TRADE_PROVENANCE with the \
                 write path it names; if it is ordinary trade data, say so in NOT_PROVENANCE",
                key.from
            );
        }
        // …and nothing is classified twice, or named for a column that has
        // since been dropped.
        for p in TRADE_PROVENANCE {
            assert!(
                !NOT_PROVENANCE.iter().any(|(c, _)| *c == p.column),
                "trades.{} is classified both ways",
                p.column
            );
        }
    }

    /// The provenance columns really exist on `trades` — a renamed or dropped
    /// column would otherwise leave both SQL builders quietly referring to
    /// nothing (and only fail at query time, inside a report).
    #[tokio::test]
    async fn every_provenance_column_exists_on_trades() {
        let pool = test_pool().await;
        for p in TRADE_PROVENANCE {
            let found: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM pragma_table_info('trades') WHERE name = '{}'",
                p.column
            )))
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(found, 1, "trades.{} exists", p.column);
        }
    }

    #[test]
    fn the_predicate_and_the_label_cover_the_same_links() {
        let predicate = operation_written_sql("t");
        let label = source_case_sql("t");
        for p in TRADE_PROVENANCE {
            assert!(predicate.contains(p.column), "{} in predicate", p.column);
            assert!(label.contains(p.source), "{} in label", p.source);
        }
        // The fee-sale Sell is on both sides too, though it carries no column.
        assert!(predicate.contains("fee_sale_trade_id"));
        assert!(label.contains("fee_sale_trade_id"));
        assert!(label.contains(ENTERED_DIRECTLY));
    }
}
