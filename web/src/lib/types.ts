/** Mirrors the `JsSkyView` / `JsSatellite` shapes serialized by gnss-wasm. */

export interface Satellite {
  /** RINEX identifier, e.g. `G07`. */
  sv: string;
  /** PRN within the constellation. SBAS reports the true PRN (131). */
  prn: number;
  /** Which toggle this satellite belongs to: `GPS`, `WAAS`, ... */
  source: string;
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

/** Dilution of precision at one epoch. Dimensionless; smaller is better. */
export interface Dop {
  gdop: number;
  pdop: number;
  hdop: number;
  vdop: number;
  tdop: number;
  /** How many satellites entered the solution. */
  satellites: number;
}

/** One satellite at one sampled instant of a series. */
export interface TrackSample {
  /** Index into `SkySeries.epochs`. */
  epochIndex: number;
  azimuth: number;
  elevation: number;
  rangeKm: number;
  ephemerisAgeS: number;
}

/** One satellite's path across the window. */
export interface SatelliteTrack {
  sv: string;
  prn: number;
  source: string;
  /**
   * Ascending by `epochIndex`, with gaps where the satellite was below the
   * mask. A break in the run is a set/rise and must not be drawn as a line —
   * see `segmentTrack` in `lib/trackLayout.ts`.
   */
  samples: TrackSample[];
}

/** Mirrors `JsSkySeries` from gnss-wasm. */
export interface SkySeries {
  /** Unix seconds, ascending and evenly spaced. */
  epochs: number[];
  tracks: SatelliteTrack[];
  /** Per epoch; `null` where the geometry admits no solution. */
  dop: (Dop | null)[];
  /** Satellites above the mask, per epoch. */
  visible: number[];
  epochsWithoutEphemeris: number;
}
