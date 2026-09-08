# Methods: mathematics, data sources, and validation

This document specifies exactly what `gnss-simulator` computes and where every
number in it comes from. It is written to be sufficient for someone to
reimplement the tool from scratch, or to decide whether its output can be
trusted for a particular purpose.

It is deliberately explicit about what is *not* modelled. A sky plot is a
statement about geometry, and geometry is only part of what determines whether
a receiver can use a satellite.

**Contents**

1. [Scope](#1-scope)
2. [Conventions and notation](#2-conventions-and-notation)
3. [Data sources](#3-data-sources)
4. [Time](#4-time)
5. [Reference frames](#5-reference-frames)
6. [Ephemeris selection](#6-ephemeris-selection)
7. [Orbit propagation](#7-orbit-propagation)
8. [Look angles and the elevation mask](#8-look-angles-and-the-elevation-mask)
9. [Dilution of precision](#9-dilution-of-precision)
10. [Time series](#10-time-series)
11. [Visualisation geometry](#11-visualisation-geometry)
12. [Validation](#12-validation)
13. [Error budget and limitations](#13-error-budget-and-limitations)
14. [Symbols](#14-symbols)
15. [References](#15-references)

---

## 1. Scope

Given a receiver position, an instant, and a set of enabled satellite sources,
the tool answers: **which satellites are above the local horizon, in what
direction, at what range, and how well does that geometry condition a position
fix?**

It computes this from *broadcast* ephemeris — the orbital data the satellites
themselves transmit — propagated with the algorithms specified in each system's
interface control document. Everything is evaluated in the browser; the only
server-side step is retrieving and trimming the source file
([ADR-0006](adr/0006-server-side-ephemeris-proxy.md)).

**In scope:** satellite position, look angles, geometric range, visibility
against an elevation mask, and dilution of precision, for GPS, Galileo, BeiDou,
QZSS and six satellite-based augmentation systems, anywhere on Earth, for any
UTC day from 2017-06-01 onward.

**Explicitly out of scope**, and therefore *not* in any number this tool
reports: signal power and link budget; ionospheric and tropospheric delay;
multipath and local obstruction; receiver clock behaviour; satellite clock
corrections (the coefficients are parsed and carried, but unused); antenna
phase-centre offsets; solid-Earth tides; relativistic corrections beyond the
Sagnac term; and any correction of the broadcast orbits against precise
post-processed products.

---

## 2. Conventions and notation

| Convention | Choice |
| --- | --- |
| Angles | Degrees at every interface; radians internally |
| Lengths | Metres internally; kilometres at the display boundary |
| Time | Seconds; GPS time internally, UTC at every interface |
| Latitude | Positive north, geodetic (not geocentric) unless stated |
| Longitude | Positive east, folded into [−180°, 180°) |
| Height | Above the WGS-84 **ellipsoid**, not mean sea level |
| Azimuth | 0° at true north, increasing clockwise through east |
| Elevation | Above the local horizontal plane; negative below it |
| Rotations | Right-handed about the named axis, rotating the *vector* |

The last row is worth stating explicitly because the interface control
documents use the opposite convention: their `Rz(θ)` and `Rx(θ)` rotate the
*frame*, which is equivalent to rotating a vector by `−θ`. Constants derived
from an ICD rotation are stored in the vector-rotation sense so that no sign
flip is left at a call site (`constants::beidou_geo::TILT_RAD`).

Vectors are column vectors. `‖v‖` is the Euclidean norm. A subscript `k`
denotes a quantity evaluated at the epoch of interest.

---

## 3. Data sources

### 3.1 The broadcast ephemeris product

All orbital data comes from a single source: the **IGS merged broadcast
ephemeris product**, `BRDC00WRD`, distributed by BKG
(Bundesamt für Kartographie und Geodäsie).

```
https://igs.bkg.bund.de/root_ftp/IGS/BRDC/<YYYY>/<DDD>/BRDC00WRD_R_<YYYY><DDD>0000_01D_MN.rnx.gz
```

One file per UTC day, RINEX 3 navigation format, gzip-compressed, typically
~1.4 MB compressed and ~8 MB as text. `DDD` is the day of year, 1-based and
zero-padded to three digits.

This product is a *merge*: IGS combines navigation messages recorded by many
tracking stations into one file per day. That has two consequences that recur
throughout this document. It is why a satellite's ephemeris can appear more
than once for the same epoch (§3.5), and it is why formatting and unit
conventions are not internally consistent (§3.5) — different contributing
converters wrote different records.

**Why BKG and not CDDIS.** CDDIS distributes the same IGS product but requires
an Earthdata login. This project handles no credentials anywhere, which is a
deliberate constraint rather than an oversight
([ADR-0004](adr/0004-rinex-nav-over-celestrak-tles.md),
[ADR-0006](adr/0006-server-side-ephemeris-proxy.md)).

**Why broadcast ephemeris and not two-line elements.** TLEs are fitted to a
different force model (SGP4) and are not what a receiver uses. Broadcast
ephemeris is the data the actual system distributes for the actual purpose, and
each constellation's ICD specifies exactly how to evaluate it — so agreement
with a real receiver is a matter of following the specification rather than of
matching a fit.

### 3.2 Coverage of the archive

| Source | Available from | Notes |
| --- | --- | --- |
| GPS, Galileo, BeiDou, QZSS | 2017-06-01 | The archive's start |
| EGNOS | ~2021-01-01 | The only augmentation present before 2025 |
| WAAS, MSAS, GAGAN, BDSBAS, SouthPAN | ~2025-01-01 | Appear together |
| SDCM | — | Absent from every day sampled since 2022 |
| KASS | — | Present only as a placeholder record (§3.5) |

These bounds were established empirically by probing the archive; they are not
documented upstream. Augmentation coverage between 2021 and 2025 is
**intermittent rather than monotone**: sampled days in 2022 carry GAGAN, SDCM
and BDSBAS, but a sampled day in mid-2023 is back to EGNOS alone. Availability
is therefore held per source (`Source.availableFrom` in
`web/src/lib/coverage.ts`) and the interface warns that a source "may be missing
or incomplete" rather than asserting it will be empty.

### 3.3 Record layout

A RINEX 3 navigation record begins with a satellite identifier and epoch at
column 0 and continues on indented lines. The Keplerian constellations use
eight lines; SBAS and GLONASS use four. Field values occupy 19 columns each,
starting at column 4 on continuation lines.

The Keplerian broadcast-orbit layout is common to GPS, Galileo, BeiDou and
QZSS for every field this tool reads:

| Line | Field 1 | Field 2 | Field 3 | Field 4 |
| --- | --- | --- | --- | --- |
| 0 | SV + epoch | `a_f0` | `a_f1` | `a_f2` |
| 1 | IOD | `C_rs` | `Δn` | `M₀` |
| 2 | `C_uc` | `e` | `C_us` | `√A` |
| 3 | `t_oe` | `C_ic` | `Ω₀` | `C_is` |
| 4 | `i₀` | `C_rc` | `ω` | `Ω̇` |
| 5 | `i̇` | (system-specific) | week | (system-specific) |
| 6 | accuracy | health | group delay | IOD (clock) |
| 7 | transmission time | (system-specific) | — | — |

Only the system-specific fields differ between constellations — Galileo's data
sources where GPS has L2 codes, BeiDou's AODC where GPS has IODC — and this
tool reads none of them. That is why one table serves all four systems in both
the Rust parser and the independent Python reference.

The SBAS (GEO) layout is four lines carrying an ECEF state vector:

| Line | Field 1 | Field 2 | Field 3 | Field 4 |
| --- | --- | --- | --- | --- |
| 0 | SV + epoch | `a_f0` | `a_f1` | transmission time |
| 1 | `X` | `Ẋ` | `Ẍ` | health |
| 2 | `Y` | `Ẏ` | `Ÿ` | accuracy |
| 3 | `Z` | `Ż` | `Z̈` | IODN |

### 3.4 Satellite identifiers

RINEX writes a satellite as a system letter plus a two-digit number: `G05`,
`E10`, `C21`, `J07`, `S31`. For SBAS this is **PRN − 100**, so WAAS PRN 131 is
written `S31`.

Internally the tool stores the true PRN, because that is what provider
assignments are published against and what FAA and RTCA documentation uses.
`Sv::rinex_id()` reconstructs the on-disk form where the RINEX specification is
being spoken; the user-facing display never does, because a marker reading
`S31` next to a label reading `131` looks like a truncation bug.

SBAS PRN assignments, as published for 2025–2026:

| Provider | PRNs |
| --- | --- |
| WAAS | 131, 133, 135, 138 |
| EGNOS | 120, 121, 123, 126, 136 |
| MSAS | 129, 137 |
| GAGAN | 127, 128, 132 |
| SDCM | 125, 140, 141 |
| BDSBAS | 130, 143, 144 |
| KASS | 134 |
| SouthPAN | 122 |

PRNs in the 120–158 band with no published assignment (142 and 148 appear in
the archive) fall to an explicit "unknown provider" category rather than being
dropped. Their geometry is as well determined as any other satellite's; it is
only the operator label that is uncertain.

### 3.5 Known defects in the source data

Each of these is handled by an explicit, documented rule rather than assumed
away. They are properties of the product, not of this tool, and anyone
processing the same files will meet them.

**Inconsistent units for the SBAS state vector.** RINEX 3 specifies kilometres
for the GEO position/velocity/acceleration triplet, but the merged product
mixes kilometres and metres between providers and even between records for the
same PRN. Rather than trusting a per-file or per-PRN rule, the scale is decided
*physically*: an SBAS satellite is geostationary, so of the two candidate
interpretations exactly one puts it at a plausible geostationary radius.

```
decode_scale(x, y, z) = 1000  if 3.5×10⁷ ≤ ‖(x,y,z)‖·1000 ≤ 4.6×10⁷
                      = 1     if 3.5×10⁷ ≤ ‖(x,y,z)‖      ≤ 4.6×10⁷
                      = reject otherwise
```

Kilometres are tried first because that is what the specification says. The
band is wide enough to admit inclined and drifting geostationary satellites
while still separating the two unit interpretations by three orders of
magnitude, so the test is never ambiguous.

**Placeholder records.** Some PRNs appear only as an almanac-shaped daily
filler with `Z = 32767` — a saturated 16-bit field. Neither unit reading of
that is a geostationary radius (it gives 53 000 km or 53 km), so the same
physical test rejects it without needing to know the sentinel exists. This is
all that KASS and PRN 142 consist of on a sampled 2026 day, which is why KASS
cannot yet be validated.

**The SBAS health word is not populated.** It is dominated by all-ones fillers
(31, 63) even for operational satellites — every WAAS record carries 31.
Applying a `health == 0` rule would discard the entire constellation, so SBAS
health is parsed and surfaced but never used for filtering.

**The QZSS health word never reads zero.** It is a per-signal bitfield whose
bits remain set for signals a given satellite does not carry: J02, J03 and J07
sit permanently at 1, and J04 and J08 at 16, across every day sampled. The same
trap as SBAS in a milder form. Only the all-ones word (63) is treated as
unusable, on the grounds that no per-signal combination can mean "every signal
on this satellite is simultaneously fine". Per-signal decoding
(IS-QZSS-PNT §4.1.2.3) is the correct fix and is deferred.

This rule must **not** be extended to Galileo. There, 16 and 130 are genuine
signal-health flags, and they are what excludes E14 and E18 — the pair stranded
in eccentric orbits and flagged unusable for the Open Service on every day of
the archive.

**Duplicate printings of the same ephemeris.** In a sampled 2026 day, *every*
Galileo (satellite, epoch) pair and 85% of BeiDou pairs appear **twice**: once
in the modern form with 13 significant digits (`5.440603435516e+03`) and once
in the legacy leading-dot form with 12 (`.544060343552e+04`). GPS and QZSS are
never duplicated. The two printings are the same ephemeris at different
precision; the difference propagates to about 44 µm of satellite position,
which is the entire explanation for the residual reported in §12.3.

**Epochs outside the file's own day.** A sampled daily file contains records
stamped from 2016 to a date four days after the file's own, presumably from
contributing stations with clock or decoding faults. These are handled by the
ordinary selection window (§6): a record too far from the requested instant is
simply never selected.

### 3.6 Retrieval and trimming

The browser cannot fetch the archive directly — it sends no
`Access-Control-Allow-Origin` header — so a same-origin route retrieves the
file, caches it, and trims it. Compute remains entirely client-side; the route
is a proxy, not a backend ([ADR-0006](adr/0006-server-side-ephemeris-proxy.md)).

Trimming applies three filters:

1. **Constellation.** Keep only the RINEX system codes the enabled sources
   need.
2. **SBAS PRN.** A full day of SBAS is ~12 800 records across every regional
   system where one provider is ~1 000.
3. **Time window.** Keep records whose epoch lies within
   `[t_start − 3 h, t_end + 3 h]`.

The three-hour margin is the widest curve-fit half-interval (2 h, §6) plus an
hour of slack, so it covers selection at either end of the window including the
block *after* the window that a nearest-epoch search legitimately reaches for.

Filters 2 and 3 are **size optimisations only**. The authoritative provider
filter is inside the propagation core, and a record whose epoch cannot be
parsed is *kept* — the failure mode of a size optimisation must be a larger
response, never a missing satellite.

A cached copy of a day is trusted only if it was written *after* that day
ended. Broadcast files accumulate through the day, so a file fetched at noon
holds half a day of records; the same rule makes today's file always refetch.

---

## 4. Time

### 4.1 GPS time as the internal scale

Every instant inside the propagation core is **continuous seconds since the GPS
epoch**, 1980-01-06T00:00:00 UTC, with no leap seconds and no week rollover.

This is a deliberate choice with one large consequence: the difference of two
instants is always the true elapsed interval, so **no ±half-week correction
appears anywhere**. The classic bug in broadcast-ephemeris code — `t_k` computed
across a week boundary without the correction from IS-GPS-200 §20.3.3.4.3.1 —
cannot occur, because the representation never wraps.

### 4.2 Leap seconds

Leap seconds appear in exactly one place: conversion to and from Unix time.

```
t_GPS = t_unix − 315 964 800 + ΔLS(t_unix)
```

`ΔLS` is a step function tabulated from the 18 leap seconds introduced between
1981-07-01 and 2017-01-01. It was 0 at the GPS epoch and is 18 s at the time of
writing; no further leap second has been announced. Adding a row to the table
is the only change required if IERS declares one.

Converting back requires care, because the offset depends on the instant being
solved for. The implementation approximates once and refines, which is exact
except within one leap second of a leap instant.

### 4.3 Constellation time scales

Broadcast records carry week numbers and seconds-of-week on their own system's
scale.

| System | Week count | Offset to GPS time |
| --- | --- | --- |
| GPS | GPS week | 0 |
| Galileo | GPS week (RINEX 3 reports the continuous GPS count) | 0 |
| QZSS | GPS week | 0 |
| BeiDou | BDT week, from 2006-01-01 = GPS week 1356 | +14 s |
| SBAS | *(none carried)* | 0 |

So for BeiDou:

```
t_oe(GPS) = (week_BDT + 1356) × 604 800 + sow + 14
```

SBAS records carry no week number at all: the state vector's reference epoch is
the record's own timestamp, which the parser has already resolved.

Note that the +14 s applies to the **absolute epoch**, while the
longitude-of-ascending-node term of the propagation algorithm (§7.1) uses the
*raw broadcast* seconds-of-week. These are different quantities and are stored
separately for that reason.

Note also that the offsets in this table are for *interpreting timestamps*.
They are not the same question as which satellites share a receiver clock,
which is treated in §9.2 and reaches a different answer for Galileo.

---

## 5. Reference frames

### 5.1 The WGS-84 ellipsoid

| Quantity | Symbol | Value |
| --- | --- | --- |
| Semi-major axis | `a` | 6 378 137.0 m |
| Flattening | `f` | 1 / 298.257 223 563 |
| Semi-minor axis | `b` | `a(1 − f)` |
| First eccentricity squared | `e²` | `f(2 − f)` |
| Second eccentricity squared | `e′²` | `(a² − b²) / b²` |
| Speed of light | `c` | 299 792 458 m/s |

Constants come from the relevant interface control documents rather than from a
generic physics table. This matters: a broadcast ephemeris is only
self-consistent when propagated with the *same* constants the control segment
used to fit it (see §7.1).

### 5.2 Geodetic to ECEF

Closed form, exact:

```
N = a / √(1 − e² sin²φ)

X = (N + h) cos φ cos λ
Y = (N + h) cos φ sin λ
Z = (N(1 − e²) + h) sin φ
```

where `N` is the radius of curvature in the prime vertical, `φ` geodetic
latitude, `λ` longitude, `h` ellipsoidal height.

### 5.3 ECEF to geodetic

Bowring's method — non-iterative and accurate to well under a millimetre
anywhere near the Earth's surface:

```
p = √(X² + Y²)
θ = atan2(Z·a, p·b)

φ = atan2(Z + e′² b sin³θ,  p − e² a cos³θ)
λ = atan2(Y, X)
h = p / cos φ − N
```

The polar axis (`p ≈ 0`) is a genuine singularity — longitude is undefined —
and is handled as an explicit special case rather than left to produce a
`NaN`.

### 5.4 ECEF to local ENU

With `d = r_sat − r_obs` the ECEF vector from observer to satellite:

```
⎡ E ⎤   ⎡    −sin λ            cos λ         0    ⎤ ⎡ d_x ⎤
⎢ N ⎥ = ⎢ −sin φ cos λ    −sin φ sin λ    cos φ   ⎥ ⎢ d_y ⎥
⎣ U ⎦   ⎣  cos φ cos λ     cos φ sin λ    sin φ   ⎦ ⎣ d_z ⎦
```

This matrix is orthogonal, so it is a change of basis and not an
approximation. The observer's *geodetic* latitude is used, which is what makes
"up" the local vertical (normal to the ellipsoid) rather than the geocentric
radial direction. The two differ by up to about 0.19°, which is far larger than
any numerical effect in this document — using the wrong one would be a real
error, not a rounding difference.

---

## 6. Ephemeris selection

A daily file holds many ephemeris blocks per satellite. Selecting one for an
instant `t` is a filter and then a minimisation.

**Filter.** A block is a candidate if

- its health word passes the constellation's rule (§3.5), unless health
  filtering is disabled; and
- `|t − t_oe| ≤ T_fit`.

| Constellation | `T_fit` |
| --- | --- |
| GPS, QZSS | 7200 s |
| Galileo, BeiDou | 7200 s |
| SBAS | 900 s |

GPS LNAV fits a four-hour arc centred on `t_oe` (IS-GPS-200 §20.3.4.4), giving
a two-hour half-interval. Galileo and BeiDou publish on a shorter cadence with
shorter nominal validity, but two hours remains a safe upper bound for
*selection* — it bounds how far the tool will extrapolate, not how long the
data is officially valid.

SBAS is different in kind. Its state vectors are published every few minutes
and are intended only for short extrapolation, so 900 s tolerates a few missed
messages. A geostationary satellite barely moves in an Earth-fixed frame over
that span, so the cost of a slightly stale record is small.

**Minimisation.** The default strategy is **nearest time-of-ephemeris**:
minimise `|t − t_oe|`. This is the right choice for post-processing, because
the curve fit is centred on `t_oe` and degrades symmetrically either side; it
can and does select a block from the future.

An alternative strategy, *latest not after*, is implemented and mirrors what a
real-time receiver can do — it cannot use an ephemeris it has not yet received.
It is not currently exposed in the interface.

**Ties.** Two blocks with the same `t_oe` is not a corner case: it is the
ordinary Galileo situation, where I/NAV and F/NAV records share a reference
epoch but are fitted separately. Ties resolve to the higher issue of data,
which is the more recent upload. Determinism here is what matters — an
arbitrary but stable choice is fine, an unstable one would make the plot
flicker.

The issue-of-data field goes by a different name in each ICD and the parser
keeps the ICD's name: `IODE` (GPS, QZSS), `IODnav` (Galileo), `AODE` (BeiDou).
All three must be read, or the tie-break and the duplicate check silently stop
working for two constellations.

**Merging across midnight.** A window may span a UTC day boundary while files
are published per day, so both days are fetched and merged. Merging is also
strictly *better* than using either file alone near the boundary: an epoch at
00:10 has its nearest `t_oe` in the previous day's file, so a single-day set
would extrapolate forward from 00:00 where a merged one interpolates.
Duplicate blocks across the overlap are dropped on `(t_oe, IOD)`.

---

## 7. Orbit propagation

### 7.1 Keplerian elements

GPS, Galileo, BeiDou and QZSS broadcast the same Keplerian parameter set and
are evaluated by the same algorithm — IS-GPS-200 Table 20-IV, reproduced
identically in the Galileo OS SIS ICD §5.1.1 and BDS-SIS-ICD-B1I §5.2.4.12.
They differ only in constants.

| Constant | GPS, QZSS | Galileo, BeiDou |
| --- | --- | --- |
| `μ` (m³/s²) | 3.986005 × 10¹⁴ | 3.986004418 × 10¹⁴ |
| `Ω̇ₑ` (rad/s) | 7.2921151467 × 10⁻⁵ | 7.2921151467 × 10⁻⁵ (Galileo)<br>7.292115 × 10⁻⁵ (BeiDou) |
| `F` (s/√m) | −4.442807633 × 10⁻¹⁰ | −4.442807309 × 10⁻¹⁰ |

`μ` is the one that bites. GPS uses the older WGS-84 value; Galileo and BeiDou
use the newer GTRF/CGCS2000 one. Using the wrong one shifts the computed mean
motion and is worth several metres of along-track error — the ephemeris was
*fitted* with a particular `μ`, so evaluating it with another is not a small
approximation but an inconsistency.

`F` is the relativistic clock-correction constant. It is carried for
completeness and is unused for look angles.

The algorithm, with `t_k = t − t_oe` on the continuous scale:

```
 1.  A      = (√A)²                                    semi-major axis
 2.  n₀     = √(μ / A³)                                computed mean motion
 3.  n      = n₀ + Δn                                  corrected mean motion
 4.  M_k    = M₀ + n·t_k                               mean anomaly
 5.  solve  M_k = E_k − e sin E_k  for E_k             Kepler's equation (§7.2)
 6.  ν_k    = atan2(√(1−e²) sin E_k,  cos E_k − e)     true anomaly
 7.  Φ_k    = ν_k + ω                                  argument of latitude
 8.  δu_k   = C_us sin 2Φ_k + C_uc cos 2Φ_k            second-harmonic corrections
     δr_k   = C_rs sin 2Φ_k + C_rc cos 2Φ_k
     δi_k   = C_is sin 2Φ_k + C_ic cos 2Φ_k
 9.  u_k    = Φ_k + δu_k                               corrected argument of latitude
     r_k    = A(1 − e cos E_k) + δr_k                  corrected radius
     i_k    = i₀ + δi_k + i̇·t_k                        corrected inclination
10.  x′_k   = r_k cos u_k                              position in the orbital plane
     y′_k   = r_k sin u_k
11.  Ω_k    = Ω₀ + (Ω̇ − Ω̇ₑ)·t_k − Ω̇ₑ·t_oe^sow         corrected node
12.  X_k    = x′_k cos Ω_k − y′_k cos i_k sin Ω_k      ECEF
     Y_k    = x′_k sin Ω_k + y′_k cos i_k cos Ω_k
     Z_k    = y′_k sin i_k
```

Two details in step 11 are easy to get wrong. The final term uses `t_oe` as
**raw seconds-of-week**, not as an absolute instant — which is why the two are
stored as separate fields rather than one being derived from the other, since
reconstructing seconds-of-week from an absolute instant is ambiguous at a week
boundary. And `Ω̇ₑ` appears twice with different roles: once rotating the node
over the elapsed time, once positioning the node relative to the start of the
week.

### 7.2 Kepler's equation

Step 5 is solved by Newton–Raphson from `E₀ = M`:

```
E ← E − (E − e sin E − M) / (1 − e cos E)
```

terminating when `|ΔE| < 10⁻¹²` rad, with a hard cap of **30 iterations**.

For GNSS eccentricities (`e < 0.03`, and as low as 2.7 × 10⁻⁴ for Galileo) the
iteration is quadratically convergent and settles in three or four passes. The
cap matters more than it looks: this code runs on a browser's main thread via
WebAssembly, where a non-terminating loop freezes the tab. Exceeding it is an
error (`KeplerDidNotConverge`), never a silently wrong answer.

An eccentricity outside `[0, 1)` is rejected before iterating.

At the GPS orbit radius, 10⁻¹² rad is well under a micrometre — some six orders
of magnitude below the accuracy of the broadcast ephemeris itself.

### 7.3 BeiDou geostationary satellites

BeiDou's geostationary satellites reach Earth-fixed coordinates by a different
route (BDS-SIS-ICD-B1I §5.2.4.12), because their elements are referred to a
plane tilted 5° out of the equator to avoid the singularity of a zero-inclination
orbit.

Steps 1–10 above are unchanged. Step 11 **omits** the `−Ω̇ₑ·t_k` term:

```
Ω_k = Ω₀ + Ω̇·t_k − Ω̇ₑ·t_oe^sow
```

and step 12 is followed by two rotations, written in the ICD's frame convention
as `Rz(Ω̇ₑ·t_k) · Rx(−5°)`. In the vector-rotation convention used throughout
this codebase, that is a rotation of **+5° about x** followed by **−Ω̇ₑ·t_k
about z**.

Omitting this entirely places the satellite thousands of kilometres from where
it is. Applying it to a non-geostationary BeiDou satellite would be equally
wrong.

**How a satellite is classified.** From its broadcast elements, not from a PRN
table:

```
geostationary  ⟺  |i₀| < 10°  and  |A − 42 164 km| < 1000 km
```

Both conditions are needed: a near-equatorial orbit at the wrong altitude is
not geostationary, and a geostationary-radius orbit at 55° inclination is
BeiDou's IGSO. The 10° threshold accommodates the fact that BeiDou's GEOs
broadcast an inclination of a few degrees rather than zero (because of the
tilted reference plane), while every IGSO and MEO satellite sits near 55° —
the two populations are an order of magnitude apart, so any threshold between
them gives the same answer.

Deciding physically rather than from a table is the same reasoning as §3.5:
PRN-to-orbit assignments change as satellites are launched and retired, and a
stale table is a silent error where a physical test is not.

The same predicate drives the square markers on the sky plot, which is why it
is computed in the core and carried across to the interface as a flag. It is no
longer true that "SBAS" and "geostationary" name the same set: BeiDou flies
C01–C05 and C59–C61, and QZSS flies J07 and J08.

### 7.4 SBAS state vectors

SBAS satellites broadcast an **ECEF state vector** rather than orbital
elements: position, velocity and acceleration at a reference epoch
(RINEX 3.05 §6.10, RTCA DO-229 message type 9). Propagation is a second-order
Taylor expansion about that epoch:

```
r(t) = r₀ + v₀·Δt + ½·a₀·Δt²        Δt = t − t_oe
```

This is not an approximation to an orbit solve — it is the propagation the
message is *designed* for. There is no orbit model to integrate, and the state
vector is refreshed every few minutes precisely because it is only intended for
short extrapolation.

### 7.5 Signal transit time and the Sagnac correction

The steps above give the satellite's position at an instant. What an observer
sees is where the satellite was when it *transmitted* the signal now arriving,
expressed in the Earth-fixed frame of the *reception* epoch. Both corrections
are applied.

Let `τ` be the signal transit time. The light-time equation is solved by
iteration:

```
τ₀ = ‖r_sat(t) − r_obs‖ / c

repeat twice:
    r_tx = r_sat(t − τ)                          position at transmission
    r    = Rz(−Ω̇ₑ·τ) · r_tx                      de-rotate into the reception frame
    τ    = ‖r − r_obs‖ / c
```

Two passes are sufficient: the range changes by at most ~1 km over one
iteration, so `τ` converges to well under a nanosecond.

The rotation is the **Sagnac** term. The Earth turns by `Ω̇ₑ·τ ≈ 5 µrad` during
the ~70 ms flight, moving a point at the satellite's radius by ~130 m; without
the de-rotation, transmitter and receiver would be expressed in frames offset
by that amount.

The net effect on look angles is small — of order 0.0005° — but it is the
physically correct quantity and costs one extra evaluation. It is applied
identically to both propagator paths, since the correction depends only on
geometry and the Earth rotation rate, not on how the position was obtained.

---

## 8. Look angles and the elevation mask

With `(E, N, U)` the ENU components of the vector from observer to satellite:

```
azimuth   = atan2(E, N)                    folded into [0°, 360°)
elevation = atan2(U, √(E² + N²))
range     = √(E² + N² + U²)
```

`atan2(U, horizontal)` rather than `asin(U / range)` — the two are
mathematically identical, but `asin` loses conditioning near the zenith, where
its argument approaches 1 and its derivative diverges. (The independent
reference implementation deliberately uses `asin` for exactly this reason: a
cross-check is only meaningful if the two implementations make different
choices. See §12.)

The reported range is **geometric**: the straight-line distance. It is not a
pseudorange. It contains no clock offsets, no atmospheric delay, and no
receiver-dependent terms.

A satellite is shown if `elevation ≥ mask`, with the mask user-selectable from
0° to 30° and defaulting to **5°**. Five degrees is the usual default for a
survey-grade receiver: below it, multipath and tropospheric delay make
observations unreliable. The mask is a flat horizon — a real local horizon
profile is phase 3.

Satellites are counted in three disjoint categories at each epoch: above the
mask, below the mask with a usable ephemeris, and with no ephemeris valid at
that instant. If every satellite falls in the third category the file does not
cover the requested instant, which is reported as an error rather than as an
empty sky. An empty sky is a legitimate answer — an observer can be masked out
— and is not conflated with missing data.

---

## 9. Dilution of precision

### 9.1 What DOP is

DOP answers: **how much does the geometry amplify ranging error into position
error?** It depends only on the directions to the visible satellites, not on any
measurement. A receiver with four satellites bunched in one quarter of the sky
has far worse DOP than one with four spread evenly, given identical signals.

The standard formulation linearises the pseudorange equations about the
receiver position. Each visible satellite contributes a row of the design
matrix `A` consisting of the negated unit line-of-sight vector in the local ENU
frame, followed by clock columns (§9.2). From `A`,

```
Q = (Aᵀ A)⁻¹
```

and the DOP scalars are square roots of sums of its diagonal.

The unit line-of-sight vector is recovered exactly from the look angles:

```
e = cos(el) sin(az)
n = cos(el) cos(az)
u = sin(el)
```

This is a change of coordinates, not a fit — a look-angle pair *is* a direction
in ENU. Taking DOP from angles rather than from ECEF vectors also keeps the
whole module a pure function of a sky view, which is what lets its tests state
geometry directly.

**Sign convention.** The line-of-sight columns are negated, matching the usual
linearisation. The sign makes no difference to the result: negating those three
columns is `A → A·D` for `D = diag(−1, −1, −1, 1, …)`, giving `Q → D Q D`,
which leaves the diagonal — and therefore every DOP scalar — unchanged.

### 9.2 Inter-system clock unknowns

A single clock column is correct only while every satellite in the solution
shares a time reference. Satellites are therefore grouped into **time systems**,
and each system present contributes its own clock column: a `1` in that
satellite's own column and `0` in the others.

| Time system | Constellations | Why |
| --- | --- | --- |
| GPS | GPS, QZSS, SBAS | QZSST is steered to GPS time and QZSS is specified for GPS-interoperable use; SBAS ranging signals are GPS-time coherent by design — that is what an augmentation system *is* |
| Galileo | Galileo | GST is held close to GPS time, but the offset is a broadcast quantity (the GGTO) rather than zero, so a receiver estimates it |
| BeiDou | BeiDou | BDT is a separate scale entirely |

The grouping is coarser than the constellation list, and deliberately so. What
matters is which satellites share a clock, not who operates them. Opening a
column per *constellation* would be wrong in the expensive direction: four GPS
satellites plus a WAAS geostationary satellite would report no solution, when
that is geometry a real receiver uses.

Two consequences follow, and neither is cosmetic:

**The minimum satellite count rises by one per system.**

```
minimum = 3 + (number of distinct time systems present)
```

Four satellites are enough for GPS alone. Four satellites split two-and-two
between GPS and Galileo are five unknowns from four equations, and must report
**no solution** rather than a plausible number.

**Adding a system's satellites can make DOP worse** than not adding them at
all, because they bring an unknown with them. This is real rather than an
artefact — it is why a receiver with only two satellites from a second
constellation may ignore them.

Column order is the time-system enumeration order, so the layout is a function
of the *set* of systems present and not of how the caller sorted its
satellites. That makes the result invariant to satellite ordering and gives
`TDOP` a stable meaning.

Full reasoning, including the alternatives rejected, is in
[ADR-0007](adr/0007-inter-system-clock-unknowns.md).

### 9.3 The scalars

With `Q` partitioned into position terms `Q_EE, Q_NN, Q_UU` and clock terms
`Q_c₁c₁ … Q_cₘcₘ`:

| Scalar | Definition |
| --- | --- |
| HDOP | `√(Q_EE + Q_NN)` |
| VDOP | `√(Q_UU)` |
| PDOP | `√(Q_EE + Q_NN + Q_UU)` |
| TDOP | `√(Q_c₁c₁)` — the *reference* system's clock |
| GDOP | `√(Q_EE + Q_NN + Q_UU + Σᵢ Q_cᵢcᵢ)` |

TDOP reports the lowest time system present, so a solution containing GPS
reports the GPS clock. The other systems' unknowns are inter-system biases
relative to it and are counted in GDOP rather than reported separately, on the
grounds that a receiver's usable time transfer is to its reference system.

GDOP over more than one clock is **not comparable** with a single-system GDOP:
it is solving for more unknowns. The count of clock unknowns is therefore part
of the result and is displayed alongside it.

VDOP is always worse than HDOP for a ground receiver. This is structural, not
incidental: every visible satellite is above the horizon, so the vertical
direction is only ever observed from one side, where the horizontal directions
are bracketed.

### 9.4 Inversion and degeneracy

`Aᵀ A` is accumulated directly as an outer-product sum rather than by forming
`A`, since the result is a fixed-size matrix of at most 6 × 6 and the series
code calls this once per epoch across hundreds of epochs.

Inversion is Gauss–Jordan elimination with partial pivoting. Pivots are
compared against the largest entry of the matrix rather than an absolute
epsilon, so the test is scale-free — `Aᵀ A` grows with the satellite count, and
a fixed threshold would quietly change meaning as satellites were added.

There is a second guard after inversion. `Q` is an inverse Gram matrix and so
positive semi-definite: its diagonal cannot be negative. If any diagonal entry
is negative or non-finite, the inversion was numerically hollow even though the
pivots cleared the tolerance, and no solution is reported.

**No solution is reported as absence, never as zero.** Too few satellites for
the number of clock unknowns, or a degenerate geometry — all satellites
coplanar with the receiver, for instance — are ordinary outcomes for a real sky
under a tight mask. Zero on a precision chart reads as *perfect* precision at
exactly the moments there is no fix, so the chart breaks its line instead.

---

## 10. Time series

A window is sampled at a uniform step, and everything else indexes the
resulting epoch axis.

Each epoch is computed by calling the same single-epoch path, not by a separate
inlined loop. That is deliberate: the interface shows a cursor on the track plot
and a table for the same instant, and two parallel implementations could drift
apart. One implementation sampled repeatedly cannot.

**Window bounds.** Windows are capped at **24 hours**, where two independent
limits meet. A GPS ground track repeats every sidereal day (~23 h 56 m), so a
longer window mostly redraws itself; and a window of at most 24 h touches at
most two daily broadcast files no matter where it starts. The minimum is 5
minutes.

**Step selection.** The step is the smallest value from
`{1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800}` seconds that keeps the
epoch count within 721. Restricting to values that divide a minute or an hour
makes epoch timestamps land on readable clock times rather than on 01:13:17.
721 is a whole number of steps for the durations people actually pick: 1 h at
5 s, 6 h at 30 s, 24 h at 2 min. A separate hard ceiling of 4096 epochs in the
core bounds the work a single call can request, so a malformed step fails
loudly instead of hanging the tab.

**Sparse tracks.** Each satellite's track carries only the epochs at which it
was above the mask, each tagged with its index into the shared epoch axis.

Sparseness is the point. A satellite rises and sets, and some rise twice within
a long window. A dense array would have to encode "not visible" as a sentinel,
and any consumer that forgot to check it would draw a line straight across the
sky between a set and the next rise. With indices, a break in the run *is* the
gap, and the drawing code is forced to look at it.

**Synchronisation.** Every array in a series indexes the same epoch axis. That
is what makes a single cursor synchronous *by construction*: the sky plot and
the DOP chart are handed the same integer, never two independently converted
timestamps.

---

## 11. Visualisation geometry

### 11.1 Sky plot projection

Azimuth is the polar angle, elevation the radius, with the zenith at the centre
and the horizon at the rim:

```
r = R (1 − el / 90°)
x = C + r sin(az)
y = C − r cos(az)
```

The sine and cosine are swapped relative to the usual mathematical convention,
and `y` is negated, because azimuth is measured clockwise from north while SVG
coordinates run right and *down*.

This is the **equidistant azimuthal** projection: elevation maps linearly to
radius. It preserves neither area nor angle. It is the standard choice for sky
plots because it makes the elevation mask a circle of constant radius and keeps
the horizon — where satellites are most marginal — at full resolution rather
than compressed at the rim.

### 11.2 Track segmentation

A track is split wherever its epoch index jumps. A gap means the satellite was
below the mask or had no ephemeris for at least one sample, so the stretches
either side are separate passes. Over a 24-hour window most MEO satellites set
and rise again, which is exactly the case that would otherwise draw a false
chord across the plot.

Single-sample runs are kept and drawn as dots: a satellite caught for one epoch
at the edge of the window is real, and silently dropping it would be worse than
a stray pixel.

### 11.3 Label placement

Markers sit at their true projected position, always. Labels do not: satellites
close together in azimuth and elevation would print overlapping and illegible.
WAAS's three geostationary satellites sit within ~17° of azimuth and ~5° of
elevation of each other from the continental US, and that was the case that
motivated the layout.

Each label is placed by a greedy outward search — candidate radii of 14 to 80
px, fanning out in azimuth by ±30° increments from a preferred direction —
taking the first position clear of every other satellite's marker and every
label already placed. A leader line is drawn when a label ends up far enough
away to need one. Labels are laid out in a stable input order, so the layout is
deterministic: the same input produces a byte-identical result, which is what
stops labels flickering as the cursor moves.

### 11.4 DOP chart

Horizontal and vertical DOP only. All five scalars are computed and available,
but plotting five overlapping lines answers no question anyone actually asks.
Horizontal and vertical are the two that behave differently (§9.3), and showing
the pair makes that asymmetry visible.

The y axis **autoscales rather than clipping**, with a floor of 3. Autoscaling
matters because a spike marks the geometry collapsing, which is the most
interesting thing the chart can show; the floor matters because a good sky sits
near HDOP 1, and scaling that tightly would turn millimetre wobble into
dramatic-looking peaks. The maximum is rounded up through 1/2/5 × 10ⁿ so the
axis lands on 3, 5, 10, 20, 50 rather than on 4.37.

Epochs with no solution break the line rather than plotting zero (§9.4).

---

## 12. Validation

### 12.1 Standard

**No source is switchable in the interface until it has an independent
cross-check appropriate to its geometry.** Three kinds exist, in increasing
order of what they establish.

**(a) A second implementation.** `tools/reference_skyplot.py` implements the
same algorithms while deliberately sharing no code and making the opposite
choice wherever one is available:

| | Rust core | Python reference |
| --- | --- | --- |
| RINEX parsing | the `rinex` crate | fixed-column slicing |
| Kepler's equation | Newton–Raphson | fixed-point iteration |
| Elevation | `atan2(U, horizontal)` | `asin(U / range)` |
| ENU transform | inlined dot products | explicit 3×3 matrix |
| DOP | normal matrix accumulated in place, Gauss–Jordan | explicit *n*×*k* design matrix, SVD pseudo-inverse |

Agreement between two implementations sharing no code is meaningful evidence;
agreement between a port and its original is not. The DOP route is worth
noting: for full-rank `A`, `pinv(A) · pinv(A)ᵀ = (Aᵀ A)⁻¹ = Q`, so the
reference reaches the same `Q` by an orthogonal factorisation rather than by
forming and inverting the normal matrix — a different algorithm with different
conditioning, not a transcription.

**(b) Closed-form geometry.** A geostationary satellite's look angles have an
exact analytic solution, so the SBAS path is checked against spherical
trigonometry rather than against code that could share a misconception. From
the observer's geodetic latitude `φ`, the satellite's sub-point `(φ_s, λ_s)`
and orbit radius `r`:

```
cos γ = sin φ sin φ_s + cos φ cos φ_s cos Δλ

el = atan( (cos γ − R_E/r) / sin γ )
az = atan2( cos φ_s sin Δλ,  cos φ sin φ_s − sin φ cos φ_s cos Δλ )
```

This never forms a position vector at all. It is written for a general
sub-latitude rather than assuming the equator, because several "geostationary"
satellites are not: GAGAN's PRN 127 sits 2.1° off the equatorial plane, which is
worth 3° of azimuth — an equator-only form would have to call that a failure
when the propagator is right and the assumption is wrong.

**(c) A second broadcast of the same spacecraft.** BDSBAS is carried on
BeiDou's geostationary satellites and MSAS on QZSS's, so the same physical
spacecraft appears in the same file twice: once as a Keplerian element set and
once as an ECEF state vector, fitted independently by different operators'
ground segments and propagated by two algorithms sharing no code.

This is the primary evidence for BeiDou's geostationary transformation (§7.3).
Extending the Python reference to cover that transformation — which was also
done — establishes less than it appears to, because both implementations would
be the same reading of the same ICD paragraph, and a shared misreading would
agree with itself perfectly. Full reasoning in
[ADR-0008](adr/0008-validation-by-dual-broadcast-satellites.md).

### 12.2 Results

**Keplerian constellations against the Python reference.**

| Case set | Coverage | Worst azimuth | Worst elevation | Worst range |
| --- | --- | --- | --- | --- |
| GPS | 143 satellites, 15 cases (5 CONUS sites × 3 epochs) | 3.1 × 10⁻¹³ ° | 9.9 × 10⁻¹⁴ ° | 1.1 × 10⁻⁸ m |
| GPS + Galileo + BeiDou + QZSS | 779 satellites, 36 cases (6 global sites × 2 epochs × 3 system combinations) | 1.1 × 10⁻⁹ ° | 4.0 × 10⁻¹⁰ ° | 6.0 × 10⁻⁵ m |

The global sites are Denver, London, Tokyo, Sydney, Nairobi and McMurdo.
McMurdo (77.8° S) is deliberate: at that latitude the MEO constellations never
pass overhead, so it is the case where geometry is worst and any
latitude-dependent sign error would show.

**DOP against the Python reference.**

| Case set | Values | Worst deviation |
| --- | --- | --- |
| GPS | 75 | 4.0 × 10⁻¹⁵ |
| Multi-constellation, 1–3 clock unknowns | 36 cases | 5.6 × 10⁻¹² |

Both implementations must also agree on **how many clock unknowns** the
solution carried. Agreeing on the numbers while disagreeing on the count would
mean agreeing by accident.

**SBAS against closed-form geometry.** All 15 satellites of the six providers
present in the fixture, each evaluated from an observer placed 20° north and 20°
west of its own sub-longitude, agree within 0.1° in azimuth and elevation. The
residual is dominated by the analytic form assuming a spherical Earth.

**The dual-broadcast pairs.**

| Keplerian | Augmentation | Slot | Separation |
| --- | --- | --- | --- |
| BeiDou C01 | BDSBAS PRN 130 | 140.1° E | 1.9 m |
| BeiDou C03 | BDSBAS PRN 143 | 110.5° E | 2.0 m |
| QZSS J07 | MSAS PRN 137 | 127.0° E | 0.7 m |
| QZSS J08 | MSAS PRN 129 | 90.5° E | 0.4 m |
| BeiDou C02 | BDSBAS PRN 144 | 80.1° E | **2826 m** |

The last row is not a failure. Its separation is *constant* — 2826 m and
2876 m at epochs half an hour apart — where a frame error would grow with time
from the reference epoch (a missing Earth-rotation term would move the
satellite ~2000 km over that span). BDS-2 and BDS-3 both fly geostationary
satellites at the same nominal slots, and operators separate co-located
satellites by a few kilometres deliberately. The test asserts both the tight
agreement for identical spacecraft and the constancy for all of them, which is
what distinguishes the two situations.

**Other closed-form anchors.** A four-satellite tetrahedron (one at the zenith,
three on the horizon 120° apart) is solved by hand and gives
`Q = (2/3, 2/3, 4/3, 1/3)` exactly. For the multi-system columns there is a
sharper identity: a system contributing exactly *one* satellite must leave
HDOP, VDOP, PDOP and TDOP bit-identical, because eliminating that system's
clock unknown by Schur complement subtracts precisely the outer product its own
row contributed. Only GDOP grows, by the new clock's variance. That is both an
analytic result and the sharpest available check that the clock columns are
wired to the right satellites — putting the same satellite in the *same* system
improves PDOP instead.

### 12.3 What the residuals actually are

The GPS agreement (~10⁻¹³ °) is floating-point ordering: two f64 evaluations of
the same closed-form expressions, differing only in the order of operations.

The multi-constellation agreement is four orders looser, and that difference is
**fully explained and is not an algorithmic disagreement**. Broken down by
constellation:

| Constellation | Worst azimuth deviation |
| --- | --- |
| GPS | 1.6 × 10⁻¹³ ° |
| QZSS | 1.8 × 10⁻¹² ° |
| Galileo | 1.1 × 10⁻⁹ ° |
| BeiDou | 1.1 × 10⁻⁹ ° |

The split follows exactly the duplicate-printing defect of §3.5. Every Galileo
and most BeiDou records appear twice in the file — once with 13 significant
digits, once in the legacy leading-dot form with 12 — and the two parsers
retain *different printings of the same ephemeris*. For Galileo E10 at
2026-08-08T11:00Z:

```
Rust    √A = 5440.60343552        (from  .544060343552e+04, 12 digits)
Python  √A = 5440.603435516       (from 5.440603435516e+03, 13 digits)
```

A relative difference of 7.4 × 10⁻¹³ in `√A` gives 1.5 × 10⁻¹² in `A`, which at
a 29 600 km orbit radius is 44 µm of position — matching the observed 3.9 ×
10⁻⁵ m range deviation. GPS and QZSS records are never duplicated, and agree
correspondingly better.

So the two implementations agree **to the limit of what the file states**, and
the residual is a property of the source data rather than of either program.
44 µm is also four to five orders of magnitude below the accuracy of the
broadcast ephemeris itself (§13), and therefore of no practical consequence.

### 12.4 What is not validated

- **Absolute orbit accuracy.** Everything here establishes that the tool
  evaluates the broadcast ephemeris correctly. It does not establish that the
  broadcast ephemeris is where the satellite is — see §13.
- **SDCM and KASS.** Neither has real data in the archive to check against
  (§3.2, §3.5), so both remain non-selectable. This is a data limitation, not a
  code limitation: they propagate by the same path as the validated providers.
- **The clock coefficients.** `a_f0`, `a_f1`, `a_f2` and the group delays are
  parsed and carried but unused, so nothing tests them. They matter from phase
  4 onward.
- **Anything outside the geometric model** — §1 lists it.

---

## 13. Error budget and limitations

Ordered by magnitude, largest first. The point of this ordering is that the
first two entries dominate everything below them by many orders of magnitude,
so effort spent tightening the numerics would be wasted.

| Source | Magnitude | Notes |
| --- | --- | --- |
| Broadcast ephemeris orbit error | ~1–5 m | The dominant term by far. Inherent to broadcast data; precise products are ~2 cm but are a different source (§3.1) |
| Extrapolation near the fit-interval edge | grows toward the 2 h limit | Reported per satellite as the signed ephemeris age, so a result leaning on a stale block is visible |
| Ellipsoidal vs orthometric height | −35 m to +5 m typical | The tool takes ellipsoidal height; entering a mean-sea-level figure introduces the geoid undulation |
| Neglected transit-time correction | ~0.0005° | Not neglected — this is what applying it changes (§7.5) |
| Numerical agreement of the two implementations | ~10⁻⁹ ° | §12.3; dominated by source-file precision |
| Kepler solver tolerance | < 1 µm | 10⁻¹² rad at orbit radius |

**Consequences for interpretation.**

An elevation angle from this tool is good to well under a thousandth of a
degree *given the broadcast ephemeris*, and the broadcast ephemeris is good to
a few metres — which at 20 000 km is about 10⁻⁵ degrees of angle. In other
words the geometry is far more accurate than any use of it requires, and the
limiting factor is the data source, not the arithmetic.

**Coverage boundaries.** Web Mercator does not draw the poles, so the map is
bounded at ±85°; the coordinate fields reach ±90° and the mathematics is valid
there. Longitudes are folded into [−180°, 180°) at the interface boundary
because panning past the edge of the world has the map library counting on into
a repeated copy of it.

**What DOP does and does not tell you.** It is computed over exactly the
satellites being plotted, so switching a source off removes it from the
geometry as well. That is the least surprising behaviour, but it means the
numbers are "DOP for the selected sources", not "DOP this receiver would
achieve". A real receiver's achieved DOP also depends on which signals it
tracks and which it chooses to use.

---

## 14. Symbols

| Symbol | Meaning | Units |
| --- | --- | --- |
| `√A`, `A` | Square root of semi-major axis; semi-major axis | √m, m |
| `e` | Orbital eccentricity | — |
| `i₀`, `i̇`, `i_k` | Inclination at reference epoch, its rate, corrected value | rad, rad/s, rad |
| `Ω₀`, `Ω̇`, `Ω_k` | Longitude of ascending node at weekly epoch, its rate, corrected value | rad, rad/s, rad |
| `ω` | Argument of perigee | rad |
| `M₀`, `M_k` | Mean anomaly at reference epoch, at `t` | rad |
| `E_k` | Eccentric anomaly | rad |
| `ν_k` | True anomaly | rad |
| `Φ_k`, `u_k` | Argument of latitude, corrected | rad |
| `r_k` | Corrected orbit radius | m |
| `Δn`, `n₀`, `n` | Mean motion difference, computed, corrected | rad/s |
| `C_uc`, `C_us` | Harmonic corrections to argument of latitude | rad |
| `C_rc`, `C_rs` | Harmonic corrections to orbit radius | m |
| `C_ic`, `C_is` | Harmonic corrections to inclination | rad |
| `t_oe`, `t_oe^sow` | Time of ephemeris: absolute, and as raw seconds-of-week | s |
| `t_k` | `t − t_oe` | s |
| `a_f0`, `a_f1`, `a_f2` | Satellite clock bias, drift, drift rate | s, s/s, s/s² |
| `μ` | Gravitational constant of the Earth | m³/s² |
| `Ω̇ₑ` | Earth rotation rate | rad/s |
| `F` | Relativistic clock-correction constant | s/√m |
| `c` | Speed of light in vacuum | m/s |
| `τ` | Signal transit time | s |
| `φ`, `λ`, `h` | Geodetic latitude, longitude, ellipsoidal height | deg, deg, m |
| `E`, `N`, `U` | Local east, north, up components | m |
| `A` (matrix) | DOP design matrix | — |
| `Q` | `(Aᵀ A)⁻¹` | — |

---

## 15. References

**Interface control documents**

- IS-GPS-200N, *Navstar GPS Space Segment / Navigation User Interfaces*.
  Table 20-IV (satellite position from broadcast ephemeris); §20.3.3.4.3.1
  (user algorithm); §20.3.4.4 (curve-fit interval); §20.3.4.3 (speed of light).
- *European GNSS (Galileo) Open Service Signal-in-Space Interface Control
  Document*, issue 2.1, §5.1.1 (satellite position) and §5.1.3 (constants).
- *BeiDou Navigation Satellite System Signal In Space Interface Control
  Document, Open Service Signal B1I*, version 3.0, §5.2.4.12 — including the
  geostationary transformation.
- *Quasi-Zenith Satellite System Interface Specification: Satellite Positioning,
  Navigation and Timing Service* (IS-QZSS-PNT), §4.1.2.3 (health word).
- RTCA DO-229, *Minimum Operational Performance Standards for GPS/WAAS Airborne
  Equipment*, message type 9 (GEO navigation message).

**Formats and frames**

- *RINEX: The Receiver Independent Exchange Format*, version 3.05. §6.10 covers
  the GEO navigation message record.
- NIMA TR8350.2, *Department of Defense World Geodetic System 1984*, third
  edition — ellipsoid parameters.
- B. R. Bowring, "Transformation from spatial to geographical coordinates",
  *Survey Review* 23(181), 1976 — the ECEF-to-geodetic method of §5.3.

**Data**

- BKG IGS mirror of the merged broadcast ephemeris product:
  <https://igs.bkg.bund.de/root_ftp/IGS/BRDC/>

**This project**

- [ADR-0004](adr/0004-rinex-nav-over-celestrak-tles.md) — why broadcast
  ephemeris rather than TLEs.
- [ADR-0005](adr/0005-hand-rolled-ephemeris-propagation.md) — why the `rinex`
  crate is used only as a parser.
- [ADR-0006](adr/0006-server-side-ephemeris-proxy.md) — why a server-side fetch
  step exists at all.
- [ADR-0007](adr/0007-inter-system-clock-unknowns.md) — one clock unknown per
  time system.
- [ADR-0008](adr/0008-validation-by-dual-broadcast-satellites.md) — validating
  geostationary propagation against a second broadcast.
