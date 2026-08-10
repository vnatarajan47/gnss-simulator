//! wasm-bindgen surface for [`gnss_core`].
//!
//! Two entry points:
//!
//! - [`compute_skyplot`] -- one shot: bytes in, satellite list out.
//! - [`Skyplotter`] -- parse a RINEX file once, then query it repeatedly.
//!   Preferred for the map UI, where every click re-evaluates the same file.
//!
//! Build with `wasm-pack build --target web`.

use gnss_core::{
    skyplot_from_set, skyplot_series, Constellation, Dop, EphemerisSet, Geodetic, GpsTime,
    SbasProvider, SkySeries, SkyView, SkyplotOptions, Sv,
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Route Rust panics to `console.error` with a readable stack.
///
/// Runs automatically on module instantiation; without it a panic surfaces in
/// the browser as an opaque `unreachable executed`.
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// One satellite, as handed to JavaScript.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsSatellite {
    /// RINEX identifier, e.g. `G07`, `S31`.
    sv: String,
    /// PRN within the constellation. SBAS reports the true PRN (131), not the
    /// two-digit RINEX form.
    prn: u8,
    /// Which toggle this satellite belongs to: `GPS`, `WAAS`, `EGNOS`, ...
    ///
    /// Lets the UI group and colour by source without duplicating the PRN-to-
    /// provider table on the JavaScript side.
    source: String,
    /// Azimuth \[deg\], 0 = true north, clockwise.
    azimuth: f64,
    /// Elevation \[deg\] above the horizon.
    elevation: f64,
    /// Geometric range \[km\].
    range_km: f64,
    /// Signed `t - ToE` of the ephemeris used \[s\].
    ephemeris_age_s: f64,
}

/// A computed sky view, as handed to JavaScript.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsSkyView {
    satellites: Vec<JsSatellite>,
    /// Count above the mask.
    visible_count: usize,
    /// Count with a valid ephemeris but below the mask.
    below_mask: usize,
    /// Count with no ephemeris valid at the requested instant.
    without_ephemeris: usize,
    /// Echo of the resolved GPS time, seconds since the GPS epoch.
    gps_seconds: f64,
}

impl From<SkyView> for JsSkyView {
    fn from(view: SkyView) -> Self {
        Self {
            satellites: view
                .satellites
                .iter()
                .map(|s| JsSatellite {
                    sv: s.sv.to_string(),
                    prn: s.sv.prn,
                    source: source_key(s.sv).to_string(),
                    azimuth: s.azimuth_deg,
                    elevation: s.elevation_deg,
                    range_km: s.range_m / 1000.0,
                    ephemeris_age_s: s.ephemeris_age_s,
                })
                .collect(),
            visible_count: view.satellites.len(),
            below_mask: view.below_mask,
            without_ephemeris: view.without_ephemeris,
            gps_seconds: view.time.seconds(),
        }
    }
}

/// Dilution of precision at one epoch, as handed to JavaScript.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsDop {
    gdop: f64,
    pdop: f64,
    hdop: f64,
    vdop: f64,
    tdop: f64,
    /// How many satellites entered the solution.
    satellites: usize,
}

impl From<Dop> for JsDop {
    fn from(d: Dop) -> Self {
        Self {
            gdop: d.gdop,
            pdop: d.pdop,
            hdop: d.hdop,
            vdop: d.vdop,
            tdop: d.tdop,
            satellites: d.satellites,
        }
    }
}

/// One satellite at one sampled instant.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsTrackSample {
    /// Index into `JsSkySeries::epochs`.
    epoch_index: usize,
    azimuth: f64,
    elevation: f64,
    range_km: f64,
    /// Signed `t - ToE` of the ephemeris used [s].
    ephemeris_age_s: f64,
}

/// One satellite's path across the window.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsTrack {
    sv: String,
    prn: u8,
    source: String,
    /// Ascending by `epochIndex`, with gaps where the satellite was not up.
    /// A break in the run is a rise/set, and the renderer must not bridge it.
    samples: Vec<JsTrackSample>,
}

/// A sky view sampled over an interval.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsSkySeries {
    /// Unix seconds, ascending and evenly spaced. Every other array indexes
    /// into this one, which is what keeps the sky plot and the DOP plot
    /// synchronised without either having to know the step.
    epochs: Vec<f64>,
    tracks: Vec<JsTrack>,
    /// Per epoch; `null` where the geometry admits no solution.
    dop: Vec<Option<JsDop>>,
    /// Satellites above the mask, per epoch.
    visible: Vec<usize>,
    /// Epochs where no satellite had a valid ephemeris.
    epochs_without_ephemeris: usize,
}

impl From<SkySeries> for JsSkySeries {
    fn from(series: SkySeries) -> Self {
        Self {
            epochs: series.epochs.iter().map(|t| t.to_unix_seconds()).collect(),
            tracks: series
                .tracks
                .iter()
                .map(|track| JsTrack {
                    sv: track.sv.to_string(),
                    prn: track.sv.prn,
                    source: source_key(track.sv).to_string(),
                    samples: track
                        .samples
                        .iter()
                        .map(|s| JsTrackSample {
                            epoch_index: s.epoch_index,
                            azimuth: s.azimuth_deg,
                            elevation: s.elevation_deg,
                            range_km: s.range_m / 1000.0,
                            ephemeris_age_s: s.ephemeris_age_s,
                        })
                        .collect(),
                })
                .collect(),
            dop: series
                .dop
                .iter()
                .map(|d| d.map(JsDop::from))
                .collect(),
            visible: series.visible.clone(),
            epochs_without_ephemeris: series.epochs_without_ephemeris,
        }
    }
}

/// The toggle a satellite belongs to.
///
/// For the Keplerian constellations this is just the constellation name; for
/// SBAS it is the operator, since a receiver cares about WAAS versus EGNOS and
/// not about the fact that both are "SBAS".
fn source_key(sv: Sv) -> &'static str {
    match sv.constellation {
        Constellation::Gps => "GPS",
        Constellation::Galileo => "GALILEO",
        Constellation::BeiDou => "BEIDOU",
        Constellation::Qzss => "QZSS",
        Constellation::Sbas => SbasProvider::from_prn(sv.prn).key(),
    }
}

/// Build options from a list of source keys.
///
/// `None` keeps the default (GPS only). An empty list is honoured literally --
/// the caller has turned everything off and should get an empty sky, not a
/// silent fallback to the default.
fn options_for(elevation_mask_deg: Option<f64>, sources: Option<Vec<String>>) -> SkyplotOptions {
    let mut options = SkyplotOptions::default();
    if let Some(mask) = elevation_mask_deg {
        options.elevation_mask_deg = mask;
    }

    if let Some(sources) = sources {
        let mut constellations = Vec::new();
        let mut providers = Vec::new();

        for key in sources {
            match key.to_ascii_uppercase().as_str() {
                "GPS" => constellations.push(Constellation::Gps),
                "GALILEO" => constellations.push(Constellation::Galileo),
                "BEIDOU" => constellations.push(Constellation::BeiDou),
                "QZSS" => constellations.push(Constellation::Qzss),
                other => {
                    if let Some(provider) = SbasProvider::from_key(other) {
                        providers.push(provider);
                        if !constellations.contains(&Constellation::Sbas) {
                            constellations.push(Constellation::Sbas);
                        }
                    }
                    // Unrecognised keys are ignored rather than erroring: the
                    // UI may know about a source this build does not.
                }
            }
        }

        options.constellations = constellations;
        options.sbas_providers = providers;
    }

    options
}

fn to_js_error(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Compute a sky plot from raw RINEX navigation bytes.
///
/// * `rinex_nav_data` -- contents of a RINEX Nav file, plain or gzipped.
/// * `lat`, `lon` -- observer position \[deg\], WGS-84.
/// * `alt` -- observer height above the ellipsoid \[m\].
/// * `timestamp` -- Unix time \[s\]. Pass `Date.now() / 1000`.
/// * `elevation_mask_deg` -- omit satellites below this; defaults to 5.
/// * `sources` -- which sources to include, e.g. `["GPS", "WAAS"]`. Omit for
///   the default (GPS only); an empty array yields an empty sky.
///
/// Re-parses the file on every call. For repeated queries use [`Skyplotter`].
#[wasm_bindgen]
pub fn compute_skyplot(
    rinex_nav_data: &[u8],
    lat: f64,
    lon: f64,
    alt: f64,
    timestamp: f64,
    elevation_mask_deg: Option<f64>,
    sources: Option<Vec<String>>,
) -> Result<JsValue, JsValue> {
    let set = gnss_core::parse_nav(rinex_nav_data).map_err(to_js_error)?;
    let view = skyplot_from_set(
        &set,
        Geodetic::new(lat, lon, alt),
        GpsTime::from_unix_seconds(timestamp),
        &options_for(elevation_mask_deg, sources),
    )
    .map_err(to_js_error)?;

    serde_wasm_bindgen::to_value(&JsSkyView::from(view)).map_err(to_js_error)
}

/// A parsed RINEX navigation file, reusable across many queries.
///
/// Parsing a daily broadcast file is the expensive part of a sky plot; the
/// geometry itself is microseconds. Holding the parsed set on the JS side
/// keeps map clicks instant.
#[wasm_bindgen]
pub struct Skyplotter {
    set: EphemerisSet,
}

#[wasm_bindgen]
impl Skyplotter {
    /// Parse a RINEX Nav file, plain or gzipped.
    #[wasm_bindgen(constructor)]
    pub fn new(rinex_nav_data: &[u8]) -> Result<Skyplotter, JsValue> {
        let set = gnss_core::parse_nav(rinex_nav_data).map_err(to_js_error)?;
        Ok(Self { set })
    }

    /// Absorb a second RINEX file into this one.
    ///
    /// Broadcast files are published one per UTC day, but a user-chosen window
    /// can straddle midnight. Rather than making the caller juggle two plotters
    /// and stitch the results, it loads each day and folds them into one set;
    /// duplicate blocks across the overlap are dropped by `EphemerisSet`.
    pub fn extend(&mut self, rinex_nav_data: &[u8]) -> Result<(), JsValue> {
        let more = gnss_core::parse_nav(rinex_nav_data).map_err(to_js_error)?;
        self.set.merge(more);
        Ok(())
    }

    /// Number of ephemeris blocks held.
    #[wasm_bindgen(getter)]
    pub fn block_count(&self) -> usize {
        self.set.len()
    }

    /// Number of distinct satellites held.
    #[wasm_bindgen(getter)]
    pub fn satellite_count(&self) -> usize {
        self.set.satellites().count()
    }

    /// Compute a sky view for an observer and instant.
    ///
    /// Arguments match [`compute_skyplot`].
    pub fn skyplot(
        &self,
        lat: f64,
        lon: f64,
        alt: f64,
        timestamp: f64,
        elevation_mask_deg: Option<f64>,
        sources: Option<Vec<String>>,
    ) -> Result<JsValue, JsValue> {
        let view = skyplot_from_set(
            &self.set,
            Geodetic::new(lat, lon, alt),
            GpsTime::from_unix_seconds(timestamp),
            &options_for(elevation_mask_deg, sources),
        )
        .map_err(to_js_error)?;

        serde_wasm_bindgen::to_value(&JsSkyView::from(view)).map_err(to_js_error)
    }

    /// Sample the sky view across an interval.
    ///
    /// * `start_timestamp`, `end_timestamp` -- Unix seconds, inclusive.
    /// * `step_s` -- sampling interval in seconds.
    ///
    /// Computing the whole series in one call rather than looping `skyplot` on
    /// the JavaScript side is worth it twice over: it crosses the WASM boundary
    /// once instead of hundreds of times, and it keeps the DOP solution beside
    /// the geometry it is derived from.
    pub fn skyplot_series(
        &self,
        lat: f64,
        lon: f64,
        alt: f64,
        start_timestamp: f64,
        end_timestamp: f64,
        step_s: f64,
        elevation_mask_deg: Option<f64>,
        sources: Option<Vec<String>>,
    ) -> Result<JsValue, JsValue> {
        let series = skyplot_series(
            &self.set,
            Geodetic::new(lat, lon, alt),
            GpsTime::from_unix_seconds(start_timestamp),
            GpsTime::from_unix_seconds(end_timestamp),
            step_s,
            &options_for(elevation_mask_deg, sources),
        )
        .map_err(to_js_error)?;

        serde_wasm_bindgen::to_value(&JsSkySeries::from(series)).map_err(to_js_error)
    }
}
