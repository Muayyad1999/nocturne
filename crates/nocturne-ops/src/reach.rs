//! Reachability over time, worked out from stored samples.
//!
//! The availability report and the handover's "went down" list both read the
//! series the monitor records at every poll and ask the same questions of it:
//! how much of the period was observed at all, in what share of the
//! observations the device answered, and when it did not.

use nocturne_app::model::Device;
use nocturne_app::store::{Point, Store};
use nocturne_app::util::now_ms;
use nocturne_app::{AppError, AppResult};
use serde::Serialize;

/// The series the monitor writes per poll: 1 = the device answered, 0 = it
/// did not (`docs/dev/features/monitor.md`, "Metrics"). No instance.
pub const METRIC: &str = "up";
const INSTANCE: &str = "";

/// The most points the store returns for one range.
const MAX_POINTS: usize = 5_000;
const FIVE_MINUTES_MS: i64 = 300_000;
const HOUR_MS: i64 = 3_600_000;
/// The store answers ranges that start within this from raw samples…
const RAW_WINDOW_MS: i64 = 48 * HOUR_MS;
/// …and within this from 5-minute roll-ups; older ranges come from hourly ones.
const FINE_WINDOW_MS: i64 = 30 * 24 * HOUR_MS;

/// What the store had for the range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Raw,
    FiveMinutes,
    Hourly,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Raw => "raw samples",
            Resolution::FiveMinutes => "5-minute roll-ups",
            Resolution::Hourly => "hourly roll-ups",
        }
    }
}

/// The samples of one range: each point stands for the polls of one bucket.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub points: Vec<Point>,
    pub bucket_ms: i64,
    pub resolution: Resolution,
}

/// A stretch in which the device did not answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Outage {
    /// The first observation without an answer.
    pub start: i64,
    /// How long the device is known to have been down.
    pub down_ms: i64,
    /// Still down when the period ended.
    pub ongoing: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    pub observations: u64,
    /// How many observations a gapless period would have had.
    pub expected: u64,
    /// The share of the period that was observed, 0 to 1.
    pub coverage: f64,
    /// The share of the observations in which the device answered; `None`
    /// when there were none.
    pub availability: Option<f64>,
    pub outages: Vec<Outage>,
}

impl Analysis {
    pub fn longest_outage(&self) -> Option<&Outage> {
        // The earliest of equally long outages.
        self.outages
            .iter()
            .rev()
            .max_by_key(|outage| outage.down_ms)
    }

    pub fn down_ms(&self) -> i64 {
        self.outages.iter().map(|outage| outage.down_ms).sum()
    }
}

/// The id of a device's reachability series, or `None` when nothing has ever
/// recorded one. (`Store::series_id` alone would create it.)
pub fn series_id(store: &Store, device_id: &str) -> AppResult<Option<i64>> {
    let recorded = store
        .series_of(device_id)?
        .iter()
        .any(|(metric, instance)| metric == METRIC && instance == INSTANCE);
    if recorded {
        store.series_id(device_id, METRIC, INSTANCE).map(Some)
    } else {
        Ok(None)
    }
}

/// The reachability samples of `[since, until)`.
pub fn read(store: &Store, series: i64, since: i64, until: i64) -> AppResult<Series> {
    // The store's range includes both ends.
    let last = until - 1;
    let points = store.samples_query(series, since, last, MAX_POINTS)?;
    let wanted = ((last - since).max(1) / MAX_POINTS as i64).max(1);
    let (bucket_ms, resolution) = resolution_of(&points, wanted, since, now_ms())?;
    Ok(Series {
        points,
        bucket_ms,
        resolution,
    })
}

/// The bucket width the store answered with.
///
/// `Store::samples_query` chooses between raw samples (bucketed `wanted` ms
/// wide to fit the point limit), 5-minute and hourly roll-ups by the age of
/// the range, and does not say which it chose. Its documented choice is
/// assumed first; the answer's own timestamps — bucket starts, so multiples
/// of the width — confirm it or point at one of the other two.
fn resolution_of(
    points: &[Point],
    wanted: i64,
    since: i64,
    now: i64,
) -> AppResult<(i64, Resolution)> {
    let raw = (wanted < FIVE_MINUTES_MS).then_some((wanted, Resolution::Raw));
    let fine = (
        (wanted / FIVE_MINUTES_MS).max(1) * FIVE_MINUTES_MS,
        Resolution::FiveMinutes,
    );
    let coarse = ((wanted / HOUR_MS).max(1) * HOUR_MS, Resolution::Hourly);
    let expected = match raw {
        Some(raw) if since >= now - RAW_WINDOW_MS => raw,
        _ if wanted < HOUR_MS && since >= now - FINE_WINDOW_MS => fine,
        _ => coarse,
    };
    [Some(expected), Some(fine), raw, Some(coarse)]
        .into_iter()
        .flatten()
        .find(|(bucket, _)| points.iter().all(|point| point.ts % bucket == 0))
        .ok_or_else(|| {
            AppError::internal(
                "the sample store answered at a resolution the reachability analysis does not know; availability cannot be worked out",
            )
        })
}

/// Coverage, availability and outages of `[since, until)` for a device polled
/// every `interval_ms`.
///
/// One observation is expected per slot, a slot being the longer of the poll
/// interval and the bucket width. A point counts as one observation; the
/// share of its polls that failed is downtime of that share of a slot.
/// Consecutive observations with failed polls form one outage; a hole in the
/// observations of more than two and a half slots ends it, because nothing is
/// known about the device while nobody was looking.
pub fn analyse(series: &Series, since: i64, until: i64, interval_ms: i64) -> Analysis {
    let slot = series.bucket_ms.max(interval_ms).max(1);
    let expected = ((until - since).max(0) / slot).max(1) as u64;
    let observations = series.points.len() as u64;
    let answered: f64 = series
        .points
        .iter()
        .map(|point| point.avg.clamp(0.0, 1.0))
        .sum();
    let hole = slot * 5 / 2;

    let mut outages: Vec<Outage> = Vec::new();
    // The outage in progress and the time of its latest observation.
    let mut open: Option<(Outage, i64)> = None;
    for point in &series.points {
        if let Some((_, last)) = &open
            && point.ts - last > hole
        {
            outages.extend(open.take().map(|(outage, _)| outage));
        }
        if point.min >= 0.5 {
            // Every poll of this observation was answered.
            outages.extend(open.take().map(|(outage, _)| outage));
            continue;
        }
        let down_ms = (slot as f64 * (1.0 - point.avg.clamp(0.0, 1.0))).round() as i64;
        match &mut open {
            Some((outage, last)) => {
                outage.down_ms += down_ms;
                *last = point.ts;
            }
            None => {
                open = Some((
                    Outage {
                        start: point.ts.max(since),
                        down_ms,
                        ongoing: false,
                    },
                    point.ts,
                ));
            }
        }
    }
    if let Some((mut outage, last)) = open {
        outage.ongoing = until - last <= hole;
        outages.push(outage);
    }

    Analysis {
        observations,
        expected,
        coverage: (observations as f64 / expected as f64).min(1.0),
        availability: (observations > 0).then(|| answered / observations as f64),
        outages,
    }
}

/// A device's reachability in `[since, until)`; `None` when the device has no
/// reachability series at all. The period starts when the device was added if
/// that is later, so a new device is not blamed for the time before it existed.
pub fn of_device(
    store: &Store,
    device: &Device,
    since: i64,
    until: i64,
) -> AppResult<Option<(Series, Analysis)>> {
    let Some(series) = series_id(store, &device.id)? else {
        return Ok(None);
    };
    let since = since.max(device.created_at).min(until);
    let samples = read(store, series, since, until)?;
    let interval_ms = i64::from(device.monitor.interval_s) * 1000;
    let analysis = analyse(&samples, since, until, interval_ms);
    Ok(Some((samples, analysis)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const MINUTE: i64 = 60_000;

    fn sample(ts: i64, up: bool) -> Point {
        let value = if up { 1.0 } else { 0.0 };
        Point {
            ts,
            min: value,
            avg: value,
            max: value,
        }
    }

    /// One raw sample per minute starting at `start`: `1` up, `0` down, `.` no sample.
    fn minute_series(start: i64, pattern: &str) -> Series {
        Series {
            points: pattern
                .chars()
                .enumerate()
                .filter(|(_, ch)| *ch != '.')
                .map(|(index, ch)| sample(start + index as i64 * MINUTE, ch == '1'))
                .collect(),
            bucket_ms: 20_000,
            resolution: Resolution::Raw,
        }
    }

    #[test]
    fn a_fully_observed_healthy_hour() {
        let series = minute_series(0, &"1".repeat(60));
        let analysis = analyse(&series, 0, 60 * MINUTE, MINUTE);
        assert_eq!((analysis.observations, analysis.expected), (60, 60));
        assert_eq!(analysis.coverage, 1.0);
        assert_eq!(analysis.availability, Some(1.0));
        assert!(analysis.outages.is_empty());
        assert_eq!(analysis.longest_outage(), None);
    }

    #[test]
    fn outages_last_from_the_first_failed_poll_to_the_next_answer() {
        // 10 polls: down at minutes 2-4 (3 min) and at minute 7 (1 min).
        let series = minute_series(0, "1100011011");
        let analysis = analyse(&series, 0, 10 * MINUTE, MINUTE);
        assert_eq!(analysis.availability, Some(0.6));
        assert_eq!(
            analysis.outages,
            [
                Outage {
                    start: 2 * MINUTE,
                    down_ms: 3 * MINUTE,
                    ongoing: false
                },
                Outage {
                    start: 7 * MINUTE,
                    down_ms: MINUTE,
                    ongoing: false
                },
            ]
        );
        assert_eq!(analysis.longest_outage().unwrap().start, 2 * MINUTE);
        assert_eq!(analysis.down_ms(), 4 * MINUTE);
    }

    #[test]
    fn gaps_reduce_coverage_and_never_count_as_up_or_down() {
        // 100 minutes: 60 observed, 30 unobserved, 10 observed.
        let pattern = format!("{}{}{}", "1".repeat(60), ".".repeat(30), "1".repeat(10));
        let analysis = analyse(&minute_series(0, &pattern), 0, 100 * MINUTE, MINUTE);
        assert_eq!((analysis.observations, analysis.expected), (70, 100));
        assert_eq!(analysis.coverage, 0.7);
        assert_eq!(analysis.availability, Some(1.0), "of what was observed");
        assert!(analysis.outages.is_empty());
    }

    #[test]
    fn a_hole_in_the_observations_ends_an_outage() {
        // Down for 2 polls, nobody looking for 8 minutes, down again for 3 polls, then up.
        let analysis = analyse(&minute_series(0, "100........0001"), 0, 15 * MINUTE, MINUTE);
        assert_eq!(
            analysis.outages,
            [
                Outage {
                    start: MINUTE,
                    down_ms: 2 * MINUTE,
                    ongoing: false
                },
                Outage {
                    start: 11 * MINUTE,
                    down_ms: 3 * MINUTE,
                    ongoing: false
                },
            ]
        );
        // One missed poll inside an outage does not split it.
        let analysis = analyse(&minute_series(0, "10.01"), 0, 5 * MINUTE, MINUTE);
        assert_eq!(analysis.outages.len(), 1);
        assert_eq!(analysis.outages[0].down_ms, 2 * MINUTE, "only what was seen");
    }

    #[test]
    fn an_outage_running_into_the_end_of_the_period_is_ongoing() {
        let analysis = analyse(&minute_series(0, "11100"), 0, 5 * MINUTE, MINUTE);
        assert_eq!(
            analysis.outages,
            [Outage {
                start: 3 * MINUTE,
                down_ms: 2 * MINUTE,
                ongoing: true
            }]
        );
        // Down when last seen, but that was long before the period ended: unknown now.
        let analysis = analyse(&minute_series(0, "11100....."), 0, 10 * MINUTE, MINUTE);
        assert!(!analysis.outages[0].ongoing);
    }

    #[test]
    fn roll_up_buckets_count_their_failed_share() {
        // 5-minute buckets of a device polled every minute: all up, 1 of 5 failed, all failed, all up.
        let bucket = |index: i64, min: f64, avg: f64, max: f64| Point {
            ts: index * FIVE_MINUTES_MS,
            min,
            avg,
            max,
        };
        let series = Series {
            points: vec![
                bucket(0, 1.0, 1.0, 1.0),
                bucket(1, 0.0, 0.8, 1.0),
                bucket(2, 0.0, 0.0, 0.0),
                bucket(3, 1.0, 1.0, 1.0),
            ],
            bucket_ms: FIVE_MINUTES_MS,
            resolution: Resolution::FiveMinutes,
        };
        let analysis = analyse(&series, 0, 4 * FIVE_MINUTES_MS, MINUTE);
        assert_eq!((analysis.observations, analysis.expected), (4, 4));
        // (1 + 0.8 + 0 + 1) / 4
        assert_eq!(analysis.availability, Some(0.7));
        // 1 minute of the second bucket and all 5 of the third.
        assert_eq!(
            analysis.outages,
            [Outage {
                start: FIVE_MINUTES_MS,
                down_ms: 6 * MINUTE,
                ongoing: false
            }]
        );
    }

    #[test]
    fn slow_pollers_are_measured_against_their_own_interval() {
        // Polled every 10 minutes for an hour: 6 samples are full coverage.
        let series = Series {
            points: (0..6).map(|index| sample(index * 10 * MINUTE, index != 2)).collect(),
            bucket_ms: 720,
            resolution: Resolution::Raw,
        };
        let analysis = analyse(&series, 0, 60 * MINUTE, 10 * MINUTE);
        assert_eq!((analysis.observations, analysis.expected), (6, 6));
        assert_eq!(analysis.outages[0].down_ms, 10 * MINUTE);
    }

    #[test]
    fn nothing_observed_is_not_zero_availability() {
        let analysis = analyse(&minute_series(0, ""), 0, 60 * MINUTE, MINUTE);
        assert_eq!(analysis.observations, 0);
        assert_eq!(analysis.coverage, 0.0);
        assert_eq!(analysis.availability, None);
    }

    #[test]
    fn the_served_resolution_is_recognised_from_the_answer() {
        let now = 1_800_000_000_000;
        let day = 24 * HOUR_MS;
        let at = |ts: i64| sample(ts, true);
        // A day's range asks for 17.28 s buckets and, being recent, gets them.
        let wanted = (day - 1) / 5_000;
        assert_eq!(wanted, 17_279);
        let raw = [at(wanted * 1_000), at(wanted * 1_003)];
        assert_eq!(
            resolution_of(&raw, wanted, now - day, now).unwrap(),
            (wanted, Resolution::Raw)
        );
        // The same range answered from roll-ups after all (the raw window got shorter).
        let rolled = [at(FIVE_MINUTES_MS * 7), at(FIVE_MINUTES_MS * 8)];
        assert_eq!(
            resolution_of(&rolled, wanted, now - day, now).unwrap(),
            (FIVE_MINUTES_MS, Resolution::FiveMinutes)
        );
        // An older range is expected from 5-minute roll-ups…
        let week = 7 * day;
        let wanted = (week - 1) / 5_000;
        assert_eq!(
            resolution_of(&rolled, wanted, now - week, now).unwrap(),
            (FIVE_MINUTES_MS, Resolution::FiveMinutes)
        );
        // …but raw samples are recognised when the store still had them.
        let raw = [at(wanted * 10), at(wanted * 11)];
        assert_eq!(
            resolution_of(&raw, wanted, now - week, now).unwrap(),
            (wanted, Resolution::Raw)
        );
        // Sparse 5-minute buckets on the hour are still 5-minute buckets.
        let on_the_hour = [at(HOUR_MS * 5), at(HOUR_MS * 9)];
        assert_eq!(
            resolution_of(&on_the_hour, wanted, now - week, now).unwrap().1,
            Resolution::FiveMinutes
        );
        // Beyond the roll-up window only hourly data is left.
        let quarter = 90 * day;
        let wanted = (quarter - 1) / 5_000;
        assert_eq!(
            resolution_of(&on_the_hour, wanted, now - quarter, now).unwrap(),
            (HOUR_MS, Resolution::Hourly)
        );
        // No points at all: the expected resolution, nothing to contradict it.
        assert_eq!(
            resolution_of(&[], wanted, now - quarter, now).unwrap().1,
            Resolution::Hourly
        );
        // Timestamps that fit no bucket width are refused, not interpreted.
        let odd = [at(1_234_567)];
        assert_eq!(
            resolution_of(&odd, 1_000, now - HOUR_MS, now).unwrap_err().code,
            "internal"
        );
    }
}
