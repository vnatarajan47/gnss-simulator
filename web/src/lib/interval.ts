/**
 * Choosing and validating the time window for a series.
 *
 * Pure functions, no React: the sample step and the day list are decisions the
 * whole app depends on (the step sets the slider's resolution, the day list
 * sets what gets fetched), and they are much easier to reason about with tests
 * than by clicking around.
 */

/**
 * Longest window the UI will sample.
 *
 * 24 hours because that is where two independent limits happen to meet. A GPS
 * ground track repeats every sidereal day (~23 h 56 m), so a day-long window
 * shows the entire repeating pattern and a longer one mostly redraws it. And
 * broadcast ephemeris is published one file per UTC day, so a window of at
 * most 24 h touches at most two files no matter where it starts.
 */
export const MAX_INTERVAL_HOURS = 24;

/** Shortest window worth sampling; below this the series is a single epoch. */
export const MIN_INTERVAL_MINUTES = 5;

/**
 * Epochs per series.
 *
 * Bounds the slider's resolution and the size of the payload crossing the WASM
 * boundary. 721 is a whole number of steps for the round durations people
 * actually pick: 1 h at 5 s, 6 h at 30 s, 24 h at 2 min.
 */
export const MAX_SAMPLES = 721;

/**
 * Sample steps offered, ascending, in seconds.
 *
 * Restricted to values that divide a minute or an hour so epoch timestamps land
 * on readable clock times rather than on 01:13:17.
 */
const STEP_LADDER_S = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800];

export interface Interval {
  /** Unix seconds. */
  startS: number;
  /** Unix seconds, inclusive. */
  endS: number;
}

/**
 * Coarsest-necessary sample step for a duration.
 *
 * Picks the *smallest* ladder entry that keeps the epoch count within
 * `MAX_SAMPLES`, so short windows get fine resolution and only long ones give
 * it up. Falls back to an exact division if even the coarsest rung overflows,
 * which cannot happen for durations within `MAX_INTERVAL_HOURS` but keeps the
 * function total.
 */
export function chooseStepSeconds(durationS: number): number {
  if (!Number.isFinite(durationS) || durationS <= 0) return STEP_LADDER_S[0];

  for (const step of STEP_LADDER_S) {
    if (Math.floor(durationS / step) + 1 <= MAX_SAMPLES) return step;
  }
  return Math.ceil(durationS / (MAX_SAMPLES - 1));
}

/** Number of epochs a window will produce at a given step, both ends included. */
export function sampleCount(durationS: number, stepS: number): number {
  if (stepS <= 0) return 0;
  return Math.floor(durationS / stepS) + 1;
}

export interface IntervalProblem {
  kind: "reversed" | "too-long" | "too-short";
  message: string;
}

/** Whatever is wrong with a window, or `null` if it is usable. */
export function validateInterval({ startS, endS }: Interval): IntervalProblem | null {
  const durationS = endS - startS;

  if (!Number.isFinite(durationS) || durationS < 0) {
    return { kind: "reversed", message: "The end time is before the start time." };
  }
  if (durationS > MAX_INTERVAL_HOURS * 3600) {
    return {
      kind: "too-long",
      message: `Windows are capped at ${MAX_INTERVAL_HOURS} hours — a GPS ground track repeats every sidereal day, so a longer window mostly redraws itself.`,
    };
  }
  if (durationS < MIN_INTERVAL_MINUTES * 60) {
    return {
      kind: "too-short",
      message: `Use a window of at least ${MIN_INTERVAL_MINUTES} minutes.`,
    };
  }
  return null;
}

/**
 * UTC days a window touches, as `YYYY-MM-DD`, ascending.
 *
 * Broadcast files are published per UTC day, so a window crossing midnight
 * needs both. Merging them is also strictly *better* than using either alone
 * near the boundary: an epoch at 00:10 has its nearest time-of-ephemeris in the
 * previous day's file.
 */
export function utcDaysSpanned({ startS, endS }: Interval): string[] {
  const days: string[] = [];
  const dayMs = 86_400_000;

  // Walk by whole UTC days from the start's midnight so the loop cannot be
  // thrown off by a window shorter than a day that still crosses one.
  const firstMidnight = Math.floor((startS * 1000) / dayMs) * dayMs;
  const lastMidnight = Math.floor((endS * 1000) / dayMs) * dayMs;

  for (let ms = firstMidnight; ms <= lastMidnight; ms += dayMs) {
    days.push(new Date(ms).toISOString().slice(0, 10));
  }
  return days;
}

/** `datetime-local` value (treated as UTC) to Unix seconds. */
export function inputToUnixSeconds(value: string): number {
  return Date.parse(`${value}:00Z`) / 1000;
}

/** Unix seconds to a `datetime-local` value, in UTC. */
export function unixSecondsToInput(seconds: number): string {
  return new Date(seconds * 1000).toISOString().slice(0, 16);
}

/** `HH:MM` in UTC, for axis ticks and the slider readout. */
export function formatClock(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toISOString().slice(11, 16);
}

/** `YYYY-MM-DD HH:MM:SS` in UTC. */
export function formatTimestamp(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toISOString().slice(0, 19).replace("T", " ");
}

/** A sample step as something readable: `30 s`, `2 min`. */
export function formatStep(stepS: number): string {
  if (stepS < 60) return `${stepS} s`;
  const minutes = stepS / 60;
  return Number.isInteger(minutes) ? `${minutes} min` : `${(stepS / 60).toFixed(1)} min`;
}
