//! Listing activity ledger: everything ever recorded against one listing, in
//! chronological order, ending in the final holding summary (units held, cost
//! base, current value).
//!
//! One row per recorded fact — every trade (labelled with the operation that
//! created it: rights exercise, buy-back, scrip exchange, demerger, worthless
//! shares, ESS vest, inheritance, transfer network fee), transfers between
//! holding accounts (the transfer group's own Sell/Buys collapse into the one
//! row, since a transfer is not a disposal), income, corporate actions, AMMA
//! and ESS statements, rights sales, DRP enrolment periods, and listing-scoped
//! investment expenses — with a running units-held balance that share
//! splits/consolidations and bonus issues re-base in place (TD 2000/10: the
//! unit count scales on the conversion date, before any same-dated trade,
//! which is already in post-split units).
//!
//! Where a row names *another* listing — the counterpart of a scrip-for-scrip
//! takeover or a demerger — it is named as at the row's own date
//! (`domain::listing_identity::RenameHistory`), so a counterpart renamed since
//! reads the way it did when the action happened rather than the way it reads
//! today.
//!
//! The holding summary reuses the portfolio overview's rows (the shared
//! adjusted-cost-base pipeline), read on this report's own transaction so the
//! ledger and the summary come from one snapshot, then valued per the
//! live-valuation rules (an explicit price wins; a fetch failure leaves the
//! holding unvalued with the reason, never a silent zero).

use crate::domain::listing_identity::RenameHistory;
use crate::entities::closing_price::{self, SharedFetcher};
use crate::entities::corporate_action::{ActionKind, CorporateAction, WorthlessEvent};
use crate::entities::drp_enrolment::DrpEnrolment;
use crate::entities::ess_statement::EssStatement;
use crate::entities::income::{Income, IncomeType};
use crate::entities::investment_expense::InvestmentExpense;
use crate::entities::trade::{Trade, TradeType};
use crate::entities::transfer::Transfer;
use crate::infra::decimal::parse_dec;
use crate::infra::fx::{FxOverride, FxRates};
use crate::infra::http::ApiError;
use crate::reports::portfolio::{self, HoldingOverview};
use axum::{Extension, Json, Router, extract::State, routing::post};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use std::collections::{HashMap, HashSet};

/// One dated row of the ledger. `quantity` is the row's signed unit effect in
/// its own date's unit basis (absent for rows that move no units), and
/// `units_after` is the whole-listing running balance after the row — a
/// split/bonus re-bases it, a transfer leaves it unchanged, so the last row's
/// balance equals the holding summary's total quantity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub date: NaiveDate,
    /// What happened — e.g. `Buy`, `Sell (buy-back)`, `DRP reinvestment`,
    /// `Transfer between accounts`, `Return of capital`, `AMMA statement`.
    pub event: String,
    /// Human-readable specifics (units @ price, per-unit amounts, ratios) in
    /// the record's own currency.
    pub detail: String,
    /// The account the row belongs to; absent where none applies (corporate
    /// actions and transfers span accounts).
    pub holding_account_id: Option<i64>,
    pub quantity: Option<Decimal>,
    pub units_after: Decimal,
    /// The row's own money figure in AUD (see the API docs for what each row
    /// kind reports); absent where the row has no single amount.
    pub amount_aud: Option<Decimal>,
}

#[derive(Debug, Deserialize)]
pub struct ActivityRequest {
    pub listing_id: i64,
    /// Current price per unit in AUD for the holding summary. Absent → the
    /// live-valuation rules; an explicit price wins.
    #[serde(default)]
    pub price: Option<Decimal>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityResponse {
    pub listing_id: i64,
    /// The full ledger, chronological.
    pub events: Vec<ActivityEvent>,
    /// Final holding summary per holding account: the portfolio overview's
    /// rows for this listing.
    pub holdings: Vec<HoldingOverview>,
}

/// One recorded ticker/exchange rename (`entities::listing_rename`).
#[derive(sqlx::FromRow)]
struct ListingRenameRow {
    id: i64,
    effective_date: NaiveDate,
    old_ticker: String,
    new_ticker: String,
    old_exchange_mic: Option<String>,
    new_exchange_mic: Option<String>,
}

/// A rights disposal row joined back to its issue's listing and currency.
struct RightsSaleRow {
    id: i64,
    date: NaiveDate,
    units: Decimal,
    proceeds_per_right: Decimal,
    fx_rate: Decimal,
    holding_account_id: i64,
    currency: String,
}

/// A ledger row before sorting: the sort key plus the row's unit effect.
struct Proto {
    date: NaiveDate,
    /// Same-date ordering: corporate actions (0) act before the day's trades
    /// (2) — a trade dated on a split's conversion date is already in
    /// post-split units — with the statement-ish rows (1) between.
    rank: u8,
    /// Stable tie-break across source tables sharing a rank.
    src: u8,
    id: i64,
    event: String,
    detail: String,
    holding_account_id: Option<i64>,
    quantity: Option<Decimal>,
    /// `(new, old)` unit re-basing from a split/consolidation/bonus issue.
    rebase: Option<(Decimal, Decimal)>,
    amount_aud: Option<Decimal>,
}

/// Everything one listing's ledger is assembled from, read on a single
/// transaction (see `Sources::load`).
struct Sources {
    trades: Vec<Trade>,
    transfers: Vec<Transfer>,
    incomes: Vec<Income>,
    actions: Vec<CorporateAction>,
    ammas: Vec<crate::entities::amma::AmmaStatement>,
    esses: Vec<EssStatement>,
    enrolments: Vec<DrpEnrolment>,
    expenses: Vec<InvestmentExpense>,
    renames: Vec<ListingRenameRow>,
    rights_sales: Vec<RightsSaleRow>,
    fx: FxRates,
    /// Names for the detail texts: accounts and (scrip/demerger counterpart)
    /// listings read as names/tickers, never raw foreign-key ids.
    account_names: HashMap<i64, String>,
    /// Every listing's *current* ticker — the fallback
    /// [`RenameHistory::ticker_as_at`] resolves back from.
    tickers: HashMap<i64, String>,
    /// The rename chains, so a counterpart listing named in an action's
    /// detail reads as it stood on the action's own date, not today
    /// (`docs/API.md`, "Ticker or name changes").
    rename_history: RenameHistory,
    /// The portfolio overview's rows for this listing, from the same snapshot.
    holdings: Vec<HoldingOverview>,
}

impl Sources {
    /// Reads the listing's whole history and its holding summary on the
    /// caller's transaction, so ledger and summary come from one consistent
    /// snapshot. `None` when the listing does not exist.
    async fn load(
        conn: &mut sqlx::SqliteConnection,
        listing_id: i64,
    ) -> Result<Option<Self>, sqlx::Error> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM listings WHERE id = ?)")
            .bind(listing_id)
            .fetch_one(&mut *conn)
            .await?;
        if !exists {
            return Ok(None);
        }

        let trades: Vec<Trade> = sqlx::query_as("SELECT * FROM trades WHERE listing_id = ?")
            .bind(listing_id)
            .fetch_all(&mut *conn)
            .await?;
        let transfers: Vec<Transfer> =
            sqlx::query_as("SELECT * FROM transfers WHERE listing_id = ?")
                .bind(listing_id)
                .fetch_all(&mut *conn)
                .await?;
        let incomes: Vec<Income> = sqlx::query_as("SELECT * FROM income WHERE listing_id = ?")
            .bind(listing_id)
            .fetch_all(&mut *conn)
            .await?;
        let actions: Vec<CorporateAction> =
            sqlx::query_as("SELECT * FROM corporate_actions WHERE listing_id = ?")
                .bind(listing_id)
                .fetch_all(&mut *conn)
                .await?;
        let ammas: Vec<crate::entities::amma::AmmaStatement> =
            sqlx::query_as("SELECT * FROM amma_statements WHERE listing_id = ?")
                .bind(listing_id)
                .fetch_all(&mut *conn)
                .await?;
        let esses: Vec<EssStatement> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM ess_statements WHERE listing_id = ?",
            crate::entities::ess_statement::COLUMNS
        )))
        .bind(listing_id)
        .fetch_all(&mut *conn)
        .await?;
        let enrolments: Vec<DrpEnrolment> =
            sqlx::query_as("SELECT * FROM drp_enrolments WHERE listing_id = ?")
                .bind(listing_id)
                .fetch_all(&mut *conn)
                .await?;
        let expenses: Vec<InvestmentExpense> =
            sqlx::query_as("SELECT * FROM investment_expenses WHERE listing_id = ?")
                .bind(listing_id)
                .fetch_all(&mut *conn)
                .await?;
        let renames: Vec<ListingRenameRow> = sqlx::query_as(
            "SELECT id, effective_date, old_ticker, new_ticker, old_exchange_mic, new_exchange_mic \
             FROM listing_renames WHERE listing_id = ?",
        )
        .bind(listing_id)
        .fetch_all(&mut *conn)
        .await?;
        let rights_sales: Vec<RightsSaleRow> = sqlx::query(
            "SELECT rs.id, rs.date, rs.units, rs.proceeds_per_right, rs.fx_rate, \
                    rs.holding_account_id, ca.currency \
             FROM rights_sales rs JOIN corporate_actions ca ON ca.id = rs.rights_action_id \
             WHERE ca.listing_id = ?",
        )
        .bind(listing_id)
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(|row| {
            Ok(RightsSaleRow {
                id: row.try_get("id")?,
                date: row.try_get("date")?,
                units: parse_dec("units", row.try_get("units")?)?,
                proceeds_per_right: parse_dec(
                    "proceeds_per_right",
                    row.try_get("proceeds_per_right")?,
                )?,
                fx_rate: parse_dec("fx_rate", row.try_get("fx_rate")?)?,
                holding_account_id: row.try_get("holding_account_id")?,
                currency: row.try_get("currency")?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;
        let fx = FxRates::load(&mut *conn).await?;
        let account_names: HashMap<i64, String> =
            sqlx::query_as("SELECT id, name FROM holding_accounts")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .collect();
        let tickers: HashMap<i64, String> = sqlx::query_as("SELECT id, ticker FROM listings")
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
        let rename_history = RenameHistory::load(&mut *conn).await?;
        let holdings: Vec<HoldingOverview> = portfolio::db_holdings_on(&mut *conn, None)
            .await?
            .into_iter()
            .filter(|h| h.listing_id == listing_id)
            .collect();

        Ok(Some(Self {
            trades,
            transfers,
            incomes,
            actions,
            ammas,
            esses,
            enrolments,
            expenses,
            renames,
            rights_sales,
            fx,
            account_names,
            tickers,
            rename_history,
            holdings,
        }))
    }

    /// Every source table's rows as unsorted protos.
    fn proto_rows(&self) -> Result<Vec<Proto>, sqlx::Error> {
        let mut rows = Vec::new();
        rows.extend(self.action_rows());
        rows.extend(self.transfer_rows());
        rows.extend(self.income_rows()?);
        rows.extend(self.statement_rows());
        rows.extend(self.rights_sale_rows()?);
        rows.extend(self.rename_rows());
        rows.extend(self.enrolment_rows());
        rows.extend(self.expense_rows()?);
        rows.extend(self.trade_rows()?);
        Ok(rows)
    }

    /// `account 'Name'` for the detail texts, falling back to the raw id only
    /// when the account has since gone.
    fn account(&self, id: i64) -> String {
        self.account_names
            .get(&id)
            .map_or_else(|| format!("account {id}"), |n| format!("account '{n}'"))
    }

    /// A counterpart listing's ticker as it stood on `date` — the current
    /// ticker unless renamed since, in which case the one in force then, so a
    /// dated ledger row never prints a ticker that did not yet exist. Falls
    /// back to the raw id only when the listing has since gone.
    fn ticker_as_at(&self, listing_id: i64, date: NaiveDate) -> String {
        let Some(current) = self.tickers.get(&listing_id) else {
            return format!("listing {listing_id}");
        };
        self.rename_history.ticker_as_at(listing_id, date, current)
    }

    fn action_rows(&self) -> Vec<Proto> {
        self.actions
            .iter()
            .map(|a| {
                let (event, detail, rebase) =
                    describe_action(a, |id| self.ticker_as_at(id, a.date));
                Proto {
                    date: a.date,
                    rank: 0,
                    src: 0,
                    id: a.id,
                    event,
                    detail,
                    holding_account_id: None,
                    quantity: None,
                    rebase,
                    amount_aud: None,
                }
            })
            .collect()
    }

    fn transfer_rows(&self) -> Vec<Proto> {
        self.transfers
            .iter()
            .map(|t| {
                // The transfer group's own Sell/Buys collapse into this row (a
                // transfer is not a disposal); the moved quantity comes from
                // the group's transfer-out Sell.
                let moved = self
                    .trades
                    .iter()
                    .find(|tr| tr.transfer_id == Some(t.id) && tr.trade_type == TradeType::Sell)
                    .map(|tr| tr.quantity);
                let (from, to) = (
                    self.account(t.from_account_id),
                    self.account(t.to_account_id),
                );
                let mut detail = match moved {
                    Some(q) => format!("{q} unit(s) from {from} to {to}"),
                    None => format!("from {from} to {to}"),
                };
                if let Some(fee_sell) = t.fee_sale_trade_id {
                    detail.push_str(&format!(" (network fee disposal: trade #{fee_sell})"));
                }
                Proto {
                    date: t.date,
                    rank: 1,
                    src: 1,
                    id: t.id,
                    event: "Transfer between accounts".to_string(),
                    detail,
                    holding_account_id: None,
                    quantity: None,
                    rebase: None,
                    amount_aud: None,
                }
            })
            .collect()
    }

    fn income_rows(&self) -> Result<Vec<Proto>, sqlx::Error> {
        self.incomes
            .iter()
            .map(|i| {
                let event = if i.income_type == IncomeType::EmploymentIncome {
                    // Remuneration recorded against the holding it was
                    // calculated from — never called a dividend (TD 2017/26).
                    "Employment income (dividend equivalent)"
                } else if i.income_type == IncomeType::OtherIncome {
                    // Ordinary income produced by the holding but paid as no
                    // distribution of it — a staking reward or an
                    // established-token airdrop (QC 69950, SCENARIOS L-03).
                    "Other income (staking reward / airdrop)"
                } else if i.buyback_trade_id.is_some() {
                    "Dividend (buy-back component)"
                } else if i.trust_income {
                    "Trust distribution"
                } else {
                    "Dividend"
                };
                let gross = i.gross_cash_income();
                let mut detail = format!("gross {gross} {}", i.currency);
                if i.franking_credits > Decimal::ZERO {
                    detail.push_str(&format!(", franking credits {}", i.franking_credits));
                }
                if let Some(trade_id) = i.reinvestment_trade_id {
                    detail.push_str(&format!(", reinvested (trade #{trade_id})"));
                }
                // The AUD month follows the tax summary, on the model's own
                // `assessment_date`: present entitlement governs a trust row
                // when recorded, otherwise the pay date.
                let amount_aud =
                    self.fx
                        .to_aud(gross, &i.currency, i.assessment_date(), FxOverride::None)?;
                Ok(Proto {
                    date: i.date_paid,
                    rank: 1,
                    src: 2,
                    id: i.id,
                    event: event.to_string(),
                    detail,
                    holding_account_id: Some(i.holding_account_id),
                    quantity: None,
                    rebase: None,
                    amount_aud: Some(amount_aud),
                })
            })
            .collect()
    }

    /// The two statement kinds — AMMA and ESS — which move no units and carry
    /// no single AUD figure.
    fn statement_rows(&self) -> Vec<Proto> {
        let ammas = self.ammas.iter().map(|a| Proto {
            date: a.tax_year_end_date,
            rank: 1,
            src: 3,
            id: a.id,
            event: "AMMA statement".to_string(),
            detail: format!(
                "FY{} attribution over {} unit(s); cost base adjustment {} {} per unit",
                a.tax_year_end_date.year(),
                a.units_held,
                a.cost_base_adjustment,
                a.currency
            ),
            holding_account_id: Some(a.holding_account_id),
            quantity: None,
            rebase: None,
            amount_aud: None,
        });
        let esses = self.esses.iter().map(|s| Proto {
            date: s.taxing_point_date,
            rank: 1,
            src: 4,
            id: s.id,
            event: "ESS statement".to_string(),
            detail: format!(
                "taxing point: {} share(s) at market value {} {}",
                s.quantity, s.market_value_per_share, s.currency
            ),
            holding_account_id: Some(s.holding_account_id),
            quantity: None,
            rebase: None,
            amount_aud: None,
        });
        ammas.chain(esses).collect()
    }

    fn rights_sale_rows(&self) -> Result<Vec<Proto>, sqlx::Error> {
        self.rights_sales
            .iter()
            .map(|rs| {
                let proceeds = rs.units * rs.proceeds_per_right;
                let amount_aud = self.fx.to_aud(
                    proceeds,
                    &rs.currency,
                    rs.date,
                    FxOverride::Fallback(rs.fx_rate),
                )?;
                Ok(Proto {
                    date: rs.date,
                    rank: 1,
                    src: 5,
                    id: rs.id,
                    event: "Rights sale/lapse".to_string(),
                    detail: format!(
                        "{} right(s) at {} {} per right (share holding untouched)",
                        rs.units, rs.proceeds_per_right, rs.currency
                    ),
                    holding_account_id: Some(rs.holding_account_id),
                    quantity: None,
                    rebase: None,
                    amount_aud: Some(amount_aud),
                })
            })
            .collect()
    }

    fn rename_rows(&self) -> Vec<Proto> {
        self.renames
            .iter()
            .map(|r| {
                let mut detail = format!("renamed {} -> {}", r.old_ticker, r.new_ticker);
                if r.old_exchange_mic != r.new_exchange_mic {
                    detail.push_str(&format!(
                        " (exchange {} -> {})",
                        r.old_exchange_mic.as_deref().unwrap_or("Crypto"),
                        r.new_exchange_mic.as_deref().unwrap_or("Crypto")
                    ));
                }
                Proto {
                    date: r.effective_date,
                    rank: 1,
                    src: 9,
                    id: r.id,
                    event: "Ticker/exchange change".to_string(),
                    detail,
                    holding_account_id: None,
                    quantity: None,
                    rebase: None,
                    amount_aud: None,
                }
            })
            .collect()
    }

    /// A DRP enrolment is one row, plus a second when it has since ended.
    fn enrolment_rows(&self) -> Vec<Proto> {
        self.enrolments
            .iter()
            .flat_map(|e| {
                let started = Proto {
                    date: e.enrolment_date,
                    rank: 1,
                    src: 6,
                    id: e.id,
                    event: "DRP enrolment".to_string(),
                    detail: format!("residual handling: {:?}", e.residual_handling),
                    holding_account_id: Some(e.holding_account_id),
                    quantity: None,
                    rebase: None,
                    amount_aud: None,
                };
                let ended = e.unenrolment_date.map(|end| Proto {
                    date: end,
                    rank: 1,
                    src: 6,
                    id: e.id,
                    event: "DRP unenrolment".to_string(),
                    detail: format!("enrolled since {}", e.enrolment_date),
                    holding_account_id: Some(e.holding_account_id),
                    quantity: None,
                    rebase: None,
                    amount_aud: None,
                });
                std::iter::once(started).chain(ended)
            })
            .collect()
    }

    fn expense_rows(&self) -> Result<Vec<Proto>, sqlx::Error> {
        self.expenses
            .iter()
            .map(|e| {
                let mut detail = format!("{:?}: {} {}", e.expense_type, e.amount, e.currency);
                if let Some(desc) = &e.description {
                    detail.push_str(&format!(" — {desc}"));
                }
                let amount_aud =
                    self.fx
                        .to_aud(e.amount, &e.currency, e.date_incurred, FxOverride::None)?;
                Ok(Proto {
                    date: e.date_incurred,
                    rank: 1,
                    src: 7,
                    id: e.id,
                    event: "Investment expense".to_string(),
                    detail,
                    holding_account_id: e.holding_account_id,
                    quantity: None,
                    rebase: None,
                    amount_aud: Some(amount_aud),
                })
            })
            .collect()
    }

    fn trade_rows(&self) -> Result<Vec<Proto>, sqlx::Error> {
        let fee_sale_ids: HashSet<i64> = self
            .transfers
            .iter()
            .filter_map(|t| t.fee_sale_trade_id)
            .collect();
        self.trades
            .iter()
            // A transfer group's trades are collapsed into the transfer's row.
            .filter(|t| t.transfer_id.is_none())
            .map(|t| {
                let signed = match t.trade_type {
                    TradeType::Buy | TradeType::DRP => t.quantity,
                    TradeType::Sell => -t.quantity,
                };
                // What the trade adds up to in its own currency — the model's
                // own `net_transaction_total`, i.e. the very figure the write
                // path cross-checks a recorded statement total against —
                // converted once with the trade's own FX precedence, the
                // cost-base pipeline's convention. A rollover/vest/inheritance-
                // created trade carries its figure on the brokerage column
                // (price 0), so the carried amount still shows.
                let amount_aud = self.fx.to_aud(
                    t.net_transaction_total(),
                    &t.currency,
                    t.date,
                    t.fx_override(),
                )?;
                Ok(Proto {
                    date: t.date,
                    rank: 2,
                    src: 8,
                    id: t.id,
                    event: trade_event(t, &fee_sale_ids),
                    detail: format!("{} @ {} {}", t.quantity, t.average_price, t.currency),
                    holding_account_id: Some(t.holding_account_id),
                    quantity: Some(signed),
                    rebase: None,
                    amount_aud: Some(amount_aud),
                })
            })
            .collect()
    }
}

/// Orders the protos and walks them into the ledger, carrying the running
/// units-held balance that a split/bonus re-bases in place before the day's
/// trades (TD 2000/10).
fn ledger(mut rows: Vec<Proto>) -> Vec<ActivityEvent> {
    rows.sort_by_key(|r| (r.date, r.rank, r.src, r.id));
    let mut balance = Decimal::ZERO;
    rows.into_iter()
        .map(|r| {
            if let Some((new, old)) = r.rebase {
                balance = balance * new / old;
            }
            if let Some(q) = r.quantity {
                balance += q;
            }
            ActivityEvent {
                date: r.date,
                event: r.event,
                detail: r.detail,
                holding_account_id: r.holding_account_id,
                quantity: r.quantity,
                units_after: balance,
                amount_aud: r.amount_aud,
            }
        })
        .collect()
}

/// Reads the listing's whole history and its holding summary on one read
/// transaction (a single consistent snapshot), then assembles the ledger.
/// `None` when the listing does not exist.
pub async fn db_activity(
    pool: &SqlitePool,
    listing_id: i64,
) -> Result<Option<(Vec<ActivityEvent>, Vec<HoldingOverview>)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let Some(sources) = Sources::load(&mut tx, listing_id).await? else {
        return Ok(None);
    };
    tx.commit().await?;

    let events = ledger(sources.proto_rows()?);
    Ok(Some((events, sources.holdings)))
}

/// The trade's event label: its type, qualified by the operation that created
/// it (the provenance columns / the transfer's network-fee link). `pub(crate)`
/// so the annual tax report's disposal schedule can label each parcel's
/// acquisition the same way this ledger does, instead of a second labeller.
pub(crate) fn trade_event(t: &Trade, fee_sale_ids: &HashSet<i64>) -> String {
    let base = match t.trade_type {
        TradeType::Buy => "Buy",
        TradeType::Sell => "Sell",
        TradeType::DRP => "DRP reinvestment",
    };
    let qualifier = if t.ess_statement_id.is_some() {
        Some("ESS vest")
    } else if t.inheritance_id.is_some() {
        Some("inheritance")
    } else if t.rights_action_id.is_some() {
        Some("rights exercise")
    } else if t.buyback_action_id.is_some() {
        Some("buy-back")
    } else if t.scrip_action_id.is_some() {
        Some("scrip exchange")
    } else if t.demerger_action_id.is_some() {
        Some("demerger")
    } else if t.worthless_action_id.is_some() {
        Some("worthless shares")
    } else if fee_sale_ids.contains(&t.id) {
        Some("transfer network fee")
    } else {
        None
    };
    match qualifier {
        Some(q) => format!("{base} ({q})"),
        None => base.to_string(),
    }
}

/// Event label, detail text, and any unit re-basing for a corporate action.
/// `ticker` names the scrip/demerger counterpart listing — details read as
/// tickers, never raw foreign-key ids, and the caller binds it to the
/// action's own date (`Sources::ticker_as_at`) so a counterpart renamed since
/// still reads the way it did when the action happened.
fn describe_action(
    a: &CorporateAction,
    ticker: impl Fn(i64) -> String,
) -> (String, String, Option<(Decimal, Decimal)>) {
    match &a.kind {
        ActionKind::ReturnOfCapital {
            amount_per_unit,
            currency,
            record_date,
        } => (
            "Return of capital".to_string(),
            // The row is dated by the payment; the record date, when recorded,
            // is what decided which parcels the payment reached, so it belongs
            // in the line explaining the row.
            match record_date {
                Some(record_date) => {
                    format!("{amount_per_unit} {currency} per unit (record date {record_date})")
                }
                None => format!("{amount_per_unit} {currency} per unit"),
            },
            None,
        ),
        ActionKind::ShareSplit {
            split_new_units,
            split_old_units,
        } => {
            let label = if split_new_units < split_old_units {
                "Share consolidation"
            } else {
                "Share split"
            };
            (
                label.to_string(),
                format!("{split_new_units}-for-{split_old_units}"),
                Some((*split_new_units, *split_old_units)),
            )
        }
        ActionKind::BonusIssue {
            bonus_units,
            bonus_held_units,
        } => (
            "Bonus issue".to_string(),
            format!("{bonus_units} bonus unit(s) per {bonus_held_units} held"),
            Some((*bonus_held_units + *bonus_units, *bonus_held_units)),
        ),
        ActionKind::RightsIssue {
            rights_units,
            rights_held_units,
            exercise_price,
            currency,
        } => (
            "Rights issue".to_string(),
            format!(
                "{rights_units}-for-{rights_held_units}, exercisable at {exercise_price} {currency}"
            ),
            None,
        ),
        ActionKind::BuyBack {
            buyback_price,
            buyback_dividend,
            currency,
            ..
        } => (
            "Buy-back offer".to_string(),
            format!("{buyback_price} {currency} per unit (dividend component {buyback_dividend})"),
            None,
        ),
        ActionKind::ScripForScrip {
            scrip_listing_id,
            scrip_new_units,
            scrip_old_units,
            scrip_cash_per_unit,
            scrip_cash_currency,
            ..
        } => {
            let mut detail = format!(
                "{scrip_new_units} unit(s) of {} per {scrip_old_units} held",
                ticker(*scrip_listing_id)
            );
            if let (Some(cash), Some(cur)) = (scrip_cash_per_unit, scrip_cash_currency) {
                detail.push_str(&format!(" plus {cash} {cur} cash per unit"));
            }
            ("Scrip-for-scrip takeover".to_string(), detail, None)
        }
        ActionKind::Demerger {
            demerger_listing_id,
            demerger_new_units,
            demerger_held_units,
            demerger_cost_base_pct,
            ..
        } => (
            "Demerger".to_string(),
            format!(
                "{demerger_new_units} unit(s) of {} per \
                 {demerger_held_units} held; {demerger_cost_base_pct}% of cost base",
                ticker(*demerger_listing_id)
            ),
            None,
        ),
        ActionKind::WorthlessShares { worthless_event } => (
            "Worthless shares".to_string(),
            match worthless_event {
                WorthlessEvent::G3Declaration => {
                    "liquidator/administrator declaration (CGT event G3)".to_string()
                }
                WorthlessEvent::C2Cancellation => {
                    "deregistration/cancellation (CGT event C2)".to_string()
                }
            },
            None,
        ),
    }
}

pub fn router() -> Router<SqlitePool> {
    Router::new().route("/portfolio/activity", post(activity_handler))
}

async fn activity_handler(
    State(pool): State<SqlitePool>,
    fetcher: Option<Extension<SharedFetcher>>,
    Json(req): Json<ActivityRequest>,
) -> Result<Json<ActivityResponse>, ApiError> {
    let Some((events, mut holdings)) = db_activity(&pool, req.listing_id)
        .await
        .map_err(ApiError::from)?
    else {
        return Err(ApiError::not_found(format!(
            "listing {} not found",
            req.listing_id
        )));
    };

    // Value the summary like the overview does: an explicit price wins and is
    // never fetched; otherwise the live-valuation rules, degrading per-holding.
    let prices: HashMap<i64, Decimal> = req
        .price
        .map(|p| HashMap::from([(req.listing_id, p)]))
        .unwrap_or_default();
    let live = closing_price::resolve_live_prices(
        &pool,
        fetcher.as_ref().map(|f| f.0.as_ref()),
        true,
        &prices,
        holdings.iter().map(|h| h.listing_id),
    )
    .await
    .map_err(ApiError::from)?;
    for h in &mut holdings {
        if let Some(&price) = prices.get(&h.listing_id) {
            h.current_price = Some(price);
            h.market_value = Some(h.quantity * price);
        } else if let Some(result) = live.get(&h.listing_id) {
            match result {
                Ok(v) => {
                    h.current_price = Some(v.aud_price);
                    h.market_value = Some(h.quantity * v.aud_price);
                    h.price_as_of = Some(v.as_of.clone());
                    h.fx_provisional = v.fx_provisional;
                }
                Err(reason) => h.price_unavailable = Some(reason.clone()),
            }
        }
    }

    Ok(Json(ActivityResponse {
        listing_id: req.listing_id,
        events,
        holdings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{corporate_action, transfer};
    use crate::test_support::{self, ApiClient, ApiResponse, allocate, dec, test_pool, ymd};
    use axum::http::StatusCode;

    async fn insert_listing(pool: &SqlitePool, id: i64, ticker: &str) {
        test_support::listing(id)
            .ticker(ticker)
            .name(ticker)
            .insert(pool)
            .await;
    }

    /// A listing quoted in a foreign currency: a trade is recorded in its
    /// listing's own currency (`trade::UpsertError::CurrencyNotListings`).
    async fn insert_listing_in(pool: &SqlitePool, id: i64, ticker: &str, currency: &str) {
        test_support::listing(id)
            .mic("XNYS")
            .ticker(ticker)
            .name(ticker)
            .currency(currency)
            .insert(pool)
            .await;
    }

    async fn events(pool: &SqlitePool, listing_id: i64) -> Vec<ActivityEvent> {
        db_activity(pool, listing_id).await.unwrap().unwrap().0
    }

    #[tokio::test]
    async fn db_unknown_listing_is_none() {
        let pool = test_pool().await;
        assert!(db_activity(&pool, 99).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn db_no_activity_is_empty_ledger() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        let (events, holdings) = db_activity(&pool, 1).await.unwrap().unwrap();
        assert!(events.is_empty());
        assert!(holdings.is_empty());
    }

    /// The core ledger: buy, split, dividend, partial sell — chronological,
    /// labelled, signed quantities, and a running balance the split re-bases.
    #[tokio::test]
    async fn db_ledger_is_chronological_with_running_balance() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        test_support::buy(1, 1)
            .qty(dec("100"))
            .price(dec("10"))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .insert(&pool)
            .await; // 2024-01-01
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 3, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        test_support::income(1, 1, ymd(2024, 4, 1))
            .with(|i| {
                i.franked_amount = dec("70");
                i.franking_credits = dec("30");
            })
            .insert(&pool)
            .await;
        test_support::sell(2, 1)
            .date(ymd(2024, 6, 3))
            .qty(dec("80"))
            .price(dec("6"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("80")).await;

        let (events, holdings) = db_activity(&pool, 1).await.unwrap().unwrap();
        let order: Vec<&str> = events.iter().map(|e| e.event.as_str()).collect();
        assert_eq!(order, vec!["Buy", "Share split", "Dividend", "Sell"]);
        assert_eq!(events[0].date, ymd(2024, 1, 2));
        assert_eq!(events[0].quantity, Some(dec("100")));
        assert_eq!(events[0].units_after, dec("100"));
        // Buy amount: 10 × 100 + 9.95 + 0.995.
        assert_eq!(events[0].amount_aud, Some(dec("1010.945")));
        // The 2-for-1 split re-bases the balance without a quantity of its own.
        assert_eq!(events[1].quantity, None);
        assert_eq!(events[1].units_after, dec("200"));
        assert_eq!(events[1].detail, "2-for-1");
        // The dividend moves no units; the gross rides in amount_aud.
        assert_eq!(events[2].units_after, dec("200"));
        assert_eq!(events[2].amount_aud, Some(dec("70")));
        assert!(events[2].detail.contains("franking credits 30"));
        // The sell is signed and in its own date's (post-split) units.
        assert_eq!(events[3].quantity, Some(dec("-80")));
        assert_eq!(events[3].units_after, dec("120"));
        // Sell amount: 6 × 80 − 0 brokerage.
        assert_eq!(events[3].amount_aud, Some(dec("480")));

        // The last row's balance reconciles with the holding summary.
        let held: Decimal = holdings.iter().map(|h| h.quantity).sum();
        assert_eq!(events.last().unwrap().units_after, held);
        assert_eq!(held, dec("120"));
    }

    /// A transfer shows as one row — its group's own Sell/Buys never appear
    /// (a transfer is not a disposal) — and the balance is unchanged.
    #[tokio::test]
    async fn db_transfer_collapses_to_one_row() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "ICE").await;
        crate::entities::holding_account::db_upsert(
            &pool,
            &crate::entities::holding_account::HoldingAccount {
                id: 2,
                name: "Personal".to_string(),
            },
        )
        .await
        .unwrap();
        test_support::buy(1, 1).qty(dec("100")).insert(&pool).await; // 2024-01-01, account 1
        transfer::db_transfer(
            &pool,
            1,
            &transfer::TransferBody {
                listing_id: 1,
                date: ymd(2024, 5, 1),
                from_account_id: 1,
                to_account_id: 2,
                allocations: vec![crate::entities::sell::AllocationInput {
                    purchase_trade_id: 1,
                    quantity_allocated: dec("40"),
                }],
                fee_allocations: vec![],
                fee_market_price: None,
                fee_fx_rate: None,
            },
        )
        .await
        .unwrap();

        let (events, holdings) = db_activity(&pool, 1).await.unwrap().unwrap();
        assert_eq!(events.len(), 2, "buy + one transfer row, no group trades");
        assert_eq!(events[1].event, "Transfer between accounts");
        // Accounts are named, never raw foreign-key ids.
        assert_eq!(
            events[1].detail,
            "40 unit(s) from account 'Default' to account 'Personal'"
        );
        assert_eq!(events[1].quantity, None);
        assert_eq!(
            events[1].units_after,
            dec("100"),
            "a transfer moves nothing"
        );
        // The summary still sees both accounts.
        let held: Decimal = holdings.iter().map(|h| h.quantity).sum();
        assert_eq!(held, dec("100"));
        assert_eq!(holdings.len(), 2);
    }

    /// A non-AUD trade's amount converts with the trade's own FX precedence
    /// (here the manual fallback rate; foreign units per 1 AUD).
    #[tokio::test]
    async fn db_non_aud_trade_amount_converted_to_aud() {
        let pool = test_pool().await;
        insert_listing_in(&pool, 1, "ICE", "USD").await;
        test_support::buy(1, 1)
            .qty(dec("10"))
            .price(dec("150"))
            .currency("USD")
            .fx_rate(dec("2"))
            .insert(&pool)
            .await;

        let events = events(&pool, 1).await;
        // 150 × 10 = 1500 USD at 2 USD/AUD → 750 AUD.
        assert_eq!(events[0].amount_aud, Some(dec("750")));
        assert_eq!(events[0].detail, "10 @ 150 USD");
    }

    /// A trade's ledger amount is what the trade adds up to — the same
    /// `net_transaction_total` the write path cross-checks a recorded
    /// `statement_total` against. So a trade whose statement total was
    /// *accepted* reports exactly that total in the ledger: a Buy's amount
    /// payable includes its costs, a Sell's net proceeds have them deducted.
    /// Both trades are AUD, so nothing converts between the two figures.
    #[tokio::test]
    async fn db_trade_amount_is_the_total_the_write_path_accepted() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        // Buy: 100 × 10 + 9.95 + 0.995 = 1010.945 payable.
        let buy_total = dec("1010.945");
        test_support::buy(1, 1)
            .qty(dec("100"))
            .price(dec("10"))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .with(|t| t.statement_total = Some(buy_total))
            .insert(&pool)
            .await;
        // Sell: 40 × 12 − 9.95 − 0.995 = 469.055 receivable.
        let sell_total = dec("469.055");
        test_support::sell(2, 1)
            .date(ymd(2024, 6, 3))
            .qty(dec("40"))
            .price(dec("12"))
            .brokerage(dec("9.95"))
            .gst_on_brokerage(dec("0.995"))
            .with(|t| t.statement_total = Some(sell_total))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("40")).await;

        let events = events(&pool, 1).await;
        let amount = |event: &str| {
            events
                .iter()
                .find(|e| e.event == event)
                .unwrap_or_else(|| panic!("{event} is in the ledger"))
                .amount_aud
        };
        assert_eq!(amount("Buy"), Some(buy_total));
        assert_eq!(amount("Sell"), Some(sell_total));
    }

    /// A trust distribution's AUD month is its *assessment* month
    /// (`Income::assessment_date`) — present entitlement, not payment — so a
    /// June-entitled distribution paid in July converts at the June rate, the
    /// same attribution the tax summary applies. The two months carry
    /// deliberately different rates so only the right one can produce the
    /// asserted figure; the ledger still *dates* the row by payment.
    #[tokio::test]
    async fn db_trust_distribution_converts_at_the_entitlement_month() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        crate::entities::rba_fx_rate::db_import_rate(&pool, "USD", "2025-06", dec("2"))
            .await
            .unwrap();
        crate::entities::rba_fx_rate::db_import_rate(&pool, "USD", "2025-07", dec("4"))
            .await
            .unwrap();
        test_support::income(1, 1, ymd(2025, 7, 15))
            .with(|i| {
                i.trust_income = true;
                i.entitlement_date = Some(ymd(2025, 6, 30));
                i.unfranked_amount = dec("100");
                i.currency = "USD".to_string();
            })
            .insert(&pool)
            .await;

        let events = events(&pool, 1).await;
        let row = events
            .iter()
            .find(|e| e.event == "Trust distribution")
            .expect("the distribution is in the ledger");
        assert_eq!(row.date, ymd(2025, 7, 15), "dated by payment");
        // 100 USD at the June rate of 2 USD/AUD → 50 AUD (the July rate of 4
        // would give 25).
        assert_eq!(row.amount_aud, Some(dec("50")));
    }

    /// Statement-ish rows all land in the ledger with their labels: return of
    /// capital, rights issue, AMMA, ESS statement, DRP enrolment, expense.
    #[tokio::test]
    async fn db_statement_rows_present_and_labelled() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAF").await;
        insert_listing(&pool, 2, "NEW").await;
        test_support::buy(1, 1).insert(&pool).await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 2, 1),
                kind: ActionKind::ReturnOfCapital {
                    amount_per_unit: dec("0.50"),
                    currency: "AUD".to_string(),
                    record_date: None,
                },
            },
        )
        .await
        .unwrap();
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 2,
                listing_id: 1,
                date: ymd(2024, 3, 1),
                kind: ActionKind::RightsIssue {
                    rights_units: dec("1"),
                    rights_held_units: dec("4"),
                    exercise_price: dec("5"),
                    currency: "AUD".to_string(),
                },
            },
        )
        .await
        .unwrap();
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 3,
                listing_id: 1,
                date: ymd(2024, 12, 1),
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: dec("2"),
                    scrip_old_units: dec("1"),
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        test_support::amma(1, 1)
            .cost_base_adjustment(dec("0.05"))
            .insert(&pool)
            .await;
        test_support::ess_statement(1, 1, ymd(2024, 9, 1))
            .insert(&pool)
            .await;
        crate::entities::drp_enrolment::db_upsert(
            &pool,
            &DrpEnrolment {
                id: 1,
                listing_id: 1,
                holding_account_id: 1,
                enrolment_date: ymd(2024, 1, 10),
                unenrolment_date: Some(ymd(2024, 10, 1)),
                residual_handling: crate::entities::drp_enrolment::ResidualHandling::CarryForward,
            },
        )
        .await
        .unwrap();
        crate::entities::investment_expense::db_upsert(
            &pool,
            &InvestmentExpense {
                id: 1,
                date_incurred: ymd(2024, 4, 1),
                expense_type: crate::entities::investment_expense::ExpenseType::ManagementFee,
                amount: dec("99"),
                gross_amount: None,
                deductible_percentage: None,
                currency: "AUD".to_string(),
                description: Some("adviser".to_string()),
                listing_id: Some(1),
                holding_account_id: None,
            },
        )
        .await
        .unwrap();

        let events = events(&pool, 1).await;
        let by_event = |name: &str| {
            events
                .iter()
                .find(|e| e.event == name)
                .unwrap_or_else(|| panic!("no {name} row"))
        };
        assert_eq!(by_event("Return of capital").detail, "0.50 AUD per unit");
        assert_eq!(
            by_event("Rights issue").detail,
            "1-for-4, exercisable at 5 AUD"
        );
        // The counterpart listing is named by ticker, never a raw id.
        assert_eq!(
            by_event("Scrip-for-scrip takeover").detail,
            "2 unit(s) of NEW per 1 held"
        );
        let amma = by_event("AMMA statement");
        assert_eq!(amma.date, ymd(2024, 6, 30));
        assert!(amma.detail.contains("FY2024"));
        assert!(
            amma.detail
                .contains("cost base adjustment 0.05 AUD per unit")
        );
        assert!(by_event("ESS statement").detail.contains("taxing point"));
        assert_eq!(
            by_event("DRP enrolment").detail,
            "residual handling: CarryForward"
        );
        assert_eq!(by_event("DRP unenrolment").date, ymd(2024, 10, 1));
        let expense = by_event("Investment expense");
        assert_eq!(expense.detail, "ManagementFee: 99 AUD — adviser");
        assert_eq!(expense.amount_aud, Some(dec("99")));
        // None of these rows move units.
        assert!(
            events
                .iter()
                .all(|e| e.event == "Buy" || e.quantity.is_none())
        );
    }

    /// A same-dated split orders before the day's trades: the trade is
    /// already in post-split units (TD 2000/10 conversion-date rule).
    #[tokio::test]
    async fn db_same_date_split_applies_before_trade() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "SPL").await;
        test_support::buy(1, 1).qty(dec("100")).insert(&pool).await; // 2024-01-01
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2024, 3, 1),
                kind: ActionKind::ShareSplit {
                    split_new_units: dec("2"),
                    split_old_units: dec("1"),
                },
            },
        )
        .await
        .unwrap();
        // A buy dated on the conversion date is post-split.
        test_support::buy(2, 1)
            .date(ymd(2024, 3, 1))
            .qty(dec("50"))
            .insert(&pool)
            .await;

        let events = events(&pool, 1).await;
        assert_eq!(events[1].event, "Share split");
        assert_eq!(events[1].units_after, dec("200"));
        assert_eq!(events[2].units_after, dec("250"));
        let (_, holdings) = db_activity(&pool, 1).await.unwrap().unwrap();
        let held: Decimal = holdings.iter().map(|h| h.quantity).sum();
        assert_eq!(events.last().unwrap().units_after, held);
    }

    // API-level tests

    async fn post_activity(pool: SqlitePool, body: serde_json::Value) -> ApiResponse {
        ApiClient::over(router().with_state(pool))
            .post("/portfolio/activity", &body)
            .await
    }

    #[tokio::test]
    async fn api_activity_with_price_values_summary() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        test_support::buy(1, 1).qty(dec("100")).insert(&pool).await;

        let resp = post_activity(pool, serde_json::json!({ "listing_id": 1, "price": "12" })).await;
        assert_eq!(resp.status, StatusCode::OK);
        let r: ActivityResponse = resp.json();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].event, "Buy");
        assert_eq!(r.holdings.len(), 1);
        assert_eq!(r.holdings[0].current_price, Some(dec("12")));
        assert_eq!(r.holdings[0].market_value, Some(dec("1200")));
    }

    /// No explicit price and no price source: the ledger still returns, and
    /// the summary degrades per holding with the reason — never a silent zero.
    #[tokio::test]
    async fn api_activity_without_price_degrades_gracefully() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        test_support::buy(1, 1).insert(&pool).await;

        let resp = post_activity(pool, serde_json::json!({ "listing_id": 1 })).await;
        assert_eq!(resp.status, StatusCode::OK);
        let r: ActivityResponse = resp.json();
        assert!(r.holdings[0].market_value.is_none());
        assert!(r.holdings[0].price_unavailable.is_some());
    }

    /// The live path values the summary from the price source when no
    /// explicit price is supplied, carrying the quote's as-of time.
    #[tokio::test]
    async fn api_activity_live_values_summary() {
        use crate::entities::closing_price::test_support::QuoteStub;
        let pool = test_pool().await;
        insert_listing(&pool, 1, "VAS").await;
        test_support::buy(1, 1).qty(dec("100")).insert(&pool).await;
        let as_of = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 7, 10, 6, 0, 0).unwrap();
        let fetcher = QuoteStub::default()
            .with_quote(1, "12.50", "AUD", as_of)
            .shared();

        let resp = ApiClient::over(router().with_state(pool).layer(axum::Extension(fetcher)))
            .post(
                "/portfolio/activity",
                &serde_json::json!({ "listing_id": 1 }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::OK);
        let r: ActivityResponse = resp.json();
        assert_eq!(r.holdings[0].market_value, Some(dec("1250.00")));
        assert!(r.holdings[0].price_as_of.is_some());
    }

    #[tokio::test]
    async fn api_activity_unknown_listing_404() {
        let pool = test_pool().await;
        let resp = post_activity(pool, serde_json::json!({ "listing_id": 42 })).await;
        assert_eq!(resp.status, StatusCode::NOT_FOUND);
    }

    /// A rename is a chronological event on the listing and belongs in the
    /// ledger, in order alongside the trades either side of it — including
    /// the exchange when a rename moves it, too.
    #[tokio::test]
    async fn db_rename_appears_as_a_chronological_ledger_event() {
        let pool = test_pool().await;
        // A second AUD market to move to: a rename onto an exchange quoting
        // another currency is refused (SCENARIOS R-01), and the trades here
        // are AUD.
        crate::entities::exchange::db_upsert(
            &pool,
            &crate::entities::exchange::Exchange {
                mic: "CXAX".to_string(),
                name: "Second AUD market".to_string(),
                country: "Australia".to_string(),
                currency: "AUD".to_string(),
                timezone: "Australia/Sydney".to_string(),
                settlement_days: 2,
                close_time: "16:00".to_string(),
            },
        )
        .await
        .unwrap();
        insert_listing(&pool, 1, "LAAC").await;
        test_support::buy(1, 1)
            .date(ymd(2024, 1, 2))
            .insert(&pool)
            .await;
        crate::entities::listing_rename::db_rename(
            &pool,
            1,
            &crate::entities::listing_rename::RenameBody {
                effective_date: ymd(2024, 6, 1),
                ticker: "LAR".to_string(),
                exchange_mic: Some("CXAX".to_string()),
                name: None,
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();
        test_support::sell(2, 1)
            .date(ymd(2024, 9, 3))
            .qty(dec("10"))
            .insert(&pool)
            .await;
        allocate(&pool, 1, 2, 1, dec("10")).await;

        let rows = events(&pool, 1).await;
        let rename_row = rows
            .iter()
            .find(|r| r.event == "Ticker/exchange change")
            .expect("rename event present");
        assert_eq!(rename_row.date, ymd(2024, 6, 1));
        assert!(
            rename_row.detail.contains("LAAC -> LAR"),
            "detail: {}",
            rename_row.detail
        );
        assert!(
            rename_row.detail.contains("XASX -> CXAX"),
            "exchange change detail: {}",
            rename_row.detail
        );

        // Chronological: Buy, rename, Sell.
        let event_names: Vec<&str> = rows.iter().map(|r| r.event.as_str()).collect();
        let buy_pos = event_names.iter().position(|e| *e == "Buy").unwrap();
        let rename_pos = event_names
            .iter()
            .position(|e| *e == "Ticker/exchange change")
            .unwrap();
        let sell_pos = event_names
            .iter()
            .position(|e| e.starts_with("Sell"))
            .unwrap();
        assert!(buy_pos < rename_pos && rename_pos < sell_pos);
    }

    async fn rename(pool: &SqlitePool, listing_id: i64, effective_date: NaiveDate, ticker: &str) {
        crate::entities::listing_rename::db_rename(
            pool,
            listing_id,
            &crate::entities::listing_rename::RenameBody {
                effective_date,
                ticker: ticker.to_string(),
                exchange_mic: None,
                name: None,
                price_symbol: None,
                note: None,
            },
        )
        .await
        .unwrap();
    }

    /// SCENARIOS R-07: the demerger row names the demerged entity at the
    /// ticker in force on the demerger date, not the one it was renamed to
    /// two years later.
    #[tokio::test]
    async fn db_demerger_counterpart_is_named_as_at_the_actions_own_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 8, "HEAD").await;
        insert_listing(&pool, 9, "SPINCO").await;
        test_support::buy(1, 8)
            .date(ymd(2023, 1, 5))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 8,
                date: ymd(2023, 10, 3),
                kind: ActionKind::Demerger {
                    demerger_listing_id: 9,
                    demerger_new_units: dec("1"),
                    demerger_held_units: dec("1"),
                    demerger_cost_base_pct: dec("30"),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();
        rename(&pool, 9, ymd(2025, 6, 1), "SPUN").await;

        let rows = events(&pool, 8).await;
        let demerger = rows
            .iter()
            .find(|r| r.event == "Demerger")
            .expect("demerger row present");
        assert_eq!(
            demerger.detail,
            "1 unit(s) of SPINCO per 1 held; 30% of cost base"
        );
    }

    /// The scrip-for-scrip counterpart resolves the same way.
    #[tokio::test]
    async fn db_scrip_counterpart_is_named_as_at_the_actions_own_date() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "ACQ").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 5))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2023, 10, 3),
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: dec("2"),
                    scrip_old_units: dec("1"),
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        rename(&pool, 2, ymd(2025, 6, 1), "ACQR").await;

        let rows = events(&pool, 1).await;
        let scrip = rows
            .iter()
            .find(|r| r.event == "Scrip-for-scrip takeover")
            .expect("scrip row present");
        assert_eq!(scrip.detail, "2 unit(s) of ACQ per 1 held");
    }

    /// A counterpart that was never renamed falls back to the current-ticker
    /// map, and an action dated after a rename reads the new ticker — the
    /// as-at resolution is a resolution, not a blanket "use the old name".
    #[tokio::test]
    async fn db_counterpart_without_a_rename_uses_the_current_ticker() {
        let pool = test_pool().await;
        insert_listing(&pool, 1, "OLD").await;
        insert_listing(&pool, 2, "NEVER").await;
        insert_listing(&pool, 3, "SPINCO").await;
        test_support::buy(1, 1)
            .date(ymd(2023, 1, 5))
            .insert(&pool)
            .await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 1,
                listing_id: 1,
                date: ymd(2023, 10, 3),
                kind: ActionKind::ScripForScrip {
                    scrip_listing_id: 2,
                    scrip_new_units: dec("2"),
                    scrip_old_units: dec("1"),
                    scrip_cash_per_unit: None,
                    scrip_market_value: None,
                    scrip_cash_currency: None,
                },
            },
        )
        .await
        .unwrap();
        // Renamed *before* this demerger, so its row reads the new ticker.
        rename(&pool, 3, ymd(2023, 6, 1), "SPUN").await;
        corporate_action::db_upsert(
            &pool,
            &corporate_action::CorporateAction {
                id: 2,
                listing_id: 1,
                date: ymd(2023, 10, 3),
                kind: ActionKind::Demerger {
                    demerger_listing_id: 3,
                    demerger_new_units: dec("1"),
                    demerger_held_units: dec("1"),
                    demerger_cost_base_pct: dec("30"),
                    demerger_close_date: None,
                    demerger_close_price: None,
                    demerger_close_sourced_from: None,
                    demerger_close_reason: None,
                },
            },
        )
        .await
        .unwrap();

        let rows = events(&pool, 1).await;
        assert_eq!(
            rows.iter()
                .find(|r| r.event == "Scrip-for-scrip takeover")
                .expect("scrip row present")
                .detail,
            "2 unit(s) of NEVER per 1 held"
        );
        assert_eq!(
            rows.iter()
                .find(|r| r.event == "Demerger")
                .expect("demerger row present")
                .detail,
            "1 unit(s) of SPUN per 1 held; 30% of cost base"
        );
    }
}
