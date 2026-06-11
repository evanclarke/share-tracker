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
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// First cell of the tax-return-label header row, naming the year of the form
/// the labels target (`docs/ato/tax-return-labels-2026.md` — labels shift year
/// to year, so the row says which year's myTax/paper return it maps to).
pub const ATO_LABELS_MARKER: &str = "ato_labels_2026";

/// Render `rows` as a downloadable CSV named `filename`. `header` must list the
/// row struct's fields in declaration order (Decimal fields serialize as plain
/// decimal strings, so figures keep their precision); `labels` is the matching
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
