# ADR-0006: A server-side ephemeris proxy, without moving compute off the client

- **Date:** 2026-08-09
- **Status:** Accepted
- **Deciders:** Claude (implementation), at the user's request
- **Amends:** [ADR-0002](./0002-wasm-client-side-execution.md)

> Written during implementation, not back-filled. Please correct it if you
> disagree with the reasoning.

## Context

Phase 1 originally shipped a single pre-fetched RINEX Nav file for one UTC day,
with the epoch picker pinned to that day. The requirement changed: the user
should be able to pick *any* time and have the app retrieve the matching
broadcast ephemeris on demand.

[ADR-0002](./0002-wasm-client-side-execution.md) states that phases 1–3 ship
"as a static frontend with no backend server required". Fetching on demand
appears to conflict with that. Two facts decide how:

1. **CORS.** BKG's IGS archive returns no `Access-Control-Allow-Origin` header
   (verified against the live host). A browser therefore cannot fetch it
   directly, whatever we do client-side. CDDIS — the source named in
   [ADR-0004](./0004-rinex-nav-over-celestrak-tles.md) — additionally requires
   an Earthdata login, which is worse: it would mean handling a credential.
2. **Payload size.** The merged daily broadcast file is ~1.4 MB gzipped and
   ~8 MB of text, of which the GPS records are ~300 kB. Shipping the whole
   thing to the browser to discard 96% of it is slow on both the wire and in
   the parser.

## Decision

Add a same-origin Next.js route handler, `/api/ephemeris`, that fetches the
day's broadcast file from BKG, caches it on disk, trims it to the requested
constellations, and returns it. **Compute does not move.** Propagation and look
angles still run entirely in WASM in the browser; the route moves *bytes*, not
arithmetic.

ADR-0002's substantive claim — that phases 1–3 do not need a compute backend —
is unchanged. Its incidental claim that they need no server at all is now
wrong, and this ADR amends it.

## Alternatives considered

### Keep bundling a static file and accept a fixed day

Rejected: it is the thing the user asked to remove. It also does not scale to
phase 2, where a time-series sky plot will want to cross day boundaries.

### Fetch from a CORS-enabled mirror directly from the browser

Rejected: no mirror of the IGS broadcast product is known to send permissive
CORS headers, and depending on an undocumented header from a third party is
fragile. It would also force the full 1.4 MB onto every client.

### Proxy through a generic CORS relay (e.g. a public `cors-anywhere`)

Rejected: it puts an uncontrolled third party in the data path for correctness-
critical input, with no availability guarantee and no cache.

### Bundle the whole archive

Rejected on arithmetic: ~1.4 MB/day × 9 years is ~4.6 GB.

## Consequences

- **Positive:** any epoch from 2017-06-01 to today now works. Verified end to
  end across RINEX 3.03–3.05 for 2017, 2019, 2021, 2023, 2025 and 2026; all
  return 32 satellites with ranges inside the GPS band.
- **Positive:** server-side trimming cuts ~8 MB of text to ~300 kB, keeping
  in-browser parse time around 20–30 ms.
- **Positive:** the disk cache in `data/cache/` means a repeated day is served
  in tens of milliseconds instead of ~5 s, and works offline once warm.
- **Negative:** phase 1 is no longer deployable as pure static hosting. It now
  needs a Node runtime (or an equivalent edge function).
- **Negative:** the app is unusable offline on a cold cache. Previously the
  bundled file made it work with no network at all. `data/` still holds one
  file for the Rust tests, but the app no longer reads it.
- **Negative:** we now depend on BKG's availability at request time. There is
  no fallback source; if BKG is down, the UI shows an error.
- **Follow-up:** cache responses are `immutable` for past days, so a change to
  the *trimming* logic would otherwise never reach a client that had already
  cached. `EPHEMERIS_FORMAT_VERSION` in `web/src/lib/coverage.ts` participates
  in the request URL to force a new cache key; bump it whenever the server-side
  transformation changes. This was found the hard way — see below.

## Note: a parser panic, and why the WASM profile changed

The first working version of the trimmer swept a file's trailing newline into
the last record, emitting a zero-length line. The `rinex` crate panics on that
(`navigation/ephemeris/parsing.rs` slices `[4..]` without a length check), and
because RINEX files from the archive routinely end with a newline, this was
reachable from perfectly ordinary input.

A Rust panic inside WASM is a trap: it poisons the module and takes the page
with it, surfacing to JS as an opaque `unreachable`. Two changes followed:

- `gnss-core::parse_nav` now wraps the third-party parser in `catch_unwind` and
  returns `Error::ParserPanicked`, so malformed input is an error, not a crash.
- `panic = "abort"` was removed from the release profile, because `catch_unwind`
  needs unwinding to be available. Cost: 130 bytes of WASM.

This is the same class of defect as the uncapped Kepler iteration noted in
[ADR-0005](./0005-hand-rolled-ephemeris-propagation.md), and reinforces that
conclusion: the parser is useful but not defensive, so we contain it.
