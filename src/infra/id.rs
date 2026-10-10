//! Typed row ids: a [`ListingId`] cannot be passed where a holding account's
//! or a trade's id is wanted, which a bare `i64` happily allows — and a swapped
//! pair of ids type-checks, writes, and costs the wrong parcel.
//!
//! Each is a transparent newtype over the `INTEGER PRIMARY KEY` it names:
//! serde and sqlx see the bare integer (the JSON shapes and the SQL binds are
//! unchanged), and the OpenAPI document describes it as an integer component.
//! `.0` is deliberately public — the escape hatch for the places that really do
//! hold an id of no particular kind (the audit trail's `row_id`, a generic CRUD
//! helper) — but nothing converts implicitly between two id types.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            utoipa::ToSchema,
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            Serialize,
            Deserialize,
            sqlx::Type,
        )]
        #[serde(transparent)]
        #[sqlx(transparent)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<i64> for $name {
            fn from(id: i64) -> Self {
                Self(id)
            }
        }

        /// Test-only, so an assertion can compare against a literal id.
        #[cfg(test)]
        impl PartialEq<i64> for $name {
            fn eq(&self, other: &i64) -> bool {
                self.0 == *other
            }
        }
    };
}

id_type!(
    /// The id of a `listings` row: one security as quoted on one exchange.
    ListingId
);

id_type!(
    /// The id of a `holding_accounts` row: the broker account or registry
    /// holding a parcel is held in.
    HoldingAccountId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_serialises_as_the_bare_integer() {
        assert_eq!(serde_json::to_string(&ListingId(7)).unwrap(), "7");
        let back: ListingId = serde_json::from_str("7").unwrap();
        assert_eq!(back, ListingId(7));
        assert_eq!(ListingId(7).to_string(), "7");
    }
}
