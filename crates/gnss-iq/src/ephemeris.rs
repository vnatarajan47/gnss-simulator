//! Where broadcast ephemeris comes from.
//!
//! ## Division of labour with the web layer
//!
//! This crate does not speak HTTP. The Next.js API route already fetches from
//! BKG and caches to `data/cache/`, and its cache rules are not trivial -- a
//! file fetched while its day is still running is necessarily partial and must
//! be refetched, which is a real bug that was already found and fixed once.
//! Reimplementing that in Rust would mean two copies of a subtle rule, and the
//! copies would drift.
//!
//! So the split is: the caller ensures the files are on disk, and
//! [`EphemerisSource`] finds and parses them. [`DiskCacheSource`] names the
//! exact upstream URL when a file is missing, so a standalone run says what to
//! fetch rather than failing opaquely.
//!
//! The trait is what makes that swappable. A future implementation that
//! fetches directly, or reads from a different archive, satisfies the same
//! interface and no pipeline stage changes.

use std::path::{Path, PathBuf};

use gnss_core::{parse_nav, EphemerisSet, GpsTime};

use crate::Error;

/// How far outside the requested window ephemeris is still needed \[s\].
///
/// Selection picks the block with the nearest ToE, which for an epoch just
/// after midnight lives in the *previous* day's file. Two hours matches the
/// GPS curve-fit half-interval, so this loads exactly the days that can
/// contribute a usable block.
const SELECTION_REACH_S: f64 = 7200.0;

/// A UTC calendar day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Day {
    /// The UTC day containing a Unix timestamp.
    pub fn from_unix_seconds(unix_seconds: f64) -> Self {
        Self::from_days_since_epoch((unix_seconds / 86_400.0).floor() as i64)
    }

    /// Civil date from a day count since 1970-01-01, by Howard Hinnant's
    /// `civil_from_days`. Written out rather than pulled in because it is
    /// twenty lines and the alternative is a date-time dependency in a crate
    /// that otherwise needs none.
    fn from_days_since_epoch(days: i64) -> Self {
        // Shift the epoch to 0000-03-01, so leap day lands at the end of the
        // 400-year era and the month arithmetic has no special cases.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        } as u32;

        Self {
            year: (year + i64::from(month <= 2)) as i32,
            month,
            day,
        }
    }

    /// Day number within the year, 1-based, as RINEX filenames use.
    pub fn day_of_year(self) -> u32 {
        const CUMULATIVE: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let leap = (self.year % 4 == 0 && self.year % 100 != 0) || self.year % 400 == 0;
        CUMULATIVE[(self.month - 1) as usize]
            + self.day
            + u32::from(leap && self.month > 2)
    }

    /// `YYYY-MM-DD`.
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// The BKG merged broadcast filename for this day.
    pub fn brdc_filename(self) -> String {
        format!(
            "BRDC00WRD_R_{:04}{:03}0000_01D_MN.rnx.gz",
            self.year,
            self.day_of_year()
        )
    }

    /// Where that file lives upstream.
    pub fn brdc_url(self) -> String {
        format!(
            "https://igs.bkg.bund.de/root_ftp/IGS/BRDC/{:04}/{:03}/{}",
            self.year,
            self.day_of_year(),
            self.brdc_filename()
        )
    }
}

/// The UTC days a window needs ephemeris for, in order.
///
/// Includes the neighbours reachable by ephemeris selection, so a window
/// starting at 00:10 gets the previous day -- where its nearest ToE actually
/// lives.
pub fn days_for_window(start_unix_s: f64, duration_s: f64) -> Vec<Day> {
    let first = Day::from_unix_seconds(start_unix_s - SELECTION_REACH_S);
    let last = Day::from_unix_seconds(start_unix_s + duration_s + SELECTION_REACH_S);

    let mut days = Vec::new();
    let mut cursor = (start_unix_s - SELECTION_REACH_S) / 86_400.0;
    let cursor_end = (start_unix_s + duration_s + SELECTION_REACH_S) / 86_400.0;
    while cursor.floor() <= cursor_end.floor() {
        let day = Day::from_days_since_epoch(cursor.floor() as i64);
        if !days.contains(&day) {
            days.push(day);
        }
        cursor += 1.0;
    }

    debug_assert_eq!(days.first(), Some(&first));
    debug_assert_eq!(days.last(), Some(&last));
    days
}

/// Supplies broadcast ephemeris for a time window.
pub trait EphemerisSource {
    /// Load and merge every file covering `[start, start + duration]`.
    fn load(&self, start_unix_s: f64, duration_s: f64) -> Result<LoadedEphemeris, Error>;
}

/// A parsed ephemeris set plus a record of where it came from.
#[derive(Debug, Clone)]
pub struct LoadedEphemeris {
    pub set: EphemerisSet,
    /// Files that contributed, for the sidecar.
    pub sources: Vec<String>,
}

/// Reads whatever files it is handed.
///
/// Used by the job pipeline, which is given paths by the layer that did the
/// fetching, and by tests, which point it at the checked-in fixture.
#[derive(Debug, Clone)]
pub struct LocalFileSource {
    paths: Vec<PathBuf>,
}

impl LocalFileSource {
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
        }
    }
}

impl EphemerisSource for LocalFileSource {
    fn load(&self, _start_unix_s: f64, _duration_s: f64) -> Result<LoadedEphemeris, Error> {
        load_paths(&self.paths)
    }
}

/// Resolves BKG filenames inside a local cache directory.
#[derive(Debug, Clone)]
pub struct DiskCacheSource {
    directory: PathBuf,
}

impl DiskCacheSource {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
}

impl EphemerisSource for DiskCacheSource {
    fn load(&self, start_unix_s: f64, duration_s: f64) -> Result<LoadedEphemeris, Error> {
        let mut paths = Vec::new();
        let mut missing = Vec::new();

        for day in days_for_window(start_unix_s, duration_s) {
            let path = self.directory.join(day.brdc_filename());
            if path.exists() {
                paths.push(path);
            } else {
                missing.push(day);
            }
        }

        // Missing neighbours are tolerable -- a window in the middle of a day
        // does not truly need them, and the archive genuinely lacks some days.
        // Missing *everything* is not.
        if paths.is_empty() {
            let day = missing.first().copied().unwrap_or_else(|| {
                Day::from_unix_seconds(start_unix_s)
            });
            return Err(Error::EphemerisUnavailable {
                date: day.iso(),
                reason: format!(
                    "no cached broadcast file in {}; fetch {} first",
                    self.directory.display(),
                    day.brdc_url()
                ),
            });
        }

        load_paths(&paths)
    }
}

fn load_paths(paths: &[PathBuf]) -> Result<LoadedEphemeris, Error> {
    let mut merged = EphemerisSet::new();
    let mut sources = Vec::new();

    for path in paths {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path.display(), e))?;
        // Merging rather than replacing: consecutive days genuinely overlap,
        // and near a boundary the union is strictly better than either file
        // alone. Dedup is `EphemerisSet`'s problem, and it handles it.
        merged.merge(parse_nav(&bytes)?);
        sources.push(file_name(path));
    }

    if merged.is_empty() {
        return Err(Error::EphemerisUnavailable {
            date: sources.join(", "),
            reason: "the files parsed but contained no usable navigation records".into(),
        });
    }

    Ok(LoadedEphemeris {
        set: merged,
        sources,
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// GPS time of a Unix timestamp, for callers outside this crate.
pub fn gps_time_of(unix_seconds: f64) -> GpsTime {
    GpsTime::from_unix_seconds(unix_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Anchor the civil-date conversion on dates whose answers are known,
    /// including the leap-year cases the algorithm exists to get right.
    #[test]
    fn unix_timestamps_map_to_the_right_civil_day() {
        let cases = [
            (0.0, (1970, 1, 1), 1),
            (1_735_689_600.0, (2025, 1, 1), 1),      // 2025-01-01
            (1_735_732_800.0, (2025, 1, 1), 1),      // midday the same day
            (1_735_775_999.0, (2025, 1, 1), 1),      // one second before midnight
            (1_735_776_000.0, (2025, 1, 2), 2),      // and just after
            (1_709_164_800.0, (2024, 2, 29), 60),    // leap day
            (1_704_067_200.0, (2024, 1, 1), 1),
            (1_767_139_200.0, (2025, 12, 31), 365),  // last day of a common year
        ];

        for (unix, (year, month, day), day_of_year) in cases {
            let actual = Day::from_unix_seconds(unix);
            assert_eq!(
                (actual.year, actual.month, actual.day),
                (year, month, day),
                "for unix {unix}"
            );
            assert_eq!(actual.day_of_year(), day_of_year, "day of year for {unix}");
        }
    }

    /// 2000 is a leap year and 1900 is not -- the case that catches a
    /// four-year-only leap rule.
    #[test]
    fn century_leap_rules_are_right() {
        let leap_2000 = Day {
            year: 2000,
            month: 3,
            day: 1,
        };
        assert_eq!(leap_2000.day_of_year(), 61);

        let common_2100 = Day {
            year: 2100,
            month: 3,
            day: 1,
        };
        assert_eq!(common_2100.day_of_year(), 60);
    }

    /// The filename is what the cache is keyed on, so it has to match the
    /// route's exactly.
    #[test]
    fn brdc_filenames_match_the_archive_layout() {
        let day = Day::from_unix_seconds(1_735_689_600.0);
        assert_eq!(day.brdc_filename(), "BRDC00WRD_R_20250010000_01D_MN.rnx.gz");
        assert!(day
            .brdc_url()
            .ends_with("/IGS/BRDC/2025/001/BRDC00WRD_R_20250010000_01D_MN.rnx.gz"));
    }

    /// A window inside one day still pulls its neighbours, because the
    /// nearest ToE for an epoch near a boundary lives in the adjacent file.
    #[test]
    fn windows_load_the_days_selection_can_reach() {
        // Midday, ten seconds: reaches 10:00 and 14:00, so one day only.
        let midday = days_for_window(1_735_732_800.0, 10.0);
        assert_eq!(midday.len(), 1);
        assert_eq!(midday[0].iso(), "2025-01-01");

        // Ten past midnight: reaches back into 2024-12-31.
        let after_midnight = days_for_window(1_735_690_200.0, 10.0);
        assert_eq!(after_midnight.len(), 2);
        assert_eq!(after_midnight[0].iso(), "2024-12-31");
        assert_eq!(after_midnight[1].iso(), "2025-01-01");

        // A window that genuinely spans midnight gets both, plus reach.
        let across = days_for_window(1_735_775_000.0, 3600.0);
        assert_eq!(across.len(), 2);
        assert_eq!(across[0].iso(), "2025-01-01");
        assert_eq!(across[1].iso(), "2025-01-02");
    }

    #[test]
    fn a_local_file_parses_and_reports_its_name() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/BRDC00WRD_R_20250010000_01D_GN.rnx");
        let loaded = LocalFileSource::new([path])
            .load(1_735_732_800.0, 10.0)
            .unwrap();

        assert!(!loaded.set.is_empty());
        assert_eq!(loaded.sources, vec!["BRDC00WRD_R_20250010000_01D_GN.rnx"]);
    }

    /// An empty cache must say which file to fetch and from where, not just
    /// that something was missing.
    #[test]
    fn a_missing_cache_entry_names_the_upstream_url() {
        let source = DiskCacheSource::new("/nonexistent-cache-directory");
        let error = source.load(1_735_732_800.0, 10.0).unwrap_err().to_string();
        assert!(error.contains("BRDC00WRD_R_20250010000_01D_MN.rnx.gz"), "{error}");
        assert!(error.contains("igs.bkg.bund.de"), "{error}");
    }
}
