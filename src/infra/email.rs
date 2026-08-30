//! Outbound email: how a message is composed, and how it is sent.
//!
//! Two scheduled jobs send report email — the weekly portfolio summary
//! (`reports::weekly_summary`) and the per-close price-change alert
//! (`entities::price_alert`). Neither knows anything about SMTP: each builds a
//! [`Document`] and hands it to a [`Mailer`], which is injected the same way
//! the price fetcher is (`SharedMailer`, built only in `main`), so no test path
//! can reach a mail server.
//!
//! Email is **optional**. Absent an `[email]` table in the config file there is
//! no mailer at all, and both jobs record a run that succeeded while doing less
//! than the whole of their work — the same `JobOutcome` note convention
//! `currency-import` uses for its credential-gated half. Failing every week
//! because a deployment does not want email would bury the failures that matter.
//!
//! # Why a `Document` rather than a rendered string
//!
//! Every message goes out as `multipart/alternative`: an HTML part laid out
//! like the screen the figures come from, and a plain-text part saying the same
//! thing. Writing those twice per message is how they drift, so a job describes
//! the message once — headings, stat pairs, tables — and [`Document::render`]
//! produces both parts from it. It also makes the jobs testable without a
//! transport: the tests assert over the rendered parts.
//!
//! Deliberately **not** an inline SVG chart. The Portfolio Overview's graph is
//! inline SVG in the browser, but Gmail and Outlook strip SVG from mail
//! entirely, so the graph's information travels here as the dated table behind
//! it (see REQUIREMENTS, "Emailed portfolio reports").

use rust_decimal::Decimal;
use std::{future::Future, pin::Pin, sync::Arc};

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Why a send failed. One variant per stage, because the operator's next step
/// differs: a message that could not be *built* is a bug or an unrepresentable
/// figure, while one that could not be *sent* is a credential, a relay or the
/// network.
///
/// Each variant carries the failure's **message** rather than the error itself,
/// and the two `From` impls below are hand-written rather than `#[from]` — the
/// one place in the tree that departs from the error-enum rule, for a stated
/// reason. [`Mailer`] is a trait with more than one implementation, and the
/// in-process test double must be able to report a send failure in this same
/// type; `lettre::transport::smtp::Error` has no public constructor, so a
/// failure has to be expressible as text or the failure path could not be
/// tested at all.
#[derive(thiserror::Error, Debug)]
pub enum MailError {
    #[error("could not build the email message: {0}")]
    Build(String),
    #[error("could not send the email: {0}")]
    Send(String),
}

impl From<lettre::error::Error> for MailError {
    fn from(error: lettre::error::Error) -> Self {
        Self::Build(error.to_string())
    }
}

impl From<lettre::transport::smtp::Error> for MailError {
    fn from(error: lettre::transport::smtp::Error) -> Self {
        Self::Send(error.to_string())
    }
}

pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

/// The one thing a job asks of a transport. Boxed-future rather than `async
/// fn`, for the reason [`crate::entities::closing_price::PriceFetcher`] is: the
/// trait has to be `dyn`-compatible to be injected as a `SharedMailer`.
pub trait Mailer: Send + Sync {
    fn send<'a>(&'a self, document: &'a Document) -> SendFuture<'a>;

    /// Where messages go, for the INFO line a job logs after sending. Never the
    /// credentials — only the envelope.
    fn describe(&self) -> String;
}

/// The injection point: `main` builds the live [`SmtpMailer`] and nothing else
/// ever does.
pub type SharedMailer = Arc<dyn Mailer>;

/// The transport plus the alert threshold — everything the two scheduled email
/// jobs need, resolved from the config file's `[email]` table.
///
/// The threshold rides here rather than beside it because it configures the
/// *emailed* alert and nothing else: with no `[email]` table there is no alert
/// to have a threshold for.
#[derive(Clone)]
pub struct Notifier {
    pub mailer: SharedMailer,
    /// A held listing whose latest stored close moved at least this many
    /// percent from the previous stored close is alerted. Always positive; the
    /// comparison is on the absolute move, so a fall alerts like a rise.
    pub price_alert_pct: Decimal,
}

/// How the SMTP connection is encrypted.
///
/// Three modes rather than a boolean because the two encrypted ones are
/// genuinely different protocols — implicit TLS wraps the socket before the
/// first byte, STARTTLS upgrades a plaintext session — and a relay generally
/// serves one of them on one port. `None` exists for a relay on the same host
/// (or an SMTP sink in a test rig); it is never a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encryption {
    /// Submissions, port 465: TLS from the first byte. The default.
    Implicit,
    /// Submission, port 587: plaintext session upgraded with `STARTTLS`.
    StartTls,
    /// Unencrypted. Refuses nothing, protects nothing.
    None,
}

impl Encryption {
    /// The names the config file accepts, in the error message's own order.
    pub const NAMES: [&'static str; 3] = ["implicit", "starttls", "none"];

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "implicit" | "tls" | "ssl" => Ok(Self::Implicit),
            "starttls" => Ok(Self::StartTls),
            "none" | "plain" => Ok(Self::None),
            other => Err(format!(
                "invalid email encryption {other:?}: expected one of {}",
                Self::NAMES.join(", ")
            )),
        }
    }

    /// The port this mode is conventionally served on, used when the config
    /// file names no `smtp_port`.
    pub fn default_port(self) -> u16 {
        match self {
            Self::Implicit => 465,
            Self::StartTls => 587,
            Self::None => 25,
        }
    }
}

/// The `[email]` table resolved and validated: every address has parsed, the
/// recipient list is non-empty, and the threshold is a positive decimal.
///
/// Validation happens at **startup** (`config::Settings::resolve`), not at send
/// time, for the reason a bad `base_path` aborts startup: a weekly job is the
/// worst possible place to discover a typo, because the failure surfaces a week
/// late, to nobody, in a log.
#[derive(Debug, Clone, PartialEq)]
pub struct EmailSettings {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub encryption: Encryption,
    pub credentials: Option<(String, String)>,
    pub from: lettre::message::Mailbox,
    pub to: Vec<lettre::message::Mailbox>,
    /// Prepended to every subject, e.g. `[share-tracker]`, so the mail can be
    /// filtered on one string. Absent by default — the subjects already say
    /// what they are.
    pub subject_prefix: Option<String>,
    pub price_alert_pct: Decimal,
}

/// The live transport. Built only in `main`.
pub struct SmtpMailer {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    from: lettre::message::Mailbox,
    to: Vec<lettre::message::Mailbox>,
    subject_prefix: Option<String>,
}

impl SmtpMailer {
    /// Build the transport from validated settings. Fallible only on the relay
    /// builder itself (an unusable host name); the addresses have already
    /// parsed at startup.
    pub fn new(settings: &EmailSettings) -> Result<Self, String> {
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, Tokio1Executor};

        let builder = match settings.encryption {
            Encryption::Implicit => {
                AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.smtp_host)
            }
            Encryption::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&settings.smtp_host)
            }
            Encryption::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &settings.smtp_host,
            )),
        }
        .map_err(|e| format!("could not configure the SMTP transport: {e}"))?;

        let mut builder = builder.port(settings.smtp_port);
        if let Some((user, password)) = &settings.credentials {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        Ok(Self {
            transport: builder.build(),
            from: settings.from.clone(),
            to: settings.to.clone(),
            subject_prefix: settings.subject_prefix.clone(),
        })
    }

    /// The envelope and both body parts, ready to hand to the transport.
    fn message(&self, document: &Document) -> Result<lettre::Message, MailError> {
        use lettre::message::MultiPart;

        let (text, html) = document.render();
        let subject = match &self.subject_prefix {
            Some(prefix) => format!("{prefix} {}", document.subject),
            None => document.subject.clone(),
        };
        // Deliberately no `ContentType` header of its own: `multipart` sets
        // the message's own `multipart/alternative` (with its boundary), and a
        // `text/html` header set here **overrides** it — which ships a message
        // whose body is a MIME multipart but whose header says it is HTML, so
        // every client renders the raw part boundaries and headers as text
        // (observed end-to-end against a local SMTP sink, 2026-08-30).
        let mut builder = lettre::Message::builder()
            .from(self.from.clone())
            .subject(subject);
        for to in &self.to {
            builder = builder.to(to.clone());
        }
        Ok(builder.multipart(MultiPart::alternative_plain_html(text, html))?)
    }
}

impl Mailer for SmtpMailer {
    fn send<'a>(&'a self, document: &'a Document) -> SendFuture<'a> {
        Box::pin(async move {
            use lettre::AsyncTransport;
            let message = self.message(document)?;
            self.transport.send(message).await?;
            Ok(())
        })
    }

    fn describe(&self) -> String {
        self.to
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

// ---------------------------------------------------------------------------
// Message composition
// ---------------------------------------------------------------------------

/// One message, described once and rendered into both parts.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub subject: String,
    /// The `<h1>` and the text part's title line. Usually the subject without
    /// its dates, so the body reads as a heading rather than as a repeat.
    pub title: String,
    pub sections: Vec<Section>,
}

/// A heading, any number of advisory notes, an optional stat grid, and an
/// optional table — the shape both report bodies happen to have.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    pub heading: String,
    /// Advisory lines shown above the figures: the `provisional` /
    /// `price_carried_forward` / `holding_excluded` warnings the underlying
    /// reports carry, which every caller of a fallback must surface.
    pub notes: Vec<String>,
    pub stats: Vec<(String, String)>,
    pub table: Option<Table>,
}

impl Section {
    pub fn new(heading: impl Into<String>) -> Self {
        Self {
            heading: heading.into(),
            ..Self::default()
        }
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn stat(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.stats.push((label.into(), value.into()));
        self
    }

    pub fn table(mut self, table: Table) -> Self {
        self.table = Some(table);
        self
    }
}

/// A rendered table: header cells, body rows, and which columns are numeric.
///
/// The numeric flags are alignment, and they are per column rather than
/// per cell so a column cannot be half right-aligned: text renders numeric
/// columns right-padded and HTML gives them `text-align: right`, which is what
/// makes a column of money readable at a glance.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub headers: Vec<String>,
    pub numeric: Vec<bool>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// `headers` is `(label, numeric)` per column. Rows are pushed with
    /// [`Self::row`], which is where a row of the wrong width would be a bug —
    /// so it is asserted rather than silently padded.
    pub fn new(headers: &[(&str, bool)]) -> Self {
        Self {
            headers: headers.iter().map(|(h, _)| (*h).to_string()).collect(),
            numeric: headers.iter().map(|(_, n)| *n).collect(),
            rows: Vec::new(),
        }
    }

    pub fn row(&mut self, cells: Vec<String>) {
        debug_assert_eq!(
            cells.len(),
            self.headers.len(),
            "a table row must have one cell per header"
        );
        self.rows.push(cells);
    }
}

impl Document {
    /// Both message parts: `(text/plain, text/html)`.
    pub fn render(&self) -> (String, String) {
        (self.render_text(), self.render_html())
    }

    fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.title);
        out.push('\n');
        out.push_str(&"=".repeat(self.title.chars().count()));
        out.push('\n');
        for section in &self.sections {
            out.push('\n');
            out.push_str(&section.heading);
            out.push('\n');
            out.push_str(&"-".repeat(section.heading.chars().count()));
            out.push('\n');
            for note in &section.notes {
                out.push_str(&wrap(note, 78));
                out.push('\n');
            }
            if !section.stats.is_empty() {
                if !section.notes.is_empty() {
                    out.push('\n');
                }
                let width = section
                    .stats
                    .iter()
                    .map(|(l, _)| l.chars().count())
                    .max()
                    .unwrap_or(0);
                for (label, value) in &section.stats {
                    out.push_str(&format!(
                        "  {label:<width$}  {value}\n",
                        label = label,
                        width = width,
                        value = value
                    ));
                }
            }
            if let Some(table) = &section.table {
                out.push('\n');
                out.push_str(&render_text_table(table));
            }
        }
        out
    }

    fn render_html(&self) -> String {
        // Inline styles on every element, never a `<style>` block: Gmail keeps
        // inline attributes and strips much of a document head, so a stylesheet
        // is the reliable way to send an unstyled table.
        let mut out = String::new();
        out.push_str(
            "<div style=\"font-family:-apple-system,Segoe UI,Helvetica,Arial,sans-serif;\
             font-size:14px;color:#1a1a1a;max-width:900px\">",
        );
        out.push_str(&format!(
            "<h1 style=\"font-size:18px;margin:0 0 16px\">{}</h1>",
            escape(&self.title)
        ));
        for section in &self.sections {
            out.push_str(&format!(
                "<h2 style=\"font-size:15px;margin:24px 0 8px\">{}</h2>",
                escape(&section.heading)
            ));
            for note in &section.notes {
                out.push_str(&format!(
                    "<p style=\"margin:0 0 8px;padding:8px;background:#fff6e0;\
                     border-left:3px solid #d08700\">{}</p>",
                    escape(note)
                ));
            }
            if !section.stats.is_empty() {
                out.push_str(
                    "<table cellpadding=\"0\" cellspacing=\"0\" style=\"margin:0 0 8px\">",
                );
                for (label, value) in &section.stats {
                    out.push_str(&format!(
                        "<tr><td style=\"padding:2px 16px 2px 0;color:#555\">{}</td>\
                         <td style=\"padding:2px 0;text-align:right;font-variant-numeric:\
                         tabular-nums\">{}</td></tr>",
                        escape(label),
                        escape(value)
                    ));
                }
                out.push_str("</table>");
            }
            if let Some(table) = &section.table {
                out.push_str(&render_html_table(table));
            }
        }
        out.push_str("</div>");
        out
    }
}

fn render_text_table(table: &Table) -> String {
    let mut widths: Vec<usize> = table.headers.iter().map(|h| h.chars().count()).collect();
    for row in &table.rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let line = |cells: &[String]| {
        let mut out = String::from("  ");
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths[i] - cell.chars().count();
            if table.numeric[i] {
                out.push_str(&" ".repeat(pad));
                out.push_str(cell);
            } else {
                out.push_str(cell);
                out.push_str(&" ".repeat(pad));
            }
            if i + 1 < cells.len() {
                out.push_str("  ");
            }
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
        out
    };
    let mut out = line(&table.headers);
    out.push_str("  ");
    out.push_str(
        &widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  "),
    );
    out.push('\n');
    for row in &table.rows {
        out.push_str(&line(row));
    }
    out
}

fn render_html_table(table: &Table) -> String {
    let mut out = String::from(
        "<table cellpadding=\"0\" cellspacing=\"0\" \
         style=\"border-collapse:collapse;width:100%;margin:0 0 8px\"><tr>",
    );
    for (i, header) in table.headers.iter().enumerate() {
        out.push_str(&format!(
            "<th style=\"padding:4px 8px;border-bottom:1px solid #999;text-align:{};\
             font-weight:600\">{}</th>",
            align(table.numeric[i]),
            escape(header)
        ));
    }
    out.push_str("</tr>");
    for row in &table.rows {
        out.push_str("<tr>");
        for (i, cell) in row.iter().enumerate() {
            out.push_str(&format!(
                "<td style=\"padding:4px 8px;border-bottom:1px solid #eee;text-align:{};\
                 font-variant-numeric:tabular-nums\">{}</td>",
                align(table.numeric[i]),
                escape(cell)
            ));
        }
        out.push_str("</tr>");
    }
    out.push_str("</table>");
    out
}

fn align(numeric: bool) -> &'static str {
    if numeric { "right" } else { "left" }
}

/// HTML-escape a cell. Every string in a [`Document`] passes through this — a
/// listing name is operator-entered text, and a `&` in one must not truncate
/// the row it is in.
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Wrap an advisory note for the plain-text part. Whole words only, and a word
/// longer than the width is left over-long rather than split — a broken URL or
/// ticker is worse than a ragged line.
fn wrap(text: &str, width: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Figure formatting
// ---------------------------------------------------------------------------

/// Round the way the web UI's `roundDecimalStr` does — **half away from zero**,
/// not `Decimal::round_dp`'s banker's rounding. The email states figures the
/// screen also states, and `5.125` reading as `5.13` there and `5.12` here
/// would be a difference a reader would have to explain to themselves.
fn rounded(value: Decimal, dp: u32) -> Decimal {
    value.round_dp_with_strategy(dp, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

/// A money figure the way the web UI's `COLUMN_KINDS` 'money' rule shows it:
/// 2 decimal places with thousands grouping.
pub fn money(value: Decimal) -> String {
    grouped(&format!("{:.2}", rounded(value, 2)))
}

/// A money figure carrying its sign explicitly, for a *change* — where `+412.00`
/// and `412.00` mean different things and the reader is scanning for the
/// difference. Plain money elsewhere: a balance has no direction to state.
///
/// A change that rounds to **zero** carries no sign at all: it is neither a
/// rise nor a fall, and `+0.00` claims one. That case is common rather than
/// exotic — every FX-movement figure in an AUD-only portfolio is one.
pub fn signed_money(value: Decimal) -> String {
    signed(money(value))
}

/// A per-unit price at the 4 decimal places the UI's `rate4` columns use
/// (`current_price`, `unit_price`), trailing zeros kept so a column lines up.
pub fn unit_price(value: Decimal) -> String {
    grouped(&format!("{:.4}", rounded(value, 4)))
}

/// A percentage at 2 decimal places, signed the same way, with the `%`.
pub fn signed_percent(value: Decimal) -> String {
    format!("{}%", signed(format!("{:.2}", rounded(value, 2))))
}

/// Prefix `+` unless the rendered figure is already negative or is zero — see
/// [`signed_money`] for why zero stays bare.
fn signed(rendered: String) -> String {
    if rendered.starts_with('-') || rendered.chars().all(|c| !c.is_ascii_digit() || c == '0') {
        rendered
    } else {
        format!("+{rendered}")
    }
}

/// Group the integer part of an already-rendered decimal in threes.
fn grouped(rendered: &str) -> String {
    let (sign, rest) = match rendered.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", rendered),
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (rest, None),
    };
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    match fraction {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

// ---------------------------------------------------------------------------
// Test double
// ---------------------------------------------------------------------------

/// An in-process mailer that records what it was asked to send.
///
/// The whole point of the [`Mailer`] injection: a job's test drives the real
/// job body and then asserts over the message, with nothing on the network.
#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Outbox {
        sent: Mutex<Vec<Document>>,
        /// When set, every send fails with this message — for the tests that
        /// pin what a job does when the relay is down.
        fail_with: Option<String>,
    }

    impl Outbox {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// An outbox whose every send fails, for the relay-down tests.
        pub fn failing(message: &str) -> Arc<Self> {
            Arc::new(Self {
                sent: Mutex::new(Vec::new()),
                fail_with: Some(message.to_string()),
            })
        }

        pub fn sent(&self) -> Vec<Document> {
            self.sent.lock().expect("outbox lock").clone()
        }

        /// The one message sent, panicking unless there is exactly one — which
        /// is what every job test that sends asserts.
        pub fn only(&self) -> Document {
            let sent = self.sent();
            assert_eq!(sent.len(), 1, "expected exactly one message");
            sent.into_iter().next().expect("one message")
        }

        /// Both rendered parts of the one message sent, concatenated: what a
        /// test asserts a figure appears in, without caring which part carried
        /// it (the two say the same thing, by construction).
        pub fn only_body(&self) -> String {
            let (text, html) = self.only().render();
            format!("{text}\n{html}")
        }

        pub fn notifier(self: &Arc<Self>, price_alert_pct: Decimal) -> Notifier {
            Notifier {
                mailer: self.clone(),
                price_alert_pct,
            }
        }
    }

    impl Mailer for Outbox {
        fn send<'a>(&'a self, document: &'a Document) -> SendFuture<'a> {
            Box::pin(async move {
                if let Some(message) = &self.fail_with {
                    return Err(MailError::Send(message.clone()));
                }
                self.sent
                    .lock()
                    .expect("outbox lock")
                    .push(document.clone());
                Ok(())
            })
        }

        fn describe(&self) -> String {
            "outbox".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::dec;

    fn document() -> Document {
        let mut table = Table::new(&[("Holding", false), ("Value", true)]);
        table.row(vec!["VAS (Default)".into(), money(dec("1234.5"))]);
        table.row(vec!["Smith & Co".into(), money(dec("-99"))]);
        Document {
            subject: "Portfolio week to 2026-08-30".into(),
            title: "Portfolio week 2026-08-24 to 2026-08-30".into(),
            sections: vec![
                Section::new("Summary")
                    .note("A conversion used a fallback-month FX rate.")
                    .stat("Opening value", money(dec("412880.11")))
                    .stat("Closing value", money(dec("418204.63"))),
                Section::new("Per holding").table(table),
            ],
        }
    }

    #[test]
    fn money_is_grouped_to_two_places_like_the_screen() {
        assert_eq!(money(dec("1234567.891")), "1,234,567.89");
        assert_eq!(money(dec("-1234.5")), "-1,234.50");
        assert_eq!(money(dec("0")), "0.00");
        assert_eq!(money(dec("999")), "999.00");
        assert_eq!(money(dec("1000")), "1,000.00");
    }

    #[test]
    fn a_change_carries_its_sign_explicitly() {
        assert_eq!(signed_money(dec("412")), "+412.00");
        assert_eq!(signed_money(dec("-412")), "-412.00");
        assert_eq!(signed_percent(dec("5.125")), "+5.13%");
        assert_eq!(signed_percent(dec("-5.125")), "-5.13%");
        // A change that rounds to zero is neither a rise nor a fall, so it
        // carries no sign — `+0.00` would claim one, and every FX-movement
        // figure in an AUD-only portfolio is this case.
        assert_eq!(signed_money(dec("0")), "0.00");
        assert_eq!(signed_money(dec("0.001")), "0.00");
        assert_eq!(signed_money(dec("-0.001")), "0.00");
        assert_eq!(signed_percent(dec("0")), "0.00%");
        // …but a thousands-grouped figure of zeros only in its fraction still
        // carries its sign: 1,000.00 is a rise.
        assert_eq!(signed_money(dec("1000")), "+1,000.00");
    }

    #[test]
    fn a_unit_price_keeps_four_places() {
        assert_eq!(unit_price(dec("12.3")), "12.3000");
        assert_eq!(unit_price(dec("0.000049")), "0.0000");
        assert_eq!(unit_price(dec("64000.12345")), "64,000.1235");
    }

    #[test]
    fn both_parts_carry_every_figure_and_note() {
        let (text, html) = document().render();
        for part in [&text, &html] {
            assert!(part.contains("412,880.11"), "{part}");
            assert!(part.contains("418,204.63"), "{part}");
            assert!(part.contains("1,234.50"), "{part}");
            assert!(part.contains("fallback-month FX rate"), "{part}");
            assert!(part.contains("Per holding"), "{part}");
        }
        // The text part aligns its columns; the HTML part is a real table.
        assert!(text.contains("Holding"), "{text}");
        assert!(html.contains("<table"), "{html}");
    }

    #[test]
    fn html_escapes_operator_entered_text() {
        let (_, html) = document().render();
        assert!(html.contains("Smith &amp; Co"), "{html}");
        assert!(!html.contains("Smith & Co"), "{html}");
    }

    #[test]
    fn a_numeric_column_is_right_aligned_in_both_parts() {
        let mut table = Table::new(&[("Holding", false), ("Value", true)]);
        table.row(vec!["A".into(), "1.00".into()]);
        table.row(vec!["BBBB".into(), "1,000.00".into()]);
        let rendered = render_text_table(&table);
        // Both columns are padded to their widest cell, the text one on the
        // right and the numeric one on the left, so the decimal points line up.
        assert!(rendered.contains("  A            1.00\n"), "{rendered}");
        assert!(rendered.contains("  BBBB     1,000.00\n"), "{rendered}");
        assert!(render_html_table(&table).contains("text-align:right"));
    }

    #[test]
    fn encryption_modes_carry_their_conventional_ports() {
        assert_eq!(Encryption::parse("implicit"), Ok(Encryption::Implicit));
        assert_eq!(Encryption::parse("STARTTLS"), Ok(Encryption::StartTls));
        assert_eq!(Encryption::parse("none"), Ok(Encryption::None));
        assert_eq!(Encryption::Implicit.default_port(), 465);
        assert_eq!(Encryption::StartTls.default_port(), 587);
        assert_eq!(Encryption::None.default_port(), 25);
        let err = Encryption::parse("tsl").expect_err("a typo is rejected");
        assert!(err.contains("tsl"), "names the bad value: {err}");
        assert!(err.contains("starttls"), "lists the valid ones: {err}");
    }

    /// Build a mailer against a relay nothing connects to — `SmtpMailer::new`
    /// opens no socket, so the envelope and both body parts can be pinned
    /// without a mail server anywhere.
    fn mailer(subject_prefix: Option<&str>) -> SmtpMailer {
        SmtpMailer::new(&EmailSettings {
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            encryption: Encryption::Implicit,
            credentials: None,
            from: "share-tracker@example.com".parse().expect("a mailbox"),
            to: vec![
                "one@example.com".parse().expect("a mailbox"),
                "two@example.com".parse().expect("a mailbox"),
            ],
            subject_prefix: subject_prefix.map(str::to_string),
            price_alert_pct: Decimal::from(5),
        })
        .expect("the transport builds")
    }

    /// The message must be `multipart/alternative` at the top level. Setting a
    /// `text/html` content type on the builder instead **overrode** that, and
    /// the shipped message announced itself as HTML while carrying a MIME
    /// multipart body — so every client rendered the part boundaries and their
    /// headers as visible text (caught end-to-end against a local SMTP sink,
    /// 2026-08-30, not by any test that existed then).
    #[test]
    fn the_message_is_a_multipart_alternative_carrying_both_parts() {
        let raw = String::from_utf8(
            mailer(None)
                .message(&document())
                .expect("builds")
                .formatted(),
        )
        .expect("utf-8");
        let headers = raw.split("\r\n\r\n").next().expect("a header block");
        assert!(
            headers.contains("Content-Type: multipart/alternative"),
            "the top-level type must be the multipart, not one of its parts: {headers}"
        );
        assert!(!headers.contains("Content-Type: text/html"), "{headers}");
        // …and both alternatives are inside it.
        assert!(raw.contains("Content-Type: text/plain"), "{raw}");
        assert!(raw.contains("Content-Type: text/html"), "{raw}");
    }

    #[test]
    fn the_envelope_carries_every_recipient_and_the_subject_prefix() {
        let raw = String::from_utf8(
            mailer(Some("[share-tracker]"))
                .message(&document())
                .expect("builds")
                .formatted(),
        )
        .expect("utf-8");
        assert!(raw.contains("From: share-tracker@example.com"), "{raw}");
        assert!(raw.contains("one@example.com"), "{raw}");
        assert!(raw.contains("two@example.com"), "{raw}");
        assert!(
            raw.contains("Subject: [share-tracker] Portfolio week to 2026-08-30"),
            "{raw}"
        );
        // Without a prefix the subject is the document's own, unchanged.
        let plain = String::from_utf8(
            mailer(None)
                .message(&document())
                .expect("builds")
                .formatted(),
        )
        .expect("utf-8");
        assert!(
            plain.contains("Subject: Portfolio week to 2026-08-30"),
            "{plain}"
        );
        // The recipients are what `describe` reports for the job's INFO line,
        // and never the credentials.
        assert_eq!(mailer(None).describe(), "one@example.com, two@example.com");
    }

    #[test]
    fn a_note_wraps_on_word_boundaries() {
        let wrapped = wrap("one two three four five six seven eight nine ten", 20);
        for line in wrapped.lines() {
            assert!(line.chars().count() <= 20, "{line:?}");
        }
        assert_eq!(
            wrapped.replace('\n', " "),
            "one two three four five six seven eight nine ten"
        );
    }
}
