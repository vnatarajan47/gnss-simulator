/* tslint:disable */
/* eslint-disable */

/**
 * A parsed RINEX navigation file, reusable across many queries.
 *
 * Parsing a daily broadcast file is the expensive part of a sky plot; the
 * geometry itself is microseconds. Holding the parsed set on the JS side
 * keeps map clicks instant.
 */
export class Skyplotter {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Absorb a second RINEX file into this one.
     *
     * Broadcast files are published one per UTC day, but a user-chosen window
     * can straddle midnight. Rather than making the caller juggle two plotters
     * and stitch the results, it loads each day and folds them into one set;
     * duplicate blocks across the overlap are dropped by `EphemerisSet`.
     */
    extend(rinex_nav_data: Uint8Array): void;
    /**
     * Parse a RINEX Nav file, plain or gzipped.
     */
    constructor(rinex_nav_data: Uint8Array);
    /**
     * Compute a sky view for an observer and instant.
     *
     * Arguments match [`compute_skyplot`].
     */
    skyplot(lat: number, lon: number, alt: number, timestamp: number, elevation_mask_deg?: number | null, sources?: string[] | null): any;
    /**
     * Sample the sky view across an interval.
     *
     * * `start_timestamp`, `end_timestamp` -- Unix seconds, inclusive.
     * * `step_s` -- sampling interval in seconds.
     *
     * Computing the whole series in one call rather than looping `skyplot` on
     * the JavaScript side is worth it twice over: it crosses the WASM boundary
     * once instead of hundreds of times, and it keeps the DOP solution beside
     * the geometry it is derived from.
     */
    skyplot_series(lat: number, lon: number, alt: number, start_timestamp: number, end_timestamp: number, step_s: number, elevation_mask_deg?: number | null, sources?: string[] | null): any;
    /**
     * Number of ephemeris blocks held.
     */
    readonly block_count: number;
    /**
     * Number of distinct satellites held.
     */
    readonly satellite_count: number;
}

/**
 * Compute a sky plot from raw RINEX navigation bytes.
 *
 * * `rinex_nav_data` -- contents of a RINEX Nav file, plain or gzipped.
 * * `lat`, `lon` -- observer position \[deg\], WGS-84.
 * * `alt` -- observer height above the ellipsoid \[m\].
 * * `timestamp` -- Unix time \[s\]. Pass `Date.now() / 1000`.
 * * `elevation_mask_deg` -- omit satellites below this; defaults to 5.
 * * `sources` -- which sources to include, e.g. `["GPS", "WAAS"]`. Omit for
 *   the default (GPS only); an empty array yields an empty sky.
 *
 * Re-parses the file on every call. For repeated queries use [`Skyplotter`].
 */
export function compute_skyplot(rinex_nav_data: Uint8Array, lat: number, lon: number, alt: number, timestamp: number, elevation_mask_deg?: number | null, sources?: string[] | null): any;

/**
 * Route Rust panics to `console.error` with a readable stack.
 *
 * Runs automatically on module instantiation; without it a panic surfaces in
 * the browser as an opaque `unreachable executed`.
 */
export function init(): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_skyplotter_free: (a: number, b: number) => void;
    readonly compute_skyplot: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly init: () => void;
    readonly skyplotter_block_count: (a: number) => number;
    readonly skyplotter_extend: (a: number, b: number, c: number) => [number, number];
    readonly skyplotter_new: (a: number, b: number) => [number, number, number];
    readonly skyplotter_satellite_count: (a: number) => number;
    readonly skyplotter_skyplot: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number, number];
    readonly skyplotter_skyplot_series: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => [number, number, number];
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __externref_table_alloc: () => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
