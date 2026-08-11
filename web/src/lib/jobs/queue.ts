/**
 * In-process job queue.
 *
 * FIFO, one job at a time, each run in its own `gnss-iq-worker` process.
 *
 * ## Why one at a time
 *
 * IQ synthesis is CPU-bound and writes hundreds of megabytes. Running several
 * concurrently would not finish any of them sooner on a single machine, and
 * would make the disk cap in `JobSpec::validate` meaningless -- that cap bounds
 * one job's output, not the sum of everything in flight.
 *
 * ## Why a subprocess
 *
 * The synthesis loop would block Next.js's event loop for the whole run, so
 * every status poll would hang behind the job it is asking about. A subprocess
 * also contains failures: a job that dies takes its own process with it.
 *
 * ## What this is not
 *
 * It is not durable. The queue is module state, so a restart loses anything
 * still waiting -- which `failInterruptedJobs` turns into a visible `failed`
 * rather than a job that silently never runs. Swapping in a real broker means
 * replacing `enqueue` and `drain`; nothing else in the app knows the queue
 * exists.
 */

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { stat } from "node:fs/promises";
import path from "node:path";
import readline from "node:readline";

import { ensureWindowCached } from "../ephemerisCache";
import { jobDirectory, specPath, updateJob, failInterruptedJobs } from "./store";
import type { JobRecord } from "./types";

/** Wall-clock budget for one job. */
const JOB_TIMEOUT_MS = 20 * 60 * 1000;

const REPO_ROOT = path.join(process.cwd(), "..");

/**
 * Locate the worker binary.
 *
 * Release first: a debug build is roughly fifty times slower at this workload,
 * slow enough that a job which should take seconds looks hung.
 */
export function workerBinary(): string | null {
  const override = process.env.GNSS_IQ_WORKER;
  if (override) return existsSync(override) ? override : null;

  for (const profile of ["release", "debug"]) {
    const candidate = path.join(REPO_ROOT, "target", profile, "gnss-iq-worker");
    if (existsSync(candidate)) return candidate;
  }
  return null;
}

const pending: string[] = [];
let running = false;

/**
 * Jobs this process has taken responsibility for.
 *
 * The crash sweep must never touch these, however slow it is: a job submitted
 * seconds after startup is legitimately `queued`, and failing it with "the
 * server restarted" would be both wrong and baffling.
 */
const owned = new Set<string>();
let recovery: Promise<number> | null = null;

export async function enqueue(id: string): Promise<void> {
  // Claimed synchronously, before any await, so the sweep below cannot observe
  // this job as unowned no matter how the two interleave.
  owned.add(id);
  recovery ??= failInterruptedJobs(owned);
  await recovery;

  pending.push(id);
  void drain();
}

async function drain(): Promise<void> {
  if (running) return;
  running = true;
  try {
    for (let id = pending.shift(); id !== undefined; id = pending.shift()) {
      await runOne(id);
    }
  } finally {
    running = false;
  }
}

async function runOne(id: string): Promise<void> {
  const record = await updateJob(id, {
    status: "processing",
    started_at: new Date().toISOString(),
    stage: "ephemeris",
  });
  if (!record) return;

  try {
    await execute(record);
  } catch (cause) {
    await updateJob(id, {
      status: "failed",
      finished_at: new Date().toISOString(),
      error: cause instanceof Error ? cause.message : String(cause),
    });
  }
}

async function execute(record: JobRecord): Promise<void> {
  const binary = workerBinary();
  if (!binary) {
    throw new Error(
      "gnss-iq-worker is not built; run `cargo build --release -p gnss-iq` " +
        "or set GNSS_IQ_WORKER to its path",
    );
  }

  // Fetch the broadcast files before the worker starts. The worker deliberately
  // speaks no HTTP -- the cache-correctness rules live in one place, on this
  // side. See crates/gnss-iq/src/ephemeris.rs.
  const { directory, files, missing } = await ensureWindowCached(
    record.spec.window.start_unix_s,
    record.spec.window.duration_s,
  );

  if (files.length === 0) {
    throw new Error(
      `no broadcast ephemeris available for this window (tried ${missing.join(", ")})`,
    );
  }

  await updateJob(record.id, { stage: "synthesis", ephemeris_files: files });

  const output = jobDirectory(record.id);
  const child = spawn(
    binary,
    [
      "--job",
      specPath(record.id),
      "--id",
      record.id,
      "--out",
      output,
      "--cache",
      directory,
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );

  type Completion = {
    binary_path?: unknown;
    sidecar_path?: unknown;
    samples?: unknown;
    satellites?: unknown;
  };

  // Held in a mutable object rather than as plain `let` bindings: these are
  // written from a stream callback, and TypeScript's control-flow analysis
  // narrows a `let` assigned only inside a closure to its initial value.
  const outcome: { completion: Completion | null; error: string | null } = {
    completion: null,
    error: null,
  };
  const stderr: string[] = [];

  // The worker emits one JSON object per line. Reading it line by line rather
  // than buffering means progress is visible while the job runs, which for a
  // multi-minute synthesis is the difference between a status endpoint that is
  // useful and one that only ever says "processing".
  const lines = readline.createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    let event: Record<string, unknown>;
    try {
      event = JSON.parse(line);
    } catch {
      return; // not a status line; ignore rather than fail the job
    }

    switch (event.event) {
      case "progress": {
        const progress = event.progress as { stage?: string; fraction?: number };
        void updateJob(record.id, {
          stage: progress?.stage,
          progress: progress?.fraction,
        });
        break;
      }
      case "complete":
        outcome.completion = event as Completion;
        break;
      case "failed":
        outcome.error = String(event.error ?? "unknown worker error");
        break;
    }
  });

  child.stderr.on("data", (chunk) => stderr.push(String(chunk)));

  const timeout = setTimeout(() => child.kill("SIGKILL"), JOB_TIMEOUT_MS);
  const code = await new Promise<number | null>((resolve, reject) => {
    child.on("error", reject);
    child.on("close", resolve);
  }).finally(() => clearTimeout(timeout));

  const completion = outcome.completion;
  if (code !== 0 || !completion) {
    throw new Error(
      outcome.error ||
        stderr.join("").trim() ||
        `worker exited with status ${code}`,
    );
  }

  const binaryPath = String(completion.binary_path);
  const sidecarPath = String(completion.sidecar_path);
  const size = await stat(binaryPath);

  await updateJob(record.id, {
    status: "complete",
    finished_at: new Date().toISOString(),
    progress: 1,
    stage: "complete",
    outputs: {
      binary: path.basename(binaryPath),
      sidecar: path.basename(sidecarPath),
      binary_bytes: size.size,
      sample_count: Number(completion.samples ?? 0),
      satellites: Number(completion.satellites ?? 0),
    },
  });
}

/** Jobs waiting, for the list endpoint. */
export function queueDepth(): number {
  return pending.length + (running ? 1 : 0);
}
