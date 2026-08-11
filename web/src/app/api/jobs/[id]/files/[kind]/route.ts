/**
 * Output download.
 *
 *   GET /api/jobs/{id}/files/binary    the interleaved IQ samples
 *   GET /api/jobs/{id}/files/sidecar   the JSON metadata
 *
 * The binary is streamed rather than read into memory: these files are
 * routinely hundreds of megabytes, and buffering one would cost that much
 * resident memory per concurrent download.
 *
 * Both filenames come from the stored record, never from the URL. The `kind`
 * path segment selects between two known fields, so no part of the request
 * reaches the filesystem as a path component.
 */

import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import path from "node:path";
import { Readable } from "node:stream";

import { jobDirectory, readJob } from "@/lib/jobs/store";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const KINDS = {
  binary: {
    field: "binary" as const,
    contentType: "application/octet-stream",
  },
  sidecar: {
    field: "sidecar" as const,
    contentType: "application/json",
  },
};

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ id: string; kind: string }> },
) {
  const { id, kind } = await params;

  const selected = KINDS[kind as keyof typeof KINDS];
  if (!selected) {
    return Response.json(
      { error: `unknown file kind ${kind}; expected binary or sidecar` },
      { status: 404 },
    );
  }

  const record = await readJob(id);
  if (!record) {
    return Response.json({ error: `no job with id ${id}` }, { status: 404 });
  }

  if (record.status !== "complete" || !record.outputs) {
    return Response.json(
      {
        error: `job ${id} is ${record.status}; outputs exist only once it is complete`,
        status: record.status,
      },
      // 409: the request is well-formed, the resource simply does not exist
      // yet. A 404 would suggest it never will.
      { status: 409 },
    );
  }

  const filename = record.outputs[selected.field];
  const file = path.join(jobDirectory(id), filename);

  let size: number;
  try {
    size = (await stat(file)).size;
  } catch {
    return Response.json(
      { error: `job ${id} is complete but ${filename} is missing from storage` },
      { status: 410 },
    );
  }

  const stream = Readable.toWeb(
    createReadStream(file),
  ) as unknown as ReadableStream;

  return new Response(stream, {
    headers: {
      "Content-Type": selected.contentType,
      "Content-Length": String(size),
      "Content-Disposition": `attachment; filename="${filename}"`,
      // Output files are immutable once written.
      "Cache-Control": "public, max-age=31536000, immutable",
    },
  });
}
