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
`web/src/wasm/`, and `npm run dev` copies `data/*.rnx` into `web/public/data/`
before starting Next.

Run the Rust test suite, including the cross-validation:

```bash
cargo test
```

## How it works

1. `web/` fetches a RINEX Nav file and hands the bytes to WASM.
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

`data/` holds a GPS-only subset of BKG's `BRDC00WRD_R_20250010000_01D_MN.rnx`
broadcast product for 2025-01-01 (454 ephemeris records, all 32 PRNs). The UI
is pinned to that UTC day because that is what the file covers.

Fetching the right day on demand from [CDDIS](https://cddis.nasa.gov/) is a
fast-follow, not part of phase 1. Note that CDDIS requires an Earthdata login,
which is one of the things ADR-0004 has to account for.

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

See [`docs/adr/`](./docs/adr/). ADRs 0001–0004 are stubs awaiting their
rationale; [ADR-0005](./docs/adr/0005-hand-rolled-ephemeris-propagation.md) is
written up and explains why the `rinex` crate is used only as a parser.
