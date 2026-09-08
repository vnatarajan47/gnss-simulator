/**
 * What the app supports, in one place.
 *
 * Coverage is global. The maths always was — the broadcast ephemeris covers
 * every satellite everywhere and the geodesy is WGS-84 worldwide — but phase 1
 * scoped the product to the continental US. Phase 5 drops that: the only
 * remaining limits are the archive's start date and, for the regional
 * augmentation systems, where their satellites can actually be seen from.
 */

/**
 * Fold a longitude into [-180, 180).
 *
 * Leaflet reports longitudes outside that range once the user pans past the
 * edge of the world onto a repeated copy of it — a click on the Americas can
 * arrive as -460 deg. The geodesy handles it correctly either way (sin and cos
 * do not care), but the number is shown to the user and round-trips through
 * the URL, so it is normalised at the boundary.
 */
export function normaliseLongitude(lon: number): number {
  // In-range values are returned untouched rather than run through the modulo
  // round trip, which is not exact: `((-104.9903 + 180) % 360 + 360) % 360 -
  // 180` comes back as -104.99029999999999. Almost every call is a value that
  // is already in range, and perturbing it by an ulp would both show a silly
  // number in the coordinate field and churn React state on a no-op.
  if (lon >= -180 && lon < 180) return lon;
  return ((((lon + 180) % 360) + 360) % 360) - 180;
}

/**
 * Whether a coordinate pair is one this app can compute for.
 *
 * Everywhere on the ellipsoid, so the only rejects are values that are not
 * coordinates at all: `NaN` from an empty numeric input, or a latitude past
 * the poles.
 */
export function isWithinCoverage(lat: number, lon: number): boolean {
  return (
    Number.isFinite(lat) && Number.isFinite(lon) && lat >= -90 && lat <= 90
  );
}

/** Earliest day in the BKG broadcast archive (probed empirically). */
export const ARCHIVE_START = "2017-06-01";

/**
 * Earliest day the archive carries the full SBAS provider set (probed
 * empirically, and revisited when the other providers were switched on).
 *
 * SBAS coverage in this product is much shallower than its GNSS coverage, and
 * it is not a single cutoff. Files before ~2021 carry no SBAS at all. From
 * 2021 only EGNOS is reliably present; sampled days in 2022 add GAGAN, SDCM
 * and BDSBAS, but a sampled day in mid-2023 is back to EGNOS alone. WAAS,
 * MSAS, KASS and SouthPAN all appear together from early 2025, and from there
 * the set is stable.
 *
 * So there are two dates rather than one, held per source in [`SOURCES`]:
 * EGNOS from 2021 and everything else from 2025. The intermittency in between
 * is why the UI warns rather than blocks — an empty SBAS layer on a 2022 date
 * is the archive's doing, not a bug.
 */
export const SBAS_AVAILABLE_FROM = "2025-01-01";

/** Earliest day EGNOS appears in the archive (probed empirically). */
export const EGNOS_AVAILABLE_FROM = "2021-01-01";

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
 * v5: responses are trimmed to the requested time window as well as to the
 *     requested constellations. A v4 client's cached body is a whole day, so
 *     it is still *correct* — but it is several times larger than it needs to
 *     be, and the two cannot share a cache entry.
 */
export const EPHEMERIS_FORMAT_VERSION = 5;

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
  /**
   * Augmentation providers are grouped separately in the UI: they are regional
   * where the core constellations are global, so which one to enable depends
   * on where the receiver is.
   */
  kind: "constellation" | "augmentation";
  /** SBAS only: the PRNs this operator flies, so the API can trim to them. */
  prns?: number[];
  /** SBAS only: roughly where its satellites are usable, for the UI. */
  region?: string;
  /**
   * Earliest UTC day this source appears in the archive, `YYYY-MM-DD`.
   *
   * Absent means "as far back as the archive goes".
   */
  availableFrom?: string;
  note: string;
}

/**
 * Everything the UI knows how to show.
 *
 * SBAS is one RINEX constellation but many independent regional systems, so it
 * is listed per operator: a receiver in Europe has no use for a satellite
 * parked over the Pacific. Adding a new SBAS provider is one entry here plus
 * one line in `SbasProvider::from_prn` on the Rust side.
 *
 * PRN assignments are as published for 2025–2026 and do change as satellites
 * are retired.
 */
export const SOURCES: Source[] = [
  {
    key: "GPS",
    label: "GPS",
    rinexCode: "G",
    kind: "constellation",
    status: "supported",
    note: "Validated against an independent reference implementation.",
  },
  {
    key: "GALILEO",
    label: "Galileo",
    rinexCode: "E",
    kind: "constellation",
    status: "supported",
    note: "Same Keplerian element set as GPS, with GTRF constants. Validated against the same independent reference.",
  },
  {
    key: "BEIDOU",
    label: "BeiDou",
    rinexCode: "C",
    kind: "constellation",
    status: "supported",
    note: "Includes the geostationary satellites, which need the ICD's separate frame transformation. Cross-checked against the BDSBAS signal from the same spacecraft.",
  },
  {
    key: "QZSS",
    label: "QZSS",
    rinexCode: "J",
    kind: "constellation",
    status: "supported",
    note: "Regional: over Asia-Pacific. Validated against the same independent reference.",
  },
  {
    key: "GLONASS",
    label: "GLONASS",
    rinexCode: "R",
    kind: "constellation",
    status: "unsupported",
    note: "Broadcasts a state vector needing numerical integration. Needs its own propagator.",
  },
  {
    key: "WAAS",
    label: "WAAS",
    rinexCode: "S",
    kind: "augmentation",
    status: "supported",
    prns: [131, 133, 135, 138],
    region: "North America",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "US augmentation. Geostationary; validated against closed-form GEO geometry.",
  },
  {
    key: "EGNOS",
    label: "EGNOS",
    rinexCode: "S",
    kind: "augmentation",
    status: "supported",
    prns: [120, 121, 123, 126, 136],
    region: "Europe, North Africa",
    availableFrom: EGNOS_AVAILABLE_FROM,
    note: "European augmentation. Its records mix kilometres and metres; the units are resolved physically.",
  },
  {
    key: "MSAS",
    label: "MSAS",
    rinexCode: "S",
    kind: "augmentation",
    status: "supported",
    prns: [129, 137],
    region: "Japan, western Pacific",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "Japanese augmentation, now broadcast from the QZSS geostationary satellites.",
  },
  {
    key: "GAGAN",
    label: "GAGAN",
    rinexCode: "S",
    kind: "augmentation",
    status: "supported",
    prns: [127, 128, 132],
    region: "India, Indian Ocean",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "Indian augmentation. PRN 127 sits about 2 deg off the equatorial plane.",
  },
  {
    key: "BDSBAS",
    label: "BDSBAS",
    rinexCode: "S",
    kind: "augmentation",
    status: "supported",
    prns: [130, 143, 144],
    region: "China, east Asia",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "Chinese augmentation, broadcast from BeiDou's geostationary satellites.",
  },
  {
    key: "SOUTHPAN",
    label: "SouthPAN",
    rinexCode: "S",
    kind: "augmentation",
    status: "supported",
    prns: [122],
    region: "Australia, New Zealand",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "Australian and New Zealand augmentation.",
  },
  {
    key: "SDCM",
    label: "SDCM",
    rinexCode: "S",
    kind: "augmentation",
    status: "unvalidated",
    prns: [125, 140, 141],
    region: "Russia",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "Russian augmentation. Absent from every archive day sampled since 2022, so there has been nothing to validate against.",
  },
  {
    key: "KASS",
    label: "KASS",
    rinexCode: "S",
    kind: "augmentation",
    status: "unvalidated",
    prns: [134],
    region: "Korea",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "Korean augmentation. Present in the archive only as a saturated daily placeholder, which the parser rejects as physically impossible — so there is no real data to validate.",
  },
  {
    key: "SBAS-OTHER",
    label: "Other SBAS",
    rinexCode: "S",
    kind: "augmentation",
    status: "supported",
    // The SBAS PRN band, minus every PRN assigned above. PRNs 142 and 148 turn
    // up in the archive with no published operator; showing them under a
    // plainly provisional label beats either guessing or hiding them.
    prns: [124, 139, 142, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158],
    region: "unassigned PRNs",
    availableFrom: SBAS_AVAILABLE_FROM,
    note: "Satellites in the SBAS PRN range with no published operator assignment. Same geometry as the rest; the label is what is uncertain, not the position.",
  },
];

/** Sources the user is allowed to switch on. */
export const SWITCHABLE = SOURCES.filter((s) => s.status === "supported");

/**
 * What is enabled when the page first loads.
 *
 * The two global constellations, and nothing regional. Augmentation is left
 * off deliberately: which provider is useful depends on where the receiver is,
 * and silently enabling one based on the map position would be a guess the
 * user cannot see. The toggles say which region each covers instead.
 */
export const DEFAULT_ENABLED: string[] = ["GPS", "GALILEO"];

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
 * every regional system, where one provider is ~1 000. The authoritative
 * filter is still the provider check inside `gnss-core`, so an over-broad list
 * here only costs bytes, never correctness.
 */
export function sbasPrnsFor(enabled: string[]): number[] {
  const prns = new Set<number>();
  for (const key of enabled) {
    for (const prn of sourceByKey(key)?.prns ?? []) prns.add(prn);
  }
  return [...prns].sort((a, b) => a - b);
}

/**
 * Enabled sources whose data does not reach back to the days being loaded.
 *
 * Returned rather than merely flagged so the UI can name them: "no SBAS before
 * 2025" was fine when WAAS was the only augmentation, but with eight providers
 * on two different start dates the user needs to know *which* of their choices
 * will come back empty.
 */
export function sourcesUnavailableOn(enabled: string[], days: string[]): Source[] {
  if (days.length === 0) return [];
  const earliest = days.reduce((a, b) => (a < b ? a : b));

  return enabled
    .map(sourceByKey)
    .filter((source): source is Source => source !== undefined)
    .filter((source) => source.availableFrom !== undefined && earliest < source.availableFrom);
}
