//! Registry rounding of a ratio-driven action's fractional entitlement: the
//! per-**holding** rounded total, and its distribution across the holding's
//! parcels (TODO "Unit granularity and registry rounding of fractional
//! entitlements", *Compute per holding, store per parcel*).
//!
//! A registry applies an issuer's fraction rule once per holding (per
//! HIN/SRN), not per parcel — parcels are the taxpayer's record, the register
//! holds one number (ASX Listing Rule 7.20 has the issuer disclose the rule).
//! So [`round_holding`] first rounds the holding's exact post-action count
//! onto its unit grid by the rule, then distributes that total across the
//! parcels by **largest remainder** on the grid: every parcel takes its exact
//! share rounded down, and the steps left over go one each to the parcels with
//! the largest remainders. Rounding is no CGT event (TD 2000/10, s 112-25):
//! each parcel keeps its whole cost base and acquisition date, spread over the
//! count it is given here. Cash in lieu (`Rounding::CashInLieu`) is a disposal
//! of the fraction, which the caller writes as a Sell; here it is the
//! rounded-down total plus the fraction carried on one parcel until then.
//!
//! Pure arithmetic over exact `Decimal` comparisons: the exact shares are kept
//! as `numerator / divisor` pairs and every floor is verified by
//! multiplication, so no rounded quotient decides which grid point a share
//! lies under.

use crate::infra::id::TradeId;
use chrono::NaiveDate;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

/// How a holding's exact post-action count becomes the count the register
/// holds — the non-`Exact` fraction rules (an `Exact` action is never
/// rounded, so it has no variant here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// Rounded down onto the grid; the fraction is forfeited.
    Down,
    /// Rounded up onto the grid.
    Up,
    /// Rounded to the nearest grid point, a half rounding up.
    Nearest,
    /// Rounded down, the fraction carried on one parcel until the
    /// cash-in-lieu Sell disposes of it.
    CashInLieu,
}

/// One parcel of the holding as it stood immediately before the action.
#[derive(Debug, Clone, Copy)]
pub struct ParcelBefore {
    /// The parcel (a Buy/DRP trade).
    pub trade_id: TradeId,
    /// Its acquisition (trade) date — the first tie-break after the
    /// remainder, so the distribution is deterministic.
    pub acquired: NaiveDate,
    /// Units it held immediately before the action, in the basis then in
    /// force (positive).
    pub units: Decimal,
}

/// One parcel's stored post-action count, as [`round_holding`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParcelCount {
    /// The parcel.
    pub trade_id: TradeId,
    /// [`ParcelBefore::units`], carried through.
    pub before: Decimal,
    /// Its count after the action: on the grid, plus [`Self::in_lieu`].
    pub after: Decimal,
    /// The cash-in-lieu fraction carried in [`Self::after`] (zero except on
    /// the one parcel a `CashInLieu` holding carries it on).
    pub in_lieu: Decimal,
}

/// Why a holding could not be rounded.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum RoundingError {
    /// The rounded total leaves a parcel that held units with none. The
    /// per-parcel model keeps each parcel's whole cost base with its own
    /// units, so a parcel at zero units would strand a cost base no sale can
    /// ever reach.
    #[error("the rounded holding leaves parcel {0} with no units")]
    ParcelEmptied(TradeId),
    /// A "units received" override that is not positive.
    #[error("units received {0} is not positive")]
    ReceivedNotPositive(Decimal),
    /// A "units received" override finer than the holding's grid.
    #[error("units received {received} is finer than the unit scale {scale}")]
    ReceivedOffGrid { received: Decimal, scale: u32 },
    /// The ratio times the holding is past what a `Decimal` can hold.
    #[error("the holding's post-action count is beyond the representable range")]
    Unrepresentable,
}

/// How many decimal places beyond the grid a cash-in-lieu fraction is carried
/// to (rounded down). The fraction is often non-terminating (a third of a
/// unit), and carried at `Decimal`'s full precision the parcel's stored count
/// leaves no headroom for the exact re-basing arithmetic every later read
/// does with it — so it is cut where no registry, and no cash amount, could
/// tell the difference.
pub const IN_LIEU_PLACES: u32 = 12;

/// `floor(num / den)` on the grid of `scale` decimal places, and the exact
/// remainder `num − floor × den` (in `[0, step × den)`), for positive `den`
/// and non-negative `num`. The quotient is only a first guess: the floor is
/// corrected by multiplication until it provably brackets `num`, so a
/// quotient rounded up onto a grid point cannot pass for one that lies on it.
fn floor_on_grid(num: Decimal, den: Decimal, scale: u32) -> Option<(Decimal, Decimal)> {
    let step = Decimal::new(1, scale);
    let mut floor = num
        .checked_div(den)?
        .round_dp_with_strategy(scale, RoundingStrategy::ToZero);
    while floor.checked_mul(den)? > num {
        floor = floor.checked_sub(step)?;
    }
    while floor.checked_add(step)?.checked_mul(den)? <= num {
        floor = floor.checked_add(step)?;
    }
    let rem = num.checked_sub(floor.checked_mul(den)?)?;
    Some((floor, rem))
}

/// The holding's count after an action re-basing every `old` units into
/// `new`, as the register holds it: its exact count `Σ units × new / old`
/// rounded onto the grid of `scale` decimal places by `rule` — or
/// `units_received` where the holding's statement says otherwise — and that
/// total distributed across `parcels` by largest remainder.
///
/// The distribution: every parcel takes its share rounded down onto the
/// grid, and the grid steps the total has left over go one each, in order, to
/// a parcel the round-down would have emptied, then to the largest
/// remainder, then to the earlier acquisition, then to the lower trade id.
/// Each parcel's share is its exact count `units × new / old` — or, under an
/// override, its pro-rata part of the override `units × received / Σ units` —
/// so every count lies within one grid step of it, and the counts sum to the
/// total exactly.
///
/// Under [`Rounding::CashInLieu`] the total is the rounded-down count and the
/// fraction (exact count less it) rides on the next parcel in that order, as
/// [`ParcelCount::in_lieu`], until the cash-in-lieu Sell takes it.
pub fn round_holding(
    parcels: &[ParcelBefore],
    new: Decimal,
    old: Decimal,
    scale: u32,
    rule: Rounding,
    units_received: Option<Decimal>,
) -> Result<Vec<ParcelCount>, RoundingError> {
    let step = Decimal::new(1, scale);
    let overflow = || RoundingError::Unrepresentable;
    let held = parcels
        .iter()
        .try_fold(Decimal::ZERO, |acc, p| acc.checked_add(p.units))
        .ok_or_else(overflow)?;
    if parcels.is_empty() || held <= Decimal::ZERO {
        return Ok(vec![]);
    }
    // The holding's exact count, `held × new / old`, as a floor and remainder
    // over the divisor `old`.
    let exact_num = held.checked_mul(new).ok_or_else(overflow)?;
    let (exact_floor, exact_rem) = floor_on_grid(exact_num, old, scale).ok_or_else(overflow)?;

    let total = match units_received {
        Some(received) => {
            if received <= Decimal::ZERO {
                return Err(RoundingError::ReceivedNotPositive(received));
            }
            if received.normalize().scale() > scale {
                return Err(RoundingError::ReceivedOffGrid { received, scale });
            }
            received
        }
        None => match rule {
            Rounding::Down | Rounding::CashInLieu => exact_floor,
            Rounding::Up if exact_rem > Decimal::ZERO => {
                exact_floor.checked_add(step).ok_or_else(overflow)?
            }
            Rounding::Up => exact_floor,
            Rounding::Nearest => {
                // The remainder is over the divisor `old`, and a step is
                // `step × old` of it: half a step or more rounds up.
                let doubled = exact_rem.checked_mul(Decimal::TWO).ok_or_else(overflow)?;
                if doubled >= step.checked_mul(old).ok_or_else(overflow)? {
                    exact_floor.checked_add(step).ok_or_else(overflow)?
                } else {
                    exact_floor
                }
            }
        },
    };

    // Each parcel's share as (numerator, divisor): its exact count, or its
    // pro-rata part of an override.
    let (multiplier, divisor) = match units_received {
        Some(received) => (received, held),
        None => (new, old),
    };
    let mut shares: Vec<(Decimal, Decimal)> = Vec::with_capacity(parcels.len());
    for p in parcels {
        let num = p.units.checked_mul(multiplier).ok_or_else(overflow)?;
        shares.push(floor_on_grid(num, divisor, scale).ok_or_else(overflow)?);
    }
    let floors = shares
        .iter()
        .try_fold(Decimal::ZERO, |acc, (f, _)| acc.checked_add(*f))
        .ok_or_else(overflow)?;
    // How many grid steps the round-down left over; at most one per parcel
    // (each remainder is under a step) — the order below hands them out.
    let extra = (total.checked_sub(floors).ok_or_else(overflow)? / step)
        .to_usize()
        .ok_or_else(overflow)?;

    let mut order: Vec<usize> = (0..parcels.len()).collect();
    order.sort_by(|&a, &b| {
        let emptied = |i: usize| shares[i].0.is_zero() && parcels[i].units > Decimal::ZERO;
        emptied(b)
            .cmp(&emptied(a))
            .then_with(|| shares[b].1.cmp(&shares[a].1))
            .then_with(|| parcels[a].acquired.cmp(&parcels[b].acquired))
            .then_with(|| parcels[a].trade_id.0.cmp(&parcels[b].trade_id.0))
    });

    let mut counts: Vec<ParcelCount> = parcels
        .iter()
        .zip(&shares)
        .map(|(p, (floor, _))| ParcelCount {
            trade_id: p.trade_id,
            before: p.units,
            after: *floor,
            in_lieu: Decimal::ZERO,
        })
        .collect();
    for &i in order.iter().take(extra) {
        counts[i].after = counts[i].after.checked_add(step).ok_or_else(overflow)?;
    }

    // Cash in lieu: the exact count less the rounded-down total, carried on
    // the next parcel in the distribution's order — to
    // [`IN_LIEU_PLACES`] places beyond the grid, rounded down.
    if rule == Rounding::CashInLieu {
        let fraction = exact_num
            .checked_sub(total.checked_mul(old).ok_or_else(overflow)?)
            .ok_or_else(overflow)?
            .checked_div(old)
            .ok_or_else(overflow)?
            .round_dp_with_strategy(scale + IN_LIEU_PLACES, RoundingStrategy::ToZero);
        if fraction > Decimal::ZERO
            && let Some(&i) = order.get(extra)
        {
            let carried = counts[i].after;
            let after = carried.checked_add(fraction).ok_or_else(overflow)?;
            counts[i].after = after;
            // Recomputed from the stored figure, so the two always reconcile
            // to the grid count exactly.
            counts[i].in_lieu = after - carried;
        }
    }

    if let Some(emptied) = counts.iter().find(|c| c.after.is_zero()) {
        return Err(RoundingError::ParcelEmptied(emptied.trade_id));
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::dec as d;
    use proptest::prelude::*;

    fn day(n: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2020, 1, n).unwrap()
    }

    fn parcel(id: i64, acquired: u32, units: &str) -> ParcelBefore {
        ParcelBefore {
            trade_id: TradeId(id),
            acquired: day(acquired),
            units: units.parse().unwrap(),
        }
    }

    fn afters(counts: &[ParcelCount]) -> Vec<Decimal> {
        counts.iter().map(|c| c.after).collect()
    }

    #[test]
    fn two_equal_parcels_consolidated_one_for_three_are_seventeen_and_sixteen() {
        // The section's own example: 100 units, 1-for-3, rounded down is 33,
        // never 16.67 + 16.67 — the earlier parcel takes the tied step.
        let parcels = [parcel(1, 1, "50"), parcel(2, 2, "50")];
        let counts =
            round_holding(&parcels, Decimal::ONE, d("3"), 0, Rounding::Down, None).unwrap();
        assert_eq!(afters(&counts), [d("17"), d("16")]);
        let up = round_holding(&parcels, Decimal::ONE, d("3"), 0, Rounding::Up, None).unwrap();
        assert_eq!(afters(&up), [d("17"), d("17")]);
        let nearest =
            round_holding(&parcels, Decimal::ONE, d("3"), 0, Rounding::Nearest, None).unwrap();
        assert_eq!(afters(&nearest), [d("17"), d("16")]);
    }

    #[test]
    fn the_largest_remainder_takes_the_step() {
        // 1-for-10 of 15 and 85: shares 1.5 and 8.5 → total 10 down; tie on
        // remainder, earlier acquisition takes it.
        let parcels = [parcel(1, 2, "14"), parcel(2, 1, "86")];
        // shares 1.4 / 8.6 → floors 1 / 8, total 10, one step to 0.6.
        let counts =
            round_holding(&parcels, Decimal::ONE, d("10"), 0, Rounding::Down, None).unwrap();
        assert_eq!(afters(&counts), [d("1"), d("9")]);
    }

    #[test]
    fn a_parcel_the_round_down_would_empty_takes_a_step_first() {
        // 1-for-10 of 3 and 97: shares 0.3 / 9.7 → total 10; the 3-unit parcel
        // would be emptied, so it takes the step despite the smaller remainder.
        let parcels = [parcel(1, 1, "3"), parcel(2, 2, "97")];
        let counts =
            round_holding(&parcels, Decimal::ONE, d("10"), 0, Rounding::Down, None).unwrap();
        assert_eq!(afters(&counts), [d("1"), d("9")]);
    }

    #[test]
    fn a_holding_too_small_for_every_parcel_is_refused() {
        let parcels = [parcel(1, 1, "3"), parcel(2, 2, "3")];
        assert_eq!(
            round_holding(&parcels, Decimal::ONE, d("10"), 0, Rounding::Down, None),
            Err(RoundingError::ParcelEmptied(TradeId(1)))
        );
    }

    #[test]
    fn cash_in_lieu_carries_the_fraction_on_the_next_parcel() {
        // 100 units 1-for-3: 33 rounded down, the fraction 1/3 carried on the
        // parcel next in line (the second, which did not take the step).
        let parcels = [parcel(1, 1, "50"), parcel(2, 2, "50")];
        let counts = round_holding(
            &parcels,
            Decimal::ONE,
            d("3"),
            0,
            Rounding::CashInLieu,
            None,
        )
        .unwrap();
        assert_eq!(counts[0].after, d("17"));
        assert_eq!(counts[0].in_lieu, Decimal::ZERO);
        assert_eq!(counts[1].after - counts[1].in_lieu, d("16"));
        assert_eq!(counts[1].in_lieu, d("0.333333333333"));
    }

    #[test]
    fn a_whole_holding_forfeited_for_cash_is_one_parcel_of_fraction() {
        // 2 units 1-for-10: nothing left on the register, the whole 0.2 paid
        // out in cash — carried on the one parcel until its Sell.
        let parcels = [parcel(1, 1, "2")];
        let counts = round_holding(
            &parcels,
            Decimal::ONE,
            d("10"),
            0,
            Rounding::CashInLieu,
            None,
        )
        .unwrap();
        assert_eq!(counts[0].after, d("0.2"));
        assert_eq!(counts[0].in_lieu, d("0.2"));
    }

    #[test]
    fn an_override_replaces_the_total_and_is_shared_pro_rata() {
        let parcels = [parcel(1, 1, "50"), parcel(2, 2, "150")];
        let counts = round_holding(
            &parcels,
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Down,
            Some(d("67")),
        )
        .unwrap();
        // 67 × 50/200 = 16.75 → 17; 67 × 150/200 = 50.25 → 50.
        assert_eq!(afters(&counts), [d("17"), d("50")]);
        assert_eq!(
            round_holding(
                &parcels,
                Decimal::ONE,
                d("3"),
                0,
                Rounding::Down,
                Some(d("6.5"))
            ),
            Err(RoundingError::ReceivedOffGrid {
                received: d("6.5"),
                scale: 0
            })
        );
        assert_eq!(
            round_holding(
                &parcels,
                Decimal::ONE,
                d("3"),
                0,
                Rounding::Down,
                Some(d("0"))
            ),
            Err(RoundingError::ReceivedNotPositive(d("0")))
        );
    }

    #[test]
    fn a_finer_grid_rounds_at_its_own_place() {
        let parcels = [parcel(1, 1, "10.01")];
        let counts =
            round_holding(&parcels, Decimal::ONE, d("3"), 2, Rounding::Down, None).unwrap();
        assert_eq!(afters(&counts), [d("3.33")]);
    }

    proptest! {
        /// For every non-`Exact` rule the stored parcel counts sum to the
        /// rounded holding total, every count is on the grid (bar the
        /// cash-in-lieu fraction, which reconciles to it exactly), and each
        /// differs from its exact share by less than one grid step.
        #[test]
        fn counts_sum_to_the_rounded_total_on_the_grid_within_a_step_of_exact(
            units in prop::collection::vec(1i64..=5_000, 1..=6),
            unit_places in 0u32..=2,
            new in 1i64..=12,
            old in 1i64..=12,
            scale in 0u32..=3,
            rule in prop_oneof![
                Just(Rounding::Down),
                Just(Rounding::Up),
                Just(Rounding::Nearest),
                Just(Rounding::CashInLieu),
            ],
        ) {
            let parcels: Vec<ParcelBefore> = units
                .iter()
                .enumerate()
                .map(|(i, u)| ParcelBefore {
                    trade_id: TradeId(i64::try_from(i).unwrap() + 1),
                    acquired: day(u32::try_from(i).unwrap() + 1),
                    units: Decimal::new(*u, unit_places.min(scale)),
                })
                .collect();
            let (new, old) = (Decimal::from(new), Decimal::from(old));
            let step = Decimal::new(1, scale);
            let Ok(counts) = round_holding(&parcels, new, old, scale, rule, None) else {
                // Refused only where the total cannot give every parcel a
                // step — check that is really so.
                let held: Decimal = parcels.iter().map(|p| p.units).sum();
                let total_steps = (held * new / old / step).ceil();
                prop_assert!(total_steps < Decimal::from(parcels.len()) + Decimal::ONE);
                return Ok(());
            };
            let held: Decimal = parcels.iter().map(|p| p.units).sum();
            let exact = held * new / old;
            let grid_total: Decimal = counts.iter().map(|c| c.after - c.in_lieu).sum();
            let expected = match rule {
                Rounding::Down | Rounding::CashInLieu => {
                    exact.round_dp_with_strategy(scale, RoundingStrategy::ToZero)
                }
                Rounding::Up => exact.round_dp_with_strategy(scale, RoundingStrategy::AwayFromZero),
                Rounding::Nearest => {
                    exact.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero)
                }
            };
            prop_assert_eq!(grid_total, expected);
            for (c, p) in counts.iter().zip(&parcels) {
                let on_grid = c.after - c.in_lieu;
                prop_assert_eq!(on_grid, on_grid.round_dp(scale));
                let share = p.units * new / old;
                prop_assert!((on_grid - share).abs() < step, "{on_grid} vs {share}");
            }
            let in_lieu: Decimal = counts.iter().map(|c| c.in_lieu).sum();
            if rule == Rounding::CashInLieu {
                prop_assert!(
                    (grid_total + in_lieu - exact).abs() < Decimal::new(1, scale + IN_LIEU_PLACES)
                );
            } else {
                prop_assert_eq!(in_lieu, Decimal::ZERO);
            }
        }
    }
}
