//! Tax-return-ready CSV export of report rows.
//!
//! Reports that support export register a `GET <report>/export` route in their
//! own router and render their year rows through [`csv_response`]: a header
//! record naming the columns, then one record per row, served as a download
//! (`text/csv` + `Content-Disposition: attachment`). The csv writer rejects a
//! record whose length differs from the header's (it is not `flexible`), so a
//! drift between a report's header list and its struct fields fails the request
//! loudly instead of shipping misaligned columns. An empty report still exports
//! the header row, so the expected columns are always visible.
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Render `rows` as a downloadable CSV named `filename`. `header` must list the
/// row struct's fields in declaration order (Decimal fields serialize as plain
/// decimal strings, so figures keep their precision).
pub fn csv_response<T: Serialize>(
    filename: &str,
    header: &[&str],
    rows: &[T],
) -> Result<Response, csv::Error> {
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(false) // the explicit header record below is the only one
        .from_writer(Vec::new());
    wtr.write_record(header)?;
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
    async fn renders_header_and_rows_with_decimal_precision() {
        let rows = vec![Row {
            year: 2024,
            amount: "1234.5678".parse().unwrap(),
        }];
        let resp = csv_response("test.csv", &["year", "amount"], &rows).unwrap();
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            resp.headers().get(header::CONTENT_DISPOSITION).unwrap(),
            "attachment; filename=\"test.csv\""
        );
        assert_eq!(body_string(resp).await, "year,amount\n2024,1234.5678\n");
    }

    #[tokio::test]
    async fn empty_report_still_exports_the_header_row() {
        let resp = csv_response("test.csv", &["year", "amount"], &Vec::<Row>::new()).unwrap();
        assert_eq!(body_string(resp).await, "year,amount\n");
    }

    #[tokio::test]
    async fn header_struct_drift_is_an_error_not_misaligned_columns() {
        let rows = vec![Row {
            year: 2024,
            amount: "1".parse().unwrap(),
        }];
        // A header missing a column (as after adding a struct field without
        // updating the header list) must fail, not ship shifted columns.
        assert!(csv_response("test.csv", &["year"], &rows).is_err());
    }
}
