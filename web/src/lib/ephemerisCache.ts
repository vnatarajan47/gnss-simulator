/**
 * Fetching and caching BKG broadcast ephemeris on the server.
 *
 * Extracted from the /api/ephemeris route because the IQ job pipeline needs the
 * same files, and the caching rules here are subtle enough that two copies
 * would drift. In particular the partial-file rule below came from a real bug:
 * a file fetched at noon holds half a day of records, and without the mtime
 * check it is served as complete forever.
 *
 * Server-only. Imports node:fs.
 */

import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";

/** Earliest day present in the BKG BRDC archive (probed empirically). */
export const ARCHIVE_START = "2017-06-01";

/** Upstream fetch budget. The archive is usually fast; a slow day should fail
 *  visibly rather than hang the caller. */
const UPSTREAM_TIMEOUT_MS = 30_000;

export const CACHE_DIR = path.join(process.cwd(), "..", "data", "cache");

export function dayOfYear(date: Date): number {
  const start = Date.UTC(date.getUTCFullYear(), 0, 1);
  return Math.floor((date.getTime() - start) / 86_400_000) + 1;
}

export function upstreamUrl(date: Date): { url: string; filename: string } {
  const year = date.getUTCFullYear();
  const doy = String(dayOfYear(date)).padStart(3, "0");
  const filename = `BRDC00WRD_R_${year}${doy}0000_01D_MN.rnx.gz`;
  return {
    url: `https://igs.bkg.bund.de/root_ftp/IGS/BRDC/${year}/${doy}/${filename}`,
    filename,
  };
}

/** Today in UTC, at midnight. */
export function todayUtc(): Date {
  const now = new Date();
  return new Date(
    Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate()),
  );
}

/** Midnight UTC on the day containing a Unix timestamp (seconds). */
export function utcDayOf(unixSeconds: number): Date {
  return new Date(Math.floor(unixSeconds / 86_400) * 86_400_000);
}

export type CachedFile = {
  raw: Buffer;
  filename: string;
  path: string;
  cached: boolean;
};

/**
 * Fetch the upstream file, using a local disk cache.
 *
 * A cached copy is only trusted if it was written *after* its day ended.
 * Broadcast files accumulate through the day, so a file fetched at noon holds
 * half a day of records and would otherwise be served as complete forever.
 * The same rule handles today automatically: end-of-day is in the future, so
 * today's file is never considered complete and is always refetched.
 */
export async function fetchWithCache(date: Date): Promise<CachedFile> {
  const { url, filename } = upstreamUrl(date);
  const cachePath = path.join(CACHE_DIR, filename);
  const endOfDay = date.getTime() + 86_400_000;

  if (existsSync(cachePath)) {
    const writtenAt = (await stat(cachePath)).mtimeMs;
    if (writtenAt > endOfDay) {
      return {
        raw: await readFile(cachePath),
        filename,
        path: cachePath,
        cached: true,
      };
    }
    // Partial: fetched while the day was still running. Fall through and
    // refetch, overwriting it.
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), UPSTREAM_TIMEOUT_MS);
  let response: Response;
  try {
    response = await fetch(url, { signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }

  if (!response.ok) {
    const error = new Error(
      `upstream returned ${response.status} for ${filename}`,
    ) as Error & { status?: number };
    error.status = response.status === 404 ? 404 : 502;
    throw error;
  }

  const raw = Buffer.from(await response.arrayBuffer());

  await mkdir(CACHE_DIR, { recursive: true });
  await writeFile(cachePath, raw);

  return { raw, filename, path: cachePath, cached: false };
}

/**
 * How far outside a window broadcast ephemeris is still needed [s].
 *
 * Must not be shorter than `SELECTION_REACH_S` in `crates/gnss-iq/src/ephemeris.rs`,
 * which is what decides the days the worker actually *reads*. This value only
 * decides what gets *fetched*: too small and the worker finds a neighbour
 * missing (which it tolerates, at some loss of accuracy near a day boundary);
 * too large and a day is downloaded for nothing.
 */
const SELECTION_REACH_S = 7200;

/**
 * Ensure every daily file an IQ job's window can reach is on disk.
 *
 * Returns the cache directory, which is what the worker is pointed at. A day
 * that is genuinely absent upstream is skipped rather than fatal: the archive
 * has real gaps, and a window in the middle of a present day does not need its
 * neighbours. The worker fails loudly if *nothing* usable is there.
 */
export async function ensureWindowCached(
  startUnixSeconds: number,
  durationSeconds: number,
): Promise<{ directory: string; files: string[]; missing: string[] }> {
  const first = utcDayOf(startUnixSeconds - SELECTION_REACH_S);
  const last = utcDayOf(startUnixSeconds + durationSeconds + SELECTION_REACH_S);

  const files: string[] = [];
  const missing: string[] = [];
  const today = todayUtc();

  for (
    let day = first;
    day.getTime() <= last.getTime();
    day = new Date(day.getTime() + 86_400_000)
  ) {
    if (day.getTime() > today.getTime()) {
      missing.push(day.toISOString().slice(0, 10));
      continue;
    }
    try {
      const { filename } = await fetchWithCache(day);
      files.push(filename);
    } catch {
      missing.push(day.toISOString().slice(0, 10));
    }
  }

  return { directory: CACHE_DIR, files, missing };
}
