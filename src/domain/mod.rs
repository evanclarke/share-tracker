//! Domain calculations shared by entities and reports — tax arithmetic that
//! belongs to no single entity module and must never diverge between its
//! callers.

pub mod adjustments;
pub mod cgt_discount;
pub mod contemplated_disposal;
pub mod cost_base;
pub mod deduction_destination;
pub mod franking_credit;
pub mod indexation;
pub mod listing_identity;
pub mod open_parcels;
pub mod rollover;
pub mod tax_year;
pub mod whole_holding;

#[cfg(test)]
mod tests {
    use crate::test_support::{code_only, rust_sources, strip_test_modules};

    /// `domain` is the lower layer: entities and reports call into it, and it
    /// reaches nothing in `entities` — the adjustment events and their loaders
    /// (`adjustments`), the AMIT reduction loader (`cost_base`), the
    /// allocation reads (`open_parcels`) all live here, and the operations'
    /// writes are `entities::rollover`'s. A domain module needing an entity's
    /// type takes the fact it reads as a parameter instead
    /// (`deduction_destination`'s `debt_deduction`). Test modules are exempt:
    /// they build their fixtures through the entities' own write paths.
    #[test]
    fn domain_depends_on_nothing_in_entities() {
        let offenders: Vec<String> = rust_sources()
            .into_iter()
            .filter(|(path, _)| path.starts_with("domain/"))
            .flat_map(|(path, raw)| {
                let code = strip_test_modules(&code_only(&raw));
                code.lines()
                    .enumerate()
                    .filter(|(_, line)| line.contains("entities::"))
                    .map(|(n, line)| format!("src/{path}:{}: {}", n + 1, line.trim()))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert!(
            offenders.is_empty(),
            "domain must not depend on entities:\n{}",
            offenders.join("\n")
        );
    }
}
