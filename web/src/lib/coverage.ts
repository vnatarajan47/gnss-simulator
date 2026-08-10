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
 * Earliest day with usable SBAS coverage in this product (probed empirically).
 *
 * The archive's SBAS coverage is much shallower than its GNSS coverage: files
 * before ~2021 carry no SBAS records at all, and from 2021 to late 2024 only
 * EGNOS (PRNs 123 and 136) is present. WAAS first appears in early 2025.
 */
export const SBAS_AVAILABLE_FROM = "2025-01-01";

/** Whether a UTC date predates usable SBAS coverage. */
export function isBeforeSbasCoverage(date: string): boolean {
  return date < SBAS_AVAILABLE_FROM;
}

/**
 * Version of the server-side RINEX trimming performed by `/api/ephemeris`.
 *
 * Responses for past days are served `immutable` with a long max-age, because
 * a published broadcast file never changes. But *our processing* of it can —
 * and when it does, already-cached clients would otherwise keep the old body
 * forever. Bumping this changes the request URL and therefore the cache key.
 *
 * v2: stopped emitting trailing blank lines, which made the RINEX parser panic.
 * v3: SBAS records are retained when requested.
 * v4: the server no longer serves a day's *partial* file as if it were
 *     complete. Clients that cached one under v3 hold it for a day, and
 *     `immutable` means they will not revalidate — this bump is the only way
 *     to reach them.
 */
export const EPHEMERIS_FORMAT_VERSION = 4;

/**
 * `supported` — validated and switchable.
 * `unvalidated` — the core propagates it, but it has not been checked against
 *   an independent implementation, so it stays off and non-switchable.
 * `unsupported` — needs a propagator we have not written.
 */
export type SourceStatus = "supported" | "unvalidated" | "unsupported";

export interface Source {
  /** Stable key, shared with the WASM boundary and the API. */
  key: string;
  label: string;
  /** RINEX 3 constellation code, used to trim the file server-side. */
  rinexCode: string;
  status: SourceStatus;
  /** SBAS only: the PRNs this operator flies, so the API can trim to them. */
  prns?: number[];
  note: string;
}

/**
 * Everything the UI knows how to show.
 *
 * SBAS is one RINEX constellation but many independent regional systems, so it
 * is listed per operator: a receiver in CONUS has no use for a satellite parked
 * over the Indian Ocean. Adding a new SBAS provider is one entry here plus one
 * line in `SbasProvider::from_prn` on the Rust side.
 *
 * PRN assignments are as published for 2025–2026 and do change as satellites
 * are retired.
 */
export const SOURCES: Source[] = [
  {
    key: "GPS",
    label: "GPS",
    rinexCode: "G",
    status: "supported",
    note: "Validated against an independent reference implementation.",
  },
  {
    key: "WAAS",
    label: "WAAS",
    rinexCode: "S",
    status: "supported",
    prns: [131, 133, 135, 138],
    note: "US SBAS. Geostationary; validated against closed-form GEO geometry. Present in the archive from 2025.",
  },
  {
    key: "EGNOS",
    label: "EGNOS",
    rinexCode: "S",
    status: "unvalidated",
    prns: [120, 121, 123, 126, 136],
    note: "European SBAS. Records in this product mix km and m units; not yet validated.",
  },
  {
    key: "MSAS",
    label: "MSAS",
    rinexCode: "S",
    status: "unvalidated",
    prns: [129, 137],
    note: "Japanese SBAS. Not visible from CONUS.",
  },
  {
    key: "GALILEO",
    label: "Galileo",
    rinexCode: "E",
    status: "unvalidated",
    note: "Core propagates it — same Keplerian set as GPS — but it is not yet validated.",
  },
  {
    key: "BEIDOU",
    label: "BeiDou",
    rinexCode: "C",
    status: "unvalidated",
    note: "MEO/IGSO would work; the GEO satellites need a separate rotation we have not written.",
  },
  {
    key: "QZSS",
    label: "QZSS",
    rinexCode: "J",
    status: "unvalidated",
    note: "Core propagates it; not yet validated. Not visible from CONUS.",
  },
  {
    key: "GLONASS",
    label: "GLONASS",
    rinexCode: "R",
    status: "unsupported",
    note: "Broadcasts a state vector needing numerical integration. Needs its own propagator.",
  },
];

/** Sources the user is allowed to switch on. */
export const SWITCHABLE = SOURCES.filter((s) => s.status === "supported");

/** What is enabled when the page first loads. */
export const DEFAULT_ENABLED: string[] = ["GPS", "WAAS"];

export function sourceByKey(key: string): Source | undefined {
  return SOURCES.find((s) => s.key === key);
}

/** RINEX constellation codes needed to satisfy a set of enabled sources. */
export function rinexCodesFor(enabled: string[]): string[] {
  const codes = new Set<string>();
  for (const key of enabled) {
    const source = sourceByKey(key);
    if (source) codes.add(source.rinexCode);
  }
  return [...codes].sort();
}

/**
 * SBAS PRNs needed for a set of enabled sources.
 *
 * Purely a payload optimisation: a full day of SBAS is ~12 800 records across
 * every regional system, where WAAS alone is ~1 000. The authoritative filter
 * is still the provider check inside `gnss-core`, so an over-broad list here
 * only costs bytes, never correctness.
 */
export function sbasPrnsFor(enabled: string[]): number[] {
  const prns = new Set<number>();
  for (const key of enabled) {
    for (const prn of sourceByKey(key)?.prns ?? []) prns.add(prn);
  }
  return [...prns].sort((a, b) => a - b);
}
