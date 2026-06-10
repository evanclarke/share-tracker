//! Domain calculations shared by entities and reports — tax arithmetic that
//! belongs to no single entity module and must never diverge between its
//! callers.

pub mod cost_base;
pub mod tax_year;
