# ADR-0007: One clock unknown per time system in the DOP solution

- **Date:** 2026-09-07
- **Status:** Accepted
- **Deciders:** Claude (implementation), at the user's request

> Written during implementation, not back-filled. Please correct it if you
> disagree with the reasoning.

## Context

Phase 2 added dilution of precision. Its design matrix carried three position
columns and **one** clock column:

```text
    [ -e_i  -n_i  -u_i  1 ]
```

That is the textbook single-system formulation, and it was correct for what
phase 2 shipped, because everything it shipped ran on GPS time. SBAS is
GPS-time coherent by design — a WAAS satellite's ranging signal is solved in
GPS time alongside the GPS satellites — so GPS + WAAS is genuinely one clock.

Phase 5 switches on Galileo, BeiDou and QZSS. Two of those are not on GPS
time:

- **Galileo System Time** is steered close to GPS time, but the offset is a
  *broadcast quantity* (the GPS-to-Galileo Time Offset), not zero. A receiver
  combining the two estimates it.
- **BeiDou Time** is a separate scale entirely, running 14 s behind GPS time.
- **QZSS** is the exception among the additions: QZSST is steered to GPS time
  and QZSS is specified for GPS-interoperable use, so it belongs with GPS.

The consequence of ignoring this is not a small bias. A single clock column
asserts that all the pseudoranges share one unknown offset, which makes the
system look better determined than it is: four satellites split two-and-two
between GPS and Galileo would report a DOP where a real receiver has no fix at
all. The number would be plausible, and wrong, at exactly the moments a
precision chart is being read.

`CLAUDE.md` recorded this before the work started: "Each added system needs its
own inter-system-bias column, which also raises the minimum satellite count by
one. Make that change in `design_matrix`, not at call sites."

## Decision

We will carry **one clock column per time system present in the solution**, and
group constellations into time systems rather than treating each constellation
separately.

A `TimeSystem` enum sits beside `Constellation`, with `Constellation::time_system()`
mapping GPS, QZSS and SBAS to one system and giving Galileo and BeiDou their
own. `dop_from_observations` builds the columns from the systems actually
present, so a GPS-only sky still solves four unknowns and a GPS + Galileo +
BeiDou sky solves six.

Three rules follow, and are enforced in `dop.rs`:

- The minimum satellite count is `3 + systems`, not 4. Below it, the answer is
  `None` — no solution — never a number.
- Column order is the enum's order, so the layout is a function of the *set* of
  systems present and not of how the caller sorted its satellites. `TDOP`
  reports the lowest system present, which is GPS whenever GPS is in the
  solution.
- `Dop::systems` is part of the result and crosses the WASM boundary, because
  the count changes what the other numbers mean.

## Alternatives considered

### Keep one clock column and accept the approximation

Defensible on the face of it: GST is held within nanoseconds of GPS time, so
the bias being ignored is small. It fails on the part that matters. The error
is not in the *value* of the DOP — it is that the system is under-determined
and the code does not know it. A four-satellite two-system sky inverts happily
and returns a confident number. Reporting no solution where there is none is
the whole reason `dop.rs` returns `Option`; keeping one column would quietly
undo that for exactly the multi-constellation case the phase exists to add.

### One clock column per constellation

Simpler to write — no grouping table, no `TimeSystem` enum — and it is what a
reader might expect. But it is wrong in the direction that costs satellites: it
would open a separate unknown for SBAS and for QZSS, both of which are on GPS
time by construction. Under that rule, four GPS satellites plus a WAAS GEO
would report *no solution*, when a real receiver uses exactly that geometry.
The grouping is the physics; the constellation list is the packaging.

### Solve the inter-system bias externally and subtract it

A real receiver can apply the broadcast GGTO instead of estimating the offset,
which would keep one clock column honest. It is the right thing to do
eventually, and it is not available here: this project computes *geometry* from
broadcast ephemeris and has no measurements to correct. DOP is a statement
about how geometry amplifies error, and a receiver that estimates the bias is
the conservative assumption — the one that cannot claim precision it does not
have.

## Consequences

- **Positive:** DOP is correct for any combination of the constellations the
  app now offers, and says so: `systems` is displayed, so a reader can see that
  a GDOP over three clocks is not comparable with a single-system one.
- **Positive:** Adding a constellation is again a table entry.
  `Constellation::time_system()` is the one place a new system declares whose
  clock it shares.
- **Negative:** Enabling a second constellation can make DOP *worse* than
  leaving it off, because its satellites bring an unknown with them. That is
  real — it is why a receiver may ignore two satellites from a second
  system — but it is counter-intuitive on a chart, and the UI does not
  currently explain it beyond showing the clock count.
- **Negative:** The normal matrix is now up to 6x6 with a runtime dimension,
  where it was a fixed 4x4. The inversion is a little more code and no longer
  a candidate for full unrolling. At one solve per epoch over a few hundred
  epochs this is not measurable.
- **Neutral / follow-ups:** GLONASS would be a fourth time system if it is ever
  added, which is a one-line change here and a whole propagator elsewhere. If
  phase 4 grows real pseudoranges, applying the broadcast GGTO instead of
  estimating it becomes possible and this decision is worth revisiting.
