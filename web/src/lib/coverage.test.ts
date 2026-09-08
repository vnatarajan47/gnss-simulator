import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  ARCHIVE_START,
  DEFAULT_ENABLED,
  SOURCES,
  SWITCHABLE,
  isWithinCoverage,
  normaliseLongitude,
  rinexCodesFor,
  sbasPrnsFor,
  sourceByKey,
  sourcesUnavailableOn,
} from "./coverage.ts";

describe("normaliseLongitude", () => {
  it("leaves ordinary longitudes alone", () => {
    for (const lon of [-179.9, -104.9903, 0, 23.5, 179.9]) {
      assert.equal(normaliseLongitude(lon), lon);
    }
  });

  /**
   * The case that makes this necessary: Leaflet keeps counting past the edge of
   * the world when the user pans onto a repeated copy of it, so a click on
   * Denver can arrive as -465 deg.
   */
  it("folds a wrapped-around longitude back onto the world", () => {
    assert.ok(Math.abs(normaliseLongitude(-464.9903) - -104.9903) < 1e-9);
    assert.ok(Math.abs(normaliseLongitude(255.0103) - -104.9897) < 1e-9);
    assert.ok(Math.abs(normaliseLongitude(720 + 23.5) - 23.5) < 1e-9);
  });

  it("puts the seam at -180 rather than +180", () => {
    assert.equal(normaliseLongitude(180), -180);
    assert.equal(normaliseLongitude(-180), -180);
  });
});

describe("isWithinCoverage", () => {
  it("accepts every real point on Earth", () => {
    assert.ok(isWithinCoverage(0, 0));
    assert.ok(isWithinCoverage(39.7392, -104.9903));
    assert.ok(isWithinCoverage(-77.8419, 166.6863), "McMurdo");
    assert.ok(isWithinCoverage(90, 0), "north pole");
    assert.ok(isWithinCoverage(-90, 180), "south pole");
  });

  it("rejects values that are not coordinates", () => {
    assert.ok(!isWithinCoverage(91, 0), "past the pole");
    assert.ok(!isWithinCoverage(Number.NaN, 0), "an empty numeric field");
    assert.ok(!isWithinCoverage(0, Number.NaN));
  });
});

describe("the source table", () => {
  it("has unique keys", () => {
    const keys = SOURCES.map((s) => s.key);
    assert.equal(new Set(keys).size, keys.length);
  });

  it("only enables validated sources by default", () => {
    for (const key of DEFAULT_ENABLED) {
      const source = sourceByKey(key);
      assert.ok(source, `${key} is not in the source table`);
      assert.equal(source.status, "supported", `${key} is enabled but unvalidated`);
    }
  });

  it("defaults to global constellations only", () => {
    // Augmentation is regional, so enabling one by default would be a guess
    // about where the receiver is that the user cannot see.
    for (const key of DEFAULT_ENABLED) {
      assert.equal(sourceByKey(key)?.kind, "constellation");
    }
  });

  it("gives every augmentation provider a region and a PRN list", () => {
    for (const source of SOURCES.filter((s) => s.kind === "augmentation")) {
      assert.ok(source.region, `${source.key} has no region`);
      assert.ok(source.prns?.length, `${source.key} has no PRNs`);
      assert.equal(source.rinexCode, "S");
    }
  });

  /**
   * `SBAS-OTHER` exists to carry PRNs in the SBAS band with no published
   * operator. If it overlapped an assigned provider it would double-count
   * those satellites in the payload and mislabel them in the UI.
   */
  it("keeps the unassigned-PRN catch-all disjoint from the named providers", () => {
    const named = new Set(
      SOURCES.filter((s) => s.kind === "augmentation" && s.key !== "SBAS-OTHER")
        .flatMap((s) => s.prns ?? []),
    );
    for (const prn of sourceByKey("SBAS-OTHER")?.prns ?? []) {
      assert.ok(!named.has(prn), `PRN ${prn} is both assigned and unassigned`);
    }
  });

  it("keeps every SBAS PRN inside the allocated band", () => {
    for (const source of SOURCES.filter((s) => s.kind === "augmentation")) {
      for (const prn of source.prns ?? []) {
        assert.ok(prn >= 120 && prn <= 158, `PRN ${prn} is outside 120-158`);
      }
    }
  });

  it("switchable is exactly the validated sources", () => {
    assert.deepEqual(
      SWITCHABLE.map((s) => s.key),
      SOURCES.filter((s) => s.status === "supported").map((s) => s.key),
    );
  });
});

describe("rinexCodesFor", () => {
  it("collapses several augmentation providers onto one constellation code", () => {
    // SBAS is one RINEX constellation however many operators are enabled;
    // asking the server for "S" five times would be a bug, not an optimisation.
    assert.deepEqual(rinexCodesFor(["WAAS", "EGNOS", "MSAS"]), ["S"]);
    assert.deepEqual(rinexCodesFor(["GPS", "GALILEO", "WAAS"]), ["E", "G", "S"]);
  });

  it("ignores keys it does not know", () => {
    assert.deepEqual(rinexCodesFor(["GPS", "NOT_A_SYSTEM"]), ["G"]);
  });
});

describe("sbasPrnsFor", () => {
  it("unions the enabled providers' PRNs, sorted", () => {
    assert.deepEqual(sbasPrnsFor(["WAAS", "SOUTHPAN"]), [122, 131, 133, 135, 138]);
  });

  it("is empty when no augmentation is enabled", () => {
    assert.deepEqual(sbasPrnsFor(["GPS", "GALILEO"]), []);
  });
});

describe("sourcesUnavailableOn", () => {
  const days = (...items: string[]) => items;

  it("flags a source whose data starts after the days requested", () => {
    const flagged = sourcesUnavailableOn(["GPS", "WAAS"], days("2019-03-15"));
    assert.deepEqual(flagged.map((s) => s.key), ["WAAS"]);
  });

  /**
   * EGNOS reaches back four years further than the rest of SBAS, which is the
   * whole reason availability is per-source rather than one date for "SBAS".
   */
  it("distinguishes providers with different start dates", () => {
    const flagged = sourcesUnavailableOn(["EGNOS", "WAAS"], days("2022-06-09"));
    assert.deepEqual(flagged.map((s) => s.key), ["WAAS"]);
  });

  it("says nothing when every source covers the window", () => {
    assert.deepEqual(sourcesUnavailableOn(["GPS", "GALILEO", "WAAS"], days("2026-08-08")), []);
  });

  it("judges by the earliest day, since a window can span midnight", () => {
    // 2024-12-31 into 2025-01-01: the earlier day is the one without WAAS.
    const flagged = sourcesUnavailableOn(["WAAS"], days("2024-12-31", "2025-01-01"));
    assert.deepEqual(flagged.map((s) => s.key), ["WAAS"]);
  });

  it("never flags the core constellations, which go back to the archive start", () => {
    assert.deepEqual(
      sourcesUnavailableOn(["GPS", "GALILEO", "BEIDOU", "QZSS"], days(ARCHIVE_START)),
      [],
    );
  });

  it("is empty when nothing has been loaded yet", () => {
    assert.deepEqual(sourcesUnavailableOn(["WAAS"], []), []);
  });
});
