//! Model and wire types: [`WorthlessEvent`], [`ActionKind`], [`CorporateAction`]
//! (the row/JSON model) and [`CorporateActionBody`] (the PUT body, validated
//! into an `ActionKind` by [`CorporateActionBody::kind`]).

use crate::infra::decimal::{row_dec, row_opt_dec};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::Row;

/// Which CGT event a [`ActionKind::WorthlessShares`] action records. Both
/// produce the identical loss arithmetic (close every open parcel at nil
/// proceeds); the discriminator captures the legal basis. Serialized as its
/// variant name, the value stored in the `worthless_event` TEXT column.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WorthlessEvent {
    /// CGT event G3 (s 104-145): a liquidator/administrator declared the shares
    /// worthless and the shareholder chose to crystallise the loss.
    G3Declaration,
    /// CGT event C2 (s 104-25): the company was deregistered and the shares
    /// cancelled.
    C2Cancellation,
}

impl WorthlessEvent {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            WorthlessEvent::G3Declaration => "G3Declaration",
            WorthlessEvent::C2Cancellation => "C2Cancellation",
        }
    }

    fn from_str(s: &str) -> Result<Self, sqlx::Error> {
        match s {
            "G3Declaration" => Ok(WorthlessEvent::G3Declaration),
            "C2Cancellation" => Ok(WorthlessEvent::C2Cancellation),
            other => Err(sqlx::Error::Decode(
                format!("unknown worthless_event {other}").into(),
            )),
        }
    }
}

/// The per-type payload of a corporate action. Each variant carries exactly
/// the fields its action type needs, so a mixed or partial payload (the
/// states the table CHECKs reject) is unrepresentable once constructed.
/// Internally tagged on `action_type` and flattened into [`CorporateAction`],
/// so the JSON wire shape stays flat: the tag plus the variant's own fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action_type")]
pub enum ActionKind {
    ReturnOfCapital {
        /// Per-unit payment amount in `currency` (positive).
        amount_per_unit: Decimal,
        currency: String,
        /// Optional record date — the date entitlement to the payment was
        /// fixed, always on or before the action's own (payment) `date`. Units
        /// held *before* it earn the payment (a parcel acquired on it is
        /// ex-entitlement), the same convention a `RightsIssue`'s `date` uses.
        /// `None` falls back to testing entitlement by the payment date, which
        /// is what every row recorded before the column existed does.
        #[serde(default)]
        record_date: Option<NaiveDate>,
    },
    ShareSplit {
        /// Every `split_old_units` existing units become `split_new_units`
        /// units on the conversion date (both positive; a consolidation has
        /// new < old).
        split_new_units: Decimal,
        split_old_units: Decimal,
    },
    BonusIssue {
        /// Every `bonus_held_units` units held receive `bonus_units`
        /// additional units on the issue date (both positive; a 1-for-10
        /// bonus issue is bonus_units=1 / bonus_held_units=10).
        bonus_units: Decimal,
        bonus_held_units: Decimal,
    },
    RightsIssue {
        /// Every `rights_held_units` units held at the record date entitle
        /// the holder to acquire `rights_units` new units (both positive; a
        /// 1-for-4 rights issue is rights_units=1 / rights_held_units=4).
        rights_units: Decimal,
        rights_held_units: Decimal,
        /// Per-new-unit price paid on exercise, in `currency` (positive).
        exercise_price: Decimal,
        currency: String,
    },
    BuyBack {
        /// Per-unit buy-back price in `currency` (positive).
        buyback_price: Decimal,
        /// Per-unit dividend component of the price (non-negative, ≤ the
        /// price; 0 for a listed-company buy-back announced after
        /// 25 Oct 2022). Assessable income, excluded from capital proceeds.
        buyback_dividend: Decimal,
        /// Per-unit franking credit attached to the dividend component
        /// (non-negative; 0 when there is no dividend component).
        buyback_franking_credit: Decimal,
        /// Per-unit market value had the buy-back not been proposed
        /// (positive). Capital proceeds can't be less than it; `None` when
        /// the buy-back price is at or above market value (the price is used).
        buyback_market_value: Option<Decimal>,
        currency: String,
    },
    ScripForScrip {
        /// The replacement listing the original holding converts into (must
        /// differ from the action's own `listing_id`).
        scrip_listing_id: i64,
        /// Every `scrip_old_units` units of the original listing held at the
        /// exchange date become `scrip_new_units` units of the replacement
        /// listing (both positive).
        scrip_new_units: Decimal,
        scrip_old_units: Decimal,
        /// Optional cash component — the partial-rollover case
        /// (`docs/ato/takeovers-and-scrip-for-scrip.md` Example 27): cash
        /// received per OLD unit exchanged, in `scrip_cash_currency`
        /// (positive). The rollover applies only to the scrip portion, so
        /// the exchange apportions each parcel's cost base between cash and
        /// scrip by market value and the cash side is assessed now. The
        /// three cash fields are present together or all absent (the
        /// all-scrip full rollover) — enforced by the body validation and
        /// the table CHECKs (0007).
        #[serde(default)]
        scrip_cash_per_unit: Option<Decimal>,
        /// Market value of one NEW (replacement) unit just after issue, in
        /// `scrip_cash_currency` (positive) — the scrip side of the
        /// market-value apportionment.
        #[serde(default)]
        scrip_market_value: Option<Decimal>,
        /// Currency of the cash and market value. Its own column: the shared
        /// `currency` column stays NULL for ScripForScrip.
        #[serde(default)]
        scrip_cash_currency: Option<String>,
    },
    Demerger {
        /// The demerged entity's listing (must differ from the action's own
        /// `listing_id`, the head entity).
        demerger_listing_id: i64,
        /// Every `demerger_held_units` units of the head entity held at the
        /// demerger date receive `demerger_new_units` units of the demerged
        /// entity (both positive; BHP Billiton's 1-for-5 demerger of BHP
        /// Steel is new=1 / held=5).
        demerger_new_units: Decimal,
        demerger_held_units: Decimal,
        /// Percentage of each parcel's cost base apportioned to the new
        /// interests in the demerged entity (0 < pct < 100; the
        /// head-entity-advised step 2 percentage — e.g. 5.063 for BHP
        /// Steel). The head parcels keep the remaining `100 − pct` percent.
        demerger_cost_base_pct: Decimal,
        /// The last **pre-demerger** trading day (strictly before the
        /// action's own `date`), and what the security **actually closed at**
        /// on it, in the listing's quote currency — the stated fact the
        /// demerger's *price* factor is derived from. A demerger changes no
        /// unit count on this listing, but the price provider restates its
        /// whole pre-demerger series by a spin-off adjustment factor all the
        /// same, so without this every stored pre-demerger close is the
        /// current adjusted level (`entities::closing_price`,
        /// [`PriceBasisEvent`](super::adjustments::PriceBasisEvent)).
        ///
        /// Optional: a demerger whose head listing has no fetched
        /// pre-demerger prices needs none, and an action recorded before this
        /// existed stays editable without one. All four fields are present
        /// together or all absent (the all-or-none shape `scrip_cash_*`
        /// already uses), CHECK-enforced by 0036.
        #[serde(default)]
        demerger_close_date: Option<NaiveDate>,
        #[serde(default)]
        demerger_close_price: Option<Decimal>,
        /// Where the stated close was taken from, and why it had to be
        /// stated — the same provenance a hand-entered closing price carries
        /// (`PUT /closing_prices/:listing_id/:price_date`). Informational:
        /// the arithmetic reads only the date and the close, and these are
        /// the audit record of the entry.
        #[serde(default)]
        demerger_close_sourced_from: Option<String>,
        #[serde(default)]
        demerger_close_reason: Option<String>,
    },
    WorthlessShares {
        /// Which CGT event the loss is recognised under (see [`WorthlessEvent`]).
        worthless_event: WorthlessEvent,
    },
}

impl ActionKind {
    /// The `action_type` column value (the serde tag).
    pub(crate) fn type_str(&self) -> &'static str {
        match self {
            ActionKind::ReturnOfCapital { .. } => "ReturnOfCapital",
            ActionKind::ShareSplit { .. } => "ShareSplit",
            ActionKind::BonusIssue { .. } => "BonusIssue",
            ActionKind::RightsIssue { .. } => "RightsIssue",
            ActionKind::BuyBack { .. } => "BuyBack",
            ActionKind::ScripForScrip { .. } => "ScripForScrip",
            ActionKind::Demerger { .. } => "Demerger",
            ActionKind::WorthlessShares { .. } => "WorthlessShares",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct CorporateAction {
    pub id: i64,
    pub listing_id: i64,
    /// ReturnOfCapital: payment date — parcels acquired on/before it are
    /// affected, unless the action carries a `record_date`, which then decides
    /// entitlement instead. ShareSplit: conversion date — parcels acquired before it are
    /// converted (a trade dated on the conversion date is already in
    /// post-split units). BonusIssue: issue date — parcels acquired before it
    /// receive bonus units (a trade dated on the issue date is ex-bonus).
    /// RightsIssue: record date — units held before it earn the entitlement
    /// (a trade dated on it is ex-rights); exercises are dated on/after it.
    /// ScripForScrip: exchange date — every parcel still open on it is
    /// exchanged; the closing Sell and replacement Buys are dated on it.
    /// Demerger: demerger date — every head parcel still open on it
    /// participates; the closing Sell and the head/demerged Buys are dated
    /// on it.
    pub date: NaiveDate,
    #[serde(flatten)]
    #[sqlx(flatten)]
    pub kind: ActionKind,
}

/// The one `FromRow` a derive cannot express: which payload columns the row carries
/// depends on its `action_type` tag, so the variant has to be chosen before the
/// columns are read. The decimal columns still decode through
/// [`infra::decimal::Money`](crate::infra::decimal::Money) (via `row_dec`/`row_opt_dec`),
/// so a malformed value is a column-named error here too.
impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for ActionKind {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        match row.try_get::<String, _>("action_type")?.as_str() {
            "ReturnOfCapital" => Ok(ActionKind::ReturnOfCapital {
                amount_per_unit: row_dec(row, "amount_per_unit")?,
                currency: row.try_get("currency")?,
                record_date: row.try_get("record_date")?,
            }),
            "ShareSplit" => Ok(ActionKind::ShareSplit {
                split_new_units: row_dec(row, "split_new_units")?,
                split_old_units: row_dec(row, "split_old_units")?,
            }),
            "BonusIssue" => Ok(ActionKind::BonusIssue {
                bonus_units: row_dec(row, "bonus_units")?,
                bonus_held_units: row_dec(row, "bonus_held_units")?,
            }),
            "RightsIssue" => Ok(ActionKind::RightsIssue {
                rights_units: row_dec(row, "rights_units")?,
                rights_held_units: row_dec(row, "rights_held_units")?,
                exercise_price: row_dec(row, "exercise_price")?,
                currency: row.try_get("currency")?,
            }),
            "BuyBack" => Ok(ActionKind::BuyBack {
                buyback_price: row_dec(row, "buyback_price")?,
                buyback_dividend: row_dec(row, "buyback_dividend")?,
                buyback_franking_credit: row_dec(row, "buyback_franking_credit")?,
                buyback_market_value: row_opt_dec(row, "buyback_market_value")?,
                currency: row.try_get("currency")?,
            }),
            "ScripForScrip" => Ok(ActionKind::ScripForScrip {
                scrip_listing_id: row.try_get("scrip_listing_id")?,
                scrip_new_units: row_dec(row, "scrip_new_units")?,
                scrip_old_units: row_dec(row, "scrip_old_units")?,
                scrip_cash_per_unit: row_opt_dec(row, "scrip_cash_per_unit")?,
                scrip_market_value: row_opt_dec(row, "scrip_market_value")?,
                scrip_cash_currency: row.try_get("scrip_cash_currency")?,
            }),
            "Demerger" => Ok(ActionKind::Demerger {
                demerger_listing_id: row.try_get("demerger_listing_id")?,
                demerger_new_units: row_dec(row, "demerger_new_units")?,
                demerger_held_units: row_dec(row, "demerger_held_units")?,
                demerger_cost_base_pct: row_dec(row, "demerger_cost_base_pct")?,
                demerger_close_date: row.try_get("demerger_close_date")?,
                demerger_close_price: row_opt_dec(row, "demerger_close_price")?,
                demerger_close_sourced_from: row.try_get("demerger_close_sourced_from")?,
                demerger_close_reason: row.try_get("demerger_close_reason")?,
            }),
            "WorthlessShares" => Ok(ActionKind::WorthlessShares {
                worthless_event: WorthlessEvent::from_str(row.try_get("worthless_event")?)?,
            }),
            other => Err(sqlx::Error::Decode(
                format!("unknown corporate action_type {other}").into(),
            )),
        }
    }
}

/// The PUT body's `action_type` tag. Deserialized separately from the
/// payload fields (unlike [`ActionKind`]) so a stray cross-type field is
/// *rejected*, not silently dropped as serde's tagged-enum decoding would.
#[derive(Debug, Clone, Copy, Deserialize)]
enum ActionType {
    ReturnOfCapital,
    ShareSplit,
    BonusIssue,
    RightsIssue,
    BuyBack,
    ScripForScrip,
    Demerger,
    WorthlessShares,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorporateActionBody {
    action_type: ActionType,
    pub listing_id: i64,
    pub date: NaiveDate,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    amount_per_unit: Option<Decimal>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    record_date: Option<NaiveDate>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    split_new_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    split_old_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    bonus_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    bonus_held_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    rights_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    rights_held_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    exercise_price: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_price: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_dividend: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_franking_credit: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_market_value: Option<Decimal>,
    #[serde(default)]
    scrip_listing_id: Option<i64>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_new_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_old_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_cash_per_unit: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_market_value: Option<Decimal>,
    #[serde(default)]
    scrip_cash_currency: Option<String>,
    #[serde(default)]
    demerger_listing_id: Option<i64>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_new_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_held_units: Option<Decimal>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_cost_base_pct: Option<Decimal>,
    #[serde(default)]
    demerger_close_date: Option<NaiveDate>,
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_close_price: Option<Decimal>,
    #[serde(default)]
    demerger_close_sourced_from: Option<String>,
    #[serde(default)]
    demerger_close_reason: Option<String>,
    #[serde(default)]
    worthless_event: Option<WorthlessEvent>,
}

impl CorporateActionBody {
    /// Each action type carries exactly its own payload (mirrors the table
    /// CHECKs, plus positivity): ReturnOfCapital needs a positive payment and
    /// a currency, and may carry a `record_date` no later than the payment
    /// date it fixes entitlement for (a later one is rejected, not ignored —
    /// it would read as an entitlement fixed after the money was paid);
    /// ShareSplit a positive conversion ratio; BonusIssue a
    /// positive bonus ratio; RightsIssue a positive entitlement ratio,
    /// exercise price, and a currency; BuyBack a positive per-unit price and
    /// a currency (dividend/franking-credit components default to 0; the
    /// dividend may not exceed the price — it is part of it — and a credit
    /// needs a dividend to attach to; market value, when given, is positive)
    /// — each with every other type's fields absent (`None` otherwise;
    /// `currency` is shared by ReturnOfCapital, RightsIssue, and BuyBack but
    /// forbidden for the ratio-only types). A zero/negative payment would
    /// silently *increase* cost bases; a zero/negative ratio would zero out
    /// or invert holdings or entitlements. ScripForScrip needs a positive
    /// exchange ratio and a replacement listing different from the original —
    /// exchanging a listing into itself would consume its parcels and
    /// recreate them in place — and may carry a cash component: per-old-unit
    /// cash, the replacement unit's market value, and their currency, all
    /// three present (cash and market value positive) or all absent.
    /// Demerger needs a positive entitlement ratio, a
    /// demerged listing different from the head, and a cost-base percentage
    /// strictly between 0 and 100 — 0 or 100 would zero out one side's cost
    /// base entirely, and anything outside would make one side negative.
    /// WorthlessShares needs only the `worthless_event` discriminator (the CGT
    /// event basis), with every numeric/listing payload absent.
    pub(crate) fn kind(self) -> Option<ActionKind> {
        let payment = self.amount_per_unit.is_some();
        let split = self.split_new_units.is_some() || self.split_old_units.is_some();
        let bonus = self.bonus_units.is_some() || self.bonus_held_units.is_some();
        let rights = self.rights_units.is_some()
            || self.rights_held_units.is_some()
            || self.exercise_price.is_some();
        let buyback = self.buyback_price.is_some()
            || self.buyback_dividend.is_some()
            || self.buyback_franking_credit.is_some()
            || self.buyback_market_value.is_some();
        let scrip = self.scrip_listing_id.is_some()
            || self.scrip_new_units.is_some()
            || self.scrip_old_units.is_some()
            || self.scrip_cash_per_unit.is_some()
            || self.scrip_market_value.is_some()
            || self.scrip_cash_currency.is_some();
        let demerger = self.demerger_listing_id.is_some()
            || self.demerger_new_units.is_some()
            || self.demerger_held_units.is_some()
            || self.demerger_cost_base_pct.is_some()
            // The stated close is part of the Demerger payload, so folding it
            // in here is what makes every *other* type's `!demerger` guard
            // reject a stray one — no arm needs its own test.
            || self.demerger_close_date.is_some()
            || self.demerger_close_price.is_some()
            || self.demerger_close_sourced_from.is_some()
            || self.demerger_close_reason.is_some();
        let worthless = self.worthless_event.is_some();
        // Entitlement can never be fixed after the payment it entitles the
        // holder to, so a record date past the payment date is rejected rather
        // than silently ignored (CHECK-enforced too, 0023).
        let record_date = match self.record_date {
            Some(rd) if rd > self.date => return None,
            other => other,
        };
        let record = record_date.is_some();
        let positive = |d: Option<Decimal>| d.filter(|v| *v > Decimal::ZERO);
        // Provenance text that is present but blank records nothing, so it is
        // refused rather than stored — the rule `PUT /closing_prices/…` already
        // applies to a manual price's own `sourced_from`/`reason`.
        let non_blank = |s: String| {
            let trimmed = s.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        };
        match self.action_type {
            ActionType::ReturnOfCapital
                if !split && !bonus && !rights && !buyback && !scrip && !demerger && !worthless =>
            {
                Some(ActionKind::ReturnOfCapital {
                    amount_per_unit: positive(self.amount_per_unit)?,
                    currency: self.currency?,
                    record_date,
                })
            }
            ActionType::ShareSplit
                if !payment
                    && !record
                    && !bonus
                    && !rights
                    && !buyback
                    && !scrip
                    && !demerger
                    && !worthless
                    && self.currency.is_none() =>
            {
                Some(ActionKind::ShareSplit {
                    split_new_units: positive(self.split_new_units)?,
                    split_old_units: positive(self.split_old_units)?,
                })
            }
            ActionType::BonusIssue
                if !payment
                    && !record
                    && !split
                    && !rights
                    && !buyback
                    && !scrip
                    && !demerger
                    && !worthless
                    && self.currency.is_none() =>
            {
                Some(ActionKind::BonusIssue {
                    bonus_units: positive(self.bonus_units)?,
                    bonus_held_units: positive(self.bonus_held_units)?,
                })
            }
            ActionType::RightsIssue
                if !payment
                    && !record
                    && !split
                    && !bonus
                    && !buyback
                    && !scrip
                    && !demerger
                    && !worthless =>
            {
                Some(ActionKind::RightsIssue {
                    rights_units: positive(self.rights_units)?,
                    rights_held_units: positive(self.rights_held_units)?,
                    exercise_price: positive(self.exercise_price)?,
                    currency: self.currency?,
                })
            }
            ActionType::ScripForScrip
                if !payment
                    && !record
                    && !split
                    && !bonus
                    && !rights
                    && !buyback
                    && !demerger
                    && !worthless
                    && self.currency.is_none() =>
            {
                let scrip_listing_id = self.scrip_listing_id.filter(|&l| l != self.listing_id)?;
                // The cash component is all-or-none: cash per old unit, the
                // replacement unit's market value (the apportionment needs
                // both sides of the consideration), and their currency. A
                // partial set would make the market-value apportionment
                // undefined; zero/negative amounts would zero or invert it.
                let (scrip_cash_per_unit, scrip_market_value, scrip_cash_currency) = match (
                    self.scrip_cash_per_unit,
                    self.scrip_market_value,
                    self.scrip_cash_currency,
                ) {
                    (None, None, None) => (None, None, None),
                    (Some(cash), Some(mv), Some(currency)) => (
                        Some(positive(Some(cash))?),
                        Some(positive(Some(mv))?),
                        Some(currency),
                    ),
                    _ => return None,
                };
                Some(ActionKind::ScripForScrip {
                    scrip_listing_id,
                    scrip_new_units: positive(self.scrip_new_units)?,
                    scrip_old_units: positive(self.scrip_old_units)?,
                    scrip_cash_per_unit,
                    scrip_market_value,
                    scrip_cash_currency,
                })
            }
            ActionType::Demerger
                if !payment
                    && !record
                    && !split
                    && !bonus
                    && !rights
                    && !buyback
                    && !scrip
                    && !worthless
                    && self.currency.is_none() =>
            {
                let demerger_listing_id =
                    self.demerger_listing_id.filter(|&l| l != self.listing_id)?;
                let demerger_cost_base_pct = self
                    .demerger_cost_base_pct
                    .filter(|p| *p > Decimal::ZERO && *p < Decimal::ONE_HUNDRED)?;
                // The stated pre-demerger close is all-or-none: the day, the
                // close, and the provenance of where it came from and why it
                // was needed. A partial set would leave a factor with only one
                // side stated, or a figure with nothing recording its source —
                // the provenance a hand-entered closing price is required to
                // carry, for exactly the same reason (it is a figure a person
                // asserted, not one the provider served). The day is the last
                // *pre*-demerger trading day, so it is strictly before the
                // demerger date: a close on or after it is already in the
                // post-demerger basis and its factor would be meaningless.
                let (
                    demerger_close_date,
                    demerger_close_price,
                    demerger_close_sourced_from,
                    demerger_close_reason,
                ) = match (
                    self.demerger_close_date,
                    self.demerger_close_price,
                    self.demerger_close_sourced_from,
                    self.demerger_close_reason,
                ) {
                    (None, None, None, None) => (None, None, None, None),
                    (Some(close_date), Some(price), Some(sourced_from), Some(reason)) => {
                        if close_date >= self.date {
                            return None;
                        }
                        (
                            Some(close_date),
                            Some(positive(Some(price))?),
                            Some(non_blank(sourced_from)?),
                            Some(non_blank(reason)?),
                        )
                    }
                    _ => return None,
                };
                Some(ActionKind::Demerger {
                    demerger_listing_id,
                    demerger_new_units: positive(self.demerger_new_units)?,
                    demerger_held_units: positive(self.demerger_held_units)?,
                    demerger_cost_base_pct,
                    demerger_close_date,
                    demerger_close_price,
                    demerger_close_sourced_from,
                    demerger_close_reason,
                })
            }
            ActionType::BuyBack
                if !payment
                    && !record
                    && !split
                    && !bonus
                    && !rights
                    && !scrip
                    && !demerger
                    && !worthless =>
            {
                let buyback_price = positive(self.buyback_price)?;
                let buyback_dividend = self.buyback_dividend.unwrap_or(Decimal::ZERO);
                if buyback_dividend < Decimal::ZERO || buyback_dividend > buyback_price {
                    return None;
                }
                let buyback_franking_credit = self.buyback_franking_credit.unwrap_or(Decimal::ZERO);
                if buyback_franking_credit < Decimal::ZERO
                    || (buyback_franking_credit > Decimal::ZERO
                        && buyback_dividend == Decimal::ZERO)
                {
                    return None;
                }
                // The maximum a company could attach to the dividend is
                // checked where the income row is actually formed — the
                // participation, which knows the units these per-unit figures
                // are multiplied by (`entities::buyback_participation`,
                // SCENARIOS G-25). A per-unit ceiling can't do that job: the
                // cent of rounding slack it would need is proportionally
                // enormous against a per-unit figure and scales up with the
                // units, so a pair passing here could still make an
                // over-credited row.
                let buyback_market_value = match self.buyback_market_value {
                    Some(mv) => Some(positive(Some(mv))?),
                    None => None,
                };
                Some(ActionKind::BuyBack {
                    buyback_price,
                    buyback_dividend,
                    buyback_franking_credit,
                    buyback_market_value,
                    currency: self.currency?,
                })
            }
            ActionType::WorthlessShares
                if !payment
                    && !record
                    && !split
                    && !bonus
                    && !rights
                    && !buyback
                    && !scrip
                    && !demerger
                    && self.currency.is_none() =>
            {
                Some(ActionKind::WorthlessShares {
                    worthless_event: self.worthless_event?,
                })
            }
            _ => None,
        }
    }
}
