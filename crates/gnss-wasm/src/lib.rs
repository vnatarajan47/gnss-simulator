//! wasm-bindgen surface for [`gnss_core`].
//!
//! Two entry points:
//!
//! - [`compute_skyplot`] -- one shot: bytes in, satellite list out.
//! - [`Skyplotter`] -- parse a RINEX file once, then query it repeatedly.
//!   Preferred for the map UI, where every click re-evaluates the same file.
//!
//! Build with `wasm-pack build --target web`.

use gnss_core::{skyplot_from_set, EphemerisSet, Geodetic, GpsTime, SkyView, SkyplotOptions};
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
    /// RINEX identifier, e.g. `G07`.
    sv: String,
    /// PRN within the constellation.
    prn: u8,
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

fn options_for(elevation_mask_deg: Option<f64>) -> SkyplotOptions {
    let mut options = SkyplotOptions::default();
    if let Some(mask) = elevation_mask_deg {
        options.elevation_mask_deg = mask;
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
) -> Result<JsValue, JsValue> {
    let set = gnss_core::parse_nav(rinex_nav_data).map_err(to_js_error)?;
    let view = skyplot_from_set(
        &set,
        Geodetic::new(lat, lon, alt),
        GpsTime::from_unix_seconds(timestamp),
        &options_for(elevation_mask_deg),
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
    ) -> Result<JsValue, JsValue> {
        let view = skyplot_from_set(
            &self.set,
            Geodetic::new(lat, lon, alt),
            GpsTime::from_unix_seconds(timestamp),
            &options_for(elevation_mask_deg),
        )
        .map_err(to_js_error)?;

        serde_wasm_bindgen::to_value(&JsSkyView::from(view)).map_err(to_js_error)
    }
}
