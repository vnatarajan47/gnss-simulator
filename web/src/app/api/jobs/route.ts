/**
 * Job submission and listing.
 *
 *   POST /api/jobs   submit a job, returns 202 with the record
 *   GET  /api/jobs   list recent jobs
 *
 * Submission is deliberately asynchronous even for jobs that would finish
 * quickly: the response time of a synchronous endpoint would depend on the
 * sample rate and window length the caller chose, which is not something a
 * client can plan around.
 */

import { randomUUID } from "node:crypto";

import { enqueue, queueDepth, workerBinary } from "@/lib/jobs/queue";
import { createJob, listJobs } from "@/lib/jobs/store";
import { JobRequestError, parseJobRequest } from "@/lib/jobs/types";

export const runtime = "nodejs";
/** Job state lives on disk and changes between requests; never cache it. */
export const dynamic = "force-dynamic";

export async function POST(request: Request) {
  let body: unknown;
  try {
    body = await request.json();
  } catch {
    return Response.json({ error: "request body must be JSON" }, { status: 400 });
  }

  let spec;
  try {
    spec = parseJobRequest(body);
  } catch (cause) {
    if (cause instanceof JobRequestError) {
      return Response.json({ error: cause.message }, { status: 400 });
    }
    throw cause;
  }

  // Checked before accepting rather than at run time: a missing binary is a
  // deployment problem, not a problem with this request, and reporting it as a
  // failed job would put the blame in the wrong place.
  if (!workerBinary()) {
    return Response.json(
      {
        error:
          "the IQ worker is not built. Run `cargo build --release -p gnss-iq` " +
          "from the repository root, or set GNSS_IQ_WORKER to its path.",
      },
      { status: 503 },
    );
  }

  const id = randomUUID();
  const record = await createJob(id, spec);
  await enqueue(id);

  return Response.json(
    { ...record, queue_depth: queueDepth() },
    {
      status: 202,
      headers: { Location: `/api/jobs/${id}` },
    },
  );
}

export async function GET(request: Request) {
  const limitParam = new URL(request.url).searchParams.get("limit");
  const limit = Math.min(Math.max(Number(limitParam) || 50, 1), 200);

  return Response.json({
    jobs: await listJobs(limit),
    queue_depth: queueDepth(),
  });
}
