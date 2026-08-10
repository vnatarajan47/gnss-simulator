/**
 * Turning a sampled series into things the sky plot can draw.
 *
 * Separate from the component for the same reason `skyPlotLayout` is: these are
 * the parts with invariants worth pinning down. The one that matters most is
 * that a satellite which sets and rises again must produce two arcs, never one
 * line drawn straight across the sky between them.
 *
 * Everything here is a pure function of the series, and everything is built
 * once per series rather than per slider tick — a 24-hour window holds tens of
 * thousands of samples, and rebuilding that on every drag would be felt.
 */

// Relative, with extensions: modules under `lib/` import each other this way
// so they run unchanged under `node --test`, which does not know the `@/`
// alias. Components outside `lib/` still use `@/`.
import { project, type Point } from "./skyPlotLayout.ts";
import type { Satellite, SatelliteTrack, SkySeries } from "./types.ts";

/** One unbroken stretch of a satellite's path. */
export interface TrackSegment {
  sv: string;
  source: string;
  /** Projected SVG points, in time order. */
  points: Point[];
  /** Epoch index of the first and last sample, for debugging and tests. */
  fromEpoch: number;
  toEpoch: number;
}

/**
 * Split a track wherever the epoch index jumps.
 *
 * A gap means the satellite was below the mask (or had no ephemeris) for at
 * least one sample, so the two stretches either side are separate passes. Over
 * a 24-hour window most GPS satellites set and rise again, which is exactly the
 * case that would otherwise draw a false chord across the plot.
 *
 * Single-sample runs are kept: a satellite caught for one epoch at the edge of
 * the window is real, and the renderer shows it as a dot rather than dropping
 * it.
 */
export function segmentTrack(track: SatelliteTrack): TrackSegment[] {
  const segments: TrackSegment[] = [];
  let run: typeof track.samples = [];

  const flush = () => {
    if (run.length === 0) return;
    segments.push({
      sv: track.sv,
      source: track.source,
      points: run.map((s) => project(s.azimuth, s.elevation)),
      fromEpoch: run[0].epochIndex,
      toEpoch: run[run.length - 1].epochIndex,
    });
    run = [];
  };

  for (const sample of track.samples) {
    const previous = run[run.length - 1];
    if (previous && sample.epochIndex !== previous.epochIndex + 1) flush();
    run.push(sample);
  }
  flush();

  return segments;
}

/** Every drawable arc in the series. */
export function trackSegments(series: SkySeries): TrackSegment[] {
  return series.tracks.flatMap(segmentTrack);
}

/**
 * A stable colour per satellite.
 *
 * Sky plots with a dozen overlapping arcs are unreadable in one colour — the
 * question "where does *this* satellite go" needs the arc to be followable
 * through a crossing. Hue comes from the identifier rather than from the array
 * position so it survives a satellite rising, setting, or being filtered out by
 * a source toggle; a position-derived colour would reshuffle the whole plot
 * when one satellite dropped out.
 *
 * Kept desaturated and light so the arcs stay behind the markers, which carry
 * the elevation colour scale.
 */
export function trackColour(sv: string, alpha = 0.55): string {
  let hash = 0;
  for (let i = 0; i < sv.length; i += 1) {
    hash = (hash * 31 + sv.charCodeAt(i)) >>> 0;
  }
  // 137.5 deg is the golden angle: successive hashes land far apart on the
  // wheel instead of clumping, so neighbouring PRNs get distinguishable hues.
  const hue = (hash * 137.5) % 360;
  return `hsl(${hue.toFixed(1)} 55% 55% / ${alpha})`;
}

/**
 * Satellites visible at each epoch, indexed by epoch.
 *
 * Built once per series so the slider is a constant-time lookup. The result
 * lines up exactly with what the single-epoch sky view would return for the
 * same instant — guaranteed on the Rust side, where the series is produced by
 * calling the single-epoch path repeatedly rather than by a parallel loop.
 */
export function satellitesByEpoch(series: SkySeries): Satellite[][] {
  const byEpoch: Satellite[][] = series.epochs.map(() => []);

  for (const track of series.tracks) {
    for (const sample of track.samples) {
      // Defensive: an index outside the epoch array would mean the series and
      // its tracks disagree, and silently dropping it beats a crash in render.
      const slot = byEpoch[sample.epochIndex];
      if (!slot) continue;

      slot.push({
        sv: track.sv,
        prn: track.prn,
        source: track.source,
        azimuth: sample.azimuth,
        elevation: sample.elevation,
        rangeKm: sample.rangeKm,
        ephemerisAgeS: sample.ephemerisAgeS,
      });
    }
  }

  // Tracks arrive ordered by constellation then PRN, and pushing in that order
  // keeps each epoch's list in the same order — which the label layout relies
  // on for a stable, non-flickering result as the slider moves.
  return byEpoch;
}

/** Clamp a slider position to a valid epoch index. */
export function clampCursor(cursor: number, epochCount: number): number {
  if (epochCount <= 0) return 0;
  return Math.min(Math.max(Math.round(cursor), 0), epochCount - 1);
}
