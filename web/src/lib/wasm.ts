/**
 * WASM module loading.
 *
 * `wasm-pack --target web` emits an ES module whose default export fetches and
 * instantiates the `.wasm` binary asynchronously. That has to happen in the
 * browser, so every caller of this module must be inside a client component.
 */

import init, { Skyplotter } from "@/wasm/gnss_wasm";

let instantiation: Promise<unknown> | null = null;

/** Instantiate the WASM module once per page load. */
function ensureInstantiated(): Promise<unknown> {
  if (!instantiation) {
    instantiation = init();
  }
  return instantiation;
}

/**
 * Fetch a RINEX Nav file and hand it to the WASM parser.
 *
 * Parsing is the expensive step (~30 ms for a daily GPS file), so the returned
 * handle is kept alive and reused for every subsequent map click.
 */
export async function loadSkyplotter(url: string): Promise<Skyplotter> {
  const [, response] = await Promise.all([ensureInstantiated(), fetch(url)]);

  if (!response.ok) {
    throw new Error(
      `could not fetch ${url}: ${response.status} ${response.statusText}`,
    );
  }

  const bytes = new Uint8Array(await response.arrayBuffer());
  return new Skyplotter(bytes);
}

export type { Skyplotter };
