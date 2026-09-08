/**
 * Broadcast ephemeris fetch proxy.
 *
 * Why this exists at all: BKG's IGS archive sends no `Access-Control-Allow-Origin`
 * header, so the browser cannot fetch it directly. This route is a same-origin
 * shim that fetches, caches and trims the file. Compute still happens entirely
 * client-side in WASM — see docs/adr/0006-server-side-ephemeris-proxy.md.
 *
 * Source is BKG rather than CDDIS (which ADR-0004 names) because CDDIS requires
 * an Earthdata login. Both distribute the same IGS merged broadcast product.
 */

import { createHash } from "node:crypto";
import { mkdir, readFile, stat, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { gunzipSync } from "node:zlib";

export const runtime = "nodejs";

/** Earliest day present in the BKG BRDC archive (probed empirically). */
const ARCHIVE_START = "2017-06-01";

/** Upstream fetch budget. The archive is usually fast; a slow day should fail
 *  visibly rather than hang the UI. */
const UPSTREAM_TIMEOUT_MS = 30_000;

/**
 * Where fetched broadcast files are kept.
 *
 * `EPHEMERIS_CACHE_DIR` first, then the repo's `data/cache` in a checkout,
 * then the system temp directory. The fallback is what makes this deployable:
 * a serverless or container filesystem is usually read-only outside `/tmp`,
 * and the cache is an optimisation — losing it costs a few seconds per cold
 * day, where crashing on it costs the page. Every cache operation below is
 * therefore best-effort.
 */
const CACHE_DIR =
  process.env.EPHEMERIS_CACHE_DIR ??
  (existsSync(path.join(process.cwd(), "..", "data"))
    ? path.join(process.cwd(), "..", "data", "cache")
    : path.join(os.tmpdir(), "gnss-simulator-ephemeris"));

/** RINEX 3 constellation codes this project can propagate. */
const SUPPORTED_CODES = new Set(["G", "E", "C", "J", "S"]);

/**
 * How far either side of the requested window records are kept \[s\].
 *
 * A record is only useful for an epoch within its curve-fit interval, which is
 * two hours for the Keplerian constellations and fifteen minutes for SBAS. An
 * hour of slack on top of the widest of those covers selection at either end
 * of the window, including the block *after* the window that a nearest-ToE
 * search legitimately reaches for.
 */
const WINDOW_MARGIN_S = 3 * 3600;

function dayOfYear(date: Date): number {
  const start = Date.UTC(date.getUTCFullYear(), 0, 1);
  return Math.floor((date.getTime() - start) / 86_400_000) + 1;
}

function upstreamUrl(date: Date): { url: string; filename: string } {
  const year = date.getUTCFullYear();
  const doy = String(dayOfYear(date)).padStart(3, "0");
  const filename = `BRDC00WRD_R_${year}${doy}0000_01D_MN.rnx.gz`;
  return {
    url: `https://igs.bkg.bund.de/root_ftp/IGS/BRDC/${year}/${doy}/${filename}`,
    filename,
  };
}

/** Today in UTC, at midnight. */
function todayUtc(): Date {
  const now = new Date();
  return new Date(
    Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate()),
  );
}

/**
 * Whether a record's epoch falls inside the requested window.
 *
 * The epoch is fixed-column on the record's first line: `YYYY MM DD hh mm ss`
 * starting at column 4. A line that does not parse is *kept* — this is a size
 * optimisation, and the failure mode of guessing wrong must be a larger
 * response, never a missing satellite.
 *
 * BeiDou stamps its epochs in BDT, 14 s from the GPS time the window is
 * expressed in. Against a three-hour margin that is not worth correcting for.
 */
function isWithinWindow(
  line: string,
  window: { fromS: number; toS: number } | null,
): boolean {
  if (window === null) return true;

  const epoch = Date.UTC(
    Number(line.slice(4, 8)),
    Number(line.slice(9, 11)) - 1,
    Number(line.slice(12, 14)),
    Number(line.slice(15, 17)),
    Number(line.slice(18, 20)),
    Number(line.slice(21, 23)),
  );
  if (Number.isNaN(epoch)) return true;

  const seconds = epoch / 1000;
  return (
    seconds >= window.fromS - WINDOW_MARGIN_S &&
    seconds <= window.toS + WINDOW_MARGIN_S
  );
}

/**
 * Split a RINEX 3 navigation file and keep only the requested constellations.
 *
 * A record begins on a line matching `<sysid><2-digit PRN>` at column 0 and runs
 * until the next such line; continuation lines are indented. Detecting the next
 * record start is more robust than assuming a fixed line count, which varies by
 * constellation (GLONASS/SBAS use 4 lines, the Keplerian systems 8) and by
 * message type.
 *
 * Records outside the requested time window (plus [`WINDOW_MARGIN_S`]) are
 * dropped too. That matters much more than it used to: with several
 * constellations switched on a whole day is several megabytes of text, most of
 * it describing hours the user is not looking at.
 *
 * Trimming matters for responsiveness: a full mixed file is ~8 MB of text, of
 * which the GPS records are ~300 kB. Parsing the trimmed file in WASM takes
 * tens of milliseconds rather than most of a second.
 */
function filterConstellations(
  text: string,
  keep: Set<string>,
  sbasPrns: Set<number> | null,
  window: { fromS: number; toS: number } | null,
): { body: string; records: number } {
  const lines = text.split("\n");
  const headerEnd = lines.findIndex((line) => line.includes("END OF HEADER"));
  if (headerEnd === -1) {
    throw new Error("RINEX header has no END OF HEADER marker");
  }

  const isRecordStart = (line: string) => /^[A-Z]\d{2}/.test(line);

  const header = lines.slice(0, headerEnd + 1).map((line) => {
    if (!line.includes("RINEX VERSION / TYPE")) return line;
    // Columns 41-60 carry the satellite system field.
    const label =
      keep.size === 1
        ? { G: "G: GPS", E: "E: GALILEO", C: "C: BEIDOU", J: "J: QZSS", S: "S: SBAS" }[
            [...keep][0]
          ] ?? "M: MIXED"
        : "M: MIXED";
    return line.slice(0, 40) + label.padEnd(20) + "RINEX VERSION / TYPE";
  });

  const kept: string[] = [];
  let records = 0;

  for (let i = headerEnd + 1; i < lines.length; ) {
    if (!isRecordStart(lines[i])) {
      i += 1;
      continue;
    }
    let end = i + 1;
    while (end < lines.length && !isRecordStart(lines[end])) end += 1;

    // SBAS is one RINEX constellation but many regional systems, and a full
    // day carries ~12 800 records across all of them. Narrowing to the PRNs the
    // caller asked for keeps the payload comparable to GPS. This is purely a
    // size optimisation -- `gnss-core` filters by provider authoritatively, so
    // an over-broad list here costs bytes, never correctness.
    const isWanted =
      keep.has(lines[i][0]) &&
      (lines[i][0] !== "S" ||
        sbasPrns === null ||
        sbasPrns.has(Number(lines[i].slice(1, 3)) + 100)) &&
      isWithinWindow(lines[i], window);

    if (isWanted) {
      // Trailing blank lines are not part of the record. Sweeping them in
      // produces zero-length lines in the output, which the `rinex` crate
      // panics on (parsing.rs slices [4..] without a length check) — and a
      // panic inside WASM is a hard trap, not a catchable error.
      let recordEnd = end;
      while (recordEnd > i && lines[recordEnd - 1].trim() === "") recordEnd -= 1;
      kept.push(...lines.slice(i, recordEnd));
      records += 1;
    }
    i = end;
  }

  // Exactly one trailing newline, no interior blanks.
  return { body: `${[...header, ...kept].join("\n")}\n`, records };
}

/**
 * Fetch the upstream file, using a local disk cache.
 *
 * A cached copy is only trusted if it was written *after* its day ended.
 * Broadcast files accumulate through the day, so a file fetched at noon holds
 * half a day of records and would otherwise be served as complete forever.
 * The same rule handles today automatically: end-of-day is in the future, so
 * today's file is never considered complete and is always refetched.
 */
async function fetchWithCache(
  date: Date,
): Promise<{ raw: Buffer; filename: string; cached: boolean }> {
  const { url, filename } = upstreamUrl(date);
  const cachePath = path.join(CACHE_DIR, filename);
  const endOfDay = date.getTime() + 86_400_000;

  try {
    if (existsSync(cachePath)) {
      const writtenAt = (await stat(cachePath)).mtimeMs;
      if (writtenAt > endOfDay) {
        return { raw: await readFile(cachePath), filename, cached: true };
      }
      // Partial: fetched while the day was still running. Fall through and
      // refetch, overwriting it.
    }
  } catch (cause) {
    // An unreadable cache is not a reason to fail the request.
    console.warn(`ephemeris cache read failed for ${filename}:`, cause);
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

  // Best-effort: a read-only or full filesystem costs the next request a
  // refetch, which is a far better outcome than a 500 on a page that already
  // has the data it needs in hand.
  try {
    await mkdir(CACHE_DIR, { recursive: true });
    await writeFile(cachePath, raw);
  } catch (cause) {
    console.warn(`ephemeris cache write failed for ${filename}:`, cause);
  }

  return { raw, filename, cached: false };
}

export async function GET(request: Request) {
  const params = new URL(request.url).searchParams;
  const dateParam = params.get("date");
  const constellationsParam = params.get("constellations") ?? "G";
  // `v` is read only so that it forms part of the cache key -- see
  // EPHEMERIS_FORMAT_VERSION in lib/coverage.ts. Nothing branches on it.
  void params.get("v");

  // Optional SBAS PRN narrowing. Absent means "keep every SBAS record".
  const sbasParam = params.get("sbas");
  const sbasPrns = sbasParam
    ? new Set(
        sbasParam
          .split(",")
          .map((value) => Number(value.trim()))
          .filter((prn) => Number.isFinite(prn)),
      )
    : null;

  // Optional time-window narrowing, in Unix seconds. Absent means "keep the
  // whole day", which is what a caller that has not been updated will get.
  const fromS = Number(params.get("from"));
  const toS = Number(params.get("to"));
  const window =
    Number.isFinite(fromS) && Number.isFinite(toS) && toS >= fromS
      ? { fromS, toS }
      : null;

  if (!dateParam || !/^\d{4}-\d{2}-\d{2}$/.test(dateParam)) {
    return Response.json(
      { error: "date query parameter required, format YYYY-MM-DD" },
      { status: 400 },
    );
  }

  const date = new Date(`${dateParam}T00:00:00Z`);
  if (Number.isNaN(date.getTime())) {
    return Response.json({ error: `not a valid date: ${dateParam}` }, { status: 400 });
  }

  const today = todayUtc();
  if (date.getTime() > today.getTime()) {
    return Response.json(
      {
        error:
          "broadcast ephemeris only exists for days that have happened; pick today or earlier",
      },
      { status: 400 },
    );
  }
  if (dateParam < ARCHIVE_START) {
    return Response.json(
      { error: `the BKG broadcast archive starts at ${ARCHIVE_START}` },
      { status: 400 },
    );
  }

  const keep = new Set(
    constellationsParam
      .toUpperCase()
      .split(",")
      .map((code) => code.trim())
      .filter((code) => SUPPORTED_CODES.has(code)),
  );
  if (keep.size === 0) {
    return Response.json(
      { error: `constellations must be a subset of ${[...SUPPORTED_CODES].join(",")}` },
      { status: 400 },
    );
  }

  try {
    const { raw, filename, cached } = await fetchWithCache(date);
    const { body, records } = filterConstellations(
      gunzipSync(raw).toString("latin1"),
      keep,
      sbasPrns,
      window,
    );

    if (records === 0) {
      return Response.json(
        {
          error:
            `no ${[...keep].join(",")} records in ${filename}` +
            (window ? " for the requested time window" : "") +
            // SBAS coverage in this product is patchy historically: files
            // before ~2021 carry none at all, and WAAS appears later still.
            (keep.has("S")
              ? " — SBAS coverage in the BKG archive is sparse before 2025"
              : ""),
        },
        { status: 404 },
      );
    }

    const isToday = date.getTime() === today.getTime();

    return new Response(body, {
      status: 200,
      headers: {
        "Content-Type": "application/octet-stream",
        // Past days are immutable once published; today's file still grows.
        "Cache-Control": isToday
          ? "public, max-age=900"
          : "public, max-age=86400, immutable",
        ETag: `"${createHash("sha1").update(body).digest("hex").slice(0, 16)}"`,
        "X-Ephemeris-Source": filename,
        "X-Ephemeris-Records": String(records),
        "X-Ephemeris-Bytes": String(Buffer.byteLength(body)),
        "X-Ephemeris-Cached": String(cached),
        // Today's file only covers the hours elapsed so far.
        "X-Ephemeris-Partial": String(isToday),
      },
    });
  } catch (cause) {
    const status = (cause as { status?: number }).status ?? 502;
    const message = cause instanceof Error ? cause.message : String(cause);
    return Response.json(
      {
        error:
          status === 404
            ? `no broadcast file published for ${dateParam}`
            : `could not retrieve ephemeris: ${message}`,
      },
      { status },
    );
  }
}
