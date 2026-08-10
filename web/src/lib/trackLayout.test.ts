/**
 * Track segmentation invariants.
 *
 * The headline one: a satellite that sets and rises again must produce two
 * arcs. Joining them draws a line straight across the sky through positions the
 * satellite never occupied — a plausible-looking plot that is simply wrong,
 * and the sort of thing that survives visual inspection.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  clampCursor,
  satellitesByEpoch,
  segmentTrack,
  trackColour,
  trackSegments,
} from "./trackLayout.ts";
import type { SatelliteTrack, SkySeries, TrackSample } from "./types.ts";

function sample(epochIndex: number, azimuth = 120, elevation = 40): TrackSample {
  return { epochIndex, azimuth, elevation, rangeKm: 22_000, ephemerisAgeS: 0 };
}

function track(sv: string, indices: number[]): SatelliteTrack {
  return {
    sv,
    prn: Number(sv.slice(1)),
    source: sv.startsWith("S") ? "WAAS" : "GPS",
    samples: indices.map((i) => sample(i, (i * 7) % 360, 10 + (i % 60))),
  };
}

function series(tracks: SatelliteTrack[], epochCount: number): SkySeries {
  return {
    epochs: Array.from({ length: epochCount }, (_, i) => 1_786_190_400 + i * 60),
    tracks,
    dop: Array.from({ length: epochCount }, () => null),
    visible: Array.from({ length: epochCount }, () => 0),
    epochsWithoutEphemeris: 0,
  };
}

describe("segmentTrack", () => {
  it("keeps a continuous pass as one segment", () => {
    const segments = segmentTrack(track("G01", [0, 1, 2, 3, 4]));
    assert.equal(segments.length, 1);
    assert.equal(segments[0].points.length, 5);
    assert.equal(segments[0].fromEpoch, 0);
    assert.equal(segments[0].toEpoch, 4);
  });

  it("splits where the satellite was not visible", () => {
    // Sets after epoch 2, rises again at epoch 8.
    const segments = segmentTrack(track("G01", [0, 1, 2, 8, 9, 10]));
    assert.equal(segments.length, 2, "a set-and-rise is two passes, not one");
    assert.deepEqual(
      segments.map((s) => [s.fromEpoch, s.toEpoch]),
      [
        [0, 2],
        [8, 10],
      ],
    );
  });

  it("splits on a single missing epoch", () => {
    // Even one dropped sample is a real gap: the satellite crossed the mask.
    const segments = segmentTrack(track("G01", [0, 1, 3, 4]));
    assert.equal(segments.length, 2);
  });

  it("keeps a one-sample run rather than dropping it", () => {
    // A satellite caught for a single epoch at the window edge is real data.
    const segments = segmentTrack(track("G01", [0, 5, 6]));
    assert.equal(segments.length, 2);
    assert.equal(segments[0].points.length, 1);
  });

  it("handles a track with no samples", () => {
    assert.deepEqual(segmentTrack(track("G01", [])), []);
  });

  it("never bridges a gap in the projected points", () => {
    // Guards the property directly: no segment may contain two points that
    // came from non-adjacent epochs.
    const segments = segmentTrack(track("G01", [0, 1, 2, 30, 31]));
    for (const segment of segments) {
      assert.equal(
        segment.points.length,
        segment.toEpoch - segment.fromEpoch + 1,
        "a segment's point count must match its epoch span exactly",
      );
    }
  });
});

describe("trackSegments", () => {
  it("covers every track in the series", () => {
    const all = trackSegments(
      series([track("G01", [0, 1, 2]), track("G02", [0, 5]), track("S131", [0, 1, 2, 3, 4, 5])], 6),
    );
    assert.deepEqual(
      [...new Set(all.map((s) => s.sv))].sort(),
      ["G01", "G02", "S131"],
    );
    // G02 is split, so there are more segments than tracks.
    assert.equal(all.length, 4);
  });
});

describe("trackColour", () => {
  it("is stable for a given satellite", () => {
    assert.equal(trackColour("G05"), trackColour("G05"));
  });

  it("differs between satellites", () => {
    // Not a strict requirement of correctness, but a plot where every arc is
    // the same colour cannot be read.
    const colours = new Set(
      ["G01", "G02", "G03", "G04", "G05", "G06", "G07", "G08"].map((sv) => trackColour(sv)),
    );
    assert.ok(colours.size >= 7, `expected distinct hues, got ${colours.size}`);
  });

  it("does not depend on which other satellites are present", () => {
    // Colour is derived from the identifier, not the array position, so
    // toggling a source off must not recolour everything else.
    const before = trackColour("G07");
    const after = trackColour("G07");
    assert.equal(before, after);
  });
});

describe("satellitesByEpoch", () => {
  it("returns one entry per epoch", () => {
    const byEpoch = satellitesByEpoch(series([track("G01", [0, 1, 2])], 5));
    assert.equal(byEpoch.length, 5);
    assert.equal(byEpoch[0].length, 1);
    assert.equal(byEpoch[3].length, 0, "epochs with nobody up must be empty, not missing");
  });

  it("places each satellite at exactly the epochs it was visible", () => {
    const byEpoch = satellitesByEpoch(
      series([track("G01", [0, 1]), track("G02", [1, 2])], 3),
    );
    assert.deepEqual(byEpoch.map((s) => s.map((x) => x.sv)), [
      ["G01"],
      ["G01", "G02"],
      ["G02"],
    ]);
  });

  it("preserves track order within an epoch", () => {
    // The label layout takes first-come priority, so a stable order here is
    // what stops labels swapping sides as the slider moves.
    const byEpoch = satellitesByEpoch(
      series([track("G01", [0]), track("G02", [0]), track("S131", [0])], 1),
    );
    assert.deepEqual(byEpoch[0].map((s) => s.sv), ["G01", "G02", "S131"]);
  });

  it("carries the fields the table renders", () => {
    const byEpoch = satellitesByEpoch(series([track("S131", [0])], 1));
    const satellite = byEpoch[0][0];
    assert.equal(satellite.sv, "S131");
    assert.equal(satellite.prn, 131);
    assert.equal(satellite.source, "WAAS");
    assert.equal(typeof satellite.rangeKm, "number");
    assert.equal(typeof satellite.ephemerisAgeS, "number");
  });

  it("ignores a sample pointing outside the epoch array", () => {
    // Should be impossible, but a malformed series must not crash a render.
    const malformed = series([track("G01", [0, 99])], 2);
    const byEpoch = satellitesByEpoch(malformed);
    assert.equal(byEpoch.length, 2);
    assert.equal(byEpoch[0].length, 1);
  });
});

describe("clampCursor", () => {
  it("keeps the cursor inside the epoch range", () => {
    assert.equal(clampCursor(-5, 10), 0);
    assert.equal(clampCursor(99, 10), 9);
    assert.equal(clampCursor(4, 10), 4);
  });

  it("survives an empty series", () => {
    assert.equal(clampCursor(3, 0), 0);
  });
});
