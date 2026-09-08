# gnss-simulator

A GNSS signal simulator, built up in phases. The geometry layer is done: click
any point on Earth, pick a time window, and get a sky plot of every visible
satellite with its track across the window, plus HDOP and VDOP against time —
azimuth and elevation computed from RINEX broadcast ephemeris, entirely in the
browser via WebAssembly.

GPS, Galileo, BeiDou and QZSS are supported, along with six satellite-based
augmentation systems (WAAS, EGNOS, MSAS, GAGAN, BDSBAS and SouthPAN).

Every source is cross-checked against an independent implementation before it
becomes selectable — the Keplerian constellations agree to ~10⁻⁹ degrees, and
the geostationary ones to metres against a second broadcast of the same
satellite. Accuracy is the point; speed is not, yet.

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

**[docs/methods.md](./docs/methods.md)** specifies the mathematics, the data
sources, and the validation in full — every algorithm, constant and known data
defect, with enough detail to reimplement the tool or to decide whether its
output can be trusted for a given purpose.

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
   caches it on disk, trims it to the active constellations *and* to the
   requested time window, and returns it. The browser hands those bytes to
   WASM. A six-hour window over GPS + Galileo is ~1.8 MB, where the whole day
   would be several times that.
2. `gnss-core` parses it into per-satellite broadcast ephemeris blocks and, for
   the requested instant, selects the block whose time-of-ephemeris is nearest
   (GPS fits a 4-hour arc centred on ToE, so a daily file holds ~12 per SV).
3. Each satellite is propagated to ECEF with the Keplerian algorithm of
   IS-GPS-200 §20.3.3.4.3 — mean anomaly, Kepler's equation, true anomaly,
   argument of latitude with second-harmonic corrections, then rotation into
   the Earth-fixed frame. Signal transit time and the Sagnac term are applied.
4. SBAS satellites take the other path: geostationary, so their broadcast state
   vector is advanced to the epoch by Taylor expansion rather than solved as an
   orbit. BeiDou's geostationary satellites stay on the Keplerian path but need
   the ICD's separate transformation into Earth-fixed coordinates
   (BDS-SIS-ICD-B1I §5.2.4.12) — a 5-degree tilt and an explicit Earth
   rotation, rather than folding the rotation into the node.
5. ECEF is transformed to topocentric ENU about the receiver and reduced to
   azimuth, elevation and range. Anything below the elevation mask (5° by
   default) is dropped.
6. The frontend renders the survivors on a polar plot: azimuth as angle,
   elevation as radius. Geostationary satellites get a square marker — which
   is decided from the broadcast orbital elements, not from the satellite
   identifier, because BeiDou and QZSS fly geostationary satellites too.
7. Dilution of precision is `(AᵀA)⁻¹` over the unit line-of-sight vectors, with
   **one clock column per time system**. GPS, QZSS and SBAS share a clock;
   Galileo and BeiDou each get their own. Every extra system costs a satellite
   before a solution exists at all — see
   [ADR-0007](./docs/adr/0007-inter-system-clock-unknowns.md).

Unhealthy satellites are excluded by default. On the bundled 2025-01-01 file
that means GPS G01 and G22, which broadcast health word 63 all day; on a 2026
file it also excludes Galileo's E14 and E18, the pair stranded in eccentric
orbits and flagged unusable for the Open Service every day of the archive.

QZSS is the exception. Its health word is a per-signal bitfield whose bits stay
set for signals a given satellite does not carry, so **no** QZSS record in this
product ever reads zero — applying the GPS rule would discard the constellation
outright. Only the all-ones word is treated as unusable.

Only the *fetch* is server-side, and only because the archive sends no CORS
header. All the arithmetic still runs in the browser — see
[ADR-0006](./docs/adr/0006-server-side-ephemeris-proxy.md).

### Validating the math

No source is switchable in the UI until it has an independent cross-check
appropriate to its geometry. There are three, in increasing order of how much
they prove.

**A second implementation.** `tools/reference_skyplot.py` implements the same
algorithm sharing no code with the Rust: fixed-column RINEX parsing instead of
the `rinex` crate, fixed-point Kepler iteration instead of Newton-Raphson,
`asin` for elevation instead of `atan2`, an explicit rotation matrix for ENU,
and — for DOP — an explicit design matrix through an SVD pseudo-inverse where
the Rust accumulates the normal matrix and eliminates.

`cargo test` compares the two twice over: 5 CONUS sites × 3 epochs on GPS, and
779 satellites over 36 cases spanning six globally spread observers (Denver,
London, Tokyo, Sydney, Nairobi, McMurdo) on GPS + Galileo + BeiDou + QZSS. The
multi-constellation cases agree to 1.1×10⁻⁹ deg in azimuth, 4.0×10⁻¹⁰ deg in
elevation and 6×10⁻⁵ m in range, and their DOP to 5.6×10⁻¹² across one, two and
three clock unknowns.

**Closed-form geometry.** A geostationary satellite's look angles have an exact
analytic solution, so every SBAS satellite of every provider is checked against
spherical trigonometry rather than against another program that could share a
misconception (`crates/gnss-core/tests/sbas_geometry.rs`).

**A second broadcast of the same satellite.** BDSBAS is carried on BeiDou's
geostationary satellites and MSAS on QZSS's, so those spacecraft appear in the
file twice — once as Keplerian elements, once as an ECEF state vector, fitted
by different ground segments and propagated by two algorithms sharing no code.
They agree to 0.4–2.0 m, which is the strongest evidence available that
BeiDou's geostationary frame transformation is right. See
[ADR-0008](./docs/adr/0008-validation-by-dual-broadcast-satellites.md); one
pair, BeiDou C02 and BDSBAS PRN 144, sits a constant 2.8 km apart because they
are *co-located* satellites sharing an orbital slot rather than one spacecraft.

```bash
python3 tools/reference_skyplot.py --report                # human-readable sky view
python3 tools/reference_skyplot.py --emit-vectors \
    > crates/gnss-core/tests/vectors/reference_vectors.json    # GPS, regenerate
python3 tools/reference_skyplot.py --emit-multi-vectors \
    > crates/gnss-core/tests/vectors/multignss_vectors.json    # multi-GNSS
```

Regenerating needs numpy, for the SVD. Running the tests does not — the vectors
are checked in.

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
locally in tens of milliseconds instead of ~5 s. Responses are also trimmed to
the requested time window, so a six-hour window costs a fraction of a whole day.

Three edges worth knowing:

- **Today's file is partial.** Broadcast files accumulate through the day, so
  an epoch later than the current hour has no ephemeris. The UI says so. A
  partial file is never written to the cache as if complete: a cached copy is
  only trusted when it was fetched *after* its day ended.
- **Augmentation coverage is much shallower than GNSS coverage, and uneven.**
  Files before ~2021 carry no SBAS at all. From 2021 only EGNOS is reliably
  present; sampled days in 2022 add GAGAN, SDCM and BDSBAS, but a sampled day
  in mid-2023 is back to EGNOS alone. WAAS, MSAS, KASS and SouthPAN appear
  together from early 2025, and from there the set is stable. Availability is
  therefore held per source rather than as one date for "SBAS", and the UI
  names the specific sources a chosen window predates.
- `data/` holds three committed fixtures, used only by `cargo test`: a GPS day
  from 2025-01-01, four hours of GPS/Galileo/BeiDou/QZSS from 2026-08-08, and
  two hours of all-provider SBAS from the same day. The app does not read them,
  so a cold cache with no network means no sky plot.

`gnss-core` accepts gzipped RINEX directly, so files can be cached as
downloaded.

## Coverage

**Spatial: global.** Any point on the WGS-84 ellipsoid. Click the map, or type
coordinates — the fields reach the latitudes Web Mercator will not draw.

**Temporal:** any UTC day from 2017-06-01 to today, in windows of up to 24 h.

## Constellation support

Sources are toggled individually in the UI, in two groups. Core constellations
are global; augmentation systems are regional, and each toggle names the area
its satellites are usable from — enabling the wrong one adds nothing, and an
empty plot is a poor way to learn that. Only validated sources can be switched
on; the rest are shown greyed with the reason on hover.

| Source | State | Why |
| --- | --- | --- |
| **GPS** | on | Validated against an independent Python implementation. |
| **Galileo** | on | Same Keplerian set with GTRF constants; validated against the same reference. |
| **BeiDou** | on | Including its geostationary satellites, which need the ICD's separate frame transformation. |
| **QZSS** | on | Regional (Asia-Pacific); validated against the same reference. |
| **WAAS, EGNOS, MSAS, GAGAN, BDSBAS, SouthPAN** | on | Each validated against closed-form geostationary geometry. |
| **Other SBAS** | on | PRNs in the SBAS band with no published operator (142, 148 turn up in the archive). The geometry is validated; only the label is uncertain. |
| SDCM | off | Absent from every archive day sampled since 2022 — nothing to validate against. |
| KASS | off | Present only as a saturated daily placeholder, which the parser rejects as physically impossible. |
| GLONASS | n/a | State vector needing numerical integration — a different propagator. |

Two propagators exist. GPS/Galileo/BeiDou/QZSS use the Keplerian algorithm;
SBAS satellites are geostationary and broadcast an ECEF **state vector**
instead, propagated by second-order Taylor expansion (RINEX 3.05 §6.10). The
split lives in `BroadcastEphemeris`, so adding a constellation means adding
constants, not restructuring.

A third case sits inside the Keplerian path rather than beside it: BeiDou's
geostationary satellites, whose elements are referred to a plane tilted 5° out
of the equator and reach Earth-fixed coordinates through an explicit rotation
(BDS-SIS-ICD-B1I §5.2.4.12). Whether a satellite needs that is read off its
broadcast inclination and semi-major axis, not a PRN table, so it does not go
stale as satellites are launched and retired.

SBAS is one RINEX constellation but many independent regional systems, so it is
listed per operator — a receiver in Europe has no use for a satellite parked
over the Pacific. Adding a provider is one entry in `web/src/lib/coverage.ts`
plus one line in `SbasProvider::from_prn`.

### Four things the broadcast data gets wrong

Worth knowing before trusting output from any source, not only this one:

- **Units are inconsistent.** RINEX specifies kilometres for the GEO state
  vector, but the merged IGS product mixes them — roughly half the records for
  PRNs 121, 123, 127, 128, 136 and 144 are in metres. `sbas::decode_scale`
  decides physically instead of trusting the file: exactly one interpretation
  puts the satellite at a plausible geostationary radius. The same check
  rejects the zero-filled placeholder records these files also contain.
- **The SBAS health word is not populated.** It is dominated by all-ones
  fillers (31, 63) even for operational satellites — every WAAS record carries
  31. Applying the GPS `health == 0` rule would silently discard all of SBAS,
  so health is surfaced but not used for filtering.
- **The QZSS health word never reads zero.** It is a per-signal bitfield whose
  bits stay set for signals a given satellite does not carry, so J02/J03/J07
  sit permanently at 1 and J04/J08 at 16. Same trap as SBAS, milder: only the
  all-ones word is treated as unusable, since no per-signal combination means
  "every signal is simultaneously fine". Decoding the bits individually
  (IS-QZSS-PNT §4.1.2.3) is the proper fix and is deferred.
- **Some satellites carry a daily placeholder instead of a state vector.** KASS
  and PRN 142 appear in the file with `z = 32767` — a saturated 16-bit field.
  Neither the kilometre nor the metre reading of that is a geostationary
  radius, so the same physical check that resolves the units rejects it,
  without needing to know about the sentinel.

## Architecture decisions

The full methods reference is [`docs/methods.md`](./docs/methods.md).

See [`docs/adr/`](./docs/adr/). ADRs 0001–0004 cover the language, execution
model, frontend framework and ephemeris source.
[ADR-0005](./docs/adr/0005-hand-rolled-ephemeris-propagation.md) explains why
the `rinex` crate is used only as a parser;
[ADR-0006](./docs/adr/0006-server-side-ephemeris-proxy.md) amends ADR-0002 to
cover the on-demand fetch route.
[ADR-0007](./docs/adr/0007-inter-system-clock-unknowns.md) explains why DOP
carries one clock unknown per time system, and
[ADR-0008](./docs/adr/0008-validation-by-dual-broadcast-satellites.md) why
BeiDou's geostationary transformation is validated against a second broadcast
of the same spacecraft rather than against a second reading of the ICD.

### A note on the `rinex` crate

It is used strictly as a parser, and three of its behaviours are worked around
locally rather than reported upstream yet: an uncapped Newton-Raphson loop for
eccentric anomaly, a panic on zero-length lines in malformed input, and — found
during this phase — orbit fields whose broadcast value is exactly zero not
being surfaced at all. The last one is the nastiest, because it is silent and
selective: QZSS's two geostationary satellites broadcast `deltaN` and `idot` as
exact zeros, and every SBAS satellite sitting precisely on the equator
broadcasts `satPosZ` as one. Requiring those fields present dropped both
groups — which was all of EGNOS and SouthPAN — without an error anywhere.
