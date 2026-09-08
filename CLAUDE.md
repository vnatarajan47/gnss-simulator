# gnss-simulator

A phased GNSS signal simulator. Phase 1 (done): click any lat/lon on a 2D map
and see a live sky plot (az/el of visible satellites), computed client-side in
WASM from RINEX Nav broadcast ephemeris fetched for any date. Phase 2 (done):
pick a time *window*, see satellite tracks swept across it and HDOP/VDOP
plotted against time, with one slider scrubbing both. Phase 5 (done, pulled
forward): coverage is global, and GPS, Galileo, BeiDou, QZSS and six
augmentation providers are validated and switchable.

Repo: https://github.com/vnatarajan47/gnss-simulator (branch `master`).

## Architecture

```
crates/gnss-core/   Rust: RINEX parsing, ephemeris propagation, az/el geometry
crates/gnss-wasm/   wasm-bindgen wrapper: compute_skyplot(), Skyplotter{skyplot,skyplot_series,extend}
web/                Next.js (App Router, TypeScript), Leaflet map + SVG sky plot
data/                test fixtures + data/cache/ (disk cache, gitignored)
docs/methods.md      full methods reference: math, data sources, validation, error budget
docs/adr/            architecture decision records (0001-0008)
tools/               reference_skyplot.py -- independent Python implementation used only to validate the Rust math
```

Data flow: browser click → `web/src/app/api/ephemeris/route.ts` (Next.js API
route, server-side) fetches/caches BKG's RINEX Nav mirror → raw bytes shipped
to the browser → `crates/gnss-wasm` (loaded via `web/src/lib/wasm.ts`) parses
and propagates entirely client-side → `SkyPlot.tsx` renders az/el.

**Why a server-side fetch step at all, if propagation is client-side?**
BKG sends no CORS headers, so the browser can't fetch the RINEX file
directly. The Next.js route is a thin same-origin proxy + disk cache, not a
compute backend -- see ADR-0006 (amends ADR-0002, which established
client-side WASM as the compute model).

### The two-propagator split (`crates/gnss-core/src/ephemeris.rs`)

```rust
pub enum BroadcastEphemeris {
    Keplerian(KeplerianEphemeris),  // GPS, Galileo, BeiDou, QZSS
    Sbas(SbasEphemeris),            // WAAS, EGNOS, ... (geostationary)
}
```

GNSS satellites broadcast Keplerian elements (IS-GPS-200 Table 20-IV: mean
anomaly → Kepler's equation → true anomaly → ECEF). SBAS satellites broadcast
an ECEF state vector instead (position/velocity/acceleration; propagated by a
second-order Taylor expansion, not an orbit solve). These are genuinely
different data models, not a special case of one -- hence the enum split
rather than a single shared struct. `propagate.rs` dispatches on the enum.
GLONASS is deliberately unsupported (different orbital element set entirely);
NavIC appears in the archive from 2025 and is not wired up.

One case sits *inside* the Keplerian arm rather than beside it. BeiDou's
geostationary satellites reach Earth-fixed coordinates by a different route
(BDS-SIS-ICD-B1I §5.2.4.12): the node term leaves out `-omega_e * t_k`, and the
position is instead put through `Rz(omega_e t_k) Rx(-5 deg)`. Skipping it puts
the satellite thousands of km away. **Whether a satellite needs it is decided
from the broadcast elements** -- `KeplerianEphemeris::is_geostationary()`, near-
zero inclination *and* near-GEO semi-major axis -- not from a PRN table, for the
same reason `decode_scale` decides units physically: assignments change, and a
stale table is a silent error where a physical test is not. That same predicate
drives the sky plot's square markers, which is why it crosses the WASM boundary
as a flag instead of being re-derived from the satellite identifier (BeiDou and
QZSS both fly GEOs, so an `S` prefix no longer means "geostationary").

### `rinex` crate: parser only, not propagator

Audited whether the `rinex` crate's `nav` feature could be used directly for
propagation (ADR-0005). Its math is correct, but pulling in that feature
drags in `anise`, which transitively needs `memmap2`/`ureq`/`pyo3` --
none of which compile to `wasm32-unknown-unknown`. So `rinex` (with
`default-features = false`) is used only to parse RINEX Nav records; all
propagation math in `propagate.rs` and `sbas.rs` is hand-rolled.

Three defects found in the crate along the way, all worked around locally
rather than reported upstream yet:
- Uncapped Newton-Raphson loop for eccentric anomaly -- ours has a hard
  iteration cap (`Error::KeplerDidNotConverge`).
- Panics (unchecked slice) on zero-length lines in malformed RINEX. Contained
  two ways: `source.rs` trims trailing blank lines before handing bytes to
  the parser, and wraps the parse call in `catch_unwind` (`Error::ParserPanicked`).
  This is *why* `profile.release` in the workspace `Cargo.toml` deliberately
  does **not** set `panic = "abort"` -- `catch_unwind` requires unwinding.
- **An orbit field whose broadcast value is exactly zero is not surfaced at
  all**, so "absent" and "zero" are indistinguishable. This one is silent and
  selective. `lift_ephemeris` now requires only the *defining* elements and
  defaults perturbations and rates to zero; `lift_sbas` defaults the position
  components too and lets `decode_scale` reject an implausible radius. Before
  that, QZSS's two geostationary satellites (which broadcast `deltaN` and
  `idot` as exact zeros) and every SBAS satellite exactly on the equator
  (`satPosZ = 0`, i.e. all of EGNOS and SouthPAN) were dropped with no error.

Also worth knowing: the issue-of-data field goes by a different name in each
ICD, and the crate keeps the ICD's name -- `iode` (GPS/QZSS), `iodnav`
(Galileo), `aode` (BeiDou). Reading only `iode` left Galileo and BeiDou at NaN,
which quietly disabled both the duplicate check in `EphemerisSet::insert`
(NaN != NaN, so re-broadcasts across the midnight overlap accumulated) and the
selection tie-break. Galileo makes that tie-break load-bearing: it broadcasts
I/NAV and F/NAV records sharing a ToE but fitted separately, so ties are
routine and the higher issue of data wins.

### Validation standard

> The full write-up -- every algorithm, constant, data defect and validation
> result -- is `docs/methods.md`. Keep it in step when any of this changes; it
> is the document a reader is pointed at first.


No source is switched on in the UI (`web/src/lib/coverage.ts`, `SOURCES[]`,
`status: "unvalidated"`) until it has an independent cross-check appropriate to
its geometry. There are three kinds, in increasing order of what they prove:

- **A second implementation.** `tools/reference_skyplot.py` -- deliberately
  independent (fixed-point Kepler vs. Newton-Raphson, `asin` vs. `atan2`,
  fixed-column parsing instead of the `rinex` crate, SVD pseudo-inverse vs. an
  in-place normal matrix for DOP). Two vector sets:
  `reference_vectors.json` (GPS, 5 CONUS sites x 3 epochs, ~3e-13 deg) and
  `multignss_vectors.json` (GPS/Galileo/BeiDou/QZSS, six globally spread
  observers x 2 epochs x 3 system combinations = 779 satellites over 36 cases,
  1.1e-9 deg az, 4.0e-10 deg el, 6e-5 m range, DOP to 5.6e-12 over 1, 2 and 3
  clock unknowns).
- **Closed-form geometry.** GEO look angles from a fixed lat/lon have an exact
  analytic solution, so every SBAS satellite of every provider is checked
  against spherical trigonometry rather than against code that could share a
  misconception (`crates/gnss-core/tests/sbas_geometry.rs`). Written for a
  general sub-latitude, not the equator: GAGAN's PRN 127 sits 2.1 deg off the
  plane, worth 3 deg of azimuth.
- **A second broadcast of the same spacecraft.** BDSBAS rides on BeiDou's GEOs
  and MSAS on QZSS's, so five satellites appear in the file twice -- once as
  Keplerian elements, once as an ECEF state vector, fitted by different ground
  segments and propagated by code paths that share nothing. They agree to
  0.4-2.0 m. This is the primary evidence for BeiDou's GEO transformation,
  because extending the Python reference to cover it would only confirm *my*
  reading of the ICD against itself. See ADR-0008. The test also checks the
  separation does not *grow*, which is what distinguishes a frame error from
  the one genuinely-offset pair (C02 / PRN 144 are co-located satellites
  sharing the 80.1 deg E slot, a constant 2.8 km apart).

Plus a four-satellite tetrahedron solved by hand in `dop.rs`
(Q = 2/3, 2/3, 4/3, 1/3), and -- for the multi-system columns -- an analytic
identity: a system contributing exactly *one* satellite must leave HDOP/VDOP/
PDOP/TDOP bit-identical, because eliminating its clock unknown by Schur
complement subtracts precisely the outer product its own row contributed.

`status: "supported"` today: GPS, Galileo, BeiDou, QZSS, and WAAS/EGNOS/MSAS/
GAGAN/BDSBAS/SouthPAN plus the unassigned-PRN catch-all. SDCM and KASS stay
`unvalidated` -- not because the code cannot propagate them, but because the
archive contains no real data for either (KASS is a saturated daily
placeholder, which `decode_scale` rejects) and there is nothing to validate.

### Known broadcast-data quirks (handled explicitly, not assumed away)

- **SBAS units**: the merged IGS product mixes km and m across files/PRNs
  inconsistently. `sbas.rs::decode_scale` disambiguates physically -- SBAS
  satellites are geostationary, so only one unit interpretation puts the
  computed radius in the plausible GEO band.
- **SBAS PRN numbering**: RINEX on-disk form is the two-digit PRN minus 100
  (e.g. `S31`), but the meaningful ID (matching FAA/RTCA naming, e.g. WAAS
  135) is the full 3-digit PRN. `source.rs::normalise_prn` restores it on
  ingest (idempotent). `Sv::rinex_id()` reconstructs the on-disk form only
  where the RINEX spec is being spoken (never in UI-facing `Display`).
- **SBAS health**: the broadcast health word is sometimes all-ones filler,
  not real data -- `health_is_meaningful()` gates whether it's trusted.
- **QZSS health**: the same trap, milder, and it bites the *Keplerian* path.
  QZSS's word is a per-signal bitfield whose bits stay set for signals a
  satellite does not carry, so no QZSS record in this product ever reads zero
  -- J02/J03/J07 sit at 1 and J04/J08 at 16. The plain `health == 0` rule
  discarded the whole constellation. `KeplerianEphemeris::is_healthy()` now
  dispatches on constellation and excludes QZSS only on the all-ones word.
  Note this must stay strict for Galileo: 16 and 130 are real signal-health
  flags there, and they are what excludes E14/E18, the pair stranded in
  eccentric orbits.
- **Daily placeholders**: some PRNs (KASS 134, PRN 142) appear only as an
  almanac-shaped filler with `z = 32767`, a saturated 16-bit field. Neither
  unit reading of that is a GEO radius, so `decode_scale` rejects it without
  needing to know the sentinel exists.

### Ephemeris cache correctness (`web/src/app/api/ephemeris/route.ts`)

Two rules that exist because of real bugs hit during development:
- `EPHEMERIS_FORMAT_VERSION` is embedded in the cache key/request URL. Bump
  it whenever server-side trimming/parsing logic changes, or the browser's
  `immutable`-cached response for an old format silently never refreshes.
- The response is trimmed to the requested time *window* as well as to the
  constellations (`from`/`to`, widened by `WINDOW_MARGIN_S = 3 h` to cover the
  curve-fit interval at either end). With several constellations enabled a
  whole day is several MB of text, most of it hours the user is not looking at.
  A record whose epoch will not parse is *kept* -- this is a size optimisation,
  and its failure mode must be a larger response, never a missing satellite.
- The cache directory is `EPHEMERIS_CACHE_DIR`, else `data/cache` in a
  checkout, else the system temp dir, and every cache operation is
  best-effort. A read-only filesystem is normal in a deployed container, and
  losing the cache costs a refetch where crashing costs the page.
- A "today" file fetched while the day is still in progress is necessarily
  partial (BKG publishes it incrementally). The cache only trusts a cached
  file once `mtime > endOfDay` for the date it covers; otherwise it refetches
  even on a cache hit.

### Time series and DOP (phase 2)

`skyplot_series()` (`crates/gnss-core/src/series.rs`) samples the single-epoch
path repeatedly rather than reimplementing it. That is deliberate: the UI shows
a cursor and a table for the same instant, and two parallel implementations
could drift apart. One implementation sampled repeatedly cannot.

Series results are **sparse** — each satellite track carries only the epochs
where it was above the mask, tagged with an `epoch_index`. A break in the run
*is* a set/rise. A dense array with a "not visible" sentinel would let any
consumer that forgot to check it draw a line straight across the sky between a
set and the next rise; with indices the drawing code is forced to look.
`segmentTrack` in `web/src/lib/trackLayout.ts` is the only place that decision
is made, and it is tested directly.

Every array in a series indexes the shared `epochs` axis. That is what makes
the single slider synchronous *by construction*: the sky plot and the DOP chart
are handed the same integer, never two independently-converted timestamps.

DOP (`crates/gnss-core/src/dop.rs`) is `(AᵀA)⁻¹` over unit line-of-sight
vectors, with **one clock column per `TimeSystem`** — the change ADR-0007
records. GPS, QZSS and SBAS share a clock (all steered to GPS time); Galileo
and BeiDou each get their own. `Constellation::time_system()` is the single
place a constellation declares whose clock it is on, and the grouping is the
physics: opening a column per *constellation* would be wrong in the expensive
direction, since four GPS satellites plus a WAAS GEO would then report no
solution where a real receiver has one.

Consequences that are easy to break:
- The minimum satellite count is `3 + systems`, not 4 (`min_satellites`).
  Four satellites split two-and-two across two systems has five unknowns and
  **no solution** — it must not fall back to the four-satellite case.
- Column order is the enum order, so the layout depends on the *set* of systems
  present and not on how the caller sorted its satellites. `tdop` is the lowest
  system present, i.e. GPS whenever GPS is in the solution.
- Adding a system's satellites can make DOP *worse*. That is real, not a bug:
  they bring an unknown with them.
- `Dop::systems` crosses the WASM boundary and is displayed, because the count
  changes what the other numbers mean.

Two rules the UI must not break:
- Too few satellites for the number of clock unknowns, or degenerate geometry,
  means **no solution** — `None`, never zero. Zero on a precision chart reads
  as *perfect* precision at exactly the moments there is no fix. `dopChart.ts`
  breaks the line instead.
- The DOP axis autoscales rather than clipping. A spike marks the geometry
  collapsing and is the most interesting thing the chart can show.

The WASM-to-TypeScript field names are a real contract with no compiler on
either side of it -- a renamed Rust field becomes `undefined` in TS silently.
`crates/gnss-wasm/src/lib.rs` tests assert the serialised key sets against what
`web/src/lib/types.ts` declares. Series epochs cross as **Unix** seconds (the
single-epoch view reports GPS seconds); that too is asserted, since confusing
them shifts the axis by decades.

Windows are capped at 24 h (`web/src/lib/interval.ts`), where two limits meet:
a GPS ground track repeats every sidereal day, so longer mostly redraws itself,
and a <=24 h window touches at most two daily broadcast files. Windows crossing
UTC midnight load both days and `EphemerisSet::merge`s them — which is also
strictly *better* than one file near a boundary, since an epoch at 00:10 has
its nearest ToE in the previous day's file.

### Sky-plot label layout (`web/src/lib/skyPlotLayout.ts`)

Pure geometry, no React/DOM, specifically so it's unit-testable
(`skyPlotLayout.test.ts`, 13 tests via Node's built-in `node:test` runner --
Node 24 strips TypeScript natively, so this added zero dependencies; run with
`npm test` in `web/`). Markers sit at their true az/el projection always;
labels are placed by a greedy outward search (clear of every other marker and
every already-placed label) with a leader line drawn when a label had to be
displaced. Labels read `G05` / `S131` (constellation code + full PRN).

This was built ahead of Phase 2 on purpose, and paid off: the slider re-lays
out labels on every tick, so the determinism test (same input → byte-identical
layout) is what stops labels flickering as the cursor moves.

## Coverage limits (`web/src/lib/coverage.ts`)

- Spatial: **global**. `isWithinCoverage` rejects only non-coordinates (NaN
  from an empty field, a latitude past the pole). `normaliseLongitude` folds
  longitudes back into [-180, 180) at the boundary, because panning past the
  edge of the world has Leaflet counting on into a repeated copy of it; it
  returns in-range values *untouched* rather than through the modulo round
  trip, which is not exact and would churn state on a no-op.
- Temporal: `ARCHIVE_START = "2017-06-01"` for the core constellations.
  Augmentation availability is **per source** (`Source.availableFrom`), not one
  date for "SBAS": EGNOS from 2021, everything else from 2025. Bounds were
  found empirically by probing the BKG archive, not documented anywhere
  upstream, and coverage between 2021 and 2025 is genuinely intermittent --
  which is why the UI says a source "may be missing or incomplete" rather than
  "will be empty", and why `sourcesUnavailableOn` names the user's own choices
  instead of the constellation.
- Data source: BKG's open IGS mirror (`BRDC00WRD_*_MN.rnx.gz`), not CDDIS --
  CDDIS requires an Earthdata login and this project handles no credentials
  anywhere.
- Sources are grouped in the UI by `Source.kind`. That is not cosmetic: core
  constellations are global, augmentation is regional, so each provider carries
  the `region` it serves. `DEFAULT_ENABLED` is GPS + Galileo and nothing
  regional -- enabling an augmentation provider by default would be a guess
  about where the receiver is that the user cannot see.

## Build & test

```bash
cargo test --workspace        # 102 tests: core math, cross-checks, WASM boundary
cd web && npm run build:wasm  # rebuild crates/gnss-wasm -> web/src/wasm/
cd web && npm test            # pure-logic tests (node --test), 92 tests
cd web && npm run typecheck   # tsc --noEmit
cd web && npm run dev         # dev server, localhost:3000
cd web && npm run build       # production build, for deployment
```

Deployment needs a writable `EPHEMERIS_CACHE_DIR` (optional -- it falls back to
the system temp dir) and outbound access to `igs.bkg.bund.de`. Nothing else.

## Roadmap

1. **Phase 1 -- done.** Static sky plot for any point/time, GPS + WAAS,
   client-side WASM propagation, collision-avoiding labels.
2. **Phase 2 -- done.** Time-window selection (24 h cap), satellite tracks
   with rise/set gaps, HDOP/VDOP against time, one slider scrubbing both
   plots, multi-day ephemeris merging across UTC midnight.
3. **Phase 5 -- done, pulled forward.** Global coverage; Galileo, BeiDou and
   QZSS validated and switched on; six augmentation providers likewise;
   multi-system DOP; deployable without a writable checkout. Pulled ahead of
   phases 3 and 4 to get the app into a shape worth showing.
4. **Phase 3.** Terrain masking (local horizon obstruction, not just the
   elevation-angle mask).
5. **Phase 4.** IQ signal generation.
6. **Phase 6.** Dynamic targets: receiver trajectories, with Doppler and delay
   evolving along the path.

Explicitly out of scope: GLONASS (a third propagator -- a state vector needing
numerical integration of the equations of motion; not planned), NavIC (in the
archive from 2025, not wired up), automated multi-source ephemeris fetching
beyond the single BKG mirror, and correcting broadcast orbits against precise
products.

Known follow-ups, deliberately not done here:
- QZSS health is decoded coarsely (all-ones word only). Per-signal decoding is
  IS-QZSS-PNT §4.1.2.3.
- The sky plot gets crowded at 50 satellites. Label layout copes, but a filter
  or a per-source visibility control is the obvious next affordance.
- SDCM and KASS cannot be validated until the archive carries real data for
  them; both are wired up and waiting.

## Design goals

- **Correctness over speed**, especially in `gnss-core`: three kinds of
  independent cross-check exist (a second implementation, closed-form geometry,
  and a second broadcast of the same spacecraft) and a source isn't user-facing
  until it has one appropriate to its geometry.
- **No rewrites for new constellations**: the `Constellation` enum, the
  `BroadcastEphemeris` Keplerian/Sbas split, and `coverage.ts`'s `SOURCES[]`
  table are the three places a new constellation touches; propagation
  dispatch and UI toggling both key off them already.
- **No credentials anywhere in the project** (informed the BKG-over-CDDIS
  choice; keep this constraint when adding data sources).
- **Fail loud, not silently wrong**: parser panics are caught and surfaced
  as errors, not swallowed; ambiguous units/health data are resolved by
  physical reasoning and documented rather than assumed.
