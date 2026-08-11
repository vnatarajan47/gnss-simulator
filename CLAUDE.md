# gnss-simulator

A phased GNSS signal simulator. Phase 1 (done): click any lat/lon in CONUS on
a 2D map and see a live sky plot (az/el of visible satellites), computed
client-side in WASM from RINEX Nav broadcast ephemeris fetched for any date.
Phase 2 (done): pick a time *window*, see satellite tracks swept across it and
HDOP/VDOP plotted against time, with one slider scrubbing both.
Phase 4 (done, ahead of phase 3): submit a job and get back a synthetic
baseband **IQ recording** -- real C/A codes, real LNAV frames, Doppler,
atmospheric delay and thermal noise -- as a raw interleaved binary plus a JSON
sidecar.

Repo: https://github.com/vnatarajan47/gnss-simulator (branch `master`).

## Architecture

```
crates/gnss-core/   Rust: RINEX parsing, ephemeris propagation, clock, atmosphere, az/el geometry
crates/gnss-iq/     Rust: IQ synthesis -- job schema, noise models, C/A codes, LNAV, sample generation
                    (native only, NOT in the wasm build) + the gnss-iq-worker binary
crates/gnss-wasm/   wasm-bindgen wrapper: compute_skyplot(), Skyplotter{skyplot,skyplot_series,extend}
web/                Next.js (App Router, TypeScript), Leaflet map + SVG sky plot, job API
data/               cached RINEX Nav files (data/cache/), job records and outputs (data/jobs/)
docs/adr/           architecture decision records (0001-0007)
tools/              reference_skyplot.py -- independent Python implementation used only to validate the Rust math
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
rather than a single shared struct. `propagate.rs` dispatches on the enum;
adding Galileo/BeiDou/QZSS is a new `Constellation` variant plus wiring, no
propagator rewrite. GLONASS is deliberately unsupported (different orbital
element set entirely).

### `rinex` crate: parser only, not propagator

Audited whether the `rinex` crate's `nav` feature could be used directly for
propagation (ADR-0005). Its math is correct, but pulling in that feature
drags in `anise`, which transitively needs `memmap2`/`ureq`/`pyo3` --
none of which compile to `wasm32-unknown-unknown`. So `rinex` (with
`default-features = false`) is used only to parse RINEX Nav records; all
propagation math in `propagate.rs` and `sbas.rs` is hand-rolled.

Two defects found in the crate along the way, both worked around locally
rather than reported upstream yet:
- Uncapped Newton-Raphson loop for eccentric anomaly -- ours has a hard
  iteration cap (`Error::KeplerDidNotConverge`).
- Panics (unchecked slice) on zero-length lines in malformed RINEX. Contained
  two ways: `source.rs` trims trailing blank lines before handing bytes to
  the parser, and wraps the parse call in `catch_unwind` (`Error::ParserPanicked`).
  This is *why* `profile.release` in the workspace `Cargo.toml` deliberately
  does **not** set `panic = "abort"` -- `catch_unwind` requires unwinding.

### Validation standard

No source is switched on in the UI (`web/src/lib/coverage.ts`, `SOURCES[]`,
`status: "unvalidated"`) until it has an independent cross-check:
- **Keplerian (GPS)**: `tools/reference_skyplot.py`, a deliberately
  independent second implementation (different Kepler solver -- fixed-point
  vs. Newton-Raphson, different elevation formula -- `asin` vs. `atan2`,
  fixed-column parsing instead of the `rinex` crate). Agreement to ~3e-13 deg
  az/el, ~1e-8 m range across 143 satellites / 15 cases
  (`crates/gnss-core/tests/reference_cross_check.rs`).
- **SBAS (WAAS)**: closed-form geostationary geometry, not a second program
  -- GEO look angles from a fixed lat/lon have an exact analytic solution, so
  this is a stronger check than another numerical implementation would be
  (`crates/gnss-core/tests/sbas_waas.rs`).
- **DOP**: the same Python reference, but reaching `Q` by an SVD pseudo-inverse
  where the Rust accumulates the normal matrix and eliminates -- a different
  algorithm, not a transcription. Agreement is 4e-15 over 75 values, so the
  test tolerance is set to 1e-12 to stay a real gate. Plus a four-satellite
  tetrahedron solved by hand in `dop.rs` (Q = 2/3, 2/3, 4/3, 1/3).
- **IQ synthesis**: a *receiver*, in `crates/gnss-iq/tests/acquisition_cross_check.rs`.
  It acquires the generated file by FFT parallel code-phase search -- a
  frequency-domain circular correlation, structurally unlike the per-sample
  forward construction -- and asserts the recovered code phase matches the
  pseudorange the observables stage computed, the recovered Doppler matches the
  value derived analytically from satellite velocity (which the synthesiser
  never uses; it gets Doppler implicitly from the range rate), and the measured
  C/N0 matches what the noise model commanded. A PRN that is not in the
  recording must *not* acquire, which is the negative control.

  This is not decoration: it found a real bug. Code phase was being computed as
  `gps_seconds * chip_rate`, a product near 1.5e15 where one f64 ulp is a
  quarter of a chip -- 73 m of range error, in a quantity the whole file is
  built on. Whole seconds are now split off first (one second is exactly 1000
  C/A code periods, so they carry no phase). Residuals went from ~50 m to under
  11 m.
- **C/A codes**: the first ten chips of all 32 PRNs against IS-GPS-200 Table
  3-Ia, an external published table the generator had no part in producing,
  plus the Gold-code structural properties (512/511 balance, three-valued auto-
  and cross-correlation).
- **LNAV**: every word checked against an independently written receiver-side
  parity checker, and the packed ephemeris decoded back out and compared to the
  source within each field's own quantisation step.

Only GPS and WAAS are `status: "supported"` today. Galileo/QZSS/BeiDou/other
SBAS providers are wired up in the type system (`Constellation`,
`SbasProvider`) but marked `unvalidated` and are not user-toggleable until
they get the same treatment.

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

### Ephemeris cache correctness (`web/src/app/api/ephemeris/route.ts`)

Two rules that exist because of real bugs hit during development:
- `EPHEMERIS_FORMAT_VERSION` is embedded in the cache key/request URL. Bump
  it whenever server-side trimming/parsing logic changes, or the browser's
  `immutable`-cached response for an old format silently never refreshes.
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
vectors, with **one clock column**. That is correct only while every satellite
shares a time reference — true for GPS + SBAS (SBAS is GPS-time coherent by
design), and false the moment Galileo or BeiDou are switched on. Each added
system needs its own inter-system-bias column, which also raises the minimum
satellite count by one. Make that change in `design_matrix`, not at call sites.

Two rules the UI must not break:
- Fewer than four satellites, or degenerate geometry, means **no solution** —
  `None`, never zero. Zero on a precision chart reads as *perfect* precision at
  exactly the moments there is no fix. `dopChart.ts` breaks the line instead.
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

- Spatial: CONUS only (`isWithinConus`); the map dims everything else and
  click is gated on it.
- Temporal: `ARCHIVE_START = "2017-06-01"` (GNSS), `SBAS_AVAILABLE_FROM =
  "2025-01-01"` (WAAS specifically -- EGNOS-only coverage exists further
  back but isn't wired up). Bounds were found empirically by probing the BKG
  archive, not documented anywhere upstream.
- Data source: BKG's open IGS mirror (`BRDC00WRD_*_MN.rnx.gz`), not CDDIS --
  CDDIS requires an Earthdata login and this project handles no credentials
  anywhere.

## Build & test

```bash
cargo test --workspace --release   # 162 tests. --release matters: the acquisition
                                   # cross-check spends minutes in the FFT otherwise
cargo build --release -p gnss-iq   # the IQ worker; the job API refuses to accept
                                   # work until this exists
cd web && npm run build:wasm       # rebuild crates/gnss-wasm -> web/src/wasm/
cd web && npm test                 # pure-logic tests (node --test), 84 tests
cd web && npm run typecheck        # tsc --noEmit
cd web && npm run dev              # dev server, localhost:3000
```

Node test files import with an explicit `.ts` extension (`./types.ts`) -- Node's
ESM resolver does not add it, even though TypeScript's does.

## Roadmap

1. **Phase 1 -- done.** Static sky plot for any CONUS point/time, GPS +
   WAAS, client-side WASM propagation, collision-avoiding labels.
2. **Phase 2 -- done.** Time-window selection (24 h cap), satellite tracks
   with rise/set gaps, HDOP/VDOP against time, one slider scrubbing both
   plots, multi-day ephemeris merging across UTC midnight.
3. **Phase 3.** Terrain masking (local horizon obstruction, not just the
   elevation-angle mask). The IQ pipeline's masking is deliberately minimal --
   a hard on/off at the mask angle with no taper -- and is where terrain
   masking will plug in.
4. **Phase 4 -- done (taken before phase 3).** Job-based GPS L1 C/A IQ
   generation: submit, queue, synthesise, download binary + sidecar.
5. **Phase 5.** Broader deployment / additional constellations validated and
   switched on (Galileo, BeiDou, QZSS, additional SBAS providers), automated
   CDDIS-or-equivalent ephemeris ingestion.

## IQ generation (phase 4)

Job lifecycle: `POST /api/jobs` -> `queued` -> `processing` -> `complete`, with
`GET /api/jobs/{id}` for polling and
`GET /api/jobs/{id}/files/{binary,sidecar}` for retrieval. The queue is
in-process, one job at a time, spawning `gnss-iq-worker` per job. Compute now
genuinely runs server-side -- see ADR-0007, which amends ADR-0002's
client-side-only model. A one-minute recording at 4 MHz int16 is 960 MB; that
was never going to be a browser tab.

### The noise interfaces are the load-bearing part

`PseudorangeNoiseModel` and `Cn0Model` (`crates/gnss-iq/src/noise.rs`) are
injected as trait objects. No pipeline stage names a concrete model, and
`build_models` is the single place a spec tag becomes a type -- adding a model
is one new impl plus one match arm.

Both traits require `describe() -> ModelDescription`, and that requirement is
what makes the injection real rather than decorative. The sidecar has to record
exactly which models ran with which parameters; if the sidecar writer had to
match on concrete types to learn them, adding a model would mean editing the
sidecar writer too, and the pipeline would still need to know every
implementation.

The pseudorange sigma is a job parameter. The C/N0 coefficients are *not* --
they are constants of `ElevationCn0Model`, and the API rejects a request that
tries to set them rather than silently ignoring it, because ignoring it leaves
the user believing a value took effect.

### Code noise is not carrier noise

Each satellite carries two ranges. `pseudorange_m` includes the noise draw and
drives the **spreading code**; `carrier_range_m` excludes it and drives the
**carrier phase**. That is physical -- pseudorange error is code measurement
error, and carrier-phase measurements on the same signal are two orders of
magnitude quieter, which is the whole basis of carrier smoothing and RTK.
Driving the carrier from the noisy range would turn an ordinary 1.5 m sigma at
0.1 s epochs into roughly +/-150 Hz of frequency jitter, which no real receiver
could track.

### Delay drives everything else

There is no separate Doppler oscillator. Code phase and carrier phase are both
derived from the interpolated range, so differentiating the carrier phase
*yields* the Doppler without it being computed, and the code rate cannot
disagree with it. Code-carrier divergence is the classic bug when a synthesiser
runs independent oscillators and has to keep them in step by hand.

The reported `doppler_hz` in the observables is therefore a diagnostic, derived
independently from the satellite velocity vector and clock drift -- and a test
asserts the two agree.

### Precision is a correctness issue here, not a nicety

GPS seconds are ~1.4e9. Multiplying by a 1.023e6 chip rate lands at ~1.5e15,
where one f64 ulp is a quarter of a chip. Any quantity that ends up modulo a
code period must be reduced **before** the multiplication, not after --
reducing afterwards cannot recover what is already gone. `synth.rs` splits off
whole seconds, which is exact because one second is exactly 1000 C/A code
periods.

### Reproducibility

Every sidecar records a seed, including for jobs that did not supply one. The
PRNG is hand-written (`rng.rs`, PCG-XSH-RR, checked against the reference
implementation's published vector) rather than taken from `rand`, whose
`StdRng` may change algorithm in a minor release -- which would silently
invalidate every stored seed.

### Ephemeris fetching stays in TypeScript

`gnss-iq` speaks no HTTP. The API populates `data/cache/` and points the worker
at it. The reason is the partial-file rule in `ephemerisCache.ts` (a file
fetched mid-day is incomplete and must be refetched until its mtime passes the
end of its day) -- a subtle rule found by hitting the bug, and not one worth
having two copies of. The day-range calculation *is* duplicated, deliberately:
TypeScript decides what to fetch, Rust decides what to read, and a disagreement
costs at most a missing neighbouring day, which the worker tolerates.

### Adding a signal

`Signal::resolve(constellation, band)` is the chokepoint. A new signal is a
variant plus a spreading-code implementation; the synthesis loop asks the
`Signal` for its chip rate, code length, carrier and data rate and never names
GPS L1. Bands with no implementation deliberately still *parse* -- so adding
GPS L5 is a resolution arm, not a schema change -- and fail with a message
naming what was asked for, never falling back to L1.

Explicitly out of scope until their phase: GLONASS (different element set,
not planned), automated multi-source ephemeris fetching beyond the current
single BKG mirror, deployment/hosting.

## Design goals

- **Correctness over speed**, especially in `gnss-core`: two independent
  numerical cross-checks exist and a source isn't user-facing until it has
  one appropriate to its geometry.
- **No rewrites for new constellations**: the `Constellation` enum, the
  `BroadcastEphemeris` Keplerian/Sbas split, and `coverage.ts`'s `SOURCES[]`
  table are the three places a new constellation touches; propagation
  dispatch and UI toggling both key off them already.
- **No credentials anywhere in the project** (informed the BKG-over-CDDIS
  choice; keep this constraint when adding data sources).
- **Fail loud, not silently wrong**: parser panics are caught and surfaced
  as errors, not swallowed; ambiguous units/health data are resolved by
  physical reasoning and documented rather than assumed.
