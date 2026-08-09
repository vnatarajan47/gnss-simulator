# ADR-0001: Rust backend over C++

Date: 2026-08-09
Status: Accepted

## Context
The GNSS simulator's compute-heavy work (ephemeris propagation, terrain masking,
IQ signal synthesis) needs a fast, compiled language. The initial instinct was
C++, the traditional choice for this domain (most commercial GNSS simulators
and SDR tooling are C++).

## Decision
Use Rust for all compute-heavy backend/core logic.

## Alternatives considered
- C++: traditional choice, mature SDR/DSP ecosystem, but weaker memory safety
  guarantees and a less ergonomic package/build story.
- Python: rejected early — too slow for real-time IQ synthesis and sample-rate
  signal generation without significant native-extension work, which would
  reintroduce the C++ problem anyway.

The Rust GNSS ecosystem turned out to be more mature than expected: an active
`rinex` crate (RINEX V2-V4 parsing, including navigation files, built on
`nalgebra`/`anise`), and an existing open-source multi-constellation SDR IQ
generator (GPS/Galileo/BeiDou/GLONASS, RINEX 3.04 parsing, AVX-512/CUDA/Rayon
acceleration) to use as a design reference. This tipped the decision — Rust
gets memory safety without sacrificing speed, and without requiring the team
to build core GNSS primitives from scratch.

## Consequences
- Gains: memory safety, comparable performance to C++, ability to compile the
  same codebase to WASM for client-side execution (see ADR-0002), and reuse of
  existing crates for RINEX parsing rather than writing a parser from scratch.
- Costs: smaller overall talent pool / Stack Overflow corpus than C++ for GNSS-
  specific work; some existing GNSS reference material and algorithms are
  described in C++ or MATLAB and will need translation.
- Establishes Rust as the single language for all backend/core compute across
  every phase, avoiding a future C++/Rust split.