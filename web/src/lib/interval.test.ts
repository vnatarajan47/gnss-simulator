/**
 * Time-window selection invariants.
 *
 * These decide the slider's resolution and which broadcast files get fetched,
 * so getting them wrong is either a silently coarse plot or a missing day of
 * ephemeris — both of which look like a data problem rather than a bug.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  MAX_INTERVAL_HOURS,
  MAX_SAMPLES,
  chooseStepSeconds,
  formatStep,
  inputToUnixSeconds,
  sampleCount,
  unixSecondsToInput,
  utcDaysSpanned,
  validateInterval,
} from "./interval.ts";

describe("sample step", () => {
  it("keeps every window within the sample ceiling", () => {
    // Sweep durations rather than spot-checking: the ladder has to hold
    // everywhere, including at the awkward values between round hours.
    for (let minutes = 5; minutes <= MAX_INTERVAL_HOURS * 60; minutes += 5) {
      const durationS = minutes * 60;
      const step = chooseStepSeconds(durationS);
      const count = sampleCount(durationS, step);
      assert.ok(
        count <= MAX_SAMPLES,
        `${minutes} min gave ${count} samples at ${step} s`,
      );
      assert.ok(count >= 2, `${minutes} min collapsed to ${count} samples`);
    }
  });

  it("gives short windows fine resolution and long ones coarse", () => {
    // The point of the ladder: resolution is spent where it is useful.
    assert.ok(chooseStepSeconds(15 * 60) <= 5);
    assert.ok(chooseStepSeconds(60 * 60) <= 10);
    assert.ok(chooseStepSeconds(24 * 3600) <= 300);
  });

  it("is monotonic — a longer window never samples more finely", () => {
    let previous = 0;
    for (let minutes = 5; minutes <= MAX_INTERVAL_HOURS * 60; minutes += 5) {
      const step = chooseStepSeconds(minutes * 60);
      assert.ok(step >= previous, `step shrank at ${minutes} min`);
      previous = step;
    }
  });

  it("picks steps that land on readable clock times", () => {
    // Steps must divide a minute or an hour, or epoch labels read 01:13:17.
    for (let minutes = 5; minutes <= MAX_INTERVAL_HOURS * 60; minutes += 5) {
      const step = chooseStepSeconds(minutes * 60);
      const divides = step < 60 ? 60 % step === 0 : step % 60 === 0 && 3600 % step === 0;
      assert.ok(divides, `${step} s does not divide a minute or an hour`);
    }
  });

  it("survives a nonsense duration", () => {
    assert.ok(chooseStepSeconds(0) > 0);
    assert.ok(chooseStepSeconds(-1) > 0);
    assert.ok(chooseStepSeconds(Number.NaN) > 0);
  });
});

describe("interval validation", () => {
  const at = (iso: string) => Date.parse(iso) / 1000;

  it("accepts an ordinary window", () => {
    assert.equal(
      validateInterval({
        startS: at("2026-08-08T00:00:00Z"),
        endS: at("2026-08-08T06:00:00Z"),
      }),
      null,
    );
  });

  it("rejects a reversed window", () => {
    const problem = validateInterval({
      startS: at("2026-08-08T06:00:00Z"),
      endS: at("2026-08-08T00:00:00Z"),
    });
    assert.equal(problem?.kind, "reversed");
  });

  it("rejects a window past the cap but accepts exactly the cap", () => {
    const startS = at("2026-08-08T00:00:00Z");
    assert.equal(
      validateInterval({ startS, endS: startS + MAX_INTERVAL_HOURS * 3600 }),
      null,
      "the cap itself must be allowed",
    );
    assert.equal(
      validateInterval({ startS, endS: startS + MAX_INTERVAL_HOURS * 3600 + 60 })?.kind,
      "too-long",
    );
  });

  it("rejects a window too short to be a series", () => {
    const startS = at("2026-08-08T00:00:00Z");
    assert.equal(validateInterval({ startS, endS: startS + 60 })?.kind, "too-short");
  });
});

describe("UTC day spanning", () => {
  const at = (iso: string) => Date.parse(iso) / 1000;

  it("returns one day for a window inside a day", () => {
    assert.deepEqual(
      utcDaysSpanned({
        startS: at("2026-08-08T01:00:00Z"),
        endS: at("2026-08-08T23:00:00Z"),
      }),
      ["2026-08-08"],
    );
  });

  it("returns both days for a window crossing midnight", () => {
    // The case that makes multi-day loading necessary: an epoch just after
    // midnight has its nearest time-of-ephemeris in the previous day's file.
    assert.deepEqual(
      utcDaysSpanned({
        startS: at("2026-08-08T22:00:00Z"),
        endS: at("2026-08-09T02:00:00Z"),
      }),
      ["2026-08-08", "2026-08-09"],
    );
  });

  it("handles a window that starts exactly at midnight", () => {
    assert.deepEqual(
      utcDaysSpanned({
        startS: at("2026-08-08T00:00:00Z"),
        endS: at("2026-08-08T12:00:00Z"),
      }),
      ["2026-08-08"],
    );
  });

  it("never returns more than two days at the interval cap", () => {
    // A 24 h window can straddle at most one midnight, so at most two files.
    // If this ever fails the fetch logic silently under-loads.
    for (let hour = 0; hour < 24; hour += 1) {
      const startS = at(`2026-08-08T${String(hour).padStart(2, "0")}:00:00Z`);
      const days = utcDaysSpanned({ startS, endS: startS + MAX_INTERVAL_HOURS * 3600 });
      assert.ok(days.length <= 2, `${hour}:00 spanned ${days.length} days`);
    }
  });
});

describe("formatting", () => {
  it("round-trips a datetime-local value through UTC seconds", () => {
    const value = "2026-08-08T14:30";
    assert.equal(unixSecondsToInput(inputToUnixSeconds(value)), value);
  });

  it("renders steps readably", () => {
    assert.equal(formatStep(30), "30 s");
    assert.equal(formatStep(120), "2 min");
    assert.equal(formatStep(300), "5 min");
  });
});
