/**
 * Layout for the HDOP/VDOP-against-time chart.
 *
 * Shares the series' epoch axis with the sky plot rather than carrying its own
 * time base. That is what makes the single slider genuinely synchronous: both
 * views are told the same integer index, so they cannot drift apart even by a
 * rounding error.
 *
 * Kept free of React so the scaling rules — particularly how gaps and spikes
 * are handled — can be tested directly.
 */

import type { Dop, SkySeries } from "./types.ts";

export const WIDTH = 440;
export const HEIGHT = 168;

export const MARGIN = { top: 10, right: 10, bottom: 22, left: 38 } as const;

export const PLOT_WIDTH = WIDTH - MARGIN.left - MARGIN.right;
export const PLOT_HEIGHT = HEIGHT - MARGIN.top - MARGIN.bottom;

/**
 * Floor for the y axis.
 *
 * A good sky sits near HDOP 1, and autoscaling that tightly would turn
 * millimetre wobble into dramatic-looking peaks. Three is comfortably above a
 * healthy day's values, so a flat line reads as flat.
 */
const MIN_Y_MAX = 3;

export interface DopSeriesSpec {
  key: "hdop" | "vdop";
  label: string;
  colour: string;
}

/**
 * Horizontal and vertical only.
 *
 * PDOP/GDOP/TDOP are computed and available, but plotting five overlapping
 * lines answers no question a user actually asks. Horizontal and vertical are
 * the two that behave differently — a ground receiver never sees satellites
 * below itself, so vertical precision is structurally worse — and showing the
 * pair makes that asymmetry visible.
 */
export const DOP_SERIES: DopSeriesSpec[] = [
  { key: "hdop", label: "HDOP", colour: "#2563eb" },
  { key: "vdop", label: "VDOP", colour: "#dc2626" },
];

export interface ChartPoint {
  x: number;
  y: number;
  epochIndex: number;
  value: number;
}

export interface ChartSeries extends DopSeriesSpec {
  /** Unbroken runs. A break is an epoch with no DOP solution. */
  segments: ChartPoint[][];
}

export interface DopChartLayout {
  series: ChartSeries[];
  /** Upper bound of the y axis. */
  yMax: number;
  yTicks: number[];
  /** Epoch indices to label on the x axis. */
  xTicks: number[];
  /** True if any epoch in the window had no DOP solution. */
  hasGaps: boolean;
  /** Largest plotted value, for reporting alongside the chart. */
  peak: number | null;
}

/** Pixel x for an epoch index. */
export function xForEpoch(epochIndex: number, epochCount: number): number {
  if (epochCount <= 1) return MARGIN.left;
  return MARGIN.left + (epochIndex / (epochCount - 1)) * PLOT_WIDTH;
}

/** Pixel y for a DOP value. */
export function yForValue(value: number, yMax: number): number {
  const clamped = Math.min(Math.max(value, 0), yMax);
  return MARGIN.top + PLOT_HEIGHT * (1 - clamped / yMax);
}

/**
 * Round an axis maximum up to something a human would have chosen.
 *
 * Walks 1/2/5 x 10^n so the axis lands on 3, 5, 10, 20, 50 rather than on
 * 4.37 — the labels have to be readable at a glance for the chart to be worth
 * having.
 */
function niceCeiling(value: number): number {
  if (value <= MIN_Y_MAX) return MIN_Y_MAX;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  for (const multiple of [1, 2, 5, 10]) {
    const candidate = multiple * magnitude;
    if (candidate >= value) return candidate;
  }
  return 10 * magnitude;
}

function ticksUpTo(yMax: number): number[] {
  // Four intervals reads as a grid without crowding a 136 px plot area.
  const step = yMax / 4;
  return [0, step, step * 2, step * 3, yMax];
}

/**
 * Epoch indices to label along the x axis.
 *
 * Always includes both ends so the window's bounds are readable off the chart.
 */
function chooseXTicks(epochCount: number): number[] {
  if (epochCount <= 1) return [0];
  const wanted = Math.min(5, epochCount);
  const ticks: number[] = [];
  for (let i = 0; i < wanted; i += 1) {
    ticks.push(Math.round((i / (wanted - 1)) * (epochCount - 1)));
  }
  return [...new Set(ticks)];
}

/**
 * Build the chart geometry for a series.
 *
 * Epochs with no DOP solution — fewer than four satellites, or degenerate
 * geometry — break the line rather than plotting as zero. Drawing a gap as
 * `0` would read as *perfect* precision at exactly the moments when there is
 * no fix at all, which is the most misleading thing this chart could do.
 */
export function dopChartLayout(series: SkySeries): DopChartLayout {
  const epochCount = series.epochs.length;

  let peak = 0;
  let hasGaps = false;
  for (const entry of series.dop) {
    if (!entry) {
      hasGaps = true;
      continue;
    }
    peak = Math.max(peak, entry.hdop, entry.vdop);
  }

  // Autoscale to the data rather than clipping at a fixed ceiling. A DOP spike
  // is the single most interesting feature this chart can show — it marks a
  // moment the geometry collapses — so it must never be silently cut off.
  const yMax = niceCeiling(peak);

  const built: ChartSeries[] = DOP_SERIES.map((spec) => {
    const segments: ChartPoint[][] = [];
    let run: ChartPoint[] = [];

    series.dop.forEach((entry: Dop | null, epochIndex: number) => {
      if (!entry) {
        if (run.length) segments.push(run);
        run = [];
        return;
      }
      const value = entry[spec.key];
      run.push({
        x: xForEpoch(epochIndex, epochCount),
        y: yForValue(value, yMax),
        epochIndex,
        value,
      });
    });
    if (run.length) segments.push(run);

    return { ...spec, segments };
  });

  return {
    series: built,
    yMax,
    yTicks: ticksUpTo(yMax),
    xTicks: chooseXTicks(epochCount),
    hasGaps,
    peak: peak > 0 ? peak : null,
  };
}
