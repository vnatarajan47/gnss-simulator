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
  /** Ephemeris records after constellation filtering. */
  records: number;
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

export interface LoadedEphemeris {
  plotter: Skyplotter;
  meta: EphemerisMeta;
}

/** Extract the UTC date portion of a `datetime-local` value. */
export function dateOf(isoLocal: string): string {
  return isoLocal.slice(0, 10);
}

export async function loadEphemeris(
  date: string,
  constellations: string,
  sbasPrns = "",
): Promise<LoadedEphemeris> {
  const started = performance.now();

  // `v` participates in the cache key only: past-day responses are immutable,
  // so a change to the server-side trimming needs a new URL to reach clients.
  const response = await fetch(
    `/api/ephemeris?date=${encodeURIComponent(date)}` +
      `&constellations=${encodeURIComponent(constellations)}` +
      `&v=${EPHEMERIS_FORMAT_VERSION}` +
      (sbasPrns ? `&sbas=${encodeURIComponent(sbasPrns)}` : ""),
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
    throw new Error(message);
  }

  const bytes = new Uint8Array(await response.arrayBuffer());
  const plotter = await createSkyplotter(bytes);

  return {
    plotter,
    meta: {
      date,
      source: response.headers.get("X-Ephemeris-Source") ?? "unknown",
      records: Number(response.headers.get("X-Ephemeris-Records") ?? 0),
      cached: response.headers.get("X-Ephemeris-Cached") === "true",
      partial: response.headers.get("X-Ephemeris-Partial") === "true",
      elapsedMs: Math.round(performance.now() - started),
    },
  };
}
