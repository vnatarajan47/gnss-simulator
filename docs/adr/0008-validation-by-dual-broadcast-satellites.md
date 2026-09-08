# ADR-0008: Validate geostationary propagation against satellites that broadcast twice

- **Date:** 2026-09-07
- **Status:** Accepted
- **Deciders:** Claude (implementation), at the user's request

> Written during implementation, not back-filled. Please correct it if you
> disagree with the reasoning.

## Context

The project's standing rule is that no source becomes user-selectable until it
has an independent cross-check appropriate to its geometry. Two mechanisms
existed:

- **Keplerian constellations** are checked against `tools/reference_skyplot.py`,
  a second implementation that shares no code with the Rust.
- **SBAS** is checked against closed-form geostationary geometry, which is
  stronger than another program because a GEO's look angles have an exact
  analytic solution.

Phase 5 adds BeiDou, and BeiDou breaks the pattern. Its geostationary
satellites do not use the same transformation into Earth-fixed coordinates as
everything else: BDS-SIS-ICD-B1I §5.2.4.12 refers their elements to a plane
tilted 5 degrees out of the equator and applies the Earth rotation as an
explicit rotation afterwards, rather than folding it into the node. Getting it
wrong puts the satellite thousands of kilometres from where it is.

That transformation is the least corroborated code in the propagator. The
Python reference can be extended to cover it, but both implementations would
then be *my* transcription of the same paragraph of the same ICD: a shared
misreading would agree with itself perfectly. The closed-form check does not
help either — it validates the SBAS path, which is a different code path.

While mapping SBAS providers against the archive, a coincidence turned out to
be a fact: **several satellites broadcast on both paths at once.**

| Satellite | Also broadcasts as | Both at |
| --- | --- | --- |
| BeiDou C01 | BDSBAS PRN 130 | 140.1 deg E |
| BeiDou C03 | BDSBAS PRN 143 | 110.5 deg E |
| QZSS J07 | MSAS PRN 137 | 127.0 deg E |
| QZSS J08 | MSAS PRN 129 | 90.5 deg E |

BDSBAS is carried on BeiDou's geostationary satellites and MSAS has moved onto
QZSS's. So the same physical spacecraft appears in the same file twice: once as
a Keplerian element set and once as an ECEF state vector, fitted independently
by different operators' ground segments and propagated by two algorithms that
share no code — an orbit solve on one side, a second-order Taylor expansion on
the other.

## Decision

We will validate geostationary propagation by requiring the two broadcast
representations of the same spacecraft to place it in the same position, and
treat that as the primary evidence for BeiDou's geostationary transformation.

The test (`crates/gnss-core/tests/sbas_geometry.rs`) asserts two things per
pair:

1. **Where they are the same spacecraft, they agree to within 50 m.** They
   actually agree to 0.4–2.0 m, which is below the accuracy of the broadcast
   ephemeris itself.
2. **The separation does not grow.** Measured at two epochs half an hour apart,
   it must stay within 100 m of itself. This is what distinguishes the two ways
   the numbers can be close: a frame error grows with time from the reference
   epoch (a missing Earth-rotation term moves a satellite ~2000 km over half an
   hour), where a genuine offset does not.

The second rule earns its keep immediately. BeiDou C02 and BDSBAS PRN 144 both
sit at 80.1 deg E but land 2.8 km apart — constant to within 50 m over half an
hour. That is not a propagation error: it is a *co-located* spacecraft. BDS-2
and BDS-3 both fly geostationary satellites at the same slots, and operators
keep co-located satellites a few kilometres apart deliberately. The test
records that pair as sharing a slot rather than being one satellite, and still
requires the separation to be constant.

## Alternatives considered

### Extend the Python reference to cover the BeiDou GEO transformation

Done anyway — the reference does implement it, and the two agree to 1e-9
degrees. But it is weak evidence on its own for the reason above: both are the
same reading of the same ICD paragraph, so the check confirms the transcription
and not the interpretation. It is kept as a regression guard, not as the
validation.

### Compare against a published precise ephemeris (SP3)

The strongest check available in principle: IGS publishes post-processed orbits
good to centimetres. It was rejected on project constraints rather than on
merit. It means a second data source with its own fetch path, format parser and
availability story, and the project deliberately runs off one archive with no
credentials anywhere. Worth revisiting if phase 4's IQ generation needs orbit
accuracy the broadcast ephemeris cannot give.

### Trust the closed-form geostationary check to cover BeiDou too

It does not reach. The analytic check verifies that a *position* corresponds to
the look angles reported for it; it takes the position from the SBAS state
vector. It says nothing about whether the Keplerian path arrives at the same
position, which is precisely the question the GEO transformation raises.

## Consequences

- **Positive:** BeiDou's geostationary satellites are validated against data,
  not against a second reading of the specification. The same test covers
  QZSS's geostationary satellites and the SBAS state-vector path in both
  directions.
- **Positive:** The co-location finding is now recorded rather than
  rediscovered. A future reader seeing C02 and PRN 144 2.8 km apart has an
  explanation and a test that would fail if the cause changed.
- **Negative:** The check depends on a coincidence of the current constellation
  design. If BDSBAS or MSAS moved onto dedicated spacecraft, the pairs would
  disappear and BeiDou's GEO transformation would fall back to the weaker
  evidence. The test would fail loudly in that case — it asserts the pairs
  exist — which is the right way for it to expire.
- **Neutral / follow-ups:** The pairing is expressed as a table in the test
  (`DUAL_BROADCAST`). Adding a pair, or reclassifying one as co-located rather
  than identical, is one line.
