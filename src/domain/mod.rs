//! Domain calculations shared by entities and reports — tax arithmetic that
//! belongs to no single entity module and must never diverge between its
//! callers.

pub mod cgt_discount;
pub mod cost_base;
pub mod deduction_destination;
pub mod franking_credit;
pub mod listing_identity;
pub mod open_parcels;
pub mod rollover;
pub mod tax_year;
pub mod whole_holding;
