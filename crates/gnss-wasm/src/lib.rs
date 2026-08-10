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
    skyplot_from_set, Constellation, EphemerisSet, Geodetic, GpsTime, SbasProvider, SkyView,
    SkyplotOptions, Sv,
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
}
