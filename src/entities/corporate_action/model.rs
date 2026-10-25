//! Model and wire types: [`WorthlessEvent`], [`ActionKind`], [`CorporateAction`]
//! (the row/JSON model) and [`CorporateActionBody`] (the PUT body, validated
//! into an `ActionKind` by [`CorporateActionBody::kind`]).

use crate::domain::entitlement_rounding::Rounding;
use crate::infra::currency::CurrencyCode;
use crate::infra::decimal::{row_dec, row_opt_dec};
use crate::infra::id::ListingId;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use std::str::FromStr;

/// Which CGT event a [`ActionKind::WorthlessShares`] action records. Both
/// produce the identical loss arithmetic (close every open parcel at nil
/// proceeds); the discriminator captures the legal basis. Serialized as its
/// variant name, the value stored in the `worthless_event` TEXT column.
#[derive(utoipa::ToSchema, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
}

/// A `worthless_event` value naming neither CGT event.
#[derive(thiserror::Error, Debug, PartialEq)]
#[error("unknown worthless_event {0}")]
pub struct UnknownWorthlessEvent(String);

/// The inverse of [`WorthlessEvent::as_str`]: the stored variant name.
impl FromStr for WorthlessEvent {
    type Err = UnknownWorthlessEvent;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "G3Declaration" => Ok(WorthlessEvent::G3Declaration),
            "C2Cancellation" => Ok(WorthlessEvent::C2Cancellation),
            other => Err(UnknownWorthlessEvent(other.to_owned())),
        }
    }
}

/// How the registry treats the fractional entitlement a ratio-driven action
/// (`ShareSplit`, `BonusIssue`, `ScripForScrip`, `Demerger`) produces where
/// its ratio does not divide a holding. The rule is the **issuer's**: ASX
/// Listing Rule 7.20 has it disclose its treatment, and it varies — rounded
/// up, rounded down with no cash, or the fractions aggregated, sold and paid
/// out as cash in lieu. It is applied once per holding, never per parcel.
/// TD 2000/10 / s 112-25: rounding is no CGT event (the cost base spreads
/// over the resulting units); cash in lieu disposes of the fraction.
/// Serialized as its variant name, the value stored in the `fraction_rule`
/// TEXT column.
///
/// A non-`Exact` rule is applied once per holding onto the holding's unit grid
/// (`domain::entitlement_rounding`), and each parcel's resulting count is
/// stored (`corporate_action_parcel_counts`); a holding with no grid (a NULL
/// unit scale) keeps the exact count (`docs/API.md` *Fractional
/// entitlements*).
#[derive(utoipa::ToSchema, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FractionRule {
    /// The registry holds the exact fractional count the ratio produces —
    /// what every action recorded before the rule existed means.
    Exact,
    /// Rounded down to the holding's unit grid; the fraction is forfeited
    /// with no cash.
    Down,
    /// Rounded up to the holding's unit grid.
    Up,
    /// Rounded to the nearest point on the holding's unit grid.
    Nearest,
    /// Rounded down, with the aggregated fractions sold and the proceeds paid
    /// out — a disposal of the fraction (a Sell dated the payment date).
    CashInLieu,
}

impl FractionRule {
    /// The rounding the rule applies to a holding on a unit grid, or `None`
    /// for `Exact` (no rounding at all).
    pub(crate) fn rounding(self) -> Option<Rounding> {
        match self {
            FractionRule::Exact => None,
            FractionRule::Down => Some(Rounding::Down),
            FractionRule::Up => Some(Rounding::Up),
            FractionRule::Nearest => Some(Rounding::Nearest),
            FractionRule::CashInLieu => Some(Rounding::CashInLieu),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            FractionRule::Exact => "Exact",
            FractionRule::Down => "Down",
            FractionRule::Up => "Up",
            FractionRule::Nearest => "Nearest",
            FractionRule::CashInLieu => "CashInLieu",
        }
    }
}

/// A `fraction_rule` value naming none of the rules.
#[derive(thiserror::Error, Debug, PartialEq)]
#[error("unknown fraction_rule {0}")]
pub struct UnknownFractionRule(String);

/// The inverse of [`FractionRule::as_str`]: the stored variant name.
impl FromStr for FractionRule {
    type Err = UnknownFractionRule;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Exact" => Ok(FractionRule::Exact),
            "Down" => Ok(FractionRule::Down),
            "Up" => Ok(FractionRule::Up),
            "Nearest" => Ok(FractionRule::Nearest),
            "CashInLieu" => Ok(FractionRule::CashInLieu),
            other => Err(UnknownFractionRule(other.to_owned())),
        }
    }
}

/// The per-type payload of a corporate action. Each variant carries exactly
/// the fields its action type needs, so a mixed or partial payload (the
/// states the table CHECKs reject) is unrepresentable once constructed.
/// Internally tagged on `action_type` and flattened into [`CorporateAction`],
/// so the JSON wire shape stays flat: the tag plus the variant's own fields.
#[derive(utoipa::ToSchema, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action_type")]
pub enum ActionKind {
    ReturnOfCapital {
        /// Per-unit payment amount in `currency` (positive).
        amount_per_unit: Decimal,
        currency: CurrencyCode,
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
        /// How the registry treats a fractional entitlement (see
        /// [`FractionRule`]).
        fraction_rule: FractionRule,
        /// Optional record date — when the register closed to fix who holds
        /// the units the action converts. Read only under a non-`Exact`
        /// [`FractionRule`]: the holding rounded is the units **registered**
        /// at it (Buys settled on or before it, Sells settled after it),
        /// and a trade straddling it and the action's `date` is converted
        /// as its own settlement obligation (`docs/API.md` *Fractional
        /// entitlements*). It may fall on either side of `date` (after it
        /// on the ASX, before it for a NYSE forward split). `None` reads the
        /// register the market timetable leaves: every trade dated before
        /// `date`. Informational on an `Exact` action.
        #[serde(default)]
        record_date: Option<NaiveDate>,
    },
    BonusIssue {
        /// Every `bonus_held_units` units held receive `bonus_units`
        /// additional units on the issue date (both positive; a 1-for-10
        /// bonus issue is `bonus_units=1` / `bonus_held_units=10`).
        bonus_units: Decimal,
        bonus_held_units: Decimal,
        /// How the registry treats a fractional entitlement (see
        /// [`FractionRule`]).
        fraction_rule: FractionRule,
        /// Optional record date — when the register closed to fix who holds
        /// the units the action converts. Read only under a non-`Exact`
        /// [`FractionRule`]: the holding rounded is the units **registered**
        /// at it (Buys settled on or before it, Sells settled after it),
        /// and a trade straddling it and the action's `date` is converted
        /// as its own settlement obligation (`docs/API.md` *Fractional
        /// entitlements*). It may fall on either side of `date` (after it
        /// on the ASX, before it for a NYSE forward split). `None` reads the
        /// register the market timetable leaves: every trade dated before
        /// `date`. Informational on an `Exact` action.
        #[serde(default)]
        record_date: Option<NaiveDate>,
    },
    RightsIssue {
        /// Every `rights_held_units` units held at the record date entitle
        /// the holder to acquire `rights_units` new units (both positive; a
        /// 1-for-4 rights issue is `rights_units=1` / `rights_held_units=4`).
        rights_units: Decimal,
        rights_held_units: Decimal,
        /// Per-new-unit price paid on exercise, in `currency` (positive).
        exercise_price: Decimal,
        currency: CurrencyCode,
        /// Whether the offer was **renounceable** — the rights can be taken
        /// up, left to lapse, *or traded in the market*. Where they cannot be
        /// traded, transferred or assigned, the offer is non-renounceable
        /// (`docs/ato/retail-premiums.md`; TR 2012/1 para 2).
        ///
        /// It decides how a **retail premium** — the payment made to a holder
        /// who did not or could not take the entitlement up — is taxed, and
        /// nothing else about the action: exercising is identical either way.
        /// Renounceable, the premium is capital proceeds on the rights (TR
        /// 2017/4), which is the sell-rights operation; non-renounceable, it
        /// is an unfranked dividend (TR 2012/1) and belongs on the income
        /// path, so `sell_rights` refuses proceeds on such an offer
        /// (`entities::rights_sale`).
        renounceable: bool,
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
        currency: CurrencyCode,
    },
    ScripForScrip {
        /// The replacement listing the original holding converts into (must
        /// differ from the action's own `listing_id`).
        scrip_listing_id: ListingId,
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
        /// `currency` column stays NULL for `ScripForScrip`.
        #[serde(default)]
        scrip_cash_currency: Option<CurrencyCode>,
        /// How the registry treats a fractional entitlement (see
        /// [`FractionRule`]).
        fraction_rule: FractionRule,
    },
    Demerger {
        /// The demerged entity's listing (must differ from the action's own
        /// `listing_id`, the head entity).
        demerger_listing_id: ListingId,
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
        /// [`PriceBasisEvent`](crate::domain::adjustments::PriceBasisEvent)).
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
        /// How the registry treats a fractional entitlement (see
        /// [`FractionRule`]).
        fraction_rule: FractionRule,
    },
    WorthlessShares {
        /// Which CGT event the loss is recognised under (see [`WorthlessEvent`]).
        worthless_event: WorthlessEvent,
    },
}

/// The one fact behind both rights operations' refusal of an amount paid to
/// acquire the rights, stated once so the two read as one rule: a
/// non-renounceable offer's entitlements "cannot be traded, transferred,
/// assigned or otherwise dealt with" (TR 2012/1 para 2 —
/// `docs/ato/retail-premiums.md`), and the company grants them, so no one can
/// have bought one. The amount is impossible in fact, not merely unusual, and
/// it stays impossible for the offers TR 2012/1's para 3 puts outside itself
/// (entitlements over trust or stapled-group equity): the Ruling declines to
/// characterise the *payment* there, but a non-renounceable entitlement is
/// non-tradeable either way.
///
/// `sell_rights` and `exercise` each answer `422` with this clause followed by
/// what the amount would have done to their own figures (SCENARIOS AA-b).
pub const NOTHING_PAID_FOR_NON_RENOUNCEABLE_RIGHTS: &str = "this rights issue is a non-renounceable offer, whose entitlements cannot be traded, \
     transferred or assigned — so nothing can have been paid to acquire the rights it issued";

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

#[derive(utoipa::ToSchema, Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct CorporateAction {
    pub id: i64,
    pub listing_id: ListingId,
    /// `ReturnOfCapital`: payment date — parcels settled (registered) on/before
    /// it are affected, unless the action carries a `record_date`, which then
    /// decides entitlement instead (settled on/before the record date). `ShareSplit`: conversion date — parcels acquired before it are
    /// converted (a trade dated on the conversion date is already in
    /// post-split units). `BonusIssue`: issue date — parcels acquired before it
    /// receive bonus units (a trade dated on the issue date is ex-bonus).
    /// `RightsIssue`: record date — units on the register at it earn the
    /// entitlement (a buy settled on/before it, less sales settled on/before
    /// it, so a trade on the ex-date is ex-rights to the buyer and cum-rights
    /// to the seller); exercises are dated on/after it.
    /// `ScripForScrip`: exchange date — every parcel still open on it is
    /// exchanged; the closing Sell and replacement Buys are dated on it.
    /// Demerger: demerger date — every head parcel still open on it
    /// participates; the closing Sell and the head/demerged Buys are dated
    /// on it.
    pub date: NaiveDate,
    #[serde(flatten)]
    #[sqlx(flatten)]
    pub kind: ActionKind,
}

/// A ratio-driven row's `fraction_rule`. No write path can leave it NULL on
/// one of those types — the PUT body must state it and `db_upsert` always
/// binds it, and 0054 backfilled every stored row to `Exact` — so the
/// fallback is only reachable by a row hand-inserted with raw SQL. It reads as
/// `Exact`: what every row recorded before the column existed meant, and what
/// 0054 backfilled them to. An unknown value is a decode error naming it.
fn fraction_rule(row: &SqliteRow) -> Result<FractionRule, sqlx::Error> {
    row.try_get::<Option<&str>, _>("fraction_rule")?
        .map_or(Ok(FractionRule::Exact), |rule| {
            rule.parse::<FractionRule>()
                .map_err(|e| sqlx::Error::Decode(Box::new(e)))
        })
}

/// The one `FromRow` a derive cannot express: which payload columns the row carries
/// depends on its `action_type` tag, so the variant has to be chosen before the
/// columns are read. The decimal columns still decode through
/// [`infra::decimal::Money`](crate::infra::decimal::Money) (via `row_dec`/`row_opt_dec`),
/// so a malformed value is a column-named error here too.
impl<'r> sqlx::FromRow<'r, SqliteRow> for ActionKind {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        match row.try_get::<String, _>("action_type")?.as_str() {
            "ReturnOfCapital" => Ok(ActionKind::ReturnOfCapital {
                amount_per_unit: row_dec(row, "amount_per_unit")?,
                currency: row.try_get("currency")?,
                record_date: row.try_get("record_date")?,
            }),
            "ShareSplit" => Ok(ActionKind::ShareSplit {
                split_new_units: row_dec(row, "split_new_units")?,
                split_old_units: row_dec(row, "split_old_units")?,
                fraction_rule: fraction_rule(row)?,
                record_date: row.try_get("rebase_record_date")?,
            }),
            "BonusIssue" => Ok(ActionKind::BonusIssue {
                bonus_units: row_dec(row, "bonus_units")?,
                bonus_held_units: row_dec(row, "bonus_held_units")?,
                fraction_rule: fraction_rule(row)?,
                record_date: row.try_get("rebase_record_date")?,
            }),
            "RightsIssue" => Ok(ActionKind::RightsIssue {
                rights_units: row_dec(row, "rights_units")?,
                rights_held_units: row_dec(row, "rights_held_units")?,
                exercise_price: row_dec(row, "exercise_price")?,
                currency: row.try_get("currency")?,
                // No write path can leave this NULL — the PUT body must state
                // it and `db_upsert` always binds it, and 0047 backfilled
                // every stored row — so the fallback is only reachable by a
                // row hand-inserted with raw SQL. It reads as renounceable:
                // what every row recorded before the column existed meant, and
                // what 0047 backfilled them to.
                renounceable: row
                    .try_get::<Option<bool>, _>("renounceable")?
                    .unwrap_or(true),
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
                fraction_rule: fraction_rule(row)?,
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
                fraction_rule: fraction_rule(row)?,
            }),
            "WorthlessShares" => Ok(ActionKind::WorthlessShares {
                worthless_event: row
                    .try_get::<&str, _>("worthless_event")?
                    .parse::<WorthlessEvent>()
                    .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
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
#[derive(utoipa::ToSchema, Debug, Clone, Copy, Deserialize)]
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

impl ActionType {
    /// The four ratio-driven types — whose ratio can produce a fractional
    /// entitlement, and so the only ones that carry a [`FractionRule`].
    fn is_ratio_driven(self) -> bool {
        matches!(
            self,
            ActionType::ShareSplit
                | ActionType::BonusIssue
                | ActionType::ScripForScrip
                | ActionType::Demerger
        )
    }
}

/// The `422` body for a ratio-driven action submitted without its
/// `fraction_rule`.
pub const FRACTION_RULE_REQUIRED: &str = "fraction_rule is required for a ShareSplit, BonusIssue, \
     ScripForScrip or Demerger — how the registry treats a fractional entitlement: Exact, Down, \
     Up, Nearest or CashInLieu";

/// The `422` body for a `fraction_rule` on an action that has no ratio to
/// produce a fraction.
pub const FRACTION_RULE_NOT_APPLICABLE: &str = "fraction_rule applies only to a ShareSplit, \
     BonusIssue, ScripForScrip or Demerger — this action type has no ratio to produce a fraction";

#[derive(utoipa::ToSchema, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorporateActionBody {
    /// The action type; the row must carry exactly that type's payload fields — a field
    /// belonging to another type is refused.
    action_type: ActionType,
    /// The listing the action applies to (the original holding for `ScripForScrip`, the head
    /// entity for Demerger).
    pub listing_id: ListingId,
    /// `YYYY-MM-DD`. `ReturnOfCapital`: payment date. `ShareSplit`: conversion date (parcels
    /// acquired before it are converted). `BonusIssue`: issue date. `RightsIssue`: record date
    /// (units held before it earn the entitlement). `BuyBack`: buy-back date. `ScripForScrip` /
    /// Demerger: exchange / demerger date (every parcel still open on it takes part).
    /// `WorthlessShares`: declaration (G3) or deregistration (C2) date.
    pub date: NaiveDate,
    /// `ReturnOfCapital` only (required): per-unit non-assessable payment in `currency`
    /// (decimal string, positive); reduces affected parcels' cost bases.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    amount_per_unit: Option<Decimal>,
    /// ISO 4217 code. Required by `ReturnOfCapital` (must match the affected parcels'
    /// currency), `RightsIssue` (the exercise price's currency) and `BuyBack` (the buy-back
    /// price's currency); refused on every other type.
    #[serde(default)]
    currency: Option<CurrencyCode>,
    /// `ReturnOfCapital` (optional): the date entitlement was fixed, on or before `date`.
    /// Parcels settled on or before it earn the payment; omitted, the payment date decides.
    /// `ShareSplit` / `BonusIssue` (optional): the date the register closed for the
    /// conversion, either side of `date`; a non-`Exact` rule rounds the units registered at
    /// it (settled on or before it), and omitted, every trade dated before `date` counts.
    #[serde(default)]
    record_date: Option<NaiveDate>,
    /// `ShareSplit` only (required): every `split_old_units` existing units become
    /// `split_new_units` units (decimal string, positive; a consolidation has new < old).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    split_new_units: Option<Decimal>,
    /// `ShareSplit` only (required): see `split_new_units`.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    split_old_units: Option<Decimal>,
    /// `BonusIssue` only (required): every `bonus_held_units` units held receive
    /// `bonus_units` additional units (decimal string, positive; a 1-for-10 issue is 1 /
    /// 10).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    bonus_units: Option<Decimal>,
    /// `BonusIssue` only (required): see `bonus_units`.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    bonus_held_units: Option<Decimal>,
    /// `RightsIssue` only (required): every `rights_held_units` units held at the record date
    /// entitle the holder to `rights_units` new units (decimal string, positive; a 1-for-4
    /// issue is 1 / 4).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    rights_units: Option<Decimal>,
    /// `RightsIssue` only (required): see `rights_units`.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    rights_held_units: Option<Decimal>,
    /// `RightsIssue` only (required): price paid per new unit on exercise, in `currency`
    /// (decimal string, positive).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    exercise_price: Option<Decimal>,
    /// `RightsIssue` only (required): whether the rights could be traded. Decides how a
    /// retail premium is taxed — renounceable: capital proceeds via `sell_rights`;
    /// non-renounceable: unfranked dividend income.
    #[serde(default)]
    renounceable: Option<bool>,
    /// `BuyBack` only (required): per-unit buy-back price in `currency` (decimal string, positive).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_price: Option<Decimal>,
    /// `BuyBack` only (optional, 0 when omitted): per-unit dividend component of the price
    /// (decimal string, non-negative, at most the price; 0 for a listed-company buy-back
    /// announced after 25 Oct 2022). Assessable income, excluded from capital proceeds.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_dividend: Option<Decimal>,
    /// `BuyBack` only (optional, 0 when omitted): per-unit franking credit attached to the
    /// dividend component (decimal string, non-negative; must be 0 when there is no
    /// dividend).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_franking_credit: Option<Decimal>,
    /// `BuyBack` only (optional): per-unit market value had the buy-back not been proposed
    /// (decimal string, positive); capital proceeds cannot be less than it. Omit when the
    /// price is at or above market value.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    buyback_market_value: Option<Decimal>,
    /// `ScripForScrip` only (required): the replacement listing the holding converts into;
    /// must differ from `listing_id`.
    #[serde(default)]
    scrip_listing_id: Option<ListingId>,
    /// `ScripForScrip` only (required): every `scrip_old_units` units held at the exchange
    /// date become `scrip_new_units` units of `scrip_listing_id` (decimal string,
    /// positive).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_new_units: Option<Decimal>,
    /// `ScripForScrip` only (required): see `scrip_new_units`.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_old_units: Option<Decimal>,
    /// `ScripForScrip` only (optional): cash received per OLD unit exchanged, in
    /// `scrip_cash_currency` (decimal string, positive) — a partial rollover. The three
    /// `scrip_cash_*`/`scrip_market_value` fields are given together or all omitted.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_cash_per_unit: Option<Decimal>,
    /// `ScripForScrip` only (with `scrip_cash_per_unit`): market value of one NEW unit just
    /// after issue, in `scrip_cash_currency` (decimal string, positive) — the scrip side of
    /// the cost-base apportionment.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    scrip_market_value: Option<Decimal>,
    /// `ScripForScrip` only (with `scrip_cash_per_unit`): ISO 4217 code of the cash and market value.
    #[serde(default)]
    scrip_cash_currency: Option<CurrencyCode>,
    /// Demerger only (required): the demerged entity's listing; must differ from `listing_id`.
    #[serde(default)]
    demerger_listing_id: Option<ListingId>,
    /// Demerger only (required): every `demerger_held_units` units of the head entity held
    /// at the demerger date receive `demerger_new_units` units of `demerger_listing_id`
    /// (decimal string, positive).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_new_units: Option<Decimal>,
    /// Demerger only (required): see `demerger_new_units`.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_held_units: Option<Decimal>,
    /// Demerger only (required): percentage of each parcel's cost base apportioned to the
    /// demerged entity, as the head entity advises (decimal string, 0 < pct < 100); the
    /// head parcels keep the rest.
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_cost_base_pct: Option<Decimal>,
    /// Demerger only (optional): the last pre-demerger trading day, strictly before `date`.
    /// With `demerger_close_price` it re-bases the listing's pre-demerger closing prices.
    /// The four `demerger_close_*` fields are given together or all omitted.
    #[serde(default)]
    demerger_close_date: Option<NaiveDate>,
    /// Demerger only (with `demerger_close_date`): what the head listing actually closed at
    /// on that day, in its quote currency (decimal string, positive).
    #[serde(
        default,
        deserialize_with = "crate::infra::decimal::strict_optional_decimal"
    )]
    demerger_close_price: Option<Decimal>,
    /// Demerger only (with `demerger_close_date`): where the stated close was taken from.
    /// Informational provenance.
    #[serde(default)]
    demerger_close_sourced_from: Option<String>,
    /// Demerger only (with `demerger_close_date`): why the close had to be stated.
    /// Informational provenance.
    #[serde(default)]
    demerger_close_reason: Option<String>,
    /// `WorthlessShares` only (required): the CGT event the loss is recognised under —
    /// `G3Declaration` (liquidator/administrator declaration) or `C2Cancellation`
    /// (deregistration).
    #[serde(default)]
    worthless_event: Option<WorthlessEvent>,
    /// `ShareSplit`, `BonusIssue`, `ScripForScrip` and Demerger only (required): how the
    /// registry treats a fractional entitlement where the ratio does not divide a holding —
    /// `Exact` (the exact fractional count is held), `Down`, `Up`, `Nearest` (rounded onto the
    /// holding's unit grid), or `CashInLieu` (rounded down, the fraction sold and paid out).
    /// Refused on every other type. Stored and validated; every rule is currently computed as
    /// `Exact` (see docs/API.md, Fractional entitlements).
    #[serde(default)]
    fraction_rule: Option<FractionRule>,
}

/// Which payload *groups* a submitted row has any column of — one bit per
/// group, so an arm of [`CorporateActionBody::kind`] states what it allows
/// rather than negating each of the other seven.
///
/// The rule every arm applies is the same: a row carries **exactly its own**
/// action type's payload. A column belonging to another type is refused, never
/// ignored — quietly dropping it would store an action that reads as one thing
/// and was entered as another. Two groups are shared rather than owned by one
/// type, and they are the only thing that varies between arms: `currency`,
/// which `ReturnOfCapital`, `RightsIssue` and `BuyBack` require and the other five
/// forbid, and `record_date`, which `ReturnOfCapital`, `ShareSplit` and `BonusIssue` may carry.
#[derive(Clone, Copy)]
struct Presence(u16);

impl Presence {
    const PAYMENT: u16 = 1 << 0;
    const RECORD: u16 = 1 << 1;
    const SPLIT: u16 = 1 << 2;
    const BONUS: u16 = 1 << 3;
    const RIGHTS: u16 = 1 << 4;
    const BUYBACK: u16 = 1 << 5;
    const SCRIP: u16 = 1 << 6;
    const DEMERGER: u16 = 1 << 7;
    const WORTHLESS: u16 = 1 << 8;
    const CURRENCY: u16 = 1 << 9;
    /// Shared by the four ratio-driven types, like `CURRENCY` is by the
    /// payment types.
    const FRACTION: u16 = 1 << 10;

    /// Every column of the row, gathered into its group. This is the one place
    /// a payload column is assigned to a type, so a column added here is
    /// forbidden by every arm that does not name its group.
    fn of(body: &CorporateActionBody) -> Self {
        let bit = |present: bool, group: u16| if present { group } else { 0 };
        Self(
            bit(body.amount_per_unit.is_some(), Self::PAYMENT)
                | bit(body.record_date.is_some(), Self::RECORD)
                | bit(
                    body.split_new_units.is_some() || body.split_old_units.is_some(),
                    Self::SPLIT,
                )
                | bit(
                    body.bonus_units.is_some() || body.bonus_held_units.is_some(),
                    Self::BONUS,
                )
                | bit(
                    body.rights_units.is_some()
                        || body.rights_held_units.is_some()
                        || body.exercise_price.is_some()
                        // Folded in here so a stray renounceable flag is
                        // refused by every arm that does not allow `RIGHTS` —
                        // no arm needs its own test.
                        || body.renounceable.is_some(),
                    Self::RIGHTS,
                )
                | bit(
                    body.buyback_price.is_some()
                        || body.buyback_dividend.is_some()
                        || body.buyback_franking_credit.is_some()
                        || body.buyback_market_value.is_some(),
                    Self::BUYBACK,
                )
                | bit(
                    body.scrip_listing_id.is_some()
                        || body.scrip_new_units.is_some()
                        || body.scrip_old_units.is_some()
                        || body.scrip_cash_per_unit.is_some()
                        || body.scrip_market_value.is_some()
                        || body.scrip_cash_currency.is_some(),
                    Self::SCRIP,
                )
                | bit(
                    body.demerger_listing_id.is_some()
                        || body.demerger_new_units.is_some()
                        || body.demerger_held_units.is_some()
                        || body.demerger_cost_base_pct.is_some()
                        // The stated close is part of the Demerger payload, so
                        // folding it in here is what makes every arm that does
                        // not allow `DEMERGER` reject a stray one — no arm
                        // needs its own test.
                        || body.demerger_close_date.is_some()
                        || body.demerger_close_price.is_some()
                        || body.demerger_close_sourced_from.is_some()
                        || body.demerger_close_reason.is_some(),
                    Self::DEMERGER,
                )
                | bit(body.worthless_event.is_some(), Self::WORTHLESS)
                | bit(body.currency.is_some(), Self::CURRENCY)
                | bit(body.fraction_rule.is_some(), Self::FRACTION),
        )
    }

    /// True when the row carries no group outside `allowed`. Stated as a
    /// permission rather than as a list of denials, which is what makes a new
    /// group forbidden everywhere it is not named: there is no per-arm list
    /// for it to be missing from.
    fn only(self, allowed: u16) -> bool {
        self.0 & !allowed == 0
    }
}

impl CorporateActionBody {
    /// The specific refusal for a `fraction_rule` that does not match the
    /// action type — missing on a ratio-driven type, or present on any other —
    /// checked ahead of [`Self::kind`] (which refuses the same rows) so the
    /// `422` says which way it is wrong rather than the generic terms message.
    pub(crate) fn fraction_rule_refusal(&self) -> Option<&'static str> {
        match (self.action_type.is_ratio_driven(), self.fraction_rule) {
            (true, None) => Some(FRACTION_RULE_REQUIRED),
            (false, Some(_)) => Some(FRACTION_RULE_NOT_APPLICABLE),
            _ => None,
        }
    }

    /// Each action type carries exactly its own payload (mirrors the table
    /// CHECKs, plus positivity): `ReturnOfCapital` needs a positive payment and
    /// a currency, and may carry a `record_date` no later than the payment
    /// date it fixes entitlement for (a later one is rejected, not ignored —
    /// it would read as an entitlement fixed after the money was paid);
    /// `ShareSplit` a positive conversion ratio; `BonusIssue` a
    /// positive bonus ratio; `RightsIssue` a positive entitlement ratio,
    /// exercise price, a currency, and whether the offer was renounceable
    /// (stated, never assumed — a retail premium's treatment turns on it);
    /// `BuyBack` a positive per-unit price and
    /// a currency (dividend/franking-credit components default to 0; the
    /// dividend may not exceed the price — it is part of it — and a credit
    /// needs a dividend to attach to; market value, when given, is positive)
    /// — each with every other type's fields absent (`None` otherwise), which
    /// each arm states as the groups it allows via [`Presence::only`] rather
    /// than by negating the other seven; `currency` is shared by
    /// `ReturnOfCapital`, `RightsIssue`, and `BuyBack` but forbidden for the
    /// ratio-only types. A zero/negative payment would
    /// silently *increase* cost bases; a zero/negative ratio would zero out
    /// or invert holdings or entitlements. `ScripForScrip` needs a positive
    /// exchange ratio and a replacement listing different from the original —
    /// exchanging a listing into itself would consume its parcels and
    /// recreate them in place — and may carry a cash component: per-old-unit
    /// cash, the replacement unit's market value, and their currency, all
    /// three present (cash and market value positive) or all absent.
    /// Demerger needs a positive entitlement ratio, a
    /// demerged listing different from the head, and a cost-base percentage
    /// strictly between 0 and 100 — 0 or 100 would zero out one side's cost
    /// base entirely, and anything outside would make one side negative.
    /// `WorthlessShares` needs only the `worthless_event` discriminator (the CGT
    /// event basis), with every numeric/listing payload absent. The four
    /// ratio-driven types (`ShareSplit`, `BonusIssue`, `ScripForScrip`,
    /// Demerger) each also need a `fraction_rule` — the issuer's treatment of
    /// a fractional entitlement (ASX Listing Rule 7.20), never assumed — and
    /// every other type refuses one ([`Presence::FRACTION`]).
    pub(crate) fn kind(self) -> Option<ActionKind> {
        // Entitlement can never be fixed after the payment it entitles the
        // holder to, so a record date past the payment date is rejected rather
        // than silently ignored (CHECK-enforced too, 0023).
        let record_date = match self.record_date {
            Some(rd)
                if rd > self.date && matches!(self.action_type, ActionType::ReturnOfCapital) =>
            {
                return None;
            }
            other => other,
        };
        let present = Presence::of(&self);
        let positive = |d: Option<Decimal>| d.filter(|v| *v > Decimal::ZERO);
        // Provenance text that is present but blank records nothing, so it is
        // refused rather than stored — the rule `PUT /closing_prices/…` already
        // applies to a manual price's own `sourced_from`/`reason`.
        let non_blank = |s: String| {
            let trimmed = s.trim().to_owned();
            (!trimmed.is_empty()).then_some(trimmed)
        };
        match self.action_type {
            ActionType::ReturnOfCapital
                if present.only(Presence::PAYMENT | Presence::RECORD | Presence::CURRENCY) =>
            {
                Some(ActionKind::ReturnOfCapital {
                    amount_per_unit: positive(self.amount_per_unit)?,
                    currency: self.currency?,
                    record_date,
                })
            }
            ActionType::ShareSplit
                if present.only(Presence::SPLIT | Presence::FRACTION | Presence::RECORD) =>
            {
                Some(ActionKind::ShareSplit {
                    split_new_units: positive(self.split_new_units)?,
                    split_old_units: positive(self.split_old_units)?,
                    fraction_rule: self.fraction_rule?,
                    record_date,
                })
            }
            ActionType::BonusIssue
                if present.only(Presence::BONUS | Presence::FRACTION | Presence::RECORD) =>
            {
                Some(ActionKind::BonusIssue {
                    bonus_units: positive(self.bonus_units)?,
                    bonus_held_units: positive(self.bonus_held_units)?,
                    fraction_rule: self.fraction_rule?,
                    record_date,
                })
            }
            ActionType::RightsIssue if present.only(Presence::RIGHTS | Presence::CURRENCY) => {
                Some(ActionKind::RightsIssue {
                    rights_units: positive(self.rights_units)?,
                    rights_held_units: positive(self.rights_held_units)?,
                    exercise_price: positive(self.exercise_price)?,
                    currency: self.currency?,
                    // Required, not defaulted: the whole point of recording it
                    // is that a retail premium's treatment turns on it, and a
                    // flag that quietly defaults would leave the same
                    // unasked assumption in place for every new entry
                    // (SCENARIOS AA-b). The offer document always states it.
                    renounceable: self.renounceable?,
                })
            }
            ActionType::ScripForScrip if present.only(Presence::SCRIP | Presence::FRACTION) => {
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
                    fraction_rule: self.fraction_rule?,
                })
            }
            ActionType::Demerger if present.only(Presence::DEMERGER | Presence::FRACTION) => {
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
                    fraction_rule: self.fraction_rule?,
                })
            }
            ActionType::BuyBack if present.only(Presence::BUYBACK | Presence::CURRENCY) => {
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
            ActionType::WorthlessShares if present.only(Presence::WORTHLESS) => {
                Some(ActionKind::WorthlessShares {
                    worthless_event: self.worthless_event?,
                })
            }
            // Each type's guard failing: its fields do not form a valid
            // action of that type.
            ActionType::ReturnOfCapital
            | ActionType::ShareSplit
            | ActionType::BonusIssue
            | ActionType::RightsIssue
            | ActionType::BuyBack
            | ActionType::ScripForScrip
            | ActionType::Demerger
            | ActionType::WorthlessShares => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// One representative column of each payload group, keyed by the
    /// [`Presence`] bit that group is. Naming a *column* is the point: what
    /// `Presence::only` refuses is a stray column, and a group is present as
    /// soon as any one of its columns is.
    const GROUP_COLUMNS: [(u16, &str, &str); 11] = [
        (Presence::PAYMENT, "amount_per_unit", r#""0.50""#),
        (Presence::RECORD, "record_date", r#""2024-11-01""#),
        (Presence::SPLIT, "split_new_units", r#""2""#),
        (Presence::BONUS, "bonus_units", r#""1""#),
        (Presence::RIGHTS, "renounceable", "true"),
        (Presence::BUYBACK, "buyback_market_value", r#""10.20""#),
        (Presence::SCRIP, "scrip_new_units", r#""2""#),
        (Presence::DEMERGER, "demerger_cost_base_pct", r#""40""#),
        (Presence::WORTHLESS, "worthless_event", r#""G3Declaration""#),
        (Presence::CURRENCY, "currency", r#""AUD""#),
        (Presence::FRACTION, "fraction_rule", r#""Down""#),
    ];

    /// A minimal valid payload for each action type, with the groups that type
    /// is allowed to carry. The allowance is what [`Presence::only`] states at
    /// each arm; the payload proves the arm is reachable in the first place, so
    /// a rejection below is the stray column and not a broken base case.
    fn valid_payloads() -> Vec<(&'static str, u16, Value)> {
        vec![
            (
                "ReturnOfCapital",
                Presence::PAYMENT | Presence::RECORD | Presence::CURRENCY,
                json!({"amount_per_unit": "0.50", "currency": "AUD"}),
            ),
            (
                "ShareSplit",
                Presence::SPLIT | Presence::FRACTION | Presence::RECORD,
                json!({"split_new_units": "2", "split_old_units": "1", "fraction_rule": "Exact"}),
            ),
            (
                "BonusIssue",
                Presence::BONUS | Presence::FRACTION | Presence::RECORD,
                json!({"bonus_units": "1", "bonus_held_units": "10", "fraction_rule": "Down"}),
            ),
            (
                "RightsIssue",
                Presence::RIGHTS | Presence::CURRENCY,
                json!({
                    "rights_units": "1",
                    "rights_held_units": "4",
                    "exercise_price": "1.80",
                    "currency": "AUD",
                    "renounceable": true,
                }),
            ),
            (
                "ScripForScrip",
                Presence::SCRIP | Presence::FRACTION,
                json!({
                    "scrip_listing_id": 2,
                    "scrip_new_units": "2",
                    "scrip_old_units": "1",
                    "fraction_rule": "CashInLieu",
                }),
            ),
            (
                "Demerger",
                Presence::DEMERGER | Presence::FRACTION,
                json!({
                    "demerger_listing_id": 2,
                    "demerger_new_units": "1",
                    "demerger_held_units": "5",
                    "demerger_cost_base_pct": "40",
                    "fraction_rule": "Nearest",
                }),
            ),
            (
                "BuyBack",
                Presence::BUYBACK | Presence::CURRENCY,
                json!({"buyback_price": "9.60", "currency": "AUD"}),
            ),
            (
                "WorthlessShares",
                Presence::WORTHLESS,
                json!({"worthless_event": "C2Cancellation"}),
            ),
        ]
    }

    fn body(action_type: &str, payload: &Value) -> CorporateActionBody {
        let mut v = json!({"action_type": action_type, "listing_id": 1, "date": "2024-11-30"});
        let map = v.as_object_mut().unwrap();
        for (k, val) in payload.as_object().unwrap() {
            map.insert(k.clone(), val.clone());
        }
        serde_json::from_value(v).unwrap()
    }

    /// Every action type carries exactly its own payload: one column of any
    /// group it is not allowed makes the row unrepresentable as an
    /// [`ActionKind`], so it is refused rather than stored with the stray
    /// column silently dropped. The whole 8 × 11 matrix, not a spot check —
    /// this is the rule `Presence::only` exists to state, and the arms differ
    /// only in the three shared groups (`currency`, `record_date`,
    /// `fraction_rule`).
    #[test]
    fn each_action_type_refuses_every_other_types_columns() {
        for (action_type, allowed, payload) in valid_payloads() {
            assert!(
                body(action_type, &payload).kind().is_some(),
                "{action_type}'s own payload should be accepted"
            );
            for (group, column, value) in GROUP_COLUMNS {
                if allowed & group != 0 {
                    continue;
                }
                let mut with_stray = payload.clone();
                let stray: Value = serde_json::from_str(value).unwrap();
                with_stray
                    .as_object_mut()
                    .unwrap()
                    .insert(column.to_owned(), stray);
                assert!(
                    body(action_type, &with_stray).kind().is_none(),
                    "{action_type} should refuse a stray {column}"
                );
            }
        }
    }

    /// The four ratio-driven types each require their `fraction_rule`: the
    /// same payload without it is unrepresentable, and the specific refusal
    /// names it as required. Every other type refuses one with the other
    /// message (its stray-column refusal is the matrix above).
    #[test]
    fn a_ratio_driven_type_requires_its_fraction_rule_and_no_other_type_takes_one() {
        for (action_type, allowed, payload) in valid_payloads() {
            let ratio_driven = allowed & Presence::FRACTION != 0;
            let mut without = payload.clone();
            without.as_object_mut().unwrap().remove("fraction_rule");
            let mut with = payload.clone();
            with.as_object_mut()
                .unwrap()
                .insert("fraction_rule".to_owned(), json!("Up"));
            if ratio_driven {
                assert!(
                    body(action_type, &without).kind().is_none(),
                    "{action_type}"
                );
                assert_eq!(
                    body(action_type, &without).fraction_rule_refusal(),
                    Some(FRACTION_RULE_REQUIRED),
                    "{action_type}"
                );
                assert_eq!(body(action_type, &with).fraction_rule_refusal(), None);
            } else {
                assert_eq!(body(action_type, &without).fraction_rule_refusal(), None);
                assert_eq!(
                    body(action_type, &with).fraction_rule_refusal(),
                    Some(FRACTION_RULE_NOT_APPLICABLE),
                    "{action_type}"
                );
            }
        }
    }

    /// Every rule round-trips through its stored name, and the serde wire
    /// name is that same string.
    #[test]
    fn every_fraction_rule_round_trips_through_its_stored_name() {
        for rule in [
            FractionRule::Exact,
            FractionRule::Down,
            FractionRule::Up,
            FractionRule::Nearest,
            FractionRule::CashInLieu,
        ] {
            assert_eq!(rule.as_str().parse::<FractionRule>(), Ok(rule));
            assert_eq!(serde_json::to_value(rule).unwrap(), json!(rule.as_str()));
        }
        assert!("Truncate".parse::<FractionRule>().is_err());
    }
}
