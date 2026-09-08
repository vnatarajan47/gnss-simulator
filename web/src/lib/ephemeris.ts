/**
 * Fetching broadcast ephemeris for an arbitrary day.
 *
 * Goes through `/api/ephemeris` rather than straight to the archive: BKG sends
 * no CORS header, so a same-origin route is required. That route also trims the
 * file to the constellations we actually use, which is what keeps parsing in
 * the tens of milliseconds instead of most of a second.
 */

import { EPHEMERIS_FORMAT_VERSION } from "@/lib/coverage";
import { createSkyplotter, type Skyplotter } from "@/lib/wasm";

export interface EphemerisMeta {
  /** UTC day requested, YYYY-MM-DD. */
  date: string;
  /** Upstream filename the data came from. */
  source: string;
  /** Ephemeris records after constellation and time filtering. */
  records: number;
  /** Size of the trimmed RINEX handed to WASM, in bytes. */
  bytes: number;
  /** Served from the server-side disk cache rather than refetched. */
  cached: boolean;
  /**
   * True when the requested day is today. Broadcast files accumulate through
   * the day, so a request for a time later than "now" will find no ephemeris.
   */
  partial: boolean;
  /** Wall-clock milliseconds for fetch + parse, for the UI to report. */
  elapsedMs: number;
}

export interface LoadedRange {
  plotter: Skyplotter;
  /** One entry per UTC day loaded, in order. */
  days: EphemerisMeta[];
  /** Wall-clock milliseconds for the whole load. */
  elapsedMs: number;
}

/**
 * Fetch every UTC day a window touches and fold them into one engine.
 *
 * A time window can straddle UTC midnight while broadcast files are published
 * one per day, so a series may need two of them. Merging is also strictly
 * better than picking one near a boundary: an epoch at 00:10 has its nearest
 * time-of-ephemeris in the *previous* day's file, so a single-day load would
 * extrapolate forward from 00:00 where a merged one interpolates.
 *
 * Days are fetched in parallel but merged in order. Duplicate blocks across the
 * midnight overlap are dropped inside `EphemerisSet`, so ordering only affects
 * which identical copy is kept.
 */
export async function loadEphemerisRange(
  dates: string[],
  constellations: string,
  sbasPrns = "",
  window?: { startS: number; endS: number },
): Promise<LoadedRange> {
  if (dates.length === 0) {
    throw new Error("no days requested");
  }

  const started = performance.now();
  const loaded = await Promise.all(
    dates.map((date) => fetchDay(date, constellations, sbasPrns, window)),
  );

  const [first, ...rest] = loaded;
  const plotter = await createSkyplotter(first.bytes);
  for (const day of rest) {
    plotter.extend(day.bytes);
  }

  return {
    plotter,
    days: loaded.map((day) => day.meta),
    elapsedMs: Math.round(performance.now() - started),
  };
}

/** Fetch and validate one day's trimmed RINEX, without parsing it. */
async function fetchDay(
  date: string,
  constellations: string,
  sbasPrns: string,
  window?: { startS: number; endS: number },
): Promise<{ bytes: Uint8Array; meta: EphemerisMeta }> {
  const started = performance.now();
  const response = await requestDay(date, constellations, sbasPrns, window);
  const bytes = new Uint8Array(await response.arrayBuffer());

  return {
    bytes,
    meta: {
      date,
      source: response.headers.get("X-Ephemeris-Source") ?? "unknown",
      records: Number(response.headers.get("X-Ephemeris-Records") ?? 0),
      bytes: bytes.byteLength,
      cached: response.headers.get("X-Ephemeris-Cached") === "true",
      partial: response.headers.get("X-Ephemeris-Partial") === "true",
      elapsedMs: Math.round(performance.now() - started),
    },
  };
}

async function requestDay(
  date: string,
  constellations: string,
  sbasPrns: string,
  window?: { startS: number; endS: number },
): Promise<Response> {
  // `v` participates in the cache key only: past-day responses are immutable,
  // so a change to the server-side trimming needs a new URL to reach clients.
  //
  // The window is sent whole rather than sliced per day: the route widens it
  // by the curve-fit interval anyway, so the two requests of a
  // midnight-spanning window differ only in `date`.
  const response = await fetch(
    `/api/ephemeris?date=${encodeURIComponent(date)}` +
      `&constellations=${encodeURIComponent(constellations)}` +
      `&v=${EPHEMERIS_FORMAT_VERSION}` +
      (sbasPrns ? `&sbas=${encodeURIComponent(sbasPrns)}` : "") +
      (window
        ? `&from=${Math.floor(window.startS)}&to=${Math.ceil(window.endS)}`
        : ""),
  );

  if (!response.ok) {
    // The route reports failures as JSON; fall back to status text if it did not.
    let message = `${response.status} ${response.statusText}`;
    try {
      const body = (await response.json()) as { error?: string };
      if (body.error) message = body.error;
    } catch {
      /* keep the status-derived message */
    }
    throw new Error(`${date}: ${message}`);
  }

  return response;
}
