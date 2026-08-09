# ADR-0002: WASM client-side execution for phases 1-3

Date: 2026-08-09
Status: Accepted

## Context
The project needs to decide where compute happens: a server-side Rust API
that the frontend calls over HTTP, or Rust compiled to WebAssembly and run
directly in the browser. This decision affects UX (latency, interactivity)
and infrastructure (whether a backend server is needed at all in early
phases).

Phases 1-2 (sky plot, DOP) are lightweight per-satellite trigonometry —
microseconds of work even without native compilation. Phases 4-5 (IQ file
generation) involve sample-rate signal synthesis, potentially large output
files, and benefit from SIMD/multithreading — workloads much better suited
to a server process than a browser tab.

## Decision
Compile the Rust core to WebAssembly (via `wasm-bindgen`/`wasm-pack`) and run
it client-side for phases 1-3 (static sky plot, time-series sky plot, terrain
masking). Defer introducing a server-side Rust REST/JSON API until phase 4
(IQ generation), where compute cost and output file size justify it.

## Alternatives considered
- Server-side API from day one: simpler mental model (one execution path for
  the whole project) but adds infrastructure and network round-trip latency
  to what should be a highly interactive, low-latency sky plot UI (e.g.
  dragging a time slider).
- WASM for the entire project including IQ generation: rejected — WASM is
  single-threaded by default (true multithreading needs Web Workers +
  SharedArrayBuffer with special HTTP headers), and generating/downloading
  large IQ files is a better fit for an async server-side job queue than
  in-browser computation.

## Consequences
- Phase 1-3 ships as a static frontend with no backend server required,
  simplifying early deployment.
- Interactive sky plot features (time scrubbing, live updates) get
  zero-network-latency responsiveness.
- Introduces WASM-specific complexity: async module instantiation (client
  components only in Next.js), harder-to-read panic/stack traces (mitigated
  with `console_error_panic_hook`), and a data-marshalling boundary between
  JS and WASM memory for anything beyond simple numeric returns.
- Phase 4 will require a second execution path (server API) to be designed
  and integrated, effectively splitting the project's compute across two
  runtimes — an intentional, deferred cost accepted for the sake of
  early-phase simplicity and responsiveness.