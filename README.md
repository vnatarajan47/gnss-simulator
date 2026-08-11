# gnss-simulator

A GNSS signal simulator, built up in phases. Phase 1 is the geometry layer:
click a point in the continental US, get a live sky plot of every visible GPS
and WAAS satellite — azimuth and elevation computed from RINEX broadcast
ephemeris, entirely in the browser via WebAssembly.

The satellite geometry is validated to ~10⁻¹³ degrees against an independent
reference implementation. Accuracy is the point; speed is not, yet.

Phase 4 adds the signal layer: submit a job and get back a synthetic baseband
**IQ recording** — real C/A codes, real navigation frames, Doppler, atmospheric
delay and thermal noise — as a raw interleaved binary plus a JSON sidecar.

```
gnss-simulator/
├── crates/
│   ├── gnss-core/     Rust: RINEX Nav parsing, propagation, clock, atmosphere, az/el
│   ├── gnss-iq/       Rust: IQ synthesis and the gnss-iq-worker binary
│   └── gnss-wasm/     wasm-bindgen wrapper around gnss-core
├── web/               Next.js frontend + job API (App Router, TypeScript)
├── data/              cached RINEX Nav files, job records and outputs
├── tools/             independent Python reference implementation
└── docs/adr/          architecture decision records
```

## Quick start

Prerequisites: a Rust toolchain, [`wasm-pack`](https://rustwasm.github.io/wasm-pack/),
and Node 20+.

```bash
cargo build --release -p gnss-iq
cd web && npm install && npm run build:wasm && npm run dev
```

Then open <http://localhost:3000>. `build:wasm` compiles `gnss-wasm` into
`web/src/wasm/`. Ephemeris is fetched on demand and needs network access on
first use of a given day. The IQ worker must be built before the job API will
accept work — it says so explicitly if it is missing.

Run the Rust test suite, including the cross-validation:

```bash
cargo test --workspace --release
```

`--release` is not optional in practice: the acquisition cross-check spends
minutes in the FFT otherwise.

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
4. SBAS satellites take the other path: geostationary, so their broadcast state
   vector is advanced to the epoch by Taylor expansion rather than solved as an
   orbit.
5. ECEF is transformed to topocentric ENU about the receiver and reduced to
   azimuth, elevation and range. Anything below the elevation mask (5° by
   default) is dropped.
6. The frontend renders the survivors on a polar plot: azimuth as angle,
   elevation as radius. Geostationary satellites get a square marker.

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

SBAS gets a stronger check, in `crates/gnss-core/tests/sbas_waas.rs`: a
geostationary satellite's look angles have a *closed form*, so the state-vector
propagation is verified against analytic spherical geometry rather than against
another program that could share a misconception.

```bash
python3 tools/reference_skyplot.py --report          # human-readable sky view
python3 tools/reference_skyplot.py --emit-vectors \
    > crates/gnss-core/tests/vectors/reference_vectors.json   # regenerate
```

## IQ generation

A job is submitted, queued, run in a subprocess, and retrieved by polling.

```bash
curl -X POST http://localhost:3000/api/jobs -H 'Content-Type: application/json' -d '{
  "receiver": {"latitude_deg": 39.7392, "longitude_deg": -104.9903, "altitude_m": 1609},
  "window":   {"start_unix_s": 1751198400, "duration_s": 2, "epoch_interval_s": 0.1},
  "output":   {"sample_rate_hz": 2600000, "quantization": "int8"},
  "noise":    {"pseudorange": {"model": "fixed", "sigma_m": 1.5}},
  "seed": 42
}'
```

Only `receiver` and `window` are required; everything else has a documented
default. Then `GET /api/jobs/{id}` until `status` is `complete`, and fetch
`/api/jobs/{id}/files/binary` and `/files/sidecar`.

The binary is interleaved `I0 Q0 I1 Q1 …`, little-endian, in the requested
format. The sidecar carries everything needed to read it *and* to reproduce it:
sample rate, centre frequency, quantisation, byte order, the receiver and
window, the elevation mask, the exact noise models with their parameters as the
models themselves report them, the seed actually used, the provenance of the
ionospheric coefficients, and a per-satellite summary.

The pipeline runs server-side, which is a departure from phases 1–3 — a
one-minute recording at 4 MHz in int16 is 960 MB, which was never going to be a
browser tab. See [ADR-0007](./docs/adr/0007-server-side-iq-worker.md).

The worker can also be driven directly, without Node:

```bash
target/release/gnss-iq-worker --job spec.json --id demo --out ./out \
    --nav data/BRDC00WRD_R_20250010000_01D_GN.rnx
```

### What is and is not modelled

GPS L1 C/A only. Real 1023-chip Gold codes, real LNAV subframes 1–3 packed with
the broadcast clock and ephemeris parameters and correct parity — so the output
is not merely acquirable but decodable. Subframes 4 and 5 are structurally valid
and carry no almanac.

Satellite clock correction (polynomial, relativistic and L1 group delay),
Klobuchar ionosphere, Saastamoinen troposphere, Doppler from the analytic
satellite velocity, and thermal noise at the commanded C/N0. Elevation masking
is a hard on/off with no taper, deliberately — that is where phase 3's terrain
masking will go.

Not modelled: multipath, multi-frequency or multi-constellation output, receiver
trajectories, front-end filtering (the signal is point-sampled, not
band-limited, which costs about a decibel of correlation).

### Validating the signal

`crates/gnss-iq/tests/acquisition_cross_check.rs` acquires the generated file
the way a receiver would — an FFT parallel code-phase search, structurally
unlike the per-sample forward construction — and checks that the recovered code
phase matches the pseudorange, the recovered Doppler matches the value derived
independently from satellite velocity, and the measured C/N0 matches what the
noise model commanded. A PRN absent from the recording must not acquire.

It earned its keep immediately: code phase was being computed as
`gps_seconds * chip_rate`, a product near 1.5e15 where one f64 ulp is a quarter
of a chip — 73 m of range error in the quantity the entire file is built on.
Residuals dropped from ~50 m to under 11 m once whole seconds were split off
before the multiplication.

## Roadmap

| Phase | Goal | Status |
| ----- | ---- | ------ |
| 1 | **Static sky plot** — click a lat/lon, see az/el of visible GPS satellites at one timestamp, computed client-side. | Done |
| 2 | **Time-series sky plot** — animate satellite tracks over a time window; visibility and DOP as functions of time. | Done |
| 3 | **Terrain masking** — replace the flat elevation mask with a real horizon profile from a DEM, so ridgelines occlude satellites. | Next |
| 4 | **Static IQ generation** — synthesise baseband IQ for a stationary receiver: C/A code, carrier Doppler, per-satellite delay and power. | Done |
| 5 | **Dynamic target support** — receiver trajectories, with Doppler and delay evolving along the path. | |

Phase 4 was taken before phase 3. Still out of scope: multi-constellation and
multi-frequency IQ, receiver trajectories, multipath, automated CDDIS fetching,
GLONASS, and deployment.

## Ephemeris data

Any UTC day from **2017-06-01 to today** works; pick an epoch and the app
fetches what it needs. Source is BKG's IGS mirror of the merged broadcast
product (`BRDC00WRD_R_*_01D_MN.rnx.gz`), not CDDIS as ADR-0004 names — CDDIS
distributes the same IGS product but requires an Earthdata login, and BKG does
not. Files land in `data/cache/` (gitignored), so a repeated day is served
locally in tens of milliseconds instead of ~5 s.

Three edges worth knowing:

- **Today's file is partial.** Broadcast files accumulate through the day, so
  an epoch later than the current hour has no ephemeris. The UI says so. A
  partial file is never written to the cache as if complete: a cached copy is
  only trusted when it was fetched *after* its day ended.
- **SBAS coverage is much shallower than GNSS coverage.** Files before ~2021
  carry no SBAS at all; from 2021 to late 2024 only EGNOS is present. WAAS
  appears from early 2025. The UI warns when the epoch predates it.
- `data/BRDC00WRD_R_20250010000_01D_GN.rnx` is still committed, but only as the
  fixture for `cargo test`. The app no longer reads it, so a cold cache with no
  network means no sky plot.

`gnss-core` accepts gzipped RINEX directly, so files can be cached as
downloaded.

## Constellation support

Sources are toggled individually in the UI. Only validated ones can be switched
on; the rest are shown greyed with the reason on hover.

| Source | State | Why |
| --- | --- | --- |
| **GPS** | on | Validated against an independent Python implementation. |
| **WAAS** | on | Validated against closed-form geostationary geometry. |
| EGNOS, MSAS | off | Propagated by the same SBAS code path, but unvalidated. |
| Galileo, QZSS | off | Same Keplerian set as GPS; unvalidated. |
| BeiDou | off | MEO/IGSO would work; its GEO satellites need a separate rotation. |
| GLONASS | n/a | State vector needing numerical integration — a different propagator. |

Two propagators exist. GPS/Galileo/BeiDou/QZSS use the Keplerian algorithm;
SBAS satellites are geostationary and broadcast an ECEF **state vector**
instead, propagated by second-order Taylor expansion (RINEX 3.05 §6.10). The
split lives in `BroadcastEphemeris`, so adding a constellation means adding
constants, not restructuring.

SBAS is one RINEX constellation but many independent regional systems, so it is
listed per operator — a receiver in CONUS has no use for a satellite parked over
the Indian Ocean. Adding a provider is one entry in `web/src/lib/coverage.ts`
plus one line in `SbasProvider::from_prn`.

### Two things the broadcast data gets wrong

Worth knowing before trusting SBAS output from any source:

- **Units are inconsistent.** RINEX specifies kilometres for the GEO state
  vector, but the merged IGS product mixes them — roughly half the records for
  PRNs 121, 123, 127, 128, 136 and 144 are in metres. `sbas::decode_scale`
  decides physically instead of trusting the file: exactly one interpretation
  puts the satellite at a plausible geostationary radius. The same check
  rejects the zero-filled placeholder records these files also contain.
- **The health word is not populated.** It is dominated by all-ones fillers
  (31, 63) even for operational satellites — every WAAS record carries 31.
  Applying the GPS `health == 0` rule would silently discard all of SBAS, so
  health is surfaced but not used for filtering.

## Architecture decisions

See [`docs/adr/`](./docs/adr/). ADRs 0001–0004 cover the language, execution
model, frontend framework and ephemeris source.
[ADR-0005](./docs/adr/0005-hand-rolled-ephemeris-propagation.md) explains why
the `rinex` crate is used only as a parser;
[ADR-0006](./docs/adr/0006-server-side-ephemeris-proxy.md) amends ADR-0002 to
cover the on-demand fetch route, and
[ADR-0007](./docs/adr/0007-server-side-iq-worker.md) amends it again for the IQ
worker — the point at which the project genuinely acquired a compute backend.
