//! Property: a daily entry fires exactly once every local day across DST.

use super::*;
use chrono::Duration as ChronoDuration;
use proptest::prelude::*;

const ZONES: [&str; 4] = [
    "Australia/Sydney",
    "America/New_York",
    "Europe/London",
    "UTC",
];

proptest! {
    /// A daily entry, walked the way `run_entry` walks it — fire,
    /// run for a while, ask for the next occurrence from the clock
    /// the job finished at — fires exactly once on every local day
    /// for a year, whatever its zone does to the clock: never twice
    /// in a repeated (fall-back) hour, never skipped in a missing
    /// (spring-forward) one, and always strictly later than the run
    /// before it.
    #[test]
    fn a_daily_entry_fires_once_every_local_day_across_dst(
        zone in 0usize..ZONES.len(),
        hour in 0u32..24,
        minute in 0u32..60,
        start in 0i64..365 * 24 * 60,
        runtimes in prop::collection::vec(0i64..3_600, 400),
    ) {
        let tz: Tz = ZONES[zone].parse().unwrap();
        let cron = Cron::from_str(&format!("{minute} {hour} * * *")).unwrap();
        let mut now = (Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
            + ChronoDuration::minutes(start))
        .with_timezone(&tz);
        let mut previous: Option<DateTime<Tz>> = None;
        for runtime in runtimes {
            let (next, delay) = next_run(&cron, now).unwrap();
            prop_assert!(next > now, "{} is not after {}", next, now);
            prop_assert!(delay <= Duration::from_secs(25 * 3600), "{:?} until {}", delay, next);
            if let Some(prev) = previous {
                prop_assert_eq!(
                    next.date_naive(),
                    prev.date_naive().succ_opt().unwrap(),
                    "{} followed {} in {}", next, prev, ZONES[zone]
                );
            }
            previous = Some(next);
            now = next + ChronoDuration::seconds(runtime);
        }
    }
}
