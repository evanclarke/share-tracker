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
//! the largest remainders. The distribution is **stable**: a parcel that
//! already has a stored count from an earlier computation keeps it wherever
//! the new total allows, and the new, back-dated or changed parcels absorb the
//! difference — a registry does not redistribute (TODO *Recompute on write
//! instead of freezing*). Rounding is no CGT event (TD 2000/10, s 112-25):
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
    /// The count stored for it by the last recompute, if any — what a
    /// registry has already put on the register for this parcel, kept where
    /// the holding's rounded total allows ([`round_holding`]'s stability).
    pub prior: Option<Prior>,
}

/// A parcel's stored count from the last recompute: what the stable
/// distribution keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prior {
    /// The [`ParcelBefore::units`] it was computed from — a parcel whose units
    /// before the action have changed since (a back-dated sale of it, an
    /// edited quantity, an earlier action's recount) is a **changed** parcel,
    /// released before any unchanged one.
    pub before: Decimal,
    /// Its count on the grid (the stored count less any cash-in-lieu
    /// fraction it carried).
    pub on_grid: Decimal,
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
/// total distributed across `parcels`.
///
/// Each parcel's **share** is its exact count `units × new / old` — or, under
/// an override, its pro-rata part of the override `units × received / Σ
/// units` — and every count this returns lies within one grid step of its
/// share: its share rounded down onto the grid (its *floor*), or one step
/// more where the share is not already on the grid. The counts sum to the
/// total exactly.
///
/// **Stability.** A registry does not redistribute a holding it has already
/// rounded, so a parcel with a [`Prior`] count keeps it wherever the total
/// allows, and the parcels without one — new, back-dated, or released —
/// absorb the difference. Precisely:
///
/// 1. A parcel is *keepable* when its prior count is still a valid count for
///    it: on the grid, positive, and its floor or (where the share is off the
///    grid) its floor plus one step. A prior made on another grid, under
///    another ratio or override, or for units that have since changed past
///    it, is not, and that parcel is *free*.
/// 2. The free parcels must absorb `rest = total − Σ kept counts`: every free
///    parcel takes its floor, plus one step for `(rest − Σ free floors) /
///    step` of them, so that is possible exactly when `Σ free floors + one
///    step per free parcel whose floor is zero ≤ rest ≤ Σ free floors + one
///    step per free parcel whose share is off the grid` (a parcel whose floor
///    is zero must take a step: a parcel with no units would strand its cost
///    base).
/// 3. While it is not, kept parcels are released one at a time — **changed**
///    ones first (their [`Prior::before`] differs from their units now), then
///    the latest acquisition, then the higher trade id — each joining the free
///    parcels with its count. Releasing never narrows the band in (2) (a
///    released count lies inside the band its parcel adds), so the first
///    release that fits is taken; with every parcel released this is the
///    distribution without priors below, which fits whenever the total gives
///    every parcel a step.
/// 4. The free parcels' extra steps go one each, in order, to a parcel the
///    round-down would have emptied, then to the largest remainder, then to
///    the earlier acquisition, then to the lower trade id.
///
/// With no priors (a first computation, or every parcel released) that is
/// plain largest remainder over the whole holding. Every step is
/// deterministic: the result is a function of the parcels, their priors, the
/// ratio, the grid, the rule and the override.
///
/// Under [`Rounding::CashInLieu`] the total is the rounded-down count and the
/// fraction (exact count less it) rides on the first parcel in (4)'s order
/// whose count is its floor, as [`ParcelCount::in_lieu`], until the
/// cash-in-lieu Sell takes it.
pub fn round_holding(
    parcels: &[ParcelBefore],
    new: Decimal,
    old: Decimal,
    scale: u32,
    rule: Rounding,
    units_received: Option<Decimal>,
) -> Result<Vec<ParcelCount>, RoundingError> {
    let overflow = || RoundingError::Unrepresentable;
    let held = parcels
        .iter()
        .try_fold(Decimal::ZERO, |acc, p| acc.checked_add(p.units))
        .ok_or_else(overflow)?;
    if parcels.is_empty() || held <= Decimal::ZERO {
        return Ok(vec![]);
    }
    let total = match units_received {
        Some(received) => checked_received(received, scale)?,
        None => rounded_total(held, new, old, scale, rule)?,
    };
    let in_lieu = in_lieu_fraction(held, total, new, old, scale, rule)?;
    // Each parcel's share as (numerator, divisor): its exact count, or its
    // pro-rata part of an override.
    let share_of = match units_received {
        Some(received) => (received, held),
        None => (new, old),
    };
    distribute(parcels, total, share_of, scale, in_lieu)
}

/// What the register held for one holding at a share split's or bonus issue's
/// **record date**, where that differs from its parcels' units before the
/// action — the input [`round_registered_holding`] rounds.
///
/// The parcels a re-base converts are the ones **traded** before the action's
/// date (a trade on or after it is stated in the post-action basis), net of
/// the sales traded before it. The register at the record date is read off
/// **settlement** dates instead (CLAUDE.md, Financial correctness). The two
/// differ only by a trade that straddles them; a Sell traded before the
/// action's date but settling after the record date still had its units on
/// the register when the holding was rounded, and delivers its own count out
/// of the rounded holding afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Register {
    /// Units of the holding on the register at the record date, in the basis
    /// in force immediately before the action: its registered parcels' units
    /// before the action plus every straddling Sell's below.
    pub units: Decimal,
    /// Each straddling Sell's units, in the same basis — a settlement
    /// obligation the clearing house converts on its own, delivered out of
    /// the holding's rounded count ([`Rounding::for_obligation`]).
    pub delivered: Vec<Decimal>,
}

/// [`round_holding`] for a holding whose register at the record date differs
/// from its parcels' units before the action, because Sells traded before the
/// action settled after the record date ([`Register`]).
///
/// The **register** is what the issuer's rule rounds: the holding's total is
/// `register.units × new / old` rounded onto the grid by `rule` — or
/// `units_received`, which states the register's figure. Each straddling Sell
/// is a settlement obligation converted on its own, `units × new / old`
/// rounded by [`Rounding::for_obligation`], and delivered out of that total;
/// what is left is the parcels' count after the action, distributed across
/// them as [`round_holding`] distributes an override — each parcel's share its
/// pro-rata part of it, every count within a step of its share, with the same
/// stability and the same tie-breaks. A cash-in-lieu fraction is the
/// register's (the registry pays the registered holder for the whole
/// holding's fraction), carried on one parcel as for [`round_holding`].
///
/// A total the deliveries leave with nothing for a parcel that still holds
/// units is refused [`RoundingError::ParcelEmptied`], as any rounding that
/// would empty a parcel is.
pub fn round_registered_holding(
    parcels: &[ParcelBefore],
    register: &Register,
    new: Decimal,
    old: Decimal,
    scale: u32,
    rule: Rounding,
    units_received: Option<Decimal>,
) -> Result<Vec<ParcelCount>, RoundingError> {
    let overflow = || RoundingError::Unrepresentable;
    let held = parcels
        .iter()
        .try_fold(Decimal::ZERO, |acc, p| acc.checked_add(p.units))
        .ok_or_else(overflow)?;
    if parcels.is_empty() || held <= Decimal::ZERO {
        return Ok(vec![]);
    }
    let registered = match units_received {
        Some(received) => checked_received(received, scale)?,
        None => rounded_total(register.units, new, old, scale, rule)?,
    };
    let mut total = registered;
    for &units in &register.delivered {
        let delivered = rounded_total(units, new, old, scale, rule.for_obligation())?;
        total = total.checked_sub(delivered).ok_or_else(overflow)?;
    }
    if total <= Decimal::ZERO {
        return Err(RoundingError::ParcelEmptied(parcels[0].trade_id));
    }
    let in_lieu = in_lieu_fraction(register.units, registered, new, old, scale, rule)?;
    distribute(parcels, total, (total, held), scale, in_lieu)
}

impl Rounding {
    /// The rounding a **settlement obligation** straddling the action is
    /// converted by: the action's own, except that a cash-in-lieu fraction
    /// belongs to the holder registered at the record date (the registry pays
    /// it for the whole registered holding), so an obligation is rounded down
    /// and carries none.
    pub fn for_obligation(self) -> Self {
        match self {
            Rounding::CashInLieu | Rounding::Down => Rounding::Down,
            Rounding::Up => Rounding::Up,
            Rounding::Nearest => Rounding::Nearest,
        }
    }
}

/// A "units received" override, checked: positive and on the grid.
fn checked_received(received: Decimal, scale: u32) -> Result<Decimal, RoundingError> {
    if received <= Decimal::ZERO {
        return Err(RoundingError::ReceivedNotPositive(received));
    }
    if received.normalize().scale() > scale {
        return Err(RoundingError::ReceivedOffGrid { received, scale });
    }
    Ok(received)
}

/// `units × new / old` rounded onto the grid of `scale` places by `rule`.
fn rounded_total(
    units: Decimal,
    new: Decimal,
    old: Decimal,
    scale: u32,
    rule: Rounding,
) -> Result<Decimal, RoundingError> {
    let step = Decimal::new(1, scale);
    let overflow = || RoundingError::Unrepresentable;
    // The exact count, `units × new / old`, as a floor and remainder over the
    // divisor `old`.
    let exact_num = units.checked_mul(new).ok_or_else(overflow)?;
    let (exact_floor, exact_rem) = floor_on_grid(exact_num, old, scale).ok_or_else(overflow)?;
    Ok(match rule {
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
    })
}

/// Under [`Rounding::CashInLieu`], the fraction of `units × new / old` that
/// `total` leaves — to [`IN_LIEU_PLACES`] places beyond the grid, rounded
/// down; `None` under any other rule.
fn in_lieu_fraction(
    units: Decimal,
    total: Decimal,
    new: Decimal,
    old: Decimal,
    scale: u32,
    rule: Rounding,
) -> Result<Option<Decimal>, RoundingError> {
    if rule != Rounding::CashInLieu {
        return Ok(None);
    }
    let overflow = || RoundingError::Unrepresentable;
    let exact_num = units.checked_mul(new).ok_or_else(overflow)?;
    Ok(Some(
        exact_num
            .checked_sub(total.checked_mul(old).ok_or_else(overflow)?)
            .ok_or_else(overflow)?
            .checked_div(old)
            .ok_or_else(overflow)?
            .round_dp_with_strategy(scale + IN_LIEU_PLACES, RoundingStrategy::ToZero),
    ))
}

/// Distribute `total` across `parcels` — each parcel's share `units ×
/// share_of.0 / share_of.1` — by [`round_holding`]'s steps (1)–(4), carrying
/// `in_lieu` (a cash-in-lieu fraction) on one parcel.
fn distribute(
    parcels: &[ParcelBefore],
    total: Decimal,
    (multiplier, divisor): (Decimal, Decimal),
    scale: u32,
    in_lieu: Option<Decimal>,
) -> Result<Vec<ParcelCount>, RoundingError> {
    let step = Decimal::new(1, scale);
    let overflow = || RoundingError::Unrepresentable;
    let mut shares: Vec<(Decimal, Decimal)> = Vec::with_capacity(parcels.len());
    for p in parcels {
        let num = p.units.checked_mul(multiplier).ok_or_else(overflow)?;
        shares.push(floor_on_grid(num, divisor, scale).ok_or_else(overflow)?);
    }

    // (4)'s order, over every parcel.
    let emptied = |i: usize| shares[i].0.is_zero() && parcels[i].units > Decimal::ZERO;
    let mut order: Vec<usize> = (0..parcels.len()).collect();
    order.sort_by(|&a, &b| {
        emptied(b)
            .cmp(&emptied(a))
            .then_with(|| shares[b].1.cmp(&shares[a].1))
            .then_with(|| parcels[a].acquired.cmp(&parcels[b].acquired))
            .then_with(|| parcels[a].trade_id.0.cmp(&parcels[b].trade_id.0))
    });

    // (1): the keepable priors.
    let mut kept: Vec<Option<Decimal>> = Vec::with_capacity(parcels.len());
    for (p, (floor, rem)) in parcels.iter().zip(&shares) {
        let ceiling = if rem.is_zero() {
            *floor
        } else {
            floor.checked_add(step).ok_or_else(overflow)?
        };
        kept.push(p.prior.map(|prior| prior.on_grid).filter(|count| {
            *count > Decimal::ZERO
                && count.normalize().scale() <= scale
                && count >= floor
                && *count <= ceiling
        }));
    }
    // (3)'s release order: changed first, then the latest acquisition, then
    // the higher trade id.
    let mut release: Vec<usize> = (0..parcels.len()).filter(|&i| kept[i].is_some()).collect();
    release.sort_by(|&a, &b| {
        let changed = |i: usize| {
            parcels[i]
                .prior
                .is_some_and(|p| p.before != parcels[i].units)
        };
        changed(b)
            .cmp(&changed(a))
            .then_with(|| parcels[b].acquired.cmp(&parcels[a].acquired))
            .then_with(|| parcels[b].trade_id.0.cmp(&parcels[a].trade_id.0))
    });
    let mut release = release.into_iter();

    // (2)/(3): release until the free parcels can absorb the rest.
    let rest = loop {
        let kept_sum = kept
            .iter()
            .flatten()
            .try_fold(Decimal::ZERO, |acc, c| acc.checked_add(*c))
            .ok_or_else(overflow)?;
        let rest = total.checked_sub(kept_sum).ok_or_else(overflow)?;
        let (mut lower, mut must, mut may) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        for i in (0..parcels.len()).filter(|&i| kept[i].is_none()) {
            lower = lower.checked_add(shares[i].0).ok_or_else(overflow)?;
            if emptied(i) {
                must += step;
            }
            if !shares[i].1.is_zero() {
                may += step;
            }
        }
        let fits = rest >= lower.checked_add(must).ok_or_else(overflow)?
            && rest <= lower.checked_add(may).ok_or_else(overflow)?;
        if fits {
            break rest;
        }
        match release.next() {
            Some(i) => kept[i] = None,
            // Nothing left to release: the plain distribution, which fits
            // unless the total cannot give every parcel a step (refused
            // below).
            None => break rest,
        }
    };

    let mut counts: Vec<ParcelCount> = parcels
        .iter()
        .zip(&shares)
        .zip(&kept)
        .map(|((p, (floor, _)), kept)| ParcelCount {
            trade_id: p.trade_id,
            before: p.units,
            after: kept.unwrap_or(*floor),
            in_lieu: Decimal::ZERO,
        })
        .collect();
    // (4): the free parcels' extra steps.
    let free_floors = counts
        .iter()
        .zip(&kept)
        .filter(|(_, k)| k.is_none())
        .try_fold(Decimal::ZERO, |acc, (c, _)| acc.checked_add(c.after))
        .ok_or_else(overflow)?;
    let extra = (rest.checked_sub(free_floors).ok_or_else(overflow)? / step)
        .to_usize()
        .ok_or_else(overflow)?;
    for &i in order.iter().filter(|&&i| kept[i].is_none()).take(extra) {
        counts[i].after = counts[i].after.checked_add(step).ok_or_else(overflow)?;
    }

    // Cash in lieu: the exact count less the rounded-down total, carried on
    // the first parcel in the distribution's order whose count is its floor.
    if let Some(fraction) = in_lieu
        && fraction > Decimal::ZERO
        && let Some(&i) = order.iter().find(|&&i| counts[i].after == shares[i].0)
    {
        let carried = counts[i].after;
        let after = carried.checked_add(fraction).ok_or_else(overflow)?;
        counts[i].after = after;
        // Recomputed from the stored figure, so the two always reconcile
        // to the grid count exactly.
        counts[i].in_lieu = after - carried;
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
            prior: None,
        }
    }

    /// `parcel`, with the count a previous computation stored for it over the
    /// same units.
    fn kept(id: i64, acquired: u32, units: &str, count: &str) -> ParcelBefore {
        ParcelBefore {
            prior: Some(Prior {
                before: units.parse().unwrap(),
                on_grid: count.parse().unwrap(),
            }),
            ..parcel(id, acquired, units)
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

    #[test]
    fn a_back_dated_parcel_absorbs_the_difference_and_a_sold_count_stays() {
        // The TODO's own case: parcel A of 100, 1-for-3 rounded up, stored 34
        // (and sold). A back-dated B of 50 makes the holding 150 → 50: A
        // keeps its 34 and B takes 16 — not the 33 / 17 a fresh largest
        // remainder hands out.
        let a = round_holding(
            &[parcel(2, 5, "100")],
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Up,
            None,
        )
        .unwrap();
        assert_eq!(afters(&a), [d("34")]);
        let fresh = round_holding(
            &[parcel(2, 5, "100"), parcel(3, 1, "50")],
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Up,
            None,
        )
        .unwrap();
        assert_eq!(afters(&fresh), [d("33"), d("17")]);
        let stable = round_holding(
            &[kept(2, 5, "100", "34"), parcel(3, 1, "50")],
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Up,
            None,
        )
        .unwrap();
        assert_eq!(afters(&stable), [d("34"), d("16")]);
    }

    #[test]
    fn a_kept_count_is_released_only_when_the_total_forces_it() {
        // 50 and 50, 1-for-3 rounded up: 17 and 17. A back-dated parcel of 1
        // has a share of a third: the total is 34, nothing is left over for
        // it, and it must have a unit — so the latest kept parcel (day 2) is
        // released, and it and the newcomer share its 17: floors 16 and 0,
        // one step over, to the parcel the round-down would empty.
        let up = round_holding(
            &[
                kept(1, 1, "50", "17"),
                kept(2, 2, "50", "17"),
                parcel(3, 3, "1"),
            ],
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Up,
            None,
        )
        .unwrap();
        assert_eq!(afters(&up), [d("17"), d("16"), d("1")]);

        // Rounded down (17 and 16, total 33) one release is not enough —
        // parcels 2 and 3 have floors of 16 and 0 against its 16 — so parcel
        // 1 goes too, and the whole holding is distributed afresh.
        let down = round_holding(
            &[
                kept(1, 1, "50", "17"),
                kept(2, 2, "50", "16"),
                parcel(3, 3, "1"),
            ],
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Down,
            None,
        )
        .unwrap();
        assert_eq!(afters(&down), [d("16"), d("16"), d("1")]);
    }

    #[test]
    fn a_changed_parcel_is_released_before_an_unchanged_one() {
        // 50, 50 and 50, 1-for-3 down: 17, 17, 16. Parcel 1 (the earliest)
        // has since been partly sold before the action, to 49: the holding is
        // 149 → 49, and kept as they were the three sum to 50. Its 17 is
        // still a valid count for it (share 16.33), but it is the changed
        // parcel, so it goes first — not the latest acquisition — and takes
        // the 16 left.
        let counts = round_holding(
            &[
                ParcelBefore {
                    prior: Some(Prior {
                        before: d("50"),
                        on_grid: d("17"),
                    }),
                    ..parcel(1, 1, "49")
                },
                kept(2, 2, "50", "17"),
                kept(3, 3, "50", "16"),
            ],
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Down,
            None,
        )
        .unwrap();
        assert_eq!(afters(&counts), [d("16"), d("17"), d("16")]);
    }

    #[test]
    fn a_count_off_the_new_grid_or_past_its_share_is_not_kept() {
        // Stored on whole units, recomputed on two places: 17 is no longer
        // within a step (0.01) of 16.666…, nor is 16, so both are free and
        // the holding is distributed afresh on the finer grid.
        let counts = round_holding(
            &[kept(1, 1, "50", "17"), kept(2, 2, "50", "16")],
            Decimal::ONE,
            d("3"),
            2,
            Rounding::Down,
            None,
        )
        .unwrap();
        assert_eq!(afters(&counts), [d("16.67"), d("16.66")]);
        // And back: 16.67 is off the whole-unit grid.
        let counts = round_holding(
            &[kept(1, 1, "50", "16.67"), kept(2, 2, "50", "16.66")],
            Decimal::ONE,
            d("3"),
            0,
            Rounding::Down,
            None,
        )
        .unwrap();
        assert_eq!(afters(&counts), [d("17"), d("16")]);
    }

    /// The band of the distribution's step (2): whether the parcels without a
    /// kept count can absorb `rest`, each taking its floor or (where its
    /// share is off the grid) one step more, every zero floor taking the step.
    #[test]
    fn a_straddling_sale_is_rounded_with_the_register_and_delivered_out_of_it() {
        // Parcels of 48 and 50 by trade date; 2 more units were sold before
        // the action but settled after the record date, so the register held
        // 100. 1-for-3, rounded down: the register is 33, the 2-unit
        // obligation delivers 0, and the parcels share 33 pro rata.
        let register = Register {
            units: d("100"),
            delivered: vec![d("2")],
        };
        let counts = round_registered_holding(
            &[parcel(1, 1, "48"), parcel(2, 2, "50")],
            &register,
            d("1"),
            d("3"),
            0,
            Rounding::Down,
            None,
        )
        .unwrap();
        assert_eq!(afters(&counts), [d("16"), d("17")]);
        // Rounded up, the register is 34 and the obligation delivers 1.
        let counts = round_registered_holding(
            &[parcel(1, 1, "48"), parcel(2, 2, "50")],
            &register,
            d("1"),
            d("3"),
            0,
            Rounding::Up,
            None,
        )
        .unwrap();
        assert_eq!(afters(&counts).iter().sum::<Decimal>(), d("33"));
        // Under cash in lieu the obligation is rounded down and the fraction
        // is the register's: 100/3 − 33 = 1/3, on one parcel.
        let counts = round_registered_holding(
            &[parcel(1, 1, "48"), parcel(2, 2, "50")],
            &register,
            d("1"),
            d("3"),
            0,
            Rounding::CashInLieu,
            None,
        )
        .unwrap();
        let in_lieu: Decimal = counts.iter().map(|c| c.in_lieu).sum();
        assert_eq!(in_lieu, d("0.333333333333"));
    }

    #[test]
    fn deliveries_that_leave_nothing_for_a_parcel_are_refused() {
        // A register of 7 rounds to 2; two 3-unit obligations deliver 1 each.
        let register = Register {
            units: d("7"),
            delivered: vec![d("3"), d("3")],
        };
        assert_eq!(
            round_registered_holding(
                &[parcel(1, 1, "1")],
                &register,
                d("1"),
                d("3"),
                0,
                Rounding::Down,
                None,
            ),
            Err(RoundingError::ParcelEmptied(TradeId(1)))
        );
    }

    fn absorbable(free: &[(Decimal, bool)], rest: Decimal, step: Decimal) -> bool {
        let lower: Decimal = free.iter().map(|(f, _)| *f).sum();
        let must = Decimal::from(free.iter().filter(|(f, _)| f.is_zero()).count()) * step;
        let may = Decimal::from(free.iter().filter(|(_, off)| *off).count()) * step;
        rest >= lower + must && rest <= lower + may
    }

    proptest! {
        /// Stability over a recompute: a holding rounded once, then changed —
        /// back-dated parcels added, parcels dropped, parcels' units changed —
        /// and recomputed with the first counts as priors. Every invariant of
        /// a fresh distribution still holds (the counts sum to the rounded
        /// total, on the grid, each positive and within a step of its share),
        /// and if any parcel whose units did not change moved, keeping every
        /// such parcel's count would have left the others unable to absorb
        /// the total — the only thing that forces a move.
        #[test]
        fn a_recompute_keeps_every_count_the_total_allows(
            units in prop::collection::vec(1i64..=500, 1..=5),
            added in prop::collection::vec((1i64..=500, 0u32..=20), 0..=3),
            dropped in prop::collection::vec(any::<bool>(), 5),
            changed in prop::collection::vec(prop::option::of(1i64..=500), 5),
            new in 1i64..=6,
            old in 1i64..=6,
            scale in 0u32..=1,
            rule in prop_oneof![
                Just(Rounding::Down),
                Just(Rounding::Up),
                Just(Rounding::Nearest),
                Just(Rounding::CashInLieu),
            ],
        ) {
            let (new, old) = (Decimal::from(new), Decimal::from(old));
            let step = Decimal::new(1, scale);
            let first: Vec<ParcelBefore> = units
                .iter()
                .enumerate()
                .map(|(i, u)| ParcelBefore {
                    trade_id: TradeId(i64::try_from(i).unwrap() + 1),
                    acquired: day(u32::try_from(i).unwrap() + 10),
                    units: Decimal::from(*u),
                    prior: None,
                })
                .collect();
            let Ok(stored) = round_holding(&first, new, old, scale, rule, None) else {
                return Ok(());
            };
            let mut second: Vec<ParcelBefore> = Vec::new();
            for (i, (p, c)) in first.iter().zip(&stored).enumerate() {
                if dropped[i] && i > 0 {
                    continue;
                }
                second.push(ParcelBefore {
                    units: changed[i].map_or(p.units, Decimal::from),
                    prior: Some(Prior { before: p.units, on_grid: c.after - c.in_lieu }),
                    ..*p
                });
            }
            for (j, (u, day_n)) in added.iter().enumerate() {
                second.push(ParcelBefore {
                    trade_id: TradeId(100 + i64::try_from(j).unwrap()),
                    acquired: day(day_n + 1),
                    units: Decimal::from(*u),
                    prior: None,
                });
            }
            let Ok(counts) = round_holding(&second, new, old, scale, rule, None) else {
                // Refused only where the plain distribution is.
                let plain: Vec<ParcelBefore> =
                    second.iter().map(|p| ParcelBefore { prior: None, ..*p }).collect();
                prop_assert!(round_holding(&plain, new, old, scale, rule, None).is_err());
                return Ok(());
            };
            let held: Decimal = second.iter().map(|p| p.units).sum();
            let exact = held * new / old;
            let total = match rule {
                Rounding::Down | Rounding::CashInLieu => {
                    exact.round_dp_with_strategy(scale, RoundingStrategy::ToZero)
                }
                Rounding::Up => exact.round_dp_with_strategy(scale, RoundingStrategy::AwayFromZero),
                Rounding::Nearest => {
                    exact.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero)
                }
            };
            let on_grid: Vec<Decimal> = counts.iter().map(|c| c.after - c.in_lieu).collect();
            prop_assert_eq!(on_grid.iter().copied().sum::<Decimal>(), total);
            let mut free = Vec::new();
            let mut kept_sum = Decimal::ZERO;
            let mut moved = false;
            for ((p, count), c) in second.iter().zip(&on_grid).zip(&counts) {
                prop_assert_eq!(*count, count.round_dp(scale));
                // Positive — bar a holding forfeited whole for cash, whose
                // one parcel carries nothing but the fraction.
                prop_assert!(c.after > Decimal::ZERO);
                let share = p.units * new / old;
                prop_assert!((*count - share).abs() < step, "{count} vs {share}");
                let floor = (share / step).floor() * step;
                match p.prior.filter(|prior| prior.before == p.units) {
                    Some(prior) => {
                        kept_sum += prior.on_grid;
                        moved |= prior.on_grid != *count;
                    }
                    None => free.push((floor, floor != share)),
                }
            }
            if moved {
                prop_assert!(
                    !absorbable(&free, total - kept_sum, step),
                    "an unchanged parcel moved though the others could absorb the total"
                );
            }
        }

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
                    prior: None,
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

        /// The register's rounding: whatever the straddling Sells deliver,
        /// the parcels' counts sum to the register's rounded total less each
        /// delivery's own rounded count, on the grid, each within a step of its
        /// pro-rata share of that — or the holding is refused because that
        /// leaves too little to give every parcel a step.
        #[test]
        fn a_registered_holding_delivers_its_obligations_and_shares_the_rest(
            units in prop::collection::vec(1i64..=500, 1..=4),
            delivered in prop::collection::vec(1i64..=200, 0..=3),
            new in 1i64..=6,
            old in 1i64..=6,
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
                .map(|(i, u)| parcel(i64::try_from(i).unwrap() + 1, 1, &u.to_string()))
                .collect();
            let held: Decimal = parcels.iter().map(|p| p.units).sum();
            let register = Register {
                units: held + delivered.iter().map(|d| Decimal::from(*d)).sum::<Decimal>(),
                delivered: delivered.iter().map(|d| Decimal::from(*d)).collect(),
            };
            let (new, old) = (Decimal::from(new), Decimal::from(old));
            let round = |x: Decimal, rule: Rounding| match rule {
                Rounding::Down | Rounding::CashInLieu => x.floor(),
                Rounding::Up => x.ceil(),
                Rounding::Nearest => {
                    x.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
                }
            };
            let target = round(register.units * new / old, rule)
                - register
                    .delivered
                    .iter()
                    .map(|d| round(*d * new / old, rule.for_obligation()))
                    .sum::<Decimal>();
            let result = round_registered_holding(&parcels, &register, new, old, 0, rule, None);
            let Ok(counts) = result else {
                // Refused only where the shares rounded down leave more
                // parcels at nothing than the leftover steps can reach.
                prop_assert!(matches!(result, Err(RoundingError::ParcelEmptied(_))));
                if target > Decimal::ZERO {
                    let floors: Vec<Decimal> =
                        parcels.iter().map(|p| (p.units * target / held).floor()).collect();
                    let zeros = floors.iter().filter(|f| f.is_zero()).count();
                    let spare = target - floors.iter().copied().sum::<Decimal>();
                    prop_assert!(Decimal::from(zeros) > spare);
                }
                return Ok(());
            };
            let on_grid: Vec<Decimal> = counts.iter().map(|c| c.after - c.in_lieu).collect();
            prop_assert_eq!(on_grid.iter().copied().sum::<Decimal>(), target);
            for (count, p) in on_grid.iter().zip(&parcels) {
                prop_assert_eq!(*count, count.trunc());
                prop_assert!(*count > Decimal::ZERO);
                let scaled = *count * held - p.units * target;
                prop_assert!(scaled.abs() < held, "{count} vs {} × {target} / {held}", p.units);
            }
        }
    }
}
