//! Sky view sampled over a time interval.
//!
//! The single-epoch [`skyplot_from_set`] answers "where is everything now".
//! This module answers "where does everything go over the next few hours",
//! which is what a track plot and a DOP-versus-time plot both need.
//!
//! ## Shape of the result
//!
//! Epochs are a shared, evenly spaced axis ([`SkySeries::epochs`]), and every
//! other array indexes into it. Satellite samples are *sparse*: a track carries
//! only the epochs where that satellite was above the mask, tagged with its
//! [`TrackSample::epoch_index`].
//!
//! Sparseness is the point. A satellite rises and sets, and some rise twice
//! within a long window. A dense array would have to encode "not visible"
//! as a sentinel, and any consumer that forgot to check it would draw a line
//! straight across the sky between a set and the next rise. With indices, a
//! break in the run *is* the gap, and the drawing code has to look at them.
//!
//! ## Consistency with the single-epoch view
//!
//! Each epoch is computed by calling [`skyplot_from_set`] rather than by a
//! separate inlined loop. That is deliberate: the UI shows a cursor on the
//! track plot and a table for the same instant, and if the two code paths could
//! drift the cursor would stop matching the table. One implementation, sampled
//! repeatedly, cannot drift.

use std::collections::BTreeMap;

use crate::dop::{dop_for, Dop};
use crate::ephemeris::{EphemerisSet, Sv};
use crate::geodesy::Geodetic;
use crate::skyplot::{skyplot_from_set, SkyplotOptions};
use crate::time::GpsTime;
use crate::Error;

/// Ceiling on sampled epochs per request.
///
/// Not a physical limit -- it bounds the work a single call can ask for, so a
/// malformed step (or a caller passing seconds where it meant minutes) fails
/// loudly instead of hanging the browser tab the WASM module runs on.
pub const MAX_EPOCHS: usize = 4096;

/// One satellite at one sampled instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSample {
    /// Index into [`SkySeries::epochs`].
    pub epoch_index: usize,
    /// Azimuth \[deg\], 0 = true north, clockwise.
    pub azimuth_deg: f64,
    /// Elevation \[deg\] above the local horizon.
    pub elevation_deg: f64,
    /// Geometric range \[m\].
    pub range_m: f64,
    /// Signed `t - ToE` of the ephemeris used \[s\].
    ///
    /// Carried per sample rather than per track because it changes across the
    /// window: a long interval walks through several broadcast blocks, and a
    /// large magnitude marks a stretch that is extrapolating rather than
    /// interpolating.
    pub ephemeris_age_s: f64,
}

/// One satellite's path across the window.
#[derive(Debug, Clone, PartialEq)]
pub struct SatelliteTrack {
    pub sv: Sv,
    /// Ascending by `epoch_index`, with gaps wherever the satellite was below
    /// the mask or had no usable ephemeris.
    pub samples: Vec<TrackSample>,
}

impl SatelliteTrack {
    /// Fraction of the window this satellite was above the mask, in \[0, 1\].
    pub fn visibility_fraction(&self, epochs: usize) -> f64 {
        if epochs == 0 {
            0.0
        } else {
            self.samples.len() as f64 / epochs as f64
        }
    }
}

/// A sky view sampled across an interval.
#[derive(Debug, Clone, PartialEq)]
pub struct SkySeries {
    pub observer: Geodetic,
    /// The sampled instants, ascending and evenly spaced.
    pub epochs: Vec<GpsTime>,
    /// One entry per satellite visible at *some* point, ordered by
    /// constellation then PRN.
    pub tracks: Vec<SatelliteTrack>,
    /// DOP at each epoch. `None` where fewer than four satellites were up, or
    /// where their geometry was degenerate -- both ordinary for a real sky, and
    /// both meaning "no solution", not "zero".
    pub dop: Vec<Option<Dop>>,
    /// Satellites above the mask at each epoch.
    pub visible: Vec<usize>,
    /// Epochs where no satellite had an ephemeris valid at that instant.
    ///
    /// Nonzero usually means the window runs past the end of the loaded
    /// broadcast file rather than that anything is wrong.
    pub epochs_without_ephemeris: usize,
}

impl SkySeries {
    /// Index of the epoch nearest a given instant, if there are any epochs.
    ///
    /// Lets a caller move a cursor by time without assuming the step.
    pub fn nearest_epoch(&self, t: GpsTime) -> Option<usize> {
        if self.epochs.is_empty() {
            return None;
        }
        let target = t.seconds();
        let mut best = 0;
        let mut best_delta = f64::INFINITY;
        for (i, epoch) in self.epochs.iter().enumerate() {
            let delta = (epoch.seconds() - target).abs();
            if delta < best_delta {
                best_delta = delta;
                best = i;
            }
        }
        Some(best)
    }
}

/// Sample a sky view from `start` to `end` inclusive, every `step_s` seconds.
///
/// `end` is included only when the interval is a whole multiple of the step;
/// otherwise the last epoch is the final whole step before `end`.
pub fn skyplot_series(
    set: &EphemerisSet,
    observer: Geodetic,
    start: GpsTime,
    end: GpsTime,
    step_s: f64,
    options: &SkyplotOptions,
) -> Result<SkySeries, Error> {
    if !step_s.is_finite() || step_s <= 0.0 {
        return Err(Error::InvalidInterval {
            reason: "sample step must be a positive number of seconds",
        });
    }

    let span_s = end.seconds() - start.seconds();
    if !span_s.is_finite() || span_s < 0.0 {
        return Err(Error::InvalidInterval {
            reason: "the interval ends before it starts",
        });
    }

    // `+ 1` because both ends are sampled: a 60 s window at 60 s gives two
    // epochs, not one.
    let count = (span_s / step_s).floor() as usize + 1;
    if count > MAX_EPOCHS {
        return Err(Error::SeriesTooLong {
            epochs: count,
            max: MAX_EPOCHS,
        });
    }

    let mut epochs = Vec::with_capacity(count);
    let mut dop = Vec::with_capacity(count);
    let mut visible = Vec::with_capacity(count);
    let mut by_satellite: BTreeMap<Sv, Vec<TrackSample>> = BTreeMap::new();
    let mut epochs_without_ephemeris = 0;

    for index in 0..count {
        let t = GpsTime::from_seconds(start.seconds() + index as f64 * step_s);
        epochs.push(t);

        // A window can legitimately extend past the ephemeris -- today's
        // broadcast file only covers the hours already elapsed. That is a
        // property of one epoch, not a failure of the request, so it is
        // counted and the series continues.
        let view = match skyplot_from_set(set, observer, t, options) {
            Ok(view) => view,
            Err(Error::NoEphemerisInRange) => {
                epochs_without_ephemeris += 1;
                dop.push(None);
                visible.push(0);
                continue;
            }
            Err(other) => return Err(other),
        };

        for satellite in &view.satellites {
            by_satellite
                .entry(satellite.sv)
                .or_default()
                .push(TrackSample {
                    epoch_index: index,
                    azimuth_deg: satellite.azimuth_deg,
                    elevation_deg: satellite.elevation_deg,
                    range_m: satellite.range_m,
                    ephemeris_age_s: satellite.ephemeris_age_s,
                });
        }

        dop.push(dop_for(&view.satellites));
        visible.push(view.satellites.len());
    }

    // Every epoch empty means the file does not cover the window at all, which
    // is a caller error worth reporting -- the same rule the single-epoch path
    // applies, lifted to the interval.
    if count > 0 && epochs_without_ephemeris == count {
        return Err(Error::NoEphemerisInRange);
    }

    // `BTreeMap<Sv, _>` already orders by constellation then PRN, matching the
    // single-epoch view's sort.
    let tracks = by_satellite
        .into_iter()
        .map(|(sv, samples)| SatelliteTrack { sv, samples })
        .collect();

    Ok(SkySeries {
        observer,
        epochs,
        tracks,
        dop,
        visible,
        epochs_without_ephemeris,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skyplot::SkyplotOptions;
    use crate::source::parse_nav;

    /// 2026-08-08T12:00Z, the epoch the other fixtures use.
    const EPOCH_UNIX: f64 = 1_786_190_400.0;

    fn fixture() -> EphemerisSet {
        let bytes = std::fs::read("../../data/BRDC00WRD_R_20250010000_01D_GN.rnx")
            .expect("GPS fixture present");
        parse_nav(&bytes).expect("fixture parses")
    }

    /// The fixture is 2025-001, so use an epoch inside that day.
    fn fixture_start() -> GpsTime {
        GpsTime::from_unix_seconds(1_735_732_800.0) // 2025-01-01T12:00Z
    }

    fn denver() -> Geodetic {
        Geodetic::new(39.7392, -104.9903, 1609.0)
    }

    #[test]
    fn epochs_are_evenly_spaced_and_inclusive() {
        let series = skyplot_series(
            &fixture(),
            denver(),
            fixture_start(),
            GpsTime::from_seconds(fixture_start().seconds() + 3600.0),
            600.0,
            &SkyplotOptions::default(),
        )
        .expect("series computes");

        // 3600 / 600 = 6 steps, so 7 epochs including both ends.
        assert_eq!(series.epochs.len(), 7);
        assert_eq!(series.dop.len(), 7);
        assert_eq!(series.visible.len(), 7);

        for pair in series.epochs.windows(2) {
            assert!((pair[1].seconds() - pair[0].seconds() - 600.0).abs() < 1e-9);
        }
    }

    #[test]
    fn a_zero_length_interval_yields_one_epoch() {
        let start = fixture_start();
        let series = skyplot_series(
            &fixture(),
            denver(),
            start,
            start,
            60.0,
            &SkyplotOptions::default(),
        )
        .expect("series computes");
        assert_eq!(series.epochs.len(), 1);
    }

    /// The whole reason samples carry an epoch index: a consumer must be able
    /// to tell a continuous arc from two passes with a gap between them.
    #[test]
    fn track_samples_are_ascending_and_may_have_gaps() {
        let series = skyplot_series(
            &fixture(),
            denver(),
            fixture_start(),
            GpsTime::from_seconds(fixture_start().seconds() + 6.0 * 3600.0),
            300.0,
            &SkyplotOptions::default(),
        )
        .expect("series computes");

        assert!(!series.tracks.is_empty());

        let mut saw_a_gap = false;
        for track in &series.tracks {
            assert!(!track.samples.is_empty(), "a track with no samples is not a track");
            for pair in track.samples.windows(2) {
                assert!(
                    pair[1].epoch_index > pair[0].epoch_index,
                    "indices must strictly ascend"
                );
                if pair[1].epoch_index != pair[0].epoch_index + 1 {
                    saw_a_gap = true;
                }
            }
            assert!(track.samples.last().unwrap().epoch_index < series.epochs.len());
        }

        // Over six hours some satellite must rise or set, so at least one track
        // must be shorter than the full window.
        assert!(
            saw_a_gap || series.tracks.iter().any(|t| t.samples.len() < series.epochs.len()),
            "expected at least one satellite not visible for the whole window"
        );
    }

    /// The cursor in the UI reads the table from the single-epoch path and the
    /// dot position from the series. They must agree exactly.
    #[test]
    fn a_sampled_epoch_matches_the_single_epoch_view() {
        let set = fixture();
        let options = SkyplotOptions::default();
        let start = fixture_start();
        let step = 900.0;

        let series =
            skyplot_series(&set, denver(), start, GpsTime::from_seconds(start.seconds() + 3600.0), step, &options)
                .expect("series computes");

        let index = 2;
        let t = series.epochs[index];
        let single = skyplot_from_set(&set, denver(), t, &options).expect("single epoch computes");

        for satellite in &single.satellites {
            let track = series
                .tracks
                .iter()
                .find(|t| t.sv == satellite.sv)
                .expect("every visible satellite has a track");
            let sample = track
                .samples
                .iter()
                .find(|s| s.epoch_index == index)
                .expect("track covers the sampled epoch");

            assert_eq!(sample.azimuth_deg, satellite.azimuth_deg);
            assert_eq!(sample.elevation_deg, satellite.elevation_deg);
            assert_eq!(sample.range_m, satellite.range_m);
        }

        assert_eq!(series.visible[index], single.satellites.len());
    }

    #[test]
    fn dop_is_present_wherever_enough_satellites_are_up() {
        let series = skyplot_series(
            &fixture(),
            denver(),
            fixture_start(),
            GpsTime::from_seconds(fixture_start().seconds() + 3.0 * 3600.0),
            600.0,
            &SkyplotOptions::default(),
        )
        .expect("series computes");

        for (index, entry) in series.dop.iter().enumerate() {
            match entry {
                Some(dop) => {
                    assert!(series.visible[index] >= 4);
                    // A real GPS sky from CONUS is never this bad; a value out
                    // here would mean the geometry assembly is wrong.
                    assert!(dop.hdop > 0.0 && dop.hdop < 10.0, "hdop {}", dop.hdop);
                    assert!(dop.vdop >= dop.hdop, "vdop should trail hdop");
                }
                None => assert!(series.visible[index] < 4),
            }
        }
    }

    #[test]
    fn rejects_a_step_that_would_blow_past_the_epoch_ceiling() {
        let start = fixture_start();
        let result = skyplot_series(
            &fixture(),
            denver(),
            start,
            GpsTime::from_seconds(start.seconds() + 86_400.0),
            1.0,
            &SkyplotOptions::default(),
        );
        assert!(matches!(result, Err(Error::SeriesTooLong { .. })));
    }

    #[test]
    fn rejects_a_nonsensical_interval() {
        let start = fixture_start();
        let set = fixture();

        assert!(matches!(
            skyplot_series(&set, denver(), start, start, 0.0, &SkyplotOptions::default()),
            Err(Error::InvalidInterval { .. })
        ));

        assert!(matches!(
            skyplot_series(
                &set,
                denver(),
                start,
                GpsTime::from_seconds(start.seconds() - 3600.0),
                60.0,
                &SkyplotOptions::default()
            ),
            Err(Error::InvalidInterval { .. })
        ));
    }

    #[test]
    fn nearest_epoch_snaps_to_the_sample_grid() {
        let start = fixture_start();
        let series = skyplot_series(
            &fixture(),
            denver(),
            start,
            GpsTime::from_seconds(start.seconds() + 3600.0),
            600.0,
            &SkyplotOptions::default(),
        )
        .expect("series computes");

        assert_eq!(series.nearest_epoch(start), Some(0));
        // 700 s in is closer to the 600 s sample than to the 1200 s one.
        assert_eq!(
            series.nearest_epoch(GpsTime::from_seconds(start.seconds() + 700.0)),
            Some(1)
        );
        // Past the end clamps to the last sample rather than failing.
        assert_eq!(
            series.nearest_epoch(GpsTime::from_seconds(start.seconds() + 99_999.0)),
            Some(6)
        );
    }
}
