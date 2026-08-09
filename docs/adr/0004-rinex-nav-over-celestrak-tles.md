# ADR-0004: RINEX Nav (via CDDIS) as the ephemeris source over CelesTrak TLEs

Date: 2026-08-09
Status: Accepted

## Context
Satellite position for sky plot and DOP computation can come from different
ephemeris sources with different accuracy/complexity tradeoffs: CelesTrak
TLEs (propagated via SGP4, simple, good to a few km) or RINEX Nav broadcast
ephemeris (propagated via the GPS ICD-200 Keplerian algorithm, more accurate,
GNSS-specific). A third option, IGS precise SP3 products, offers cm-level
accuracy but requires post-processed data not available in real time.

Initial recommendation was to start with CelesTrak TLEs for simplicity and
defer RINEX Nav to a later accuracy pass. This was overridden based on
existing familiarity with the RINEX Nav format and the certainty that RINEX
Nav would be needed eventually (it's required for pseudorange/IQ generation
in later phases regardless).

## Decision
Use RINEX Nav broadcast ephemeris, sourced from CDDIS, as the ephemeris
source starting in phase 1, rather than starting with CelesTrak TLEs and
migrating later.

## Alternatives considered
- CelesTrak TLEs + SGP4: simpler parsing, faster to a working prototype, but
  would require a migration to RINEX Nav later anyway once IQ generation
  (phase 4) requires broadcast-ephemeris-grade accuracy and satellite clock
  correction data that TLEs don't provide.
- IGS precise SP3 products: highest accuracy, but post-processed (not
  real-time) and unnecessary for phase 1-2's visibility/DOP use case;
  revisit if cm-level accuracy is needed for IQ generation truth data later.

## Consequences
- Avoids a future migration/rewrite of the ephemeris ingestion and
  propagation code when moving from sky plots to IQ generation.
- Requires implementing the fuller Keplerian broadcast algorithm (ICD-GPS-200
  §20.3.3.4.3) and RINEX Nav parsing from the start, rather than the simpler
  SGP4 propagation — more upfront implementation work in phase 1.
- Ties phase 1 to ephemeris validity-window handling (selecting the correct
  broadcast ephemeris block per satellite per timestamp) earlier than
  strictly necessary for a sky-plot-only use case.
- Automated fetching from CDDIS is explicitly deferred (phase 1 will use a
  static/pre-fetched RINEX Nav file); this ADR covers the source choice, not
  the ingestion pipeline.