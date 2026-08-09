// Copy cached RINEX Nav files from the repo-level data/ directory into
// public/ so the browser can fetch them.
//
// data/ stays the single source of truth; public/data is a build artifact.
// Runs automatically before `dev` and `build`.

import { cp, mkdir, readdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const source = join(here, "..", "..", "data");
const destination = join(here, "..", "public", "data");

if (!existsSync(source)) {
  console.error(`[sync-data] no data/ directory at ${source}`);
  process.exit(1);
}

const entries = (await readdir(source)).filter(
  (name) => name.endsWith(".rnx") || name.endsWith(".rnx.gz"),
);

if (entries.length === 0) {
  console.error("[sync-data] data/ contains no RINEX Nav files");
  process.exit(1);
}

await mkdir(destination, { recursive: true });
for (const name of entries) {
  await cp(join(source, name), join(destination, name));
}

console.log(`[sync-data] copied ${entries.length} file(s) to public/data`);
