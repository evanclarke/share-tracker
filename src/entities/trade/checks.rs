//! Write-time invariant checks shared by the trade and Sell write paths
//! (so the two can never drift): the GST-inclusive brokerage split, the
//! spot-rate override, the core-figures check (incl. the pre-CGT floor and
//! the today ceiling on the trade date), and the statement-total
//! cross-check — each with its 422 detail text.

use super::model::TradeType;
use chrono::NaiveDate;
use rust_decimal::Decimal;

/// Split a GST-inclusive brokerage amount into its (ex-GST brokerage, GST)
/// components. Australian GST is 10%, so an inclusive amount carries 1/11
/// GST; the GST is rounded to the cent (half away from zero, matching how
/// broker statements quote it) and the brokerage keeps the exact remainder,
/// so the pair always sums back to the amount actually paid.
pub(crate) fn split_gst_inclusive(amount: Decimal) -> (Decimal, Decimal) {
    let gst = (amount / Decimal::from(11))
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    (amount - gst, gst)
}

/// Resolve a request's brokerage pair: the server-side ÷11 split when the
/// amount was entered GST-inclusive (any supplied GST value is ignored —
/// deriving it is the point of the flag), or the values as entered otherwise.
pub(crate) fn resolve_brokerage(
    includes_gst: bool,
    brokerage: Decimal,
    gst_on_brokerage: Decimal,
) -> (Decimal, Decimal) {
    if includes_gst {
        split_gst_inclusive(brokerage)
    } else {
        (brokerage, gst_on_brokerage)
    }
}

/// Why a supplied spot-rate override was rejected (both map to 422).
#[derive(thiserror::Error, Debug, PartialEq)]
pub(crate) enum SpotFxRateError {
    /// Zero or negative: the rate divides the amount (AUD = foreign / rate).
    #[error("the spot FX rate must be greater than zero")]
    NotPositive,
    /// On an AUD trade nothing converts, so the override could never apply —
    /// accepting it would silently ignore a deliberate entry.
    #[error("a spot FX rate cannot apply to an AUD trade")]
    AudTrade,
}

/// Validate an optional deliberate spot-rate override against the trade's
/// currency. `None` — no override — is always fine (the unchanged default
/// conversion behaviour). Shared by the trade and Sell write paths so the two
/// can't drift.
pub(crate) fn validate_spot_fx_rate(
    currency: &str,
    spot_fx_rate: Option<Decimal>,
) -> Result<(), SpotFxRateError> {
    let Some(rate) = spot_fx_rate else {
        return Ok(());
    };
    if rate <= Decimal::ZERO {
        return Err(SpotFxRateError::NotPositive);
    }
    if currency.eq_ignore_ascii_case("AUD") {
        return Err(SpotFxRateError::AudTrade);
    }
    Ok(())
}

/// Human-readable body for a spot-rate 422 (shown by the web UI).
pub(crate) fn spot_fx_rate_detail(e: &SpotFxRateError) -> &'static str {
    match e {
        SpotFxRateError::NotPositive => "spot_fx_rate must be a positive foreign-per-AUD rate",
        SpotFxRateError::AudTrade => {
            "spot_fx_rate only applies to a non-AUD trade — an AUD amount never converts"
        }
    }
}

/// Start of CGT, 20 September 1985: an asset acquired before this date is
/// pre-CGT — outside CGT entirely — and pre-CGT holdings are not modelled
/// (docs/API.md Known limitations), so a trade dated before it is rejected
/// rather than wrongly computing a capital gain or loss on the parcel.
/// Shared with the inheritance entry path (`entities::inheritance`), whose
/// pre-CGT interactions anchor on the same date.
pub(crate) const CGT_START: NaiveDate = match NaiveDate::from_ymd_opt(1985, 9, 20) {
    Some(d) => d,
    None => unreachable!(),
};

/// Why a trade's core figures were rejected (all map to 422): a degenerate
/// value — zero/negative quantity, a negative price or cost, a non-positive
/// FX rate, a settlement before the trade date, or a trade date outside the
/// [`CGT_START`]..=today window — corrupts every downstream report without
/// failing anything, so it is rejected at write time. Shared by the trade and
/// Sell write paths so the two can't drift.
#[derive(thiserror::Error, Debug, PartialEq)]
pub(crate) enum AmountsError {
    /// Zero or negative quantity: a trade of nothing (or of negative units)
    /// has no meaning, and a negative parcel/sale silently skews holdings.
    #[error("the quantity must be greater than zero")]
    QuantityNotPositive,
    /// Negative unit price (zero is legitimate — e.g. a worthless-shares
    /// closing Sell at nil proceeds).
    #[error("the unit price cannot be negative")]
    PriceNegative,
    /// Negative brokerage.
    #[error("the brokerage cannot be negative")]
    BrokerageNegative,
    /// Negative GST on brokerage.
    #[error("the GST on brokerage cannot be negative")]
    GstNegative,
    /// The brokerage was recorded in a different currency from the trade
    /// itself. Every figure the brokerage feeds — the parcel's cost base
    /// (`domain::cost_base`), a Sell's net proceeds, the activity ledger's
    /// transaction total — is a single-currency sum of consideration and
    /// costs, so a fee in another currency would be added at the trade
    /// currency's scale and silently mis-cost the parcel. Rejected at write
    /// time rather than invented an FX conversion for, matching the
    /// statement-total cross-check's refusal to reconcile across currencies.
    #[error("the brokerage currency differs from the trade currency")]
    BrokerageCurrencyMismatch,
    /// Zero or negative fallback FX rate: the rate divides the amount
    /// (AUD = foreign / rate), so it can never be a real exchange rate.
    #[error("the fallback FX rate must be greater than zero")]
    FxRateNotPositive,
    /// Settlement dated before the trade itself.
    #[error("the settlement date is before the trade date")]
    SettlementBeforeTrade,
    /// Dated before the start of CGT (20 September 1985): a pre-CGT holding
    /// is outside CGT and not modelled — every report would wrongly compute
    /// a capital gain or loss on it (see [`CGT_START`]).
    #[error("the trade date is before the start of CGT (20 September 1985)")]
    PreCgtDate,
    /// Dated after today — [`PreCgtDate`](AmountsError::PreCgtDate)'s natural
    /// twin, one bounding the trade date below and this one above (SCENARIOS
    /// S-10). A trade records a transaction that has already happened, so a
    /// future date is a typo (a 2027-for-2026 slip on a July trade is exactly
    /// the shape this catches), and it puts a financial year that has not
    /// begun on the annual tax report's year picker
    /// (`GET /reports/tax-report/years`). The rest of the system already
    /// bounds its dated facts this way: a listing rename
    /// (`listing_rename::RenameError::FutureDated`), a closing price whose
    /// close is not final yet, and the net-capital-gain report's
    /// quiet-carry-forward year.
    ///
    /// `settlement_date` is deliberately **not** bounded: a T+2 settlement of
    /// a trade dated today is legitimately in the future.
    #[error("the trade date is after today")]
    FutureDate,
}

/// The core figures every trade/Sell write must satisfy, gathered for
/// [`check_amounts`]. Named fields keep the adjacent `Decimal` amounts from
/// being transposed at the call site (mirrors [`StatementTotalCheck`]).
pub(crate) struct AmountsCheck<'a> {
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    pub fx_rate: Decimal,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
    /// The trade's own currency, and the currency the brokerage was billed
    /// in — the pair [`AmountsError::BrokerageCurrencyMismatch`] requires to
    /// be the same.
    pub currency: &'a str,
    pub brokerage_currency: &'a str,
}

/// Validate a trade's core figures: quantity > 0, price ≥ 0, brokerage and
/// GST ≥ 0, brokerage billed in the trade's own currency, fx_rate > 0,
/// settlement on or after the trade date, and the trade date itself inside
/// [`CGT_START`]..=today. Brokerage is checked post-GST-split (see
/// [`resolve_brokerage`]), so a negative GST-inclusive entry is caught
/// through its negative parts.
///
/// The upper date bound reads the clock (`infra::date::today`) rather than
/// taking "now" as an argument, exactly as `listing_rename`'s future-dated
/// refusal does: both call sites mean the server's today and nothing else,
/// and a parameter would only offer a way for the two write paths to disagree
/// about it.
pub(crate) fn check_amounts(c: &AmountsCheck<'_>) -> Result<(), AmountsError> {
    if c.quantity <= Decimal::ZERO {
        return Err(AmountsError::QuantityNotPositive);
    }
    if c.average_price < Decimal::ZERO {
        return Err(AmountsError::PriceNegative);
    }
    if c.brokerage < Decimal::ZERO {
        return Err(AmountsError::BrokerageNegative);
    }
    if c.gst_on_brokerage < Decimal::ZERO {
        return Err(AmountsError::GstNegative);
    }
    if !c.currency.eq_ignore_ascii_case(c.brokerage_currency) {
        return Err(AmountsError::BrokerageCurrencyMismatch);
    }
    if c.fx_rate <= Decimal::ZERO {
        return Err(AmountsError::FxRateNotPositive);
    }
    if c.settlement_date < c.date {
        return Err(AmountsError::SettlementBeforeTrade);
    }
    if c.date < CGT_START {
        return Err(AmountsError::PreCgtDate);
    }
    if c.date > crate::infra::date::today() {
        return Err(AmountsError::FutureDate);
    }
    Ok(())
}

/// Human-readable body for an amounts 422 (shown by the web UI).
pub(crate) fn amounts_detail(e: &AmountsError) -> &'static str {
    match e {
        AmountsError::QuantityNotPositive => "quantity must be positive",
        AmountsError::PriceNegative => "average_price cannot be negative",
        AmountsError::BrokerageNegative => "brokerage cannot be negative",
        AmountsError::GstNegative => "gst_on_brokerage cannot be negative",
        AmountsError::BrokerageCurrencyMismatch => {
            "brokerage_currency must equal the trade's currency — a fee billed in another \
             currency has to be entered converted into the trade's currency, since the cost \
             base, net proceeds and transaction total are all single-currency sums"
        }
        AmountsError::FxRateNotPositive => {
            "fx_rate must be a positive foreign-per-AUD rate (1 for an AUD trade)"
        }
        AmountsError::SettlementBeforeTrade => "settlement_date cannot be before the trade date",
        AmountsError::PreCgtDate => {
            "the trade is dated before 20 September 1985 — a pre-CGT holding is outside CGT \
             and not modelled, so recording it would wrongly compute a capital gain or loss"
        }
        AmountsError::FutureDate => {
            "the trade is dated after today — a trade records a transaction that has already \
             happened, and a future date would offer a financial year that has not begun on \
             the annual tax report; the settlement date may still be in the future"
        }
    }
}

/// Why a supplied statement total failed to reconcile (maps to 422). A
/// mixed-currency trade can't reach here: [`check_amounts`] runs first on both
/// write paths and rejects a `brokerage_currency` differing from the trade's
/// ([`AmountsError::BrokerageCurrencyMismatch`]), so every trade this check
/// sees adds up in one currency.
#[derive(thiserror::Error, Debug, PartialEq)]
pub(crate) enum StatementTotalError {
    /// The supplied total does not equal the computed figure (carried so
    /// the rejection can say what the trade actually adds up to).
    #[error("the supplied statement total does not equal the computed {expected}")]
    TotalMismatch { expected: Decimal },
}

/// A trade's core money figures, in its own currency. Named fields keep the
/// four adjacent `Decimal` amounts from being transposed at the call site.
pub(crate) struct TradeAmounts {
    pub trade_type: TradeType,
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
}

impl TradeAmounts {
    /// What the trade adds up to in its own currency: the amount *payable* on
    /// a Buy/DRP — consideration plus the incidental costs — or the net
    /// proceeds *receivable* on a Sell, where a statement nets those costs
    /// out of the consideration instead.
    ///
    /// One definition with two readers: [`check_statement_total`] rejects a
    /// recorded `statement_total` that doesn't equal it, and the activity
    /// ledger reports it as the row's amount. So the ledger can never print a
    /// figure the write path would have refused to accept as the trade's own.
    pub(crate) fn net_transaction_total(&self) -> Decimal {
        let costs = self.brokerage + self.gst_on_brokerage;
        match self.trade_type {
            TradeType::Buy | TradeType::DRP => self.quantity * self.average_price + costs,
            TradeType::Sell => self.quantity * self.average_price - costs,
        }
    }
}

/// The figures a recorded statement total is cross-checked against, gathered
/// for [`check_statement_total`].
pub(crate) struct StatementTotalCheck {
    pub statement_total: Option<Decimal>,
    pub amounts: TradeAmounts,
}

/// Cross-check an optionally supplied statement total against what the trade
/// itself adds up to ([`TradeAmounts::net_transaction_total`]). Contract notes
/// print the consideration rounded to the cent, so the total also passes when
/// it equals the computed figure rounded to 2 dp (half away from zero, as
/// statements round). Comparison is numeric (`Decimal` equality ignores
/// trailing zeros: 1234.50 matches 1234.5). `None` means the statement
/// total wasn't recorded — nothing to check.
pub(crate) fn check_statement_total(c: StatementTotalCheck) -> Result<(), StatementTotalError> {
    let Some(total) = c.statement_total else {
        return Ok(());
    };
    let expected = c.amounts.net_transaction_total();
    let cent_rounded =
        expected.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    if total != expected && total != cent_rounded {
        return Err(StatementTotalError::TotalMismatch { expected });
    }
    Ok(())
}

/// Human-readable body for a statement-total 422 (shown by the web UI).
pub(crate) fn statement_total_detail(e: &StatementTotalError) -> String {
    match e {
        StatementTotalError::TotalMismatch { expected } => {
            format!("statement_total does not reconcile: the trade computes to {expected}")
        }
    }
}
