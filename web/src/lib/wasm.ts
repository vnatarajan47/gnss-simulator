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
export function ensureWasm(): Promise<unknown> {
  if (!instantiation) {
    instantiation = init();
  }
  return instantiation;
}

/** Parse RINEX bytes into a reusable sky-plot engine. */
export async function createSkyplotter(bytes: Uint8Array): Promise<Skyplotter> {
  await ensureWasm();
  return new Skyplotter(bytes);
}

export type { Skyplotter };
