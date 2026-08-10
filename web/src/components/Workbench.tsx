"use client";

import dynamic from "next/dynamic";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import DopPlot from "@/components/DopPlot";
import SkyPlot from "@/components/SkyPlot";
import SourceToggles from "@/components/SourceToggles";
import TimeControls from "@/components/TimeControls";
import {
  ARCHIVE_START,
  DEFAULT_ENABLED,
  isBeforeSbasCoverage,
  rinexCodesFor,
  sbasPrnsFor,
  sourceByKey,
} from "@/lib/coverage";
import { loadEphemerisRange, type EphemerisMeta } from "@/lib/ephemeris";
import {
  chooseStepSeconds,
  inputToUnixSeconds,
  unixSecondsToInput,
  utcDaysSpanned,
  validateInterval,
} from "@/lib/interval";
import { clampCursor, satellitesByEpoch, trackSegments } from "@/lib/trackLayout";
import type { Observer, SkySeries } from "@/lib/types";
import type { Skyplotter } from "@/lib/wasm";

// Leaflet reaches for `window` during module evaluation, so the map cannot be
// server-rendered.
const MapPanel = dynamic(() => import("@/components/MapPanel"), {
  ssr: false,
  loading: () => <Placeholder>Loading map…</Placeholder>,
});

const DEFAULT_OBSERVER: Observer = { lat: 39.7392, lon: -104.9903, altM: 1609 };

/** Default window: six hours from midday on the most recent complete UTC day. */
const DEFAULT_WINDOW_HOURS = 6;

function toInputValue(date: Date): string {
  return date.toISOString().slice(0, 16);
}

/**
 * Yesterday, because today's broadcast file only covers the hours already
 * elapsed — opening on it would show a window the data cannot answer for.
 */
function defaultWindow(): { start: string; end: string } {
  const yesterday = new Date(Date.now() - 86_400_000);
  const day = yesterday.toISOString().slice(0, 10);
  const startS = Date.parse(`${day}T12:00:00Z`) / 1000;
  return {
    start: unixSecondsToInput(startS),
    end: unixSecondsToInput(startS + DEFAULT_WINDOW_HOURS * 3600),
  };
}

export default function Workbench() {
  const [observer, setObserver] = useState<Observer>(DEFAULT_OBSERVER);
  const [{ start, end }, setWindow] = useState(defaultWindow);
  const [maskDeg, setMaskDeg] = useState(5);
  const [enabled, setEnabled] = useState<string[]>(DEFAULT_ENABLED);
  const [cursor, setCursor] = useState(0);

  const [plotter, setPlotter] = useState<Skyplotter | null>(null);
  const [days, setDays] = useState<EphemerisMeta[]>([]);
  const [loadMs, setLoadMs] = useState(0);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [computeError, setComputeError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [series, setSeries] = useState<SkySeries | null>(null);

  const startS = inputToUnixSeconds(start);
  const endS = inputToUnixSeconds(end);
  const problem = useMemo(() => validateInterval({ startS, endS }), [startS, endS]);
  const stepS = useMemo(() => chooseStepSeconds(endS - startS), [startS, endS]);

  // The fetch depends on which UTC days the window touches and on which records
  // we need trimmed from them, so all three participate in the effect key.
  const dayList = useMemo(
    () => (problem ? [] : utcDaysSpanned({ startS, endS })),
    [problem, startS, endS],
  );
  const dayKey = dayList.join(",");
  const rinexCodes = useMemo(() => rinexCodesFor(enabled).join(","), [enabled]);
  const sbasPrns = useMemo(() => sbasPrnsFor(enabled).join(","), [enabled]);
  const maxEpoch = useMemo(() => toInputValue(new Date()), []);

  // Guards against a slow response for an old window overwriting a newer one.
  const latestRequest = useRef(0);

  useEffect(() => {
    if (dayList.length === 0) return;

    const token = ++latestRequest.current;
    setLoading(true);
    setLoadError(null);

    loadEphemerisRange(dayList, rinexCodes, sbasPrns)
      .then(({ plotter: instance, days: info, elapsedMs }) => {
        if (token !== latestRequest.current) return;
        setPlotter(instance);
        setDays(info);
        setLoadMs(elapsedMs);
      })
      .catch((cause: unknown) => {
        if (token !== latestRequest.current) return;
        setPlotter(null);
        setDays([]);
        setSeries(null);
        setLoadError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (token === latestRequest.current) setLoading(false);
      });
    // `dayKey` stands in for `dayList`, whose identity changes every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dayKey, rinexCodes, sbasPrns]);

  // Recompute the series whenever the observer, window, mask or sources change.
  useEffect(() => {
    if (!plotter || problem) return;
    try {
      const result = plotter.skyplot_series(
        observer.lat,
        observer.lon,
        observer.altM,
        startS,
        endS,
        stepS,
        maskDeg,
        enabled,
      ) as SkySeries;
      setSeries(result);
      setComputeError(null);
    } catch (cause: unknown) {
      setSeries(null);
      setComputeError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [plotter, observer, startS, endS, stepS, maskDeg, enabled, problem]);

  // Everything below is derived once per series rather than per slider tick: a
  // 24-hour window holds tens of thousands of samples, and re-deriving them on
  // every drag would be felt.
  const segments = useMemo(() => (series ? trackSegments(series) : []), [series]);
  const byEpoch = useMemo(() => (series ? satellitesByEpoch(series) : []), [series]);

  const epochCount = series?.epochs.length ?? 0;
  const safeCursor = clampCursor(cursor, epochCount);
  const satellites = byEpoch[safeCursor] ?? [];

  // Keep the cursor proportionally where it was when the window is re-sampled,
  // so nudging the end time does not throw the view back to the start.
  const previousCount = useRef(epochCount);
  useEffect(() => {
    if (epochCount === 0 || previousCount.current === epochCount) {
      previousCount.current = epochCount;
      return;
    }
    setCursor((current) => {
      const fraction = previousCount.current > 1 ? current / (previousCount.current - 1) : 0;
      previousCount.current = epochCount;
      return clampCursor(Math.round(fraction * (epochCount - 1)), epochCount);
    });
  }, [epochCount]);

  const onSelect = useCallback((lat: number, lon: number) => {
    // Altitude is not resolved from the map yet — a DEM lookup is phase 3.
    // Ellipsoidal height affects elevation angle by well under 0.01 deg.
    setObserver((previous) => ({ ...previous, lat, lon }));
    setNotice(null);
  }, []);

  const onRejected = useCallback(() => {
    setNotice("Outside the supported region — phase 1 covers the continental US.");
  }, []);

  const error = loadError ?? computeError;
  const anyPartial = days.some((day) => day.partial);
  const showSbasWarning =
    dayList.some(isBeforeSbasCoverage) &&
    enabled.some((key) => sourceByKey(key)?.rinexCode === "S");

  return (
    <main style={styles.page}>
      <header style={styles.header}>
        <h1 style={styles.title}>GNSS sky plot</h1>
        <span style={styles.subtitle}>
          Broadcast ephemeris · computed client-side in WASM
        </span>
      </header>

      <div style={styles.columns}>
        <section style={styles.mapColumn}>
          <div style={styles.mapFrame}>
            <MapPanel observer={observer} onSelect={onSelect} onRejected={onRejected} />
          </div>
          <p style={styles.hint}>
            <span style={styles.legendSwatch} /> Click inside the outlined region to
            move the receiver. Shaded areas are outside phase-1 coverage.
          </p>
          {notice && <p style={styles.notice}>{notice}</p>}
        </section>

        <section style={styles.plotColumn}>
          <SourceToggles
            enabled={enabled}
            onToggle={(key) =>
              setEnabled((previous) =>
                previous.includes(key)
                  ? previous.filter((k) => k !== key)
                  : [...previous, key],
              )
            }
          />

          <TimeControls
            startValue={start}
            endValue={end}
            minValue={`${ARCHIVE_START}T00:00`}
            maxValue={maxEpoch}
            onStartChange={(value) => setWindow((w) => ({ ...w, start: value }))}
            onEndChange={(value) => setWindow((w) => ({ ...w, end: value }))}
            problem={problem}
            epochs={series?.epochs ?? []}
            stepS={stepS}
            cursor={safeCursor}
            onCursorChange={setCursor}
            disabled={loading || !!error}
          />

          <label style={styles.label}>
            Elevation mask: {maskDeg}&deg;
            <input
              type="range"
              min={0}
              max={30}
              step={1}
              value={maskDeg}
              onChange={(event) => setMaskDeg(Number(event.target.value))}
            />
          </label>

          {enabled.length === 0 && (
            <p style={styles.warning}>
              Every source is switched off — turn one on to see satellites.
            </p>
          )}
          {loading && enabled.length > 0 && !problem && (
            <Placeholder>
              Fetching ephemeris for {dayList.join(" and ")}…
            </Placeholder>
          )}
          {error && <p style={styles.error}>{error}</p>}

          {/* SBAS coverage in this archive starts much later than GNSS
              coverage, so an old date silently yields no SBAS satellites.
              Say so rather than leaving the user to wonder. */}
          {!loading && !error && showSbasWarning && (
            <p style={styles.warning}>
              The broadcast archive carries no usable SBAS before 2025 — SBAS
              sources will be empty in this window.
            </p>
          )}

          {anyPartial && !loading && !error && (
            <p style={styles.warning}>
              Today&rsquo;s broadcast file is still accumulating — epochs later than
              the current hour will have no ephemeris.
            </p>
          )}

          {series && !loading && (
            <>
              <SkyPlot
                satellites={satellites}
                elevationMaskDeg={maskDeg}
                tracks={segments}
              />

              <DopPlot series={series} cursor={safeCursor} onCursorChange={setCursor} />

              <dl style={styles.stats}>
                <Stat label="Visible now" value={satellites.length} />
                <Stat label="Tracks in window" value={series.tracks.length} />
                <Stat label="Epochs" value={series.epochs.length} />
              </dl>

              {series.epochsWithoutEphemeris > 0 && (
                <p style={styles.warning}>
                  {series.epochsWithoutEphemeris} of {series.epochs.length} epochs
                  have no ephemeris — the window runs past the end of the
                  broadcast file.
                </p>
              )}

              <table style={styles.table}>
                <thead>
                  <tr>
                    <th style={styles.th}>SV</th>
                    <th style={styles.th}>Source</th>
                    <th style={styles.thNum}>Az&deg;</th>
                    <th style={styles.thNum}>El&deg;</th>
                    <th style={styles.thNum}>Range km</th>
                  </tr>
                </thead>
                <tbody>
                  {satellites.map((satellite) => (
                    <tr key={satellite.sv}>
                      <td style={styles.td}>{satellite.sv}</td>
                      <td style={styles.tdMuted}>{satellite.source}</td>
                      <td style={styles.tdNum}>{satellite.azimuth.toFixed(2)}</td>
                      <td style={styles.tdNum}>{satellite.elevation.toFixed(2)}</td>
                      <td style={styles.tdNum}>{satellite.rangeKm.toFixed(1)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}

          <p style={styles.footnote}>
            Receiver {observer.lat.toFixed(4)}&deg;, {observer.lon.toFixed(4)}&deg;,{" "}
            {observer.altM} m ellipsoidal
            {days.length > 0 && (
              <>
                <br />
                {days.map((day) => day.date).join(" + ")} ·{" "}
                {days.reduce((total, day) => total + day.records, 0)} records ·{" "}
                {days.every((day) => day.cached) ? "cached" : "fetched"} in {loadMs} ms
              </>
            )}
          </p>
        </section>
      </div>
    </main>
  );
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div>
      <dt style={styles.statLabel}>{label}</dt>
      <dd style={styles.statValue}>{value}</dd>
    </div>
  );
}

function Placeholder({ children }: { children: React.ReactNode }) {
  return <p style={styles.placeholder}>{children}</p>;
}

// Inline styles on purpose: phase 1 explicitly does not need styling polish,
// and this keeps the whole UI legible in one file.
const styles: Record<string, React.CSSProperties> = {
  page: { padding: 16, maxWidth: 1400, margin: "0 auto" },
  header: { display: "flex", alignItems: "baseline", gap: 12, marginBottom: 12 },
  title: { fontSize: 20, margin: 0 },
  subtitle: { fontSize: 13, color: "#6b7280" },
  columns: { display: "flex", gap: 20, flexWrap: "wrap", alignItems: "flex-start" },
  mapColumn: { flex: "1 1 520px", minWidth: 320 },
  mapFrame: {
    height: 560,
    border: "1px solid #d1d5db",
    borderRadius: 6,
    overflow: "hidden",
  },
  hint: { fontSize: 12, color: "#6b7280", marginTop: 6 },
  legendSwatch: {
    display: "inline-block",
    width: 18,
    height: 10,
    border: "2px dashed #2563eb",
    marginRight: 6,
    verticalAlign: "middle",
  },
  notice: { fontSize: 12, color: "#b45309", marginTop: 4 },
  plotColumn: { flex: "0 1 460px", minWidth: 320 },
  label: { display: "flex", flexDirection: "column", gap: 4, fontSize: 13, marginBottom: 12 },
  stats: { display: "flex", gap: 24, margin: "12px 0" },
  statLabel: { fontSize: 11, color: "#6b7280", textTransform: "uppercase" },
  statValue: { fontSize: 18, margin: 0, fontVariantNumeric: "tabular-nums" },
  table: { width: "100%", borderCollapse: "collapse", fontSize: 13 },
  th: { textAlign: "left", borderBottom: "1px solid #d1d5db", padding: "4px 6px" },
  thNum: { textAlign: "right", borderBottom: "1px solid #d1d5db", padding: "4px 6px" },
  td: { padding: "3px 6px", borderBottom: "1px solid #f3f4f6" },
  tdNum: {
    padding: "3px 6px",
    borderBottom: "1px solid #f3f4f6",
    textAlign: "right",
    fontVariantNumeric: "tabular-nums",
  },
  tdMuted: {
    padding: "3px 6px",
    borderBottom: "1px solid #f3f4f6",
    color: "#6b7280",
    fontSize: 12,
  },
  placeholder: { color: "#6b7280", fontSize: 14 },
  warning: { color: "#b45309", fontSize: 12 },
  error: { color: "#b91c1c", fontSize: 13, whiteSpace: "pre-wrap" },
  footnote: { fontSize: 12, color: "#6b7280", marginTop: 12, lineHeight: 1.5 },
};
