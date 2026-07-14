//! The trade model: the stored row (`Trade`), its wire presentation, the
//! `PUT` request body (`TradeBody`), and the `TradeType` enum.

use crate::infra::decimal::{row_dec, row_opt_dec};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::Row;

// `DRP` is serialized verbatim to JSON and persisted to the TEXT `trade_type`
// column (matched by a CHECK constraint), so the acronym spelling is the
// wire/storage format and must not be camel-cased.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, sqlx::Type)]
pub enum TradeType {
    Buy,
    Sell,
    DRP,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: i64,
    pub trade_type: TradeType,
    pub date: NaiveDate,
    pub settlement_date: NaiveDate,
    pub listing_id: i64,
    pub average_price: Decimal,
    pub quantity: Decimal,
    pub currency: String,
    /// Always stored ex-GST: when a request flags `brokerage_includes_gst`,
    /// the entered inclusive amount is split at write time (see
    /// `split_gst_inclusive`) before it lands here, so the cost-base
    /// arithmetic (`brokerage + gst_on_brokerage`) holds unconditionally.
    /// On the wire, though, a flagged trade reads back with `brokerage` as
    /// the one GST-inclusive amount — the same shape the write path expects
    /// — so a GET → PUT round-trip is lossless (see [`Trade::present`]).
    pub brokerage: Decimal,
    pub gst_on_brokerage: Decimal,
    /// Records that the brokerage amount was *entered* GST-inclusive and
    /// server-split. Persisted so reads can re-present `brokerage` as the
    /// inclusive amount (see [`Trade::present`]) and so a re-PUT keeps the
    /// same split semantics; the stored money columns are already split, so
    /// nothing else reads it.
    pub brokerage_includes_gst: bool,
    pub brokerage_currency: String,
    /// Manual foreign-per-AUD override (same convention as the ATO rate: AUD =
    /// foreign / fx_rate). Reports prefer the ATO RBA rate for the trade's month
    /// and fall back to this field only when no ATO rate exists (see `infra::fx`).
    /// 1.0 for AUD trades.
    pub fx_rate: Decimal,
    /// Deliberate transaction-date spot-rate override (same foreign-per-AUD
    /// convention): when set it wins over the ATO monthly rate everywhere
    /// this trade's amounts convert to AUD — QC 18020 says an average rate is
    /// not appropriate for a one-off purchase or sale of a large capital
    /// asset (see `infra::fx::FxOverride`). `None` keeps the unchanged
    /// default (monthly rate first, `fx_rate` fallback). Rejected at write
    /// time unless positive and on a non-AUD trade (where it could apply).
    pub spot_fx_rate: Option<Decimal>,
    pub contract_note_ref: Option<String>,
    /// The broker statement's net transaction total in the brokerage
    /// currency, kept for cross-referencing against the contract note.
    /// Validated at write time (see `check_statement_total`) — a value that
    /// doesn't reconcile with quantity × price ± (brokerage + GST) is
    /// rejected — and informational-only after that: no report or
    /// calculation uses it.
    pub statement_total: Option<Decimal>,
    /// DRP reinvestment residual cash (DRP trades only; 0 for Buy/Sell). When a
    /// distribution doesn't divide evenly into whole shares, the leftover is
    /// carried forward to the next reinvestment or paid out. These are populated
    /// by the reinvestment operation (see `entities::drp_reinvestment`); a
    /// manually entered DRP trade leaves them 0.
    pub residual_brought_forward: Decimal,
    pub residual_carried_forward: Decimal,
    pub residual_paid_out: Decimal,
    /// Provenance link from a rights-exercise Buy back to its `RightsIssue`
    /// corporate action (`None` for every other trade). Set only by
    /// `POST /corporate_actions/:id/exercise` (`entities::rights_exercise`),
    /// which uses it to cap cumulative exercised units at the entitlement; a
    /// trade carrying it is rejected by `PUT /trades` (delete and re-exercise
    /// instead), and the action it references cannot be edited or deleted
    /// while the trade exists.
    pub rights_action_id: Option<i64>,
    /// Provenance link from a buy-back participation Sell back to its
    /// `BuyBack` corporate action (`None` for every other trade). Set only by
    /// `POST /corporate_actions/:id/participate`
    /// (`entities::buyback_participation`). A trade carrying it is rejected
    /// by `PUT /sells` (delete it — which also removes the linked
    /// dividend-component income row — and re-participate instead), and the
    /// action it references cannot be edited or deleted while the trade
    /// exists.
    pub buyback_action_id: Option<i64>,
    /// Provenance link from a scrip-for-scrip exchange trade — the closing
    /// Sell on the original listing or a replacement Buy on the new one —
    /// back to its `ScripForScrip` corporate action (`None` for every other
    /// trade). Set only by `POST /corporate_actions/:id/exchange`
    /// (`entities::scrip_exchange`). The trades carrying one action id form
    /// the exchange group: each is rejected by `PUT /sells` and
    /// `PUT`/`DELETE /trades`; `DELETE /sells` on the closing Sell removes
    /// the whole group; and the action cannot be edited or deleted while any
    /// exists.
    pub scrip_action_id: Option<i64>,
    /// Provenance link from a demerger trade — the closing Sell on the head
    /// listing, a head replacement Buy, or a demerged-entity Buy — back to
    /// its `Demerger` corporate action (`None` for every other trade). Set
    /// only by `POST /corporate_actions/:id/demerge` (`entities::demerger`).
    /// The trades carrying one action id form the demerger group: each is
    /// rejected by `PUT /sells` and `PUT`/`DELETE /trades`; `DELETE /sells`
    /// on the closing Sell removes the whole group; and the action cannot be
    /// edited or deleted while any exists.
    pub demerger_action_id: Option<i64>,
    /// The CGT acquisition date deemed for this parcel when it differs from
    /// `date`: set only on scrip-for-scrip replacement Buys and demerger
    /// head/demerged Buys, carrying the consumed parcel's acquisition date
    /// (the rollovers count the combined holding period — see
    /// `docs/ato/takeovers-and-scrip-for-scrip.md` and `docs/ato/demergers.md`).
    /// Drives the 12-month discount clock and the AUD translation month of
    /// the cost base in the reports; split/return-of-capital applicability
    /// stays on the actual `date` (the replacement shares only exist in
    /// their listing from the exchange/demerger on). `None` = the trade's
    /// own date.
    pub deemed_acquisition_date: Option<NaiveDate>,
    /// The holding account the trade sits in (see
    /// `entities::holding_account`): the same listing can be held in more
    /// than one account at once — e.g. RSU-vested shares in an employer plan
    /// account alongside DRP-enrolled shares in a personal broker account.
    /// Defaults to the seeded default account when omitted from a request.
    pub holding_account_id: i64,
    /// Provenance link from a transfer-out Sell / transfer-in Buy back to its
    /// holding-account transfer (`None` for every other trade). Set only by
    /// `PUT /transfers/:id` (`entities::transfer`). The trades carrying one
    /// transfer id form the transfer group: each is rejected by `PUT /sells`
    /// and `PUT`/`DELETE /trades`; `DELETE /transfers/:id` removes the whole
    /// group.
    pub transfer_id: Option<i64>,
    /// Provenance link from a cost-base-reset ESS vest Buy back to its
    /// `ess_statements` row (`None` for every other trade). Set only by
    /// `POST /ess_statements/:id/vest` (`entities::ess_vest`). A trade carrying
    /// it is the statement's vest: it is rejected by `PUT /trades` (delete and
    /// re-vest instead), and never deleted individually — `DELETE
    /// /ess_statements/:id` removes it (refused while it is drawn on by a Sell
    /// allocation or AMIT adjustment), and the statement is frozen against
    /// edits while it exists.
    pub ess_statement_id: Option<i64>,
    /// Provenance link from a worthless-shares recognise closing Sell back to
    /// its `WorthlessShares` corporate action (`None` for every other trade).
    /// Set only by `POST /corporate_actions/:id/recognise`
    /// (`entities::worthless`). The Sell carrying it is rejected by `PUT /sells`
    /// and `PUT`/`DELETE /trades`; `DELETE /sells` removes it and restores the
    /// holding; and the action cannot be edited or deleted while it exists.
    /// Unlike the rollover provenance columns it does **not** exclude the Sell
    /// from the realised-gains report — its nil proceeds recognise the capital
    /// loss (see `docs/ato/worthless-shares.md`).
    pub worthless_action_id: Option<i64>,
    /// Provenance link from an inherited-parcel Buy back to its `inheritances`
    /// row (`None` for every other trade). Set only by `PUT /inheritances/:id`
    /// (`entities::inheritance`): the Buy carries the inheritance's cost base
    /// and s 115-30 discount clock, so it is rejected by `PUT`/`DELETE
    /// /trades` — edit or delete the inheritance instead (refused while the
    /// parcel is drawn on by a Sell allocation or AMIT adjustment).
    pub inheritance_id: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Trade {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Trade {
            id: row.try_get("id")?,
            trade_type: row.try_get::<TradeType, _>("trade_type")?,
            date: row.try_get("date")?,
            settlement_date: row.try_get("settlement_date")?,
            listing_id: row.try_get("listing_id")?,
            average_price: row_dec(row, "average_price")?,
            quantity: row_dec(row, "quantity")?,
            currency: row.try_get("currency")?,
            brokerage: row_dec(row, "brokerage")?,
            gst_on_brokerage: row_dec(row, "gst_on_brokerage")?,
            brokerage_includes_gst: row.try_get("brokerage_includes_gst")?,
            brokerage_currency: row.try_get("brokerage_currency")?,
            fx_rate: row_dec(row, "fx_rate")?,
            spot_fx_rate: row_opt_dec(row, "spot_fx_rate")?,
            contract_note_ref: row.try_get("contract_note_ref")?,
            statement_total: row_opt_dec(row, "statement_total")?,
            residual_brought_forward: row_dec(row, "residual_brought_forward")?,
            residual_carried_forward: row_dec(row, "residual_carried_forward")?,
            residual_paid_out: row_dec(row, "residual_paid_out")?,
            rights_action_id: row.try_get("rights_action_id")?,
            buyback_action_id: row.try_get("buyback_action_id")?,
            scrip_action_id: row.try_get("scrip_action_id")?,
            demerger_action_id: row.try_get("demerger_action_id")?,
            deemed_acquisition_date: row.try_get("deemed_acquisition_date")?,
            holding_account_id: row.try_get("holding_account_id")?,
            transfer_id: row.try_get("transfer_id")?,
            ess_statement_id: row.try_get("ess_statement_id")?,
            worthless_action_id: row.try_get("worthless_action_id")?,
            inheritance_id: row.try_get("inheritance_id")?,
        })
    }
}

impl Trade {
    /// The wire presentation of a trade (`GET /trades`, `GET /trades/:id`):
    /// identical to the stored row, except that a GST-inclusive entry
    /// (`brokerage_includes_gst`) re-presents `brokerage` as the one
    /// inclusive amount — the stored ex-GST brokerage and GST recombined,
    /// exactly what was entered. That is the same shape a flagged write
    /// expects, so PUTting a response body back verbatim re-splits it to
    /// the identical stored pair: the GET → PUT round-trip is lossless
    /// (REQUIREMENTS 2026-07-13). `gst_on_brokerage` stays the derived
    /// component (informational on reads; a flagged write ignores it), and
    /// unflagged trades read back as stored. Applies only at the HTTP
    /// boundary — internal callers of `db_get`/`db_list` keep the stored
    /// ex-GST split.
    pub(super) fn present(mut self) -> Self {
        if self.brokerage_includes_gst {
            self.brokerage += self.gst_on_brokerage;
        }
        self
    }
}

#[derive(Debug, Deserialize)]
pub struct TradeBody {
    pub trade_type: TradeType,
    pub date: NaiveDate,
    #[serde(default)]
    pub settlement_date: Option<NaiveDate>,
    pub listing_id: i64,
    pub average_price: Decimal,
    pub quantity: Decimal,
    pub currency: String,
    /// GST-inclusive when `brokerage_includes_gst` is set (the server splits
    /// it; any supplied `gst_on_brokerage` is ignored), ex-GST otherwise.
    /// Reads present the same shape (see [`Trade::present`]), so a GET → PUT
    /// round-trip is lossless.
    pub brokerage: Decimal,
    #[serde(default)]
    pub gst_on_brokerage: Decimal,
    #[serde(default)]
    pub brokerage_includes_gst: bool,
    pub brokerage_currency: String,
    pub fx_rate: Decimal,
    /// Optional deliberate spot-rate override; see `Trade::spot_fx_rate`.
    #[serde(default)]
    pub spot_fx_rate: Option<Decimal>,
    #[serde(default)]
    pub contract_note_ref: Option<String>,
    /// Optional statement cross-check; see `Trade::statement_total`.
    #[serde(default)]
    pub statement_total: Option<Decimal>,
    #[serde(default)]
    pub residual_brought_forward: Decimal,
    #[serde(default)]
    pub residual_carried_forward: Decimal,
    #[serde(default)]
    pub residual_paid_out: Decimal,
    /// Defaults to the seeded default holding account when omitted, so
    /// single-account clients never see the dimension.
    #[serde(default = "crate::entities::holding_account::default_holding_account_id")]
    pub holding_account_id: i64,
}
