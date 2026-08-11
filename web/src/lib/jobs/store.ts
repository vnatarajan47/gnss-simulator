/**
 * Disk-backed job records.
 *
 * One directory per job under `data/jobs/<id>/`, holding the record, the spec
 * handed to the worker, and the two output files. Keeping the outputs beside
 * the record means a job is one directory to inspect, archive or delete.
 *
 * A directory rather than a database because the MVP's storage is local disk
 * and the records are a few kilobytes each. The access pattern here -- read
 * one by id, list all -- is the one a filesystem is already good at, and
 * swapping in object storage later means reimplementing this module's five
 * functions, not unpicking queries.
 *
 * ## Crash consistency
 *
 * Records are written to a temporary file and renamed, which is atomic within
 * a filesystem. Without that, a status poll landing during a write sees a
 * truncated JSON file and reports a job as broken when it is merely being
 * updated.
 */

import { mkdir, readFile, readdir, rename, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";

import { JOB_SCHEMA_VERSION, isTerminal, type JobRecord, type JobSpec } from "./types";

export const JOBS_DIR = path.join(process.cwd(), "..", "data", "jobs");

export function jobDirectory(id: string): string {
  return path.join(JOBS_DIR, id);
}

function recordPath(id: string): string {
  return path.join(jobDirectory(id), "job.json");
}

export function specPath(id: string): string {
  return path.join(jobDirectory(id), "spec.json");
}

/**
 * Job ids are used as path components, so anything that is not a plain UUID is
 * refused. This is the only defence between a URL and the filesystem.
 */
export function isValidJobId(id: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id);
}

export async function createJob(id: string, spec: JobSpec): Promise<JobRecord> {
  await mkdir(jobDirectory(id), { recursive: true });

  const record: JobRecord = {
    id,
    status: "queued",
    schema_version: JOB_SCHEMA_VERSION,
    spec,
    submitted_at: new Date().toISOString(),
  };

  // The spec goes out as its own file because that is what the worker reads:
  // passing a multi-kilobyte JSON document as an argv element would work today
  // and break on a longer one.
  await writeFile(specPath(id), JSON.stringify(spec, null, 2));
  await writeRecord(record);
  return record;
}

export async function readJob(id: string): Promise<JobRecord | null> {
  if (!isValidJobId(id) || !existsSync(recordPath(id))) return null;
  try {
    return JSON.parse(await readFile(recordPath(id), "utf8")) as JobRecord;
  } catch {
    return null;
  }
}

/** Distinguishes concurrent writes to the same record within one process. */
let writeCounter = 0;

async function writeRecord(record: JobRecord): Promise<void> {
  const target = recordPath(record.id);
  // The suffix must be unique per *write*, not per process. Two concurrent
  // updates to one job sharing a temp path interleave inside it and rename a
  // spliced file into place -- which is how a record ends up reading
  // `} "progress": 1 }`.
  const temporary = `${target}.${process.pid}.${writeCounter++}.tmp`;
  await writeFile(temporary, JSON.stringify(record, null, 2));
  await rename(temporary, target);
}

/**
 * Serialises work per job id.
 *
 * `updateJob` is a read-modify-write, and its callers are inherently
 * concurrent: the queue sets status while the worker's progress events arrive
 * on a stream. Without this, two updates read the same record and the second
 * rename discards the first's fields.
 *
 * The chain is per id, so unrelated jobs never wait on each other, and it
 * absorbs rejections so one failed write cannot wedge every later one.
 */
const writeChains = new Map<string, Promise<unknown>>();

function withJobLock<T>(id: string, work: () => Promise<T>): Promise<T> {
  const previous = writeChains.get(id) ?? Promise.resolve();
  const next = previous.then(work, work);
  writeChains.set(
    id,
    next.catch(() => undefined),
  );
  return next;
}

/**
 * Apply a patch to a stored record.
 *
 * Re-reads under the lock before writing, so a progress update from the worker
 * and a status change from the queue cannot clobber each other.
 */
export async function updateJob(
  id: string,
  patch: Partial<JobRecord>,
): Promise<JobRecord | null> {
  return withJobLock(id, async () => {
    const existing = await readJob(id);
    if (!existing) return null;
    const updated = { ...existing, ...patch };
    await writeRecord(updated);
    return updated;
  });
}

export async function listJobs(limit = 50): Promise<JobRecord[]> {
  if (!existsSync(JOBS_DIR)) return [];

  const entries = await readdir(JOBS_DIR, { withFileTypes: true });
  const records = await Promise.all(
    entries
      .filter((entry) => entry.isDirectory() && isValidJobId(entry.name))
      .map((entry) => readJob(entry.name)),
  );

  return records
    .filter((record): record is JobRecord => record !== null)
    .sort((a, b) => b.submitted_at.localeCompare(a.submitted_at))
    .slice(0, limit);
}

/**
 * Fail any job left mid-flight by a *previous* process.
 *
 * The queue lives in process memory, so a restart loses every in-flight job
 * while its record on disk still says `processing`. Without this sweep those
 * records stay `processing` forever and a client polls indefinitely.
 *
 * `owned` is the set of ids this process has taken responsibility for, and it
 * is read at decision time rather than copied, so a job submitted while the
 * sweep is still walking the directory is already excluded by the time its
 * record is examined. Without that, the sweep races every job submitted just
 * after startup and kills it with a message blaming a restart that did not
 * happen.
 */
export async function failInterruptedJobs(
  owned: ReadonlySet<string>,
): Promise<number> {
  const stale = (await listJobs(1000)).filter(
    (record) => !isTerminal(record.status) && !owned.has(record.id),
  );

  for (const record of stale) {
    await updateJob(record.id, {
      status: "failed",
      finished_at: new Date().toISOString(),
      error:
        "the server restarted while this job was queued or running; resubmit it",
    });
  }

  return stale.length;
}
