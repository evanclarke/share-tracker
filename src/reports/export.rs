//! Tax-return-ready CSV export of report rows.
//!
//! Reports that support export register a `GET <report>/export` route in their
//! own router and render their year rows through [`csv_response`]: a header
//! record naming the columns, a second header record carrying each column's
//! ATO tax-return label (see [`ATO_LABELS_MARKER`]), then one record per row,
//! served as a download (`text/csv` + `Content-Disposition: attachment`). The
//! csv writer rejects a record whose length differs from the header's (it is
//! not `flexible`), so a drift between a report's header list, its label list,
//! and its struct fields fails the request loudly instead of shipping
//! misaligned columns. An empty report still exports both header rows, so the
//! expected columns are always visible.
//!
//! Money columns are exported to the cent, through [`Cents`] (SCENARIOS W-c):
//! a CSV row is a *projection* of the report record whose money fields are
//! `Cents`, not `Decimal`, so which columns round is decided by the field's
//! **type** rather than by a list of column names duplicated from the web
//! UI's `COLUMN_KINDS`. The JSON reports keep the exact figure; a `Decimal`
//! field left as a `Decimal` (a rate, a quantity) still exports verbatim.
use crate::infra::decimal::to_cents;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use rust_decimal::Decimal;
use serde::{Serialize, Serializer};
use std::fmt;

/// First cell of the tax-return-label header row, naming the year of the form
/// the labels target (`docs/ato/tax-return-labels-2026.md` — labels shift year
/// to year, so the row says which year's myTax/paper return it maps to).
pub const ATO_LABELS_MARKER: &str = "ato_labels_2026";

/// A money figure as a tax-return-ready CSV cell: rounded to the cent, half
/// away from zero, and always written with both decimal places (a nil figure
/// is `0.00`).
///
/// The rounding itself is [`crate::infra::decimal::to_cents`], the one
/// statement of the rule — shared with the web UI's money columns
/// (`src/web/util.js`'s `roundDecimalStr(value, 2)`, keyed off
/// `COLUMN_KINDS`) and with the [annual tax
/// report](crate::reports::tax_report)'s disposal schedule. So a figure copied
/// off the CSV reads identically to the screen it mirrors and to the ATO label
/// it is transcribed onto: the exports carry ATO tax-return labels, and 18V
/// arriving as twenty-four zeros after the point was what raised SCENARIOS
/// W-c. Only the presentation rounds — the JSON reports and every stored
/// figure keep full precision, so nothing downstream of a report is computed
/// from a rounded number.
///
/// Wrap a money field of a CSV projection struct in this; leave a rate or
/// quantity field a plain `Decimal` to export it verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cents(pub Decimal);

impl From<Decimal> for Cents {
    fn from(d: Decimal) -> Self {
        Cents(d)
    }
}

impl fmt::Display for Cents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The rounding itself is `infra::decimal::to_cents` — the one
        // statement of the rule, shared with the annual tax report's disposal
        // schedule (SCENARIOS W-d), which needs the rounded *value* to sum
        // rather than a rendered cell. All this type adds is the rendering:
        // always both decimal places, so a column of exported money lines up
        // on the point (`0.00`, never `0`).
        write!(f, "{:.2}", to_cents(self.0))
    }
}

impl Serialize for Cents {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Render `rows` as a downloadable CSV named `filename`. `header` must list the
/// row struct's fields in declaration order (a [`Cents`] field renders to the
/// cent, a plain `Decimal` field verbatim); `labels` is the matching
/// ATO tax-return label per column (empty where a column reports at no label),
/// led by [`ATO_LABELS_MARKER`].
pub fn csv_response<T: Serialize>(
    filename: &str,
    header: &[&str],
    labels: &[&str],
    rows: &[T],
) -> Result<Response, csv::Error> {
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false) // the explicit header records below are the only ones
        .from_writer(Vec::new());
    wtr.write_record(header)?;
    wtr.write_record(labels)?;
    for row in rows {
        wtr.serialize(row)?;
    }
    let body = wtr
        .into_inner()
        .expect("flushing a CSV into an in-memory Vec cannot fail");
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[derive(Serialize)]
    struct Row {
        year: i32,
        amount: rust_decimal::Decimal,
    }

    /// A CSV projection as the two exports declare one: a money column typed
    /// `Cents`, a non-money `Decimal` column left alone.
    #[derive(Serialize)]
    struct MoneyRow {
        year: i32,
        amount: Cents,
        fx_rate: Decimal,
    }

    fn cents(v: &str) -> String {
        Cents(v.parse().unwrap()).to_string()
    }

    async fn body_string(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn renders_headers_labels_and_rows_with_decimal_precision() {
        let rows = vec![Row {
            year: 2024,
            amount: "1234.5678".parse().unwrap(),
        }];
        let resp = csv_response(
            "test.csv",
            &["year", "amount"],
            &[ATO_LABELS_MARKER, "18A"],
            &rows,
        )
        .unwrap();
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"test.csv\""
        );
        assert_eq!(
            body_string(resp).await,
            "year,amount\nato_labels_2026,18A\n2024,1234.5678\n"
        );
    }

    /// SCENARIOS W-c: a money column reaches the CSV at two decimal places,
    /// while a `Decimal` column that is not money is untouched — the split is
    /// the field's type, not a list of column names.
    #[tokio::test]
    async fn a_money_column_rounds_to_the_cent_and_a_plain_decimal_does_not() {
        let rows = vec![MoneyRow {
            year: 2026,
            amount: "39592.120176274130543388699381"
                .parse::<Decimal>()
                .unwrap()
                .into(),
            fx_rate: "0.657123".parse().unwrap(),
        }];
        let resp = csv_response(
            "test.csv",
            &["year", "amount", "fx_rate"],
            &[ATO_LABELS_MARKER, "18A", ""],
            &rows,
        )
        .unwrap();
        assert_eq!(
            body_string(resp).await,
            "year,amount,fx_rate\nato_labels_2026,18A,\n2026,39592.12,0.657123\n"
        );
    }

    /// The direction is the one the screens use (`roundDecimalStr`, half away
    /// from zero) — the bit that silently differs between implementations.
    #[test]
    fn a_half_cent_rounds_away_from_zero_in_both_directions() {
        assert_eq!(cents("0.005"), "0.01");
        assert_eq!(cents("-0.005"), "-0.01");
        assert_eq!(cents("1.005"), "1.01");
        assert_eq!(cents("-1.005"), "-1.01");
        assert_eq!(cents("2.675"), "2.68"); // the classic binary-float trap
        // Either side of the half stays where it belongs.
        assert_eq!(cents("1.00499999"), "1.00");
        assert_eq!(cents("1.00500001"), "1.01");
        assert_eq!(cents("-1.00499999"), "-1.00");
    }

    /// A nil figure prints `0.00`, and a figure that rounds to nil from below
    /// prints `0.00` too — not `-0.00`, which is what the screens show.
    #[test]
    fn a_nil_money_figure_is_two_zero_decimals() {
        assert_eq!(cents("0"), "0.00");
        assert_eq!(cents("0.000000000000000000000000"), "0.00");
        assert_eq!(cents("-0.001"), "0.00");
        assert_eq!(cents("0.004"), "0.00");
    }

    /// A short or whole figure is padded rather than left ragged, so a column
    /// of exported money lines up on the point.
    #[test]
    fn a_whole_or_short_money_figure_is_padded_to_the_cent() {
        assert_eq!(cents("35"), "35.00");
        assert_eq!(cents("30.5"), "30.50");
        assert_eq!(cents("-12345"), "-12345.00");
        // No thousands grouping: the screens group, a CSV cell must not (the
        // separator is the delimiter, and a spreadsheet reads the raw figure).
        assert_eq!(cents("1234567.891"), "1234567.89");
    }

    #[tokio::test]
    async fn empty_report_still_exports_both_header_rows() {
        let resp = csv_response(
            "test.csv",
            &["year", "amount"],
            &[ATO_LABELS_MARKER, ""],
            &Vec::<Row>::new(),
        )
        .unwrap();
        assert_eq!(body_string(resp).await, "year,amount\nato_labels_2026,\n");
    }

    #[tokio::test]
    async fn header_struct_drift_is_an_error_not_misaligned_columns() {
        let rows = vec![Row {
            year: 2024,
            amount: "1".parse().unwrap(),
        }];
        // A header missing a column (as after adding a struct field without
        // updating the header list) must fail, not ship shifted columns.
        assert!(csv_response("test.csv", &["year"], &[ATO_LABELS_MARKER], &rows).is_err());
    }

    #[tokio::test]
    async fn label_row_drift_is_an_error_not_misaligned_columns() {
        // A label list shorter than the header (as after adding a column
        // without mapping its label) must fail, not ship a shifted label row.
        let resp = csv_response(
            "test.csv",
            &["year", "amount"],
            &[ATO_LABELS_MARKER],
            &Vec::<Row>::new(),
        );
        assert!(resp.is_err());
    }
}
