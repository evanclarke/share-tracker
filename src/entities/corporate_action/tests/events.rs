//! The pure event math over loaded actions: split ratios, re-basing a
//! quantity or a price across the events between two dates, and a
//! return of capital's per-unit entitlement.

use super::*;

#[test]
fn split_ratio_covers_half_open_interval() {
    let splits = vec![
        split_event(d(2024, 6, 1), "2", "1"),
        split_event(d(2025, 1, 1), "1", "10"),
    ];
    // Acquired before both: both apply → 2/10.
    assert_eq!(
        split_ratio(&splits, d(2024, 1, 1), None),
        SplitRatio {
            new: Decimal::from(2),
            old: Decimal::from(10)
        }
    );
    // Acquired on the first conversion date: already post-split — only the
    // second applies.
    assert_eq!(
        split_ratio(&splits, d(2024, 6, 1), None),
        SplitRatio {
            new: Decimal::ONE,
            old: Decimal::from(10)
        }
    );
    // Re-basing to a date on the second conversion: it applies (inclusive end).
    assert_eq!(
        split_ratio(&splits, d(2024, 1, 1), Some(d(2025, 1, 1))),
        SplitRatio {
            new: Decimal::from(2),
            old: Decimal::from(10)
        }
    );
    // Re-basing to a date before the second conversion: only the first.
    assert_eq!(
        split_ratio(&splits, d(2024, 1, 1), Some(d(2024, 12, 31))),
        SplitRatio {
            new: Decimal::from(2),
            old: Decimal::ONE
        }
    );
}

/// A price scales the opposite way to a quantity: what a re-basing event
/// multiplies the unit count by, it divides the per-unit price by. Only
/// events between the price date and the observation restate the figure,
/// so a price observed before one is already in its own day's basis.
#[test]
fn contemporaneous_price_undoes_the_events_between_the_day_and_the_observation() {
    let splits: Vec<PriceBasisEvent> = [split_event(d(2024, 6, 10), "10", "1")]
        .iter()
        .map(PriceBasisEvent::from)
        .collect();
    let observed = |price: &str, on: NaiveDate| {
        contemporaneous_price(price.parse().unwrap(), &splits, d(2024, 6, 7), on)
    };
    // Observed after the split: the provider's series is restated, so the
    // 7 June close comes back ten times the figure served.
    assert_eq!(
        observed("120.888", d(2024, 6, 20)),
        "1208.880".parse().unwrap()
    );
    // Observed before it: nothing to undo.
    assert_eq!(
        observed("1208.88", d(2024, 6, 8)),
        "1208.88".parse().unwrap()
    );
    // Observed on the event date itself — already restated (the interval
    // is closed at that end, as `split_ratio` documents).
    assert_eq!(
        observed("120.888", d(2024, 6, 10)),
        "1208.880".parse().unwrap()
    );
    // An event on the price date has already restated that day's close.
    assert_eq!(
        contemporaneous_price(
            "120.888".parse().unwrap(),
            &splits,
            d(2024, 6, 10),
            d(2024, 7, 1)
        ),
        "120.888".parse().unwrap()
    );
    // A consolidation runs the other way.
    let consol: Vec<PriceBasisEvent> = [split_event(d(2024, 6, 10), "1", "10")]
        .iter()
        .map(PriceBasisEvent::from)
        .collect();
    assert_eq!(
        contemporaneous_price(
            "12088.8".parse().unwrap(),
            &consol,
            d(2024, 6, 7),
            d(2024, 7, 1)
        ),
        "1208.88".parse().unwrap()
    );
}

/// The structural separation: the price re-basing event set is a strict
/// superset of the quantity one. A demerger's factor restates the price
/// series and composes with a split in the same walk, while the quantity
/// walk — which `domain::cost_base`, `domain::open_parcels`, the AMIT
/// re-basing and the allocation-capacity checks all read — never sees it.
#[test]
fn the_price_event_set_is_a_superset_of_the_quantity_event_set() {
    let split = split_event(d(2024, 6, 10), "2", "1");
    // The demerger's derived factor: the security actually closed at 24.90
    // on the last pre-demerger day, and the provider's figure for it — once
    // the later split has been divided out — is 10.13.
    let demerger = PriceBasisEvent {
        date: d(2024, 3, 1),
        recover_new: "24.90".parse().unwrap(),
        recover_old: "10.13".parse().unwrap(),
    };
    let price_events = vec![PriceBasisEvent::from(&split), demerger];

    // A pre-demerger day observed after both events recovers through both:
    // twice for the split, then 24.90/10.13 for the demerger.
    let recovered = contemporaneous_price(
        "5.065".parse().unwrap(),
        &price_events,
        d(2024, 2, 20),
        d(2026, 7, 26),
    );
    assert_eq!(recovered, "24.90".parse::<Decimal>().unwrap());

    // The very same demerger contributes nothing to a quantity: it is not
    // a `SplitEvent` and has no way to become one.
    let quantity_events = vec![split];
    assert_eq!(
        split_ratio(&quantity_events, d(2024, 2, 20), Some(d(2026, 7, 26))),
        SplitRatio {
            new: Decimal::from(2),
            old: Decimal::ONE
        },
        "only the split converts unit bases — the demerger issues units of \
             another listing and moves no count here"
    );
}

/// A quantity carries the date whose basis it is in, so re-basing runs in
/// whichever direction the target date sits: a parcel acquired after a
/// split, asked for in an earlier date's basis (a rollover replacement
/// against an AMMA year end before it — SCENARIOS N-06), converts *back*
/// across the split rather than passing through at ×1.
#[test]
fn a_quantity_re_bases_backward_to_an_earlier_date() {
    let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
    // 200 units held after the split were 100 units before it …
    let after = UnitsAt::new(Decimal::from(200), d(2024, 9, 1));
    let before = after.rebased(&splits, Some(d(2024, 3, 1)));
    assert_eq!(before, Decimal::from(100));
    assert_eq!(before.basis(), Some(d(2024, 3, 1)));
    // … and forward again is the original count.
    assert_eq!(
        before.rebased(&splits, Some(d(2024, 9, 1))),
        Decimal::from(200)
    );
    // A parcel acquired after the split, asked for in a pre-split basis.
    assert_eq!(
        split_adjusted_quantity(
            AcquiredUnits::new(Decimal::from(200)),
            &splits,
            d(2024, 9, 1),
            Some(d(2024, 3, 1))
        ),
        Decimal::from(100)
    );
    // A date on the same side of every split is the identity.
    assert_eq!(
        after.rebased(&splits, Some(d(2024, 12, 1))),
        Decimal::from(200)
    );
}

#[test]
fn split_adjusted_and_as_acquired_quantities_are_inverse() {
    let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
    // 100 as-acquired units are 200 post-split units…
    assert_eq!(
        split_adjusted_quantity(
            AcquiredUnits::new(Decimal::from(100)),
            &splits,
            d(2024, 1, 1),
            None
        ),
        Decimal::from(200)
    );
    // …and 80 post-split units sold come from 40 as-acquired units.
    assert_eq!(
        as_acquired_quantity(
            UnitsAt::new(Decimal::from(80), d(2024, 9, 1)),
            &splits,
            d(2024, 1, 1)
        ),
        Decimal::from(40)
    );
    // A consolidation shrinks: 1-for-10 turns 100 into 10.
    let consol = vec![split_event(d(2024, 6, 1), "1", "10")];
    assert_eq!(
        split_adjusted_quantity(
            AcquiredUnits::new(Decimal::from(100)),
            &consol,
            d(2024, 1, 1),
            None
        ),
        Decimal::from(10)
    );
}

/// Every payment is judged on its own, so a parcel acquired between two
/// payments carries only the later one's reduction — `domain::cost_base`'s
/// walk sums these per-event figures, and this is the bound it sums under.
#[test]
fn per_unit_for_applies_each_payment_from_its_own_acquisition_bound() {
    let events = [
        RocEvent {
            date: d(2024, 1, 1),
            amount_per_unit: "0.10".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
        RocEvent {
            date: d(2024, 6, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
        RocEvent {
            date: d(2025, 1, 1),
            amount_per_unit: "0.40".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
    ];
    let per_unit = |i: usize, acquired| {
        events[i]
            .per_unit_for(&[], "AUD", acquired, acquired, None, None)
            .expect("same currency")
    };
    // Acquired between the first and second events: the first doesn't apply.
    assert_eq!(per_unit(0, d(2024, 3, 1)), None);
    assert_eq!(per_unit(1, d(2024, 3, 1)), Some("0.20".parse().unwrap()));
    assert_eq!(per_unit(2, d(2024, 3, 1)), Some("0.40".parse().unwrap()));
    // Acquired on the event date: held on the payment date, so it applies.
    assert_eq!(per_unit(1, d(2024, 6, 1)), Some("0.20".parse().unwrap()));
}

/// A rollover replacement parcel's entitlement is decided by when its
/// units joined *this listing's* register, not by the parcel's own trade
/// date — which is the operation date, and so always on or after a record
/// date the operation fell after.
///
/// A **transfer** keeps the units registered throughout (they move between
/// the taxpayer's own accounts), so a payment they were entitled to at its
/// record date still reduces their cost base in the replacement parcel. A
/// **scrip exchange** does not: those units are of a listing the taxpayer
/// was not on the register of when the record date passed.
#[test]
fn per_unit_for_dates_a_replacement_parcels_entitlement_from_the_register() {
    let event = RocEvent {
        date: d(2023, 11, 1),
        amount_per_unit: "0.50".parse().unwrap(),
        currency: "AUD".into(),
        record_date: Some(d(2023, 9, 25)),
    };
    // Units bought 10 Jan, rolled over 3 Oct — inside the window.
    let operation = d(2023, 10, 3);
    let acquired = d(2023, 1, 10);
    let per_unit = |event: &RocEvent, rollover| {
        event
            .per_unit_for(&[], "AUD", operation, operation, Some(rollover), None)
            .expect("same currency")
    };

    let transfer = RolloverOrigin {
        on: operation,
        registered_from: acquired,
    };
    assert_eq!(
        per_unit(&event, transfer),
        Some("0.50".parse::<Decimal>().unwrap())
    );

    // A scrip exchange's replacement: registered only from the operation.
    let scrip = RolloverOrigin {
        on: operation,
        registered_from: operation,
    };
    assert_eq!(per_unit(&event, scrip), None);

    // Either way, a payment on or before the operation date is already in
    // the carried cost base and must not reduce it twice (SCENARIOS N-06).
    let already_carried = RocEvent {
        date: operation,
        ..event
    };
    assert_eq!(per_unit(&already_carried, transfer), None);
}

#[test]
fn per_unit_for_bounds_a_payment_at_the_sale_date() {
    let events = [
        RocEvent {
            date: d(2024, 6, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
        RocEvent {
            date: d(2025, 1, 1),
            amount_per_unit: "0.40".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
    ];
    let per_unit = |i: usize, up_to| {
        events[i]
            .per_unit_for(&[], "AUD", d(2024, 1, 1), d(2024, 1, 1), None, up_to)
            .expect("same currency")
    };
    // Sold between the payments: only the one received while held applies.
    assert_eq!(
        per_unit(0, Some(d(2024, 9, 1))),
        Some("0.20".parse::<Decimal>().unwrap())
    );
    assert_eq!(per_unit(1, Some(d(2024, 9, 1))), None);
    // Sold on the payment date: still held at the payment, so it applies.
    assert_eq!(
        per_unit(1, Some(d(2025, 1, 1))),
        Some("0.40".parse::<Decimal>().unwrap())
    );
    // Sold before any payment: unaffected.
    assert_eq!(per_unit(0, Some(d(2024, 5, 1))), None);
    assert_eq!(per_unit(1, Some(d(2024, 5, 1))), None);
}

/// A payment after a split is per *post-split* unit: each as-acquired unit
/// became `new/old` units, so the per-as-acquired-unit reduction scales by
/// the split ratio.
#[test]
fn per_unit_for_scales_a_payment_across_a_split() {
    let events = [
        // Before the split: per as-acquired unit as-is.
        RocEvent {
            date: d(2024, 3, 1),
            amount_per_unit: "0.30".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
        // After a 2-for-1 split: each as-acquired unit receives it twice.
        RocEvent {
            date: d(2024, 9, 1),
            amount_per_unit: "0.20".parse().unwrap(),
            currency: "AUD".into(),
            record_date: None,
        },
    ];
    let splits = vec![split_event(d(2024, 6, 1), "2", "1")];
    let per_unit = |i: usize, acquired| {
        events[i]
            .per_unit_for(&splits, "AUD", acquired, acquired, None, None)
            .expect("same currency")
    };
    // 0.30 + 0.20 × 2 = 0.70 per as-acquired unit across the two payments.
    assert_eq!(
        per_unit(0, d(2024, 1, 1)),
        Some("0.30".parse::<Decimal>().unwrap())
    );
    assert_eq!(
        per_unit(1, d(2024, 1, 1)),
        Some("0.40".parse::<Decimal>().unwrap())
    );

    // A parcel acquired after the split holds post-split units already:
    // the later payment applies unscaled.
    assert_eq!(
        per_unit(1, d(2024, 7, 1)),
        Some("0.20".parse::<Decimal>().unwrap())
    );
}

#[test]
fn per_unit_for_rejects_a_currency_mismatch() {
    let payment = RocEvent {
        date: d(2024, 6, 1),
        amount_per_unit: "0.20".parse().unwrap(),
        currency: "USD".into(),
        record_date: None,
    };
    // Never net amounts across currencies: fail loudly, don't skip or zero.
    assert!(
        payment
            .per_unit_for(&[], "AUD", d(2024, 1, 1), d(2024, 1, 1), None, None)
            .is_err()
    );
    // An out-of-range payment in another currency is not an error — it
    // doesn't participate in the calculation at all.
    assert!(
        payment
            .per_unit_for(&[], "AUD", d(2024, 7, 1), d(2024, 7, 1), None, None)
            .is_ok()
    );
}

/// The window, the currency guard and the split re-basing above are all
/// `RocEvent::per_unit_for`'s. What only the method itself exposes is the
/// `Option`: a payment that doesn't reach the units *declines* rather than
/// reducing them by nil — which is what lets `domain::cost_base`'s
/// reduction walk leave the payment out of the itemised breakdown entirely
/// instead of printing a zero row.
#[test]
fn per_unit_for_declines_a_payment_outside_the_holding_period() {
    let payment = RocEvent {
        date: d(2024, 6, 1),
        amount_per_unit: "0.20".parse().unwrap(),
        currency: "AUD".into(),
        record_date: None,
    };
    let pu = |acquired, up_to| {
        payment
            .per_unit_for(&[], "AUD", acquired, acquired, None, up_to)
            .expect("same currency")
    };
    assert_eq!(pu(d(2024, 1, 1), None), Some("0.20".parse().unwrap()));
    assert_eq!(pu(d(2024, 7, 1), None), None, "acquired after the payment");
    assert_eq!(
        pu(d(2024, 1, 1), Some(d(2024, 5, 1))),
        None,
        "sold before the payment"
    );
}

/// Entitlement to a return of capital is fixed at its **record date**, weeks
/// before the money arrives (SCENARIOS B-09): a parcel registered after
/// it receives nothing, so nothing reduces it. "Registered" is the
/// parcel's **settlement** date, not its trade date — under T+2 the
/// ex-date is the business day before the record date, and a buy on it
/// settles the day after the record date. Without a record date the
/// payment date stands in — the behaviour of every action recorded before
/// the field existed.
#[test]
fn per_unit_for_tests_entitlement_at_the_record_date() {
    let payment = |record_date| RocEvent {
        date: d(2025, 3, 1),
        amount_per_unit: "0.50".parse().unwrap(),
        currency: "AUD".into(),
        record_date,
    };
    // Monday 10 Feb 2025: ex-date Friday 7 Feb.
    let record = d(2025, 2, 10);
    let pu = |event: RocEvent, acquired, settled, up_to| {
        event
            .per_unit_for(&[], "AUD", acquired, settled, None, up_to)
            .expect("same currency")
    };
    let paid = Some("0.50".parse::<Decimal>().unwrap());

    // Bought the business day before the ex-date (Thu 6 Feb), settled on
    // the record date: registered for it, so entitled.
    assert_eq!(pu(payment(Some(record)), d(2025, 2, 6), record, None), paid);
    // Bought on the ex-date (Fri 7 Feb), settled Tue 11 Feb — after the
    // record date — ex-entitlement, though traded before it.
    assert_eq!(
        pu(payment(Some(record)), d(2025, 2, 7), d(2025, 2, 11), None),
        None
    );
    // Bought anywhere in the window up to the payment: ex-entitlement.
    assert_eq!(
        pu(payment(Some(record)), d(2025, 2, 15), d(2025, 2, 18), None),
        None
    );
    // A same-day-settled parcel (a DRP, an ESS vest) on the record date
    // itself is on the register at it.
    assert_eq!(pu(payment(Some(record)), record, record, None), paid);
    // No record date recorded: the payment date decides, against the
    // settlement date too.
    assert_eq!(
        pu(payment(None), d(2025, 2, 15), d(2025, 2, 18), None),
        paid
    );
    assert_eq!(pu(payment(None), d(2025, 2, 27), d(2025, 3, 3), None), None);
    // The other half of the test is unchanged by a record date: a parcel
    // entitled at the record date but sold before the payment is still not
    // reduced (G1 adjusts the shares owned at the time of the payment).
    assert_eq!(
        pu(
            payment(Some(record)),
            d(2025, 2, 6),
            record,
            Some(d(2025, 2, 20))
        ),
        None,
        "sold between the record date and the payment"
    );
}
