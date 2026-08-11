/**
 * Job request and record shapes, mirroring `crates/gnss-iq/src/job.rs`.
 *
 * ## Which side validates what
 *
 * This module checks *structure*: that the fields exist, are the right type,
 * and name a variant that exists. `JobSpec::validate` in Rust checks *physics
 * and limits*: sample rate against the code's main lobe, output size against
 * the disk cap, centre frequency against the sampled band.
 *
 * The split is deliberate rather than lazy. Structural errors have to be
 * caught here because a malformed request should never reach a subprocess, and
 * because `400 Bad Request` is the honest status for them. Physical limits are
 * checked once, in the crate that owns the constants -- duplicating them in
 * TypeScript would guarantee the two drift, and the drift would show as a job
 * that submits successfully and then fails a second later.
 */

/** Bumped when the wire format changes; matches SCHEMA_VERSION in Rust. */
export const JOB_SCHEMA_VERSION = 1;

export type Quantization = "int8" | "int16" | "float32";

export type ConstellationName = "gps" | "galileo" | "beidou" | "qzss" | "sbas";

export type BandName = "l1" | "l2" | "l5" | "e1" | "e5a" | "e5b" | "b1i" | "b2a";

export type ReceiverSpec = {
  kind: "static";
  latitude_deg: number;
  longitude_deg: number;
  altitude_m: number;
};

export type WindowSpec = {
  start_unix_s: number;
  duration_s: number;
  epoch_interval_s: number;
};

export type OutputSpec = {
  sample_rate_hz: number;
  quantization: Quantization;
  center_frequency_hz: number;
};

export type PseudorangeNoiseSpec = { model: "fixed"; sigma_m: number };

export type Cn0Spec = { model: "elevation" };

export type NoiseSpec = {
  pseudorange: PseudorangeNoiseSpec;
  cn0: Cn0Spec;
};

export type JobSpec = {
  constellation: ConstellationName;
  band: BandName;
  receiver: ReceiverSpec;
  window: WindowSpec;
  output: OutputSpec;
  elevation_mask_deg: number;
  noise: NoiseSpec;
  seed?: number;
};

/**
 * Job lifecycle.
 *
 * `queued` and `processing` are non-terminal; `complete` and `failed` are
 * terminal. Nothing outside this module should test status by string equality
 * against a terminal state -- use `isTerminal`, so that adding `cancelled`
 * later does not require finding every comparison.
 */
export type JobStatus = "queued" | "processing" | "complete" | "failed";

export function isTerminal(status: JobStatus): boolean {
  return status === "complete" || status === "failed";
}

export type JobOutputs = {
  binary: string;
  sidecar: string;
  binary_bytes: number;
  sample_count: number;
  satellites: number;
};

export type JobRecord = {
  id: string;
  status: JobStatus;
  schema_version: number;
  spec: JobSpec;
  /** ISO 8601 UTC. */
  submitted_at: string;
  started_at?: string;
  finished_at?: string;
  /** 0 to 1 during synthesis; absent before it starts. */
  progress?: number;
  /** Human-readable stage, for a status display. */
  stage?: string;
  error?: string;
  outputs?: JobOutputs;
  /** Broadcast files the worker was given. */
  ephemeris_files?: string[];
};

/** Defaults applied to omitted optional request fields. */
export const JOB_DEFAULTS = {
  /** 5 degrees is the usual survey-receiver mask, matching SkyplotOptions. */
  elevationMaskDeg: 5,
  /** 1 s of geometry per 100 ms is fine for a static receiver. */
  epochIntervalSeconds: 0.1,
  /** Just above the 2.046 MHz main lobe of the C/A code. */
  sampleRateHz: 2.6e6,
  quantization: "int8" as Quantization,
  pseudorangeSigmaM: 1.5,
};

const CONSTELLATIONS: ConstellationName[] = [
  "gps",
  "galileo",
  "beidou",
  "qzss",
  "sbas",
];
const BANDS: BandName[] = ["l1", "l2", "l5", "e1", "e5a", "e5b", "b1i", "b2a"];
const QUANTIZATIONS: Quantization[] = ["int8", "int16", "float32"];

export class JobRequestError extends Error {}

function requireFiniteNumber(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new JobRequestError(`${field} must be a finite number`);
  }
  return value;
}

function requireOneOf<T extends string>(
  value: unknown,
  allowed: readonly T[],
  field: string,
): T {
  if (typeof value !== "string" || !allowed.includes(value as T)) {
    throw new JobRequestError(
      `${field} must be one of ${allowed.join(", ")}; got ${JSON.stringify(value)}`,
    );
  }
  return value as T;
}

function asObject(value: unknown, field: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new JobRequestError(`${field} must be an object`);
  }
  return value as Record<string, unknown>;
}

/**
 * Parse a submitted request body into a `JobSpec`.
 *
 * Throws `JobRequestError` with a message naming the offending field. Fills in
 * documented defaults for everything optional, so a minimal request -- a
 * position and a time window -- is enough to submit a job.
 */
export function parseJobRequest(body: unknown): JobSpec {
  const root = asObject(body, "request body");

  const constellation = requireOneOf(
    root.constellation ?? "gps",
    CONSTELLATIONS,
    "constellation",
  );
  const band = requireOneOf(root.band ?? "l1", BANDS, "band");

  const receiverRaw = asObject(root.receiver, "receiver");
  const kind = receiverRaw.kind ?? "static";
  if (kind !== "static") {
    // Trajectory support is future work. Rejecting by name rather than by
    // shape means the message says what is missing instead of complaining
    // about an unexpected field.
    throw new JobRequestError(
      `receiver.kind must be "static"; moving receivers are not supported yet`,
    );
  }
  const receiver: ReceiverSpec = {
    kind: "static",
    latitude_deg: requireFiniteNumber(
      receiverRaw.latitude_deg,
      "receiver.latitude_deg",
    ),
    longitude_deg: requireFiniteNumber(
      receiverRaw.longitude_deg,
      "receiver.longitude_deg",
    ),
    altitude_m: requireFiniteNumber(
      receiverRaw.altitude_m ?? 0,
      "receiver.altitude_m",
    ),
  };

  const windowRaw = asObject(root.window, "window");
  const window: WindowSpec = {
    start_unix_s: requireFiniteNumber(
      windowRaw.start_unix_s,
      "window.start_unix_s",
    ),
    duration_s: requireFiniteNumber(windowRaw.duration_s, "window.duration_s"),
    epoch_interval_s: requireFiniteNumber(
      windowRaw.epoch_interval_s ?? JOB_DEFAULTS.epochIntervalSeconds,
      "window.epoch_interval_s",
    ),
  };

  const outputRaw = asObject(root.output ?? {}, "output");
  const sampleRate = requireFiniteNumber(
    outputRaw.sample_rate_hz ?? JOB_DEFAULTS.sampleRateHz,
    "output.sample_rate_hz",
  );
  const output: OutputSpec = {
    sample_rate_hz: sampleRate,
    quantization: requireOneOf(
      outputRaw.quantization ?? JOB_DEFAULTS.quantization,
      QUANTIZATIONS,
      "output.quantization",
    ),
    // Defaults to the signal's own carrier, i.e. a true baseband recording.
    // Resolved here rather than in Rust so the stored record says explicitly
    // what was used instead of leaving it implicit.
    center_frequency_hz: requireFiniteNumber(
      outputRaw.center_frequency_hz ?? carrierFor(band),
      "output.center_frequency_hz",
    ),
  };

  const noiseRaw = asObject(root.noise ?? {}, "noise");
  const pseudorangeRaw = asObject(
    noiseRaw.pseudorange ?? { model: "fixed" },
    "noise.pseudorange",
  );
  if ((pseudorangeRaw.model ?? "fixed") !== "fixed") {
    throw new JobRequestError(
      `noise.pseudorange.model must be "fixed"; no other pseudorange noise model is implemented`,
    );
  }
  const cn0Raw = asObject(noiseRaw.cn0 ?? { model: "elevation" }, "noise.cn0");
  if ((cn0Raw.model ?? "elevation") !== "elevation") {
    throw new JobRequestError(
      `noise.cn0.model must be "elevation"; no other C/N0 model is implemented`,
    );
  }
  // The elevation model's coefficients are constants of the model, not job
  // parameters. Silently ignoring them would leave a user believing a value
  // they supplied had taken effect.
  for (const key of ["cn0_max", "attenuation", "decay_deg"]) {
    if (key in cn0Raw) {
      throw new JobRequestError(
        `noise.cn0.${key} is not a job parameter; the elevation C/N0 model's ` +
          `coefficients are fixed and are recorded in the output sidecar`,
      );
    }
  }

  const noise: NoiseSpec = {
    pseudorange: {
      model: "fixed",
      sigma_m: requireFiniteNumber(
        pseudorangeRaw.sigma_m ?? JOB_DEFAULTS.pseudorangeSigmaM,
        "noise.pseudorange.sigma_m",
      ),
    },
    cn0: { model: "elevation" },
  };

  const spec: JobSpec = {
    constellation,
    band,
    receiver,
    window,
    output,
    elevation_mask_deg: requireFiniteNumber(
      root.elevation_mask_deg ?? JOB_DEFAULTS.elevationMaskDeg,
      "elevation_mask_deg",
    ),
    noise,
  };

  if (root.seed !== undefined && root.seed !== null) {
    const seed = requireFiniteNumber(root.seed, "seed");
    if (!Number.isInteger(seed) || seed < 0) {
      throw new JobRequestError("seed must be a non-negative integer");
    }
    spec.seed = seed;
  }

  return spec;
}

/** Nominal carrier frequency of a band [Hz]. Mirrors `Band::centre_hz`. */
export function carrierFor(band: BandName): number {
  switch (band) {
    case "l1":
    case "e1":
    case "b1i":
      return 1_575.42e6;
    case "l2":
      return 1_227.6e6;
    case "l5":
    case "e5a":
    case "b2a":
      return 1_176.45e6;
    case "e5b":
      return 1_207.14e6;
  }
}
