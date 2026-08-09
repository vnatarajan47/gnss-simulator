# gnss-simulator

A GNSS signal simulator, built up in phases. Phase 1 is the geometry layer:
click a point in the continental US, get a live sky plot of every visible GPS
satellite — azimuth and elevation computed from RINEX broadcast ephemeris,
entirely in the browser via WebAssembly.

The satellite geometry is validated to ~10⁻¹³ degrees against an independent
reference implementation. Accuracy is the point; speed is not, yet.

```
gnss-simulator/
├── crates/
│   ├── gnss-core/     Rust: RINEX Nav parsing, ephemeris propagation, az/el
│   └── gnss-wasm/     wasm-bindgen wrapper around gnss-core
├── web/               Next.js frontend (App Router, TypeScript)
├── data/              cached RINEX Nav files
├── tools/             independent Python reference implementation
└── docs/adr/          architecture decision records
```

## Quick start

Prerequisites: a Rust toolchain, [`wasm-pack`](https://rustwasm.github.io/wasm-pack/),
and Node 20+.

```bash
cd web && npm install && npm run build:wasm && npm run dev
```

Then open <http://localhost:3000>. `build:wasm` compiles `gnss-wasm` into
`web/src/wasm/`. Ephemeris is fetched on demand and needs network access on
first use of a given day.

Run the Rust test suite, including the cross-validation:

```bash
cargo test
```

## How it works

1. `/api/ephemeris?date=…` fetches that UTC day's broadcast file from BKG,
   caches it in `data/cache/`, trims it to the active constellations, and
   returns it. The browser hands those bytes to WASM.
2. `gnss-core` parses it into per-satellite broadcast ephemeris blocks and, for
   the requested instant, selects the block whose time-of-ephemeris is nearest
   (GPS fits a 4-hour arc centred on ToE, so a daily file holds ~12 per SV).
3. Each satellite is propagated to ECEF with the Keplerian algorithm of
   IS-GPS-200 §20.3.3.4.3 — mean anomaly, Kepler's equation, true anomaly,
   argument of latitude with second-harmonic corrections, then rotation into
   the Earth-fixed frame. Signal transit time and the Sagnac term are applied.
4. ECEF is transformed to topocentric ENU about the receiver and reduced to
   azimuth, elevation and range. Anything below the elevation mask (5° by
   default) is dropped.
5. The frontend renders the survivors on a polar plot: azimuth as angle,
   elevation as radius.

Unhealthy satellites are excluded by default. On the bundled 2025-01-01 file
that means G01 and G22, which broadcast health word 63 all day.

Only the *fetch* is server-side, and only because the archive sends no CORS
header. All the arithmetic still runs in the browser — see
[ADR-0006](./docs/adr/0006-server-side-ephemeris-proxy.md).

### Validating the math

`tools/reference_skyplot.py` is a second implementation of the same algorithm
that deliberately shares no code with the Rust: fixed-column RINEX parsing
instead of the `rinex` crate, fixed-point Kepler iteration instead of
Newton-Raphson, `asin` for elevation instead of `atan2`, an explicit rotation
matrix for ENU. `cargo test` compares the two across 5 CONUS sites × 3 epochs
× ~10 satellites.

```bash
python3 tools/reference_skyplot.py --report          # human-readable sky view
python3 tools/reference_skyplot.py --emit-vectors \
    > crates/gnss-core/tests/vectors/reference_vectors.json   # regenerate
```

## Roadmap

| Phase | Goal | Status |
| ----- | ---- | ------ |
| 1 | **Static sky plot** — click a lat/lon, see az/el of visible GPS satellites at one timestamp, computed client-side. | Done |
| 2 | **Time-series sky plot** — animate satellite tracks over a time window; visibility and DOP as functions of time. | Next |
| 3 | **Terrain masking** — replace the flat elevation mask with a real horizon profile from a DEM, so ridgelines occlude satellites. | |
| 4 | **Static IQ generation** — synthesise baseband IQ for a stationary receiver: C/A code, carrier Doppler, per-satellite delay and power. | |
| 5 | **Dynamic target support** — receiver trajectories, with Doppler and delay evolving along the path. | |

Phase 1 is deliberately narrow. Out of scope for it: animated plots, terrain,
IQ generation, automated CDDIS fetching, GLONASS, and deployment.

## Ephemeris data

Any UTC day from **2017-06-01 to today** works; pick an epoch and the app
fetches what it needs. Source is BKG's IGS mirror of the merged broadcast
product (`BRDC00WRD_R_*_01D_MN.rnx.gz`), not CDDIS as ADR-0004 names — CDDIS
distributes the same IGS product but requires an Earthdata login, and BKG does
not. Files land in `data/cache/` (gitignored), so a repeated day is served
locally in tens of milliseconds instead of ~5 s.

Two edges worth knowing:

- **Today's file is partial.** Broadcast files accumulate through the day, so
  an epoch later than the current hour has no ephemeris. The UI says so.
- `data/BRDC00WRD_R_20250010000_01D_GN.rnx` is still committed, but only as the
  fixture for `cargo test`. The app no longer reads it, so a cold cache with no
  network means no sky plot.

`gnss-core` accepts gzipped RINEX directly, so files can be cached as
downloaded.

## Constellation support

GPS today. Galileo, BeiDou and QZSS broadcast the same Keplerian parameter set,
and `gnss-core` is parameterised over constellation — gravitational constant,
Earth rotation rate, time-system offset — so enabling them is a matter of
adding to `SkyplotOptions::constellations` and validating, not restructuring.

GLONASS is genuinely different: it broadcasts a position/velocity state vector
requiring numerical integration rather than orbital elements. It would need its
own propagator and is out of scope.

## Architecture decisions

See [`docs/adr/`](./docs/adr/). ADRs 0001–0004 cover the language, execution
model, frontend framework and ephemeris source.
[ADR-0005](./docs/adr/0005-hand-rolled-ephemeris-propagation.md) explains why
the `rinex` crate is used only as a parser;
[ADR-0006](./docs/adr/0006-server-side-ephemeris-proxy.md) amends ADR-0002 to
cover the on-demand fetch route.
