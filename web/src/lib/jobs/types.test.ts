import assert from "node:assert/strict";
import test, { describe } from "node:test";

import {
  JobRequestError,
  carrierFor,
  isTerminal,
  parseJobRequest,
  JOB_DEFAULTS,
} from "./types.ts";

/** The smallest request that should be accepted: where and when. */
const minimal = {
  receiver: { latitude_deg: 39.7392, longitude_deg: -104.9903 },
  window: { start_unix_s: 1_735_732_800, duration_s: 10 },
};

describe("job request parsing", () => {
  test("a minimal request fills in every documented default", () => {
    const spec = parseJobRequest(minimal);

    assert.equal(spec.constellation, "gps");
    assert.equal(spec.band, "l1");
    assert.equal(spec.receiver.kind, "static");
    assert.equal(spec.receiver.altitude_m, 0);
    assert.equal(spec.window.epoch_interval_s, JOB_DEFAULTS.epochIntervalSeconds);
    assert.equal(spec.output.sample_rate_hz, JOB_DEFAULTS.sampleRateHz);
    assert.equal(spec.output.quantization, JOB_DEFAULTS.quantization);
    assert.equal(spec.elevation_mask_deg, JOB_DEFAULTS.elevationMaskDeg);
    assert.equal(spec.noise.pseudorange.sigma_m, JOB_DEFAULTS.pseudorangeSigmaM);
    assert.equal(spec.noise.cn0.model, "elevation");
    // Omitted seed stays omitted; the worker chooses one and records it.
    assert.equal(spec.seed, undefined);
  });

  test("the centre frequency defaults to the band's own carrier", () => {
    // Left at the carrier the recording is true baseband, which is the useful
    // default; anything else has to be asked for.
    assert.equal(parseJobRequest(minimal).output.center_frequency_hz, 1_575.42e6);

    const withIf = parseJobRequest({
      ...minimal,
      output: { center_frequency_hz: 1_575.42e6 - 4e6 },
    });
    assert.equal(withIf.output.center_frequency_hz, 1_571.42e6);
  });

  test("explicit values override every default", () => {
    const spec = parseJobRequest({
      ...minimal,
      constellation: "gps",
      band: "l1",
      receiver: { ...minimal.receiver, altitude_m: 1609 },
      window: { ...minimal.window, epoch_interval_s: 0.05 },
      output: { sample_rate_hz: 5e6, quantization: "float32" },
      elevation_mask_deg: 12.5,
      noise: { pseudorange: { model: "fixed", sigma_m: 0.4 } },
      seed: 99,
    });

    assert.equal(spec.receiver.altitude_m, 1609);
    assert.equal(spec.window.epoch_interval_s, 0.05);
    assert.equal(spec.output.sample_rate_hz, 5e6);
    assert.equal(spec.output.quantization, "float32");
    assert.equal(spec.elevation_mask_deg, 12.5);
    assert.equal(spec.noise.pseudorange.sigma_m, 0.4);
    assert.equal(spec.seed, 99);
  });

  test("a zero sigma survives, rather than being treated as absent", () => {
    // `?? default` rather than `|| default` is what makes this work, and a
    // noise-free reference run is a thing users legitimately want.
    const spec = parseJobRequest({
      ...minimal,
      noise: { pseudorange: { model: "fixed", sigma_m: 0 } },
    });
    assert.equal(spec.noise.pseudorange.sigma_m, 0);

    // Same trap for a zero elevation mask.
    assert.equal(
      parseJobRequest({ ...minimal, elevation_mask_deg: 0 }).elevation_mask_deg,
      0,
    );
  });

  test("missing required fields are named in the error", () => {
    for (const [body, expected] of [
      [{}, "receiver"],
      [{ receiver: minimal.receiver }, "window"],
      [{ ...minimal, receiver: { longitude_deg: 0 } }, "latitude_deg"],
      [{ ...minimal, window: { duration_s: 10 } }, "start_unix_s"],
    ] as const) {
      assert.throws(
        () => parseJobRequest(body),
        (error: Error) => {
          assert.ok(error instanceof JobRequestError);
          assert.match(error.message, new RegExp(expected));
          return true;
        },
        `expected a message mentioning ${expected}`,
      );
    }
  });

  test("unknown enum members are rejected with the allowed set", () => {
    assert.throws(
      () => parseJobRequest({ ...minimal, constellation: "glonass" }),
      /must be one of gps, galileo, beidou, qzss, sbas/,
    );
    assert.throws(
      () => parseJobRequest({ ...minimal, output: { quantization: "int32" } }),
      /must be one of int8, int16, float32/,
    );
  });

  test("bands with no implementation still parse, and fail in the crate", () => {
    // The schema accepts every real band on purpose: adding GPS L5 should be a
    // resolution arm in Rust, not a schema change here. What must not happen is
    // a request for L5 being silently treated as L1.
    const spec = parseJobRequest({ ...minimal, band: "l5" });
    assert.equal(spec.band, "l5");
    assert.equal(spec.output.center_frequency_hz, 1_176.45e6);
  });

  test("a moving receiver is refused by name, not by shape", () => {
    assert.throws(
      () => parseJobRequest({ ...minimal, receiver: { kind: "trajectory" } }),
      /moving receivers are not supported yet/,
    );
  });

  test("C/N0 coefficients are refused rather than ignored", () => {
    // Accepting and dropping them would leave a user believing they had tuned
    // the model.
    for (const key of ["cn0_max", "attenuation", "decay_deg"]) {
      assert.throws(
        () =>
          parseJobRequest({
            ...minimal,
            noise: { cn0: { model: "elevation", [key]: 40 } },
          }),
        new RegExp(`noise.cn0.${key} is not a job parameter`),
      );
    }
  });

  test("non-numeric and non-finite values are rejected", () => {
    for (const value of ["10", null, Number.NaN, Number.POSITIVE_INFINITY, {}]) {
      assert.throws(
        () =>
          parseJobRequest({
            ...minimal,
            window: { ...minimal.window, duration_s: value },
          }),
        JobRequestError,
        `duration_s = ${String(value)} should have been refused`,
      );
    }
  });

  test("a fractional or negative seed is refused", () => {
    assert.throws(() => parseJobRequest({ ...minimal, seed: 1.5 }), /integer/);
    assert.throws(() => parseJobRequest({ ...minimal, seed: -1 }), /integer/);
    // Explicit null means "not supplied", same as omitting it.
    assert.equal(parseJobRequest({ ...minimal, seed: null }).seed, undefined);
  });

  test("a non-object body is refused before any field is read", () => {
    for (const body of [null, 42, "hello", [1, 2]]) {
      assert.throws(() => parseJobRequest(body), /must be an object/);
    }
  });
});

describe("job status", () => {
  test("only finished states are terminal", () => {
    assert.equal(isTerminal("queued"), false);
    assert.equal(isTerminal("processing"), false);
    assert.equal(isTerminal("complete"), true);
    assert.equal(isTerminal("failed"), true);
  });
});

describe("band carriers", () => {
  test("bands sharing a carrier agree, matching the Rust table", () => {
    assert.equal(carrierFor("l1"), carrierFor("e1"));
    assert.equal(carrierFor("l1"), carrierFor("b1i"));
    assert.equal(carrierFor("l5"), carrierFor("e5a"));
    assert.equal(carrierFor("l5"), carrierFor("b2a"));
    assert.ok(carrierFor("l2") < carrierFor("l1"));
  });
});
