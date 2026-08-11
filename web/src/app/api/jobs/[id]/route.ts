/**
 * Job status.
 *
 *   GET /api/jobs/{id}
 *
 * Polling endpoint. Once `status` is `complete` the response carries download
 * links for both output files; until then it carries a stage and, during
 * synthesis, a fraction.
 *
 * The response shape is the stored record plus a `links` object. Adding a
 * webhook or an email notification later means adding fields to the record and
 * a delivery step to the queue -- clients polling this endpoint keep working
 * unchanged, which is why the status model is a plain state field rather than
 * something coupled to how the client found out.
 */

import { readJob } from "@/lib/jobs/store";
import { isTerminal } from "@/lib/jobs/types";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ id: string }> },
) {
  const { id } = await params;
  const record = await readJob(id);

  if (!record) {
    return Response.json({ error: `no job with id ${id}` }, { status: 404 });
  }

  const links =
    record.status === "complete" && record.outputs
      ? {
          binary: `/api/jobs/${id}/files/binary`,
          sidecar: `/api/jobs/${id}/files/sidecar`,
        }
      : undefined;

  return Response.json(
    { ...record, links },
    {
      headers: {
        // A finished job never changes again, so it is safe to cache; one in
        // flight must not be.
        "Cache-Control": isTerminal(record.status)
          ? "public, max-age=3600"
          : "no-store",
      },
    },
  );
}
