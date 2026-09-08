/**
 * Sky-plot label layout invariants.
 *
 * Run with `npm test` (Node's built-in runner; Node 24 strips the types, so
 * this needs no test framework or transpiler).
 *
 * The point of these tests is phase 2. A time-series sky plot will push
 * satellites through clustered and crossing configurations continuously
 * rather than at one static instant, and a label layout that only happens to
 * work for today's geometry would fail silently. So the invariants are
 * asserted against adversarial inputs, not just a real epoch.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  CANVAS_MARGIN,
  MARKER_RADIUS,
  SIZE,
  layoutLabels,
  project,
  rectsOverlap,
  type LabelPlacement,
} from "./skyPlotLayout.ts";
import type { Satellite } from "./types.ts";

function satellite(
  sv: string,
  azimuth: number,
  elevation: number,
  geostationary = sv.startsWith("S"),
): Satellite {
  return {
    sv,
    prn: Number(sv.slice(1)),
    source: sv.startsWith("S") ? "WAAS" : "GPS",
    azimuth,
    elevation,
    rangeKm: 22_000,
    ephemerisAgeS: 0,
    geostationary,
  };
}

/** Marker bounding box, matching what the component draws. */
function markerRect(placement: LabelPlacement) {
  return {
    x: placement.marker.x - MARKER_RADIUS,
    y: placement.marker.y - MARKER_RADIUS,
    w: MARKER_RADIUS * 2,
    h: MARKER_RADIUS * 2,
  };
}

function labelPairOverlaps(placements: LabelPlacement[]): string[] {
  const clashes: string[] = [];
  for (let i = 0; i < placements.length; i += 1) {
    for (let j = i + 1; j < placements.length; j += 1) {
      if (rectsOverlap(placements[i].label, placements[j].label)) {
        clashes.push(`${placements[i].satellite.sv}/${placements[j].satellite.sv}`);
      }
    }
  }
  return clashes;
}

/** Labels covering *another* satellite's marker hide a real data point. */
function labelsCoveringForeignMarkers(placements: LabelPlacement[]): string[] {
  const clashes: string[] = [];
  placements.forEach((placement, i) => {
    placements.forEach((other, j) => {
      if (i !== j && rectsOverlap(placement.label, markerRect(other))) {
        clashes.push(`${placement.satellite.sv} covers ${other.satellite.sv}`);
      }
    });
  });
  return clashes;
}

function offCanvas(placements: LabelPlacement[]): string[] {
  return placements
    .filter(
      ({ label }) =>
        label.x < CANVAS_MARGIN ||
        label.y < CANVAS_MARGIN ||
        label.x + label.w > SIZE - CANVAS_MARGIN ||
        label.y + label.h > SIZE - CANVAS_MARGIN,
    )
    .map((p) => p.satellite.sv);
}

/** The real 2026-08-08 Denver sky: eight GPS plus the three WAAS GEOs. */
const DENVER_EPOCH: Satellite[] = [
  satellite("G05", 173.16, 58.25),
  satellite("G06", 62.12, 9.15),
  satellite("G11", 51.43, 44.07),
  satellite("G12", 194.3, 49.6),
  satellite("G18", 253.51, 9.26),
  satellite("G21", 85.39, 61.01),
  satellite("G25", 259.65, 54.74),
  satellite("G29", 307.15, 38.32),
  satellite("S131", 198.41, 42.39),
  satellite("S133", 215.1, 37.7),
  satellite("S135", 209.71, 39.56),
];

describe("projection", () => {
  it("puts the zenith at the centre and the horizon on the rim", () => {
    const zenith = project(0, 90);
    assert.equal(Math.round(zenith.x), SIZE / 2);
    assert.equal(Math.round(zenith.y), SIZE / 2);

    // Due north on the horizon is straight up in SVG coordinates.
    const north = project(0, 0);
    assert.equal(Math.round(north.x), SIZE / 2);
    assert.ok(north.y < SIZE / 2);
  });

  it("increases azimuth clockwise through east", () => {
    const east = project(90, 0);
    const west = project(270, 0);
    assert.ok(east.x > SIZE / 2, "east should be right of centre");
    assert.ok(west.x < SIZE / 2, "west should be left of centre");
  });
});

describe("label layout", () => {
  it("labels every satellite exactly once", () => {
    const placements = layoutLabels(DENVER_EPOCH);
    assert.equal(placements.length, DENVER_EPOCH.length);
    assert.deepEqual(
      placements.map((p) => p.label.text),
      DENVER_EPOCH.map((s) => s.sv),
    );
  });

  it("includes the constellation code in the label", () => {
    const placements = layoutLabels(DENVER_EPOCH);
    assert.ok(placements.every((p) => /^[A-Z]\d+$/.test(p.label.text)));
    assert.ok(placements.some((p) => p.label.text.startsWith("G")));
    assert.ok(placements.some((p) => p.label.text.startsWith("S")));
  });

  it("keeps labels clear of one another on a real epoch", () => {
    const placements = layoutLabels(DENVER_EPOCH);
    assert.deepEqual(labelPairOverlaps(placements), []);
  });

  it("never covers another satellite's marker", () => {
    const placements = layoutLabels(DENVER_EPOCH);
    assert.deepEqual(labelsCoveringForeignMarkers(placements), []);
  });

  it("keeps every label on the canvas", () => {
    assert.deepEqual(offCanvas(layoutLabels(DENVER_EPOCH)), []);
  });

  /**
   * The case that motivated all this: WAAS's three geostationary satellites
   * are only ~17 deg of azimuth and ~5 deg of elevation apart from CONUS, so
   * their markers nearly coincide.
   */
  it("separates the tightly clustered WAAS satellites", () => {
    const waas = DENVER_EPOCH.filter((s) => s.sv.startsWith("S"));
    const placements = layoutLabels(waas);

    assert.deepEqual(labelPairOverlaps(placements), []);
    assert.ok(
      placements.every((p) => p.placed),
      "all three should find a clean position, not the fallback",
    );
  });

  it("survives satellites at identical positions", () => {
    // Degenerate but reachable: two satellites projecting to the same point.
    const stacked = [
      satellite("G01", 120, 40),
      satellite("G02", 120, 40),
      satellite("G03", 120, 40),
      satellite("S131", 120, 40),
    ];
    const placements = layoutLabels(stacked);
    assert.equal(placements.length, 4);
    assert.deepEqual(labelPairOverlaps(placements), []);
    assert.deepEqual(offCanvas(placements), []);
  });

  it("handles a dense all-constellations sky", () => {
    // 32 satellites on a coarse spiral: denser than any real single epoch,
    // and a reasonable stand-in for a phase-2 frame with many tracks visible.
    const dense: Satellite[] = Array.from({ length: 32 }, (_, i) =>
      satellite(
        `G${String(i + 1).padStart(2, "0")}`,
        (i * 360) / 32,
        5 + ((i * 17) % 80),
      ),
    );
    const placements = layoutLabels(dense);

    assert.equal(placements.length, 32);
    assert.deepEqual(offCanvas(placements), []);
    // Under this much pressure a few labels may fall back, but the great
    // majority must still find clean positions.
    const cleanlyPlaced = placements.filter((p) => p.placed).length;
    assert.ok(
      cleanlyPlaced >= 28,
      `expected >=28 of 32 cleanly placed, got ${cleanlyPlaced}`,
    );
  });

  it("is deterministic, so labels do not flicker between frames", () => {
    // Phase 2 re-lays out on every time step; identical input must give an
    // identical layout or labels will jump around as the animation runs.
    const a = layoutLabels(DENVER_EPOCH);
    const b = layoutLabels(DENVER_EPOCH);
    assert.deepEqual(
      a.map((p) => [p.label.text, p.label.x, p.label.y]),
      b.map((p) => [p.label.text, p.label.x, p.label.y]),
    );
  });

  it("draws a leader line only when the label is pushed away", () => {
    // A lone satellite has no competition, so its label sits adjacent and
    // needs no line.
    const [solo] = layoutLabels([satellite("G01", 45, 45)]);
    assert.equal(solo.leader, false);

    // Clustered satellites must be displaced, so at least one needs a line.
    const clustered = layoutLabels(DENVER_EPOCH.filter((s) => s.sv.startsWith("S")));
    assert.ok(clustered.some((p) => p.leader));
  });

  it("copes with an empty sky", () => {
    assert.deepEqual(layoutLabels([]), []);
  });
});
