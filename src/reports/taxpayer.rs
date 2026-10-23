//! The taxpayer assumption every tax report states on its rows.

/// Taxpayer assumption stated on every tax-report row: the rates are hard-wired
/// for an Australian-resident *individual* — the 50% CGT discount and the 50%
/// LIC capital gain deduction. Other entity types (SMSF/complying super 33⅓%,
/// company 0%, trust/partnership flow-through) are deliberately not modelled
/// (scope decision, 2026-06-07). Kept comma-free so it stays a single CSV field.
pub const TAXPAYER_BASIS: &str = "individual resident: 50% CGT discount; 50% LIC deduction";
