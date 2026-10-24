//! Domain calculations shared by entities and reports — tax arithmetic that
//! belongs to no single entity module and must never diverge between its
//! callers.

pub mod adjustments;
pub mod cgt_discount;
pub mod contemplated_disposal;
pub mod cost_base;
pub mod deduction_destination;
pub mod entitlement_rounding;
pub mod franking_credit;
pub mod indexation;
pub mod listing_identity;
pub mod open_parcels;
pub mod rollover;
pub mod tax_year;
pub mod whole_holding;

#[cfg(test)]
mod tests {
    use crate::test_support::rust_sources;

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
            .iter()
            .filter(|source| source.path.starts_with("domain/"))
            .flat_map(|source| {
                let path = &source.path;
                source
                    .production
                    .lines()
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

    /// The `pub`/`pub(crate)` fn, type, const and trait definitions in
    /// `production` (comments, literals and test modules blanked) that carry
    /// no `///` doc in `raw` (the same file as written, line for line): the
    /// lines above each definition are walked back over its attributes —
    /// a multi-line `#[derive(…)]` included — and any plain `//` comment,
    /// and the first other line must be a `///`.
    fn undocumented_public_items(raw: &str, production: &str) -> Vec<String> {
        const KEYWORDS: [&str; 7] = ["fn", "struct", "enum", "type", "const", "static", "trait"];
        let raw: Vec<&str> = raw.lines().collect();
        let mut found = Vec::new();
        for (n, line) in production.lines().enumerate() {
            let line = line.trim_start();
            let Some(mut rest) = line
                .strip_prefix("pub(crate) ")
                .or_else(|| line.strip_prefix("pub "))
            else {
                continue;
            };
            // The qualifiers ahead of the keyword; `const` is one only before
            // `fn`/`unsafe` (otherwise it is the item keyword itself).
            while let Some(after) = ["async ", "unsafe ", "const "]
                .iter()
                .filter_map(|q| rest.strip_prefix(q))
                .find(|after| {
                    !rest.starts_with("const ")
                        || after.starts_with("fn ")
                        || after.starts_with("unsafe ")
                })
            {
                rest = after;
            }
            let Some(keyword) = KEYWORDS.iter().find(|k| {
                rest.strip_prefix(**k)
                    .is_some_and(|after| after.starts_with(' '))
            }) else {
                continue;
            };
            let mut above = n;
            let documented = loop {
                let Some(prev) = above.checked_sub(1) else {
                    break false;
                };
                above = prev;
                let text = raw[above].trim();
                if text.starts_with("///") {
                    break true;
                }
                if text.ends_with(']') && !text.starts_with("#[") {
                    // The tail of a multi-line attribute: back to its `#[`.
                    while above > 0 && !raw[above].trim_start().starts_with("#[") {
                        above -= 1;
                    }
                    continue;
                }
                if !(text.starts_with("#[") || text.starts_with("//")) {
                    break false;
                }
            };
            if !documented {
                let name: String = rest[keyword.len()..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                found.push(format!("{}: {keyword} {name}", n + 1));
            }
        }
        found
    }

    #[test]
    fn the_item_doc_scan_flags_only_undocumented_public_definitions() {
        let raw = "/// Documented.\n#[derive(\n    Debug,\n)]\npub struct A;\n\
                   pub(crate) async fn bare() {}\n\
                   /// Documented.\n#[must_use]\n// why\npub const fn b() {}\n\
                   fn private() {}\npub use x::y;\npub mod m;\n\
                   impl A {\n    pub const ZERO: u8 = 0;\n}\n";
        assert_eq!(
            undocumented_public_items(raw, raw),
            ["6: fn bare", "15: const ZERO"]
        );
    }

    /// Every `pub`/`pub(crate)` fn, type, const and trait in `domain`'s
    /// production code has a `///` doc saying what it computes, and where it
    /// implements one, the tax rule (`docs/ato/OVERVIEW.md` indexes the
    /// references). `domain` is the financial lower layer every report and
    /// operation calls into, so its items are documented where the rest of
    /// the tree's are left to their module header — a crate-wide
    /// `missing_docs` cannot scope it (a binary crate exports nothing).
    #[test]
    fn every_public_domain_item_is_documented() {
        let found: Vec<String> = rust_sources()
            .iter()
            .filter(|source| source.path.starts_with("domain/"))
            .flat_map(|source| {
                undocumented_public_items(&source.raw, &source.production)
                    .into_iter()
                    .map(|item| format!("src/{}:{item}", source.path))
            })
            .collect();
        assert!(
            found.is_empty(),
            "document each of these `domain` items with a `///` stating what it \
             computes (and the ATO rule, where it implements one):\n{}",
            found.join("\n")
        );
    }
}
