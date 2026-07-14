//! Write-time invariant checks shared by the trade and Sell write paths
//! (so the two can never drift): the GST-inclusive brokerage split, the
//! spot-rate override, the core-figures check (incl. the pre-CGT cutoff),
//! and the statement-total cross-check — each with its 422 detail text.

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
#[derive(Debug, PartialEq)]
pub(crate) enum SpotFxRateError {
    /// Zero or negative: the rate divides the amount (AUD = foreign / rate).
    NotPositive,
    /// On an AUD trade nothing converts, so the override could never apply —
    /// accepting it would silently ignore a deliberate entry.
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
/// FX rate, a settlement before the trade date, or a pre-CGT trade date —
/// corrupts every downstream report without failing anything, so it is
/// rejected at write time. Shared by the trade and Sell write paths so the
/// two can't drift.
#[derive(Debug, PartialEq)]
pub(crate) enum AmountsError {
    /// Zero or negative quantity: a trade of nothing (or of negative units)
    /// has no meaning, and a negative parcel/sale silently skews holdings.
    QuantityNotPositive,
    /// Negative unit price (zero is legitimate — e.g. a worthless-shares
    /// closing Sell at nil proceeds).
    PriceNegative,
    /// Negative brokerage.
    BrokerageNegative,
    /// Negative GST on brokerage.
    GstNegative,
    /// Zero or negative fallback FX rate: the rate divides the amount
    /// (AUD = foreign / rate), so it can never be a real exchange rate.
    FxRateNotPositive,
    /// Settlement dated before the trade itself.
    SettlementBeforeTrade,
    /// Dated before the start of CGT (20 September 1985): a pre-CGT holding
    /// is outside CGT and not modelled — every report would wrongly compute
    /// a capital gain or loss on it (see [`CGT_START`]).
    PreCgtDate,
}

/// The core figures every trade/Sell write must satisfy, gathered for
/// [`check_amounts`]. Named fields keep the adjacent `Decimal` amounts from
/// being transposed at the call site (mirrors [`StatementTotalCheck`]).
pub(crate) struct AmountsCheck {
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    pub fx_rate: Decimal,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
}

/// Validate a trade's core figures: quantity > 0, price ≥ 0, brokerage and
/// GST ≥ 0, fx_rate > 0, settlement on or after the trade date. Brokerage is
/// checked post-GST-split (see [`resolve_brokerage`]), so a negative
/// GST-inclusive entry is caught through its negative parts.
pub(crate) fn check_amounts(c: &AmountsCheck) -> Result<(), AmountsError> {
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
    if c.fx_rate <= Decimal::ZERO {
        return Err(AmountsError::FxRateNotPositive);
    }
    if c.settlement_date < c.date {
        return Err(AmountsError::SettlementBeforeTrade);
    }
    if c.date < CGT_START {
        return Err(AmountsError::PreCgtDate);
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
        AmountsError::FxRateNotPositive => {
            "fx_rate must be a positive foreign-per-AUD rate (1 for an AUD trade)"
        }
        AmountsError::SettlementBeforeTrade => "settlement_date cannot be before the trade date",
        AmountsError::PreCgtDate => {
            "the trade is dated before 20 September 1985 — a pre-CGT holding is outside CGT \
             and not modelled, so recording it would wrongly compute a capital gain or loss"
        }
    }
}

/// Why a supplied statement total failed to reconcile (both map to 422).
#[derive(Debug, PartialEq)]
pub(crate) enum StatementTotalError {
    /// The trade and brokerage currencies differ, so no single-currency
    /// total exists to check against — supplying one is rejected rather
    /// than inventing an FX conversion.
    CurrencyMismatch,
    /// The supplied total does not equal the computed figure (carried so
    /// the rejection can say what the trade actually adds up to).
    TotalMismatch { expected: Decimal },
}

/// The figures a recorded statement total is cross-checked against, gathered
/// for [`check_statement_total`]. Named fields keep the four adjacent `Decimal`
/// amounts from being transposed at the call site.
pub(crate) struct StatementTotalCheck<'a> {
    pub statement_total: Option<Decimal>,
    pub trade_type: TradeType,
    pub quantity: Decimal,
    pub average_price: Decimal,
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    pub currency: &'a str,
    pub brokerage_currency: &'a str,
}

/// Cross-check an optionally supplied statement total against the trade's
/// own figures: quantity × price + brokerage + GST for a Buy/DRP (amount
/// payable), quantity × price − brokerage − GST for a Sell (net proceeds
/// receivable — the statement nets costs out). Contract notes print the
/// consideration rounded to the cent, so the total also passes when it
/// equals the computed figure rounded to 2 dp (half away from zero, as
/// statements round). Comparison is numeric (`Decimal` equality ignores
/// trailing zeros: 1234.50 matches 1234.5). `None` means the statement
/// total wasn't recorded — nothing to check.
pub(crate) fn check_statement_total(c: StatementTotalCheck) -> Result<(), StatementTotalError> {
    let Some(total) = c.statement_total else {
        return Ok(());
    };
    if c.currency != c.brokerage_currency {
        return Err(StatementTotalError::CurrencyMismatch);
    }
    let costs = c.brokerage + c.gst_on_brokerage;
    let expected = match c.trade_type {
        TradeType::Buy | TradeType::DRP => c.quantity * c.average_price + costs,
        TradeType::Sell => c.quantity * c.average_price - costs,
    };
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
        StatementTotalError::CurrencyMismatch => {
            "statement_total can only be checked when the trade and brokerage \
             currencies match — omit it for mixed-currency trades"
                .to_string()
        }
        StatementTotalError::TotalMismatch { expected } => {
            format!("statement_total does not reconcile: the trade computes to {expected}")
        }
    }
}
