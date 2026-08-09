/**
 * What the app currently supports, in one place.
 *
 * Note the distinction: the CONUS restriction is a *product scope* decision
 * for phase 1, not a limitation of `gnss-core`. The math is global — the
 * broadcast ephemeris covers every satellite everywhere, and the geodesy is
 * WGS-84 worldwide. Widening the region is a change to this file.
 */

/** Clickable region: roughly the continental United States. */
export const CONUS = {
  south: 24.4,
  north: 49.5,
  west: -125.0,
  east: -66.9,
} as const;

export function isWithinConus(lat: number, lon: number): boolean {
  return (
    lat >= CONUS.south &&
    lat <= CONUS.north &&
    lon >= CONUS.west &&
    lon <= CONUS.east
  );
}

/** Earliest day in the BKG broadcast archive (probed empirically). */
export const ARCHIVE_START = "2017-06-01";

/**
 * Version of the server-side RINEX trimming performed by `/api/ephemeris`.
 *
 * Responses for past days are served `immutable` with a long max-age, because
 * a published broadcast file never changes. But *our processing* of it can —
 * and when it does, already-cached clients would otherwise keep the old body
 * forever. Bumping this changes the request URL and therefore the cache key.
 *
 * v2: stopped emitting trailing blank lines, which made the RINEX parser panic.
 */
export const EPHEMERIS_FORMAT_VERSION = 2;

export type ConstellationStatus = "active" | "available" | "unsupported";

export interface ConstellationInfo {
  /** RINEX 3 code. */
  code: string;
  name: string;
  status: ConstellationStatus;
  note: string;
}

/**
 * Constellation support, stated honestly.
 *
 * `gnss-core` propagates any constellation that broadcasts Keplerian elements,
 * so Galileo/BeiDou/QZSS already work at the code level — but only GPS has been
 * validated against the independent reference implementation, so only GPS is
 * switched on. GLONASS is a genuinely different problem: it broadcasts a
 * position/velocity state vector needing numerical integration, not orbital
 * elements, so it needs its own propagator.
 */
export const CONSTELLATIONS: ConstellationInfo[] = [
  {
    code: "G",
    name: "GPS",
    status: "active",
    note: "Validated against an independent reference implementation.",
  },
  {
    code: "E",
    name: "Galileo",
    status: "available",
    note: "Core propagates it; not yet validated, so not enabled in phase 1.",
  },
  {
    code: "C",
    name: "BeiDou",
    status: "available",
    note: "Core propagates MEO/IGSO; GEO needs a separate rotation. Not enabled.",
  },
  {
    code: "J",
    name: "QZSS",
    status: "available",
    note: "Core propagates it; not yet validated, so not enabled in phase 1.",
  },
  {
    code: "R",
    name: "GLONASS",
    status: "unsupported",
    note: "Broadcasts a state vector, not Keplerian elements. Needs its own propagator.",
  },
];

/** Constellations actually switched on, as a RINEX code list for the API. */
export const ACTIVE_CONSTELLATION_CODES = CONSTELLATIONS.filter(
  (c) => c.status === "active",
)
  .map((c) => c.code)
  .join(",");
