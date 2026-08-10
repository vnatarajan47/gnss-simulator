/**
 * DOP chart layout invariants.
 *
 * The one that matters most: an epoch with no DOP solution must break the line,
 * never plot as zero. Zero on a precision chart reads as *perfect* precision at
 * exactly the moments when there is no fix at all — the most actively
 * misleading thing this chart could do.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  DOP_SERIES,
  MARGIN,
  PLOT_HEIGHT,
  WIDTH,
  dopChartLayout,
  xForEpoch,
  yForValue,
} from "./dopChart.ts";
import type { Dop, SkySeries } from "./types.ts";

function dop(hdop: number, vdop: number): Dop {
  return { gdop: 2, pdop: 1.8, hdop, vdop, tdop: 0.9, satellites: 9 };
}

function series(values: (Dop | null)[]): SkySeries {
  return {
    epochs: values.map((_, i) => 1_786_190_400 + i * 60),
    tracks: [],
    dop: values,
    visible: values.map((v) => (v ? v.satellites : 0)),
    epochsWithoutEphemeris: 0,
  };
}

describe("scales", () => {
  it("maps the first and last epoch to the plot edges", () => {
    assert.equal(xForEpoch(0, 10), MARGIN.left);
    assert.equal(xForEpoch(9, 10), WIDTH - MARGIN.right);
  });

  it("puts zero at the bottom and the maximum at the top", () => {
    assert.equal(yForValue(0, 5), MARGIN.top + PLOT_HEIGHT);
    assert.equal(yForValue(5, 5), MARGIN.top);
  });

  it("survives a single-epoch series", () => {
    assert.equal(xForEpoch(0, 1), MARGIN.left);
  });
});

describe("axis scaling", () => {
  it("does not autoscale a healthy day into fake drama", () => {
    // Real HDOP hovers near 1. Scaling tightly to that would turn ordinary
    // wobble into alarming-looking peaks.
    const layout = dopChartLayout(series([dop(0.9, 1.3), dop(1.0, 1.4), dop(0.95, 1.35)]));
    assert.ok(layout.yMax >= 3, `expected a floor of 3, got ${layout.yMax}`);
  });

  it("never clips a spike", () => {
    // A DOP spike marks the geometry collapsing — the single most interesting
    // thing this chart can show. It must not be cut off by a fixed ceiling.
    const layout = dopChartLayout(series([dop(1, 1.4), dop(37, 44), dop(1, 1.4)]));
    assert.ok(layout.yMax >= 44, `axis ${layout.yMax} would clip the peak`);
    assert.equal(layout.peak, 44);

    for (const built of layout.series) {
      for (const segment of built.segments) {
        for (const point of segment) {
          assert.ok(
            point.y >= MARGIN.top - 1e-9,
            "a plotted point must stay inside the plot area",
          );
        }
      }
    }
  });

  it("chooses round axis maxima", () => {
    for (const [peak, expected] of [
      [1, 3],
      [4, 5],
      [7, 10],
      [12, 20],
      [44, 50],
    ] as const) {
      assert.equal(dopChartLayout(series([dop(peak, peak)])).yMax, expected);
    }
  });
});

describe("gaps", () => {
  it("breaks the line where there is no solution", () => {
    const layout = dopChartLayout(
      series([dop(1, 1.4), dop(1.1, 1.5), null, null, dop(1.2, 1.6)]),
    );
    assert.equal(layout.hasGaps, true);

    for (const built of layout.series) {
      assert.equal(built.segments.length, 2, `${built.key} should be split in two`);
      assert.deepEqual(
        built.segments.map((s) => s.map((p) => p.epochIndex)),
        [[0, 1], [4]],
      );
    }
  });

  it("never emits a point for a missing epoch", () => {
    // The zero-plotting failure, asserted directly.
    const layout = dopChartLayout(series([dop(1, 1.4), null, dop(1, 1.4)]));
    for (const built of layout.series) {
      const plotted = built.segments.flat().map((p) => p.epochIndex);
      assert.ok(!plotted.includes(1), "epoch 1 has no solution and must not be plotted");
      assert.ok(built.segments.flat().every((p) => p.value > 0));
    }
  });

  it("handles a series with no solution anywhere", () => {
    const layout = dopChartLayout(series([null, null, null]));
    assert.equal(layout.hasGaps, true);
    assert.equal(layout.peak, null);
    for (const built of layout.series) {
      assert.deepEqual(built.segments, []);
    }
    // Still needs a usable axis so the empty chart renders rather than throwing.
    assert.ok(layout.yMax >= 3);
    assert.ok(Number.isFinite(layout.yMax));
  });

  it("reports no gaps for a complete series", () => {
    assert.equal(dopChartLayout(series([dop(1, 1.4), dop(1, 1.4)])).hasGaps, false);
  });
});

describe("series selection", () => {
  it("plots horizontal and vertical", () => {
    const layout = dopChartLayout(series([dop(1, 1.4)]));
    assert.deepEqual(layout.series.map((s) => s.key), ["hdop", "vdop"]);
    assert.equal(layout.series.length, DOP_SERIES.length);
  });

  it("reads the value each series names", () => {
    const layout = dopChartLayout(series([dop(1.23, 4.56)]));
    assert.equal(layout.series[0].segments[0][0].value, 1.23);
    assert.equal(layout.series[1].segments[0][0].value, 4.56);
  });
});

describe("x ticks", () => {
  it("always labels both ends of the window", () => {
    const layout = dopChartLayout(series(Array.from({ length: 100 }, () => dop(1, 1.4))));
    assert.equal(layout.xTicks[0], 0);
    assert.equal(layout.xTicks[layout.xTicks.length - 1], 99);
  });

  it("does not repeat ticks on a very short series", () => {
    const layout = dopChartLayout(series([dop(1, 1.4), dop(1, 1.4)]));
    assert.deepEqual(layout.xTicks, [...new Set(layout.xTicks)]);
  });
});
