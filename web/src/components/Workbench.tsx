"use client";

import dynamic from "next/dynamic";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import SkyPlot from "@/components/SkyPlot";
import SourceToggles from "@/components/SourceToggles";
import {
  ARCHIVE_START,
  DEFAULT_ENABLED,
  isBeforeSbasCoverage,
  rinexCodesFor,
  sbasPrnsFor,
  sourceByKey,
} from "@/lib/coverage";
import { dateOf, loadEphemeris, type EphemerisMeta } from "@/lib/ephemeris";
import type { Observer, SkyView } from "@/lib/types";
import type { Skyplotter } from "@/lib/wasm";

// Leaflet reaches for `window` during module evaluation, so the map cannot be
// server-rendered.
const MapPanel = dynamic(() => import("@/components/MapPanel"), {
  ssr: false,
  loading: () => <Placeholder>Loading map…</Placeholder>,
});

const DEFAULT_OBSERVER: Observer = { lat: 39.7392, lon: -104.9903, altM: 1609 };

/** `datetime-local` wants `YYYY-MM-DDTHH:MM`; we treat the value as UTC. */
function toInputValue(date: Date): string {
  return date.toISOString().slice(0, 16);
}

/**
 * Default to midday on the most recent complete UTC day.
 *
 * Today's broadcast file only covers the hours already elapsed, so yesterday
 * avoids opening on an epoch the data cannot answer for.
 */
function defaultEpoch(): string {
  const yesterday = new Date(Date.now() - 86_400_000);
  return `${yesterday.toISOString().slice(0, 10)}T12:00`;
}

function isoToUnixSeconds(value: string): number {
  return Date.parse(`${value}:00Z`) / 1000;
}

export default function Workbench() {
  const [observer, setObserver] = useState<Observer>(DEFAULT_OBSERVER);
  const [epoch, setEpoch] = useState(defaultEpoch);
  const [maskDeg, setMaskDeg] = useState(5);
  const [enabled, setEnabled] = useState<string[]>(DEFAULT_ENABLED);

  const [plotter, setPlotter] = useState<Skyplotter | null>(null);
  const [meta, setMeta] = useState<EphemerisMeta | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [computeError, setComputeError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [view, setView] = useState<SkyView | null>(null);

  const requestedDate = dateOf(epoch);
  // The fetch depends on the day *and* on which records we need trimmed from
  // it, so both participate in the effect key.
  const rinexCodes = useMemo(() => rinexCodesFor(enabled).join(","), [enabled]);
  const sbasPrns = useMemo(() => sbasPrnsFor(enabled).join(","), [enabled]);
  const maxEpoch = useMemo(() => toInputValue(new Date()), []);

  // Guards against a slow response for an old date overwriting a newer one.
  const latestRequest = useRef(0);

  // Refetch only when the UTC *day* changes — moving the clock within a day
  // reuses the parsed ephemeris.
  useEffect(() => {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(requestedDate)) return;

    const token = ++latestRequest.current;
    setLoading(true);
    setLoadError(null);

    loadEphemeris(requestedDate, rinexCodes, sbasPrns)
      .then(({ plotter: instance, meta: info }) => {
        if (token !== latestRequest.current) return;
        setPlotter(instance);
        setMeta(info);
      })
      .catch((cause: unknown) => {
        if (token !== latestRequest.current) return;
        setPlotter(null);
        setMeta(null);
        setView(null);
        setLoadError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (token === latestRequest.current) setLoading(false);
      });
  }, [requestedDate, rinexCodes, sbasPrns]);

  // Recompute whenever the observer, epoch or mask changes.
  useEffect(() => {
    if (!plotter) return;
    try {
      const result = plotter.skyplot(
        observer.lat,
        observer.lon,
        observer.altM,
        isoToUnixSeconds(epoch),
        maskDeg,
        enabled,
      ) as SkyView;
      setView(result);
      setComputeError(null);
    } catch (cause: unknown) {
      setView(null);
      setComputeError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [plotter, observer, epoch, maskDeg, enabled]);

  const onSelect = useCallback((lat: number, lon: number) => {
    // Altitude is not resolved from the map yet — a DEM lookup is phase 3.
    // Ellipsoidal height affects elevation angle by well under 0.01 deg.
    setObserver((previous) => ({ ...previous, lat, lon }));
    setNotice(null);
  }, []);

  const onRejected = useCallback(() => {
    setNotice("Outside the supported region — phase 1 covers the continental US.");
  }, []);

  const satellites = useMemo(() => view?.satellites ?? [], [view]);
  const error = loadError ?? computeError;

  return (
    <main style={styles.page}>
      <header style={styles.header}>
        <h1 style={styles.title}>GNSS sky plot</h1>
        <span style={styles.subtitle}>
          GPS broadcast ephemeris · computed client-side in WASM
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

          <div style={styles.controls}>
            <label style={styles.label}>
              Epoch (UTC)
              <input
                type="datetime-local"
                value={epoch}
                min={`${ARCHIVE_START}T00:00`}
                max={maxEpoch}
                onChange={(event) => setEpoch(event.target.value)}
                style={styles.input}
              />
            </label>
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
          </div>

          {enabled.length === 0 && (
            <p style={styles.warning}>
              Every source is switched off — turn one on to see satellites.
            </p>
          )}
          {loading && enabled.length > 0 && (
            <Placeholder>Fetching ephemeris for {requestedDate}…</Placeholder>
          )}
          {error && <p style={styles.error}>{error}</p>}

          {/* SBAS coverage in this archive starts much later than GNSS
              coverage, so an old date silently yields no SBAS satellites.
              Say so rather than leaving the user to wonder. */}
          {!loading &&
            !error &&
            isBeforeSbasCoverage(requestedDate) &&
            enabled.some((key) => sourceByKey(key)?.rinexCode === "S") && (
              <p style={styles.warning}>
                The broadcast archive carries no usable SBAS before{" "}
                {"2025"} — SBAS sources will be empty at this epoch.
              </p>
            )}

          {meta?.partial && !loading && !error && (
            <p style={styles.warning}>
              Today&rsquo;s broadcast file is still accumulating — epochs later than
              the current hour will have no ephemeris.
            </p>
          )}

          {view && !loading && (
            <>
              <SkyPlot satellites={satellites} elevationMaskDeg={maskDeg} />
              <dl style={styles.stats}>
                <Stat label="Visible" value={view.visibleCount} />
                <Stat label="Below mask" value={view.belowMask} />
                <Stat label="No ephemeris" value={view.withoutEphemeris} />
              </dl>
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
            {meta && (
              <>
                <br />
                {meta.source} · {meta.records} records ·{" "}
                {meta.cached ? "cached" : "fetched"} in {meta.elapsedMs} ms
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
  controls: { display: "flex", flexDirection: "column", gap: 8, marginBottom: 12 },
  label: { display: "flex", flexDirection: "column", gap: 4, fontSize: 13 },
  input: { padding: 4, fontSize: 13 },
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
