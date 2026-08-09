/** Mirrors the `JsSkyView` / `JsSatellite` shapes serialized by gnss-wasm. */

export interface Satellite {
  /** RINEX identifier, e.g. `G07`. */
  sv: string;
  /** PRN within the constellation. */
  prn: number;
  /** Azimuth [deg], 0 = true north, clockwise. */
  azimuth: number;
  /** Elevation [deg] above the horizon. */
  elevation: number;
  /** Geometric range [km]. */
  rangeKm: number;
  /** Signed `t - ToE` of the ephemeris used [s]. */
  ephemerisAgeS: number;
}

export interface SkyView {
  satellites: Satellite[];
  visibleCount: number;
  belowMask: number;
  withoutEphemeris: number;
  gpsSeconds: number;
}

export interface Observer {
  lat: number;
  lon: number;
  /** Height above the WGS-84 ellipsoid [m]. */
  altM: number;
}
