# ADR-0005: Use `rinex` as a parser only, and implement ephemeris propagation ourselves

- **Date:** 2026-08-09
- **Status:** Accepted
- **Deciders:** Claude (implementation audit), pending review

> Written during phase 1 implementation, not back-filled. It records a decision
> made in response to an audit finding rather than one taken up front, so the
> reasoning below is the actual reasoning — please correct it if you disagree
> with the conclusion.

## Context

[ADR-0004](./0004-rinex-nav-over-celestrak-tles.md) commits us to RINEX
broadcast ephemeris, and [ADR-0002](./0002-wasm-client-side-execution.md)
commits us to running the core in the browser. The [`rinex`][rinex] crate
(v0.22) is the obvious way to read those files, and it advertises a `nav`
feature that already implements orbit propagation — `Ephemeris::kepler2position`
and a `Helper` type carrying the full Keplerian solution.

The phase-1 brief asked whether that implementation is good enough to use
instead of writing our own. We audited it directly from the published crate
source.

Two findings:

**1. The math is correct.** `src/navigation/ephemeris/kepler/helper.rs`
implements IS-GPS-200 §20.3.3.4.3 faithfully. It expresses the final rotation
as `Rz(Ωk) · Rx(ik)` applied to the in-plane position vector rather than as the
ICD's three explicit component equations, but the two are algebraically
identical. It also derives ToE as an absolute `hifitime` epoch, which handles
week rollover more robustly than the ICD's ±302 400 s correction. We found one
genuine robustness defect — the Kepler fixed-point loop has no iteration cap;
`MAX_KEPLER_ITER` is compared only *after* the loop exits, so a malformed
eccentricity spins forever. In a browser that hangs the tab.

**2. The feature does not build for our target.** `nav = ["anise", "nalgebra"]`,
and `kepler2position` returns an `anise::prelude::Orbit`, so `anise` is not
avoidable by calling a narrower API. `anise` v0.9 lists `memmap2`, `ureq` and
`pyo3` as **non-optional** dependencies (plus `ureq` again as a *build*
dependency). None of those compile for `wasm32-unknown-unknown`. There is no
feature flag that removes them.

So the choice was not "our math versus their math". It was "their math, or the
browser".

## Decision

We will depend on `rinex` with `default-features = false` and **without** the
`nav` feature, using it purely to parse RINEX files into `Ephemeris` records,
and read the orbital elements out via `get_orbit_f64`. Propagation, coordinate
transforms and look angles are implemented in `crates/gnss-core`
(`propagate.rs`, `geodesy.rs`) directly against IS-GPS-200.

## Alternatives considered

### Use the `nav` feature and give up on client-side WASM

Rejected: it inverts ADR-0002 to save perhaps 150 lines of well-specified
arithmetic. The propagation algorithm is a fixed, published, testable
procedure — it is not where the project's risk lives.

### Patch `anise` (fork, or `[patch.crates-io]`) to make its deps optional

Rejected for phase 1. Making `memmap2`/`ureq`/`pyo3` optional in `anise` is
plausible upstream work, but it puts a fork of a large astrodynamics crate on
the critical path of a sky plot, and we would still be carrying `nalgebra` for
three rotations. Worth revisiting only if we later need `anise` for something
it is actually good at, such as high-fidelity frame transformations.

### Vendor just the `kepler` module out of `rinex`

Rejected: it is MPL-2.0 (file-level copyleft, so vendoring carries obligations),
it is coupled to `anise`'s `Vector3` and `nalgebra`'s `Rotation3`, and it
inherits the unbounded-iteration defect. Writing it fresh against the ICD was
cheaper than disentangling it.

## Consequences

- **Positive:** `gnss-core` builds for `wasm32-unknown-unknown` and has a
  dependency surface of `rinex` + `flate2` + `thiserror`. The propagation code
  is constellation-parameterised from the start, so Galileo and BeiDou need
  constants, not a rewrite. We control the Kepler solver: Newton-Raphson with a
  hard iteration cap, which cannot hang a browser tab.

- **Positive:** because we own the code, we could add the signal transit-time
  and Sagnac correction, which `rinex`'s `kepler2position` does not apply. Its
  effect on look angles is small (~5·10⁻⁴ deg) but it is the physically correct
  quantity and it matters more once phase 4 needs pseudoranges.

- **Negative:** we now own the correctness of the propagation math, and cannot
  inherit upstream fixes. This is mitigated by validation rather than by trust:
  `crates/gnss-core/tests/reference_cross_check.rs` checks 143 satellite
  geometries across 5 CONUS sites and 3 epochs against
  `tools/reference_skyplot.py`, an independent implementation that shares no
  code with the Rust (different RINEX parser, different Kepler solver,
  different elevation formulation). Current worst-case agreement is
  3·10⁻¹³ deg in azimuth and 1·10⁻⁸ m in range.

- **Negative:** we still pull `rinex`'s own dependency tree for parsing, which
  transitively includes `geo`/`geojson`/`spade` via `gnss-rs`'s non-optional
  `sbas` feature. That is most of the 439 kB WASM binary. If bundle size
  becomes a problem, replacing the parser with a ~200-line fixed-column reader
  is a contained piece of work — the RINEX 3 navigation record format is
  rigidly specified, and `tools/reference_skyplot.py` already contains a
  working one.

- **Neutral / follow-up:** report the uncapped Kepler iteration upstream to
  [nav-solutions/rinex][rinex]; it affects anyone using the `nav` feature.

[rinex]: https://github.com/nav-solutions/rinex
