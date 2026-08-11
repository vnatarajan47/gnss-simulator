# ADR-0007: A server-side worker for IQ generation

- **Date:** 2026-08-10
- **Status:** Accepted
- **Deciders:** Claude (implementation), at the user's request
- **Amends:** [ADR-0002](./0002-wasm-client-side-execution.md), [ADR-0006](./0006-server-side-ephemeris-proxy.md)

> Written during implementation, not back-filled. Please correct it if you
> disagree with the reasoning.

## Context

Phase 4 generates baseband IQ recordings: a user submits a receiver position, a
time window and an output format, and gets back a raw interleaved sample file
plus a JSON sidecar.

[ADR-0002](./0002-wasm-client-side-execution.md) established that computation
runs client-side in WebAssembly, and [ADR-0006](./0006-server-side-ephemeris-proxy.md)
narrowed the server's role to moving bytes: it fetches and trims RINEX, and
does no arithmetic. Both were right for phases 1–3, where the output of a whole
computation is a few dozen azimuth/elevation pairs.

IQ generation breaks the assumption those rest on, for reasons of scale rather
than taste:

1. **Output size.** One second of GPS L1 C/A at the minimum usable sample rate
   (2.046 MHz complex, int8) is 4.1 MB. A one-minute recording at 4 MHz in
   int16 is 960 MB. These are not values a browser tab can hold, and
   `MAX_OUTPUT_BYTES` caps a single job at 2 GiB.
2. **Run time.** Synthesis evaluates a carrier and a code phase per satellite
   per sample — of order 10⁸ trigonometric evaluations for a short recording.
   In WASM on a page's main thread that freezes the tab; the existing code
   already treats a non-terminating loop as a hazard worth a hard iteration cap
   in `PropagationConfig` for exactly this reason.
3. **What the artefact is for.** A sky plot is looked at and discarded. An IQ
   file is fed to GNU Radio, a LabSAT, or receiver software. It needs to exist
   as a file with a stable URL, which means it has to be written somewhere that
   outlives the page that asked for it.

## Decision

Add a server-side, job-based pipeline.

- **`crates/gnss-iq`** holds the synthesis library and a `gnss-iq-worker`
  binary. It is native-only and is *not* part of the WASM build.
- **The Next.js app owns the job API**: `POST /api/jobs` validates and enqueues,
  `GET /api/jobs/{id}` reports status, and
  `GET /api/jobs/{id}/files/{binary,sidecar}` streams the outputs.
- **The queue is in-process and runs one job at a time**, spawning the worker as
  a child process per job.
- **Ephemeris fetching stays on the TypeScript side.** The worker speaks no
  HTTP; the API populates the disk cache and points the worker at it.

ADR-0002 and ADR-0006 remain accurate for what they cover. Phases 1–3 still
need no compute backend, and `/api/ephemeris` is still a byte-moving proxy.
This ADR adds a second, separate server role for phase 4 rather than
reinterpreting the first one.

## Consequences

**The project now has a compute backend, and deployment is no longer static.**
That was already on the roadmap for phase 5 but arrives here. Hosting must
provide a filesystem, a long-lived process, and enough disk for the outputs.

**One job at a time.** Concurrent synthesis would not finish anything sooner on
one machine, and it would make the per-job size cap meaningless — that cap
bounds one output, not the sum of everything in flight.

**The queue is not durable.** It is module state, so a restart loses anything
still waiting. `failInterruptedJobs` converts orphaned records into a visible
`failed` with an explanatory message, rather than leaving a client polling a job
that will never run. Swapping in a real broker means replacing `enqueue` and
`drain` in `web/src/lib/jobs/queue.ts`; nothing else in the app knows the queue
exists.

**A subprocess rather than in-process work.** Synthesis would block Next.js's
event loop for the whole run, so every status poll would queue behind the job it
is asking about. A subprocess also contains failures, and lets the worker be
driven from a shell without Node.

### Why the worker does not fetch its own ephemeris

It would be the more self-contained design, and it was rejected for one
specific reason: the cache-correctness rules in
`web/src/lib/ephemerisCache.ts` are subtle and were arrived at by hitting the
bug. A broadcast file fetched while its day is still running is necessarily
partial, so a cached copy is only trusted once its mtime is past the end of the
day it covers. Implementing that a second time in Rust would mean two copies of
a rule that is easy to get subtly wrong, and they would drift.

The cost is that `gnss-iq-worker` cannot fetch a file it does not have. It is
mitigated rather than ignored: `EphemerisSource` is a trait, `DiskCacheSource`
names the exact upstream URL when a file is missing, and `--nav` accepts
explicit paths. A future direct-fetch implementation satisfies the same trait
and changes no pipeline stage.

The duplication that *does* exist is the day-range calculation, which appears in
both languages. That is deliberate and asymmetric: the TypeScript side decides
what to *fetch*, the Rust side decides what to *read*. If they disagree, the
worst case is a missing neighbouring day, which the worker tolerates with some
loss of accuracy near a UTC boundary — not a wrong answer.

### Reproducibility

Every job records a seed, and the random number generator is written out in
`crates/gnss-iq/src/rng.rs` rather than taken from `rand`, whose `StdRng` is
explicitly allowed to change algorithm in a minor release. A stored seed that
stops meaning what it meant is worse than no seed at all. The generator is
checked against the reference implementation's published test vector.

## Alternatives considered

**Generate in the browser and stream to a download.** Removes the backend, and
fails on both size and time: a 960 MB `Blob` is not viable, and the tab freezes.

**A Python service, matching the shape the requirements were written in.** The
noise-model interfaces in the brief are Python ABCs. Rejected because the
propagation, clock and geometry math already exists in `gnss-core` and is
cross-validated against an independent implementation; a Python pipeline would
either reimplement it — creating a second, unvalidated copy of the thing the
project's validation standard exists to protect — or bind to it over FFI, which
buys the complexity of both languages and the benefits of neither. The
interfaces survive as Rust traits with the same injection semantics: the
pipeline names no concrete model, and `build_models` is the only place a spec
tag becomes a type.

**A real broker (Redis, SQS) from the start.** Correct for a multi-machine
deployment and pure overhead for one whose outputs land on local disk. The
queue is one module behind a two-function interface precisely so this can change
without touching anything else.
