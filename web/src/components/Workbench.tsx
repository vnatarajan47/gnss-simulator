"use client";

import dynamic from "next/dynamic";
import { useCallback, useEffect, useMemo, useState } from "react";

import SkyPlot from "@/components/SkyPlot";
import { loadSkyplotter, type Skyplotter } from "@/lib/wasm";
import type { Observer, SkyView } from "@/lib/types";

// Leaflet reaches for `window` during module evaluation, so the map cannot be
// server-rendered.
const MapPanel = dynamic(() => import("@/components/MapPanel"), {
  ssr: false,
  loading: () => <Placeholder>Loading map…</Placeholder>,
});

/**
 * The RINEX Nav file shipped in `data/`, copied into `public/` by
 * `scripts/sync-data.mjs`. Phase 1 uses a single static file; fetching the
 * right day from CDDIS on demand is a fast-follow.
 */
const NAV_FILE = "/data/BRDC00WRD_R_20250010000_01D_GN.rnx";

/** The UTC day the bundled file covers. */
const COVERAGE = {
  startIso: "2025-01-01T00:00",
  endIso: "2025-01-02T00:00",
  defaultIso: "2025-01-01T12:00",
};

const DEFAULT_OBSERVER: Observer = { lat: 39.7392, lon: -104.9903, altM: 1609 };

/** Interpret a `datetime-local` value as UTC rather than browser-local. */
function isoToUnixSeconds(value: string): number {
  return Date.parse(`${value}:00Z`) / 1000;
}

export default function Workbench() {
  const [plotter, setPlotter] = useState<Skyplotter | null>(null);
  const [observer, setObserver] = useState<Observer>(DEFAULT_OBSERVER);
  const [epoch, setEpoch] = useState(COVERAGE.defaultIso);
  const [maskDeg, setMaskDeg] = useState(5);
  const [view, setView] = useState<SkyView | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Parse the ephemeris file once, then reuse it for every click.
  useEffect(() => {
    let cancelled = false;
    loadSkyplotter(NAV_FILE)
      .then((instance) => {
        if (!cancelled) setPlotter(instance);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(String(cause));
      });
    return () => {
      cancelled = true;
    };
  }, []);

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
      ) as SkyView;
      setView(result);
      setError(null);
    } catch (cause: unknown) {
      setView(null);
      setError(String(cause));
    }
  }, [plotter, observer, epoch, maskDeg]);

  const onSelect = useCallback((lat: number, lon: number) => {
    // Altitude is not resolved from the map yet -- a DEM lookup is phase 3.
    // Ellipsoidal height affects elevation angle by well under 0.01 deg.
    setObserver((previous) => ({ ...previous, lat, lon }));
  }, []);

  const satellites = useMemo(() => view?.satellites ?? [], [view]);

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
            <MapPanel observer={observer} onSelect={onSelect} />
          </div>
          <p style={styles.hint}>Click anywhere on the map to move the receiver.</p>
        </section>

        <section style={styles.plotColumn}>
          <div style={styles.controls}>
            <label style={styles.label}>
              Epoch (UTC)
              <input
                type="datetime-local"
                value={epoch}
                min={COVERAGE.startIso}
                max={COVERAGE.endIso}
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

          {!plotter && !error && <Placeholder>Loading ephemeris…</Placeholder>}
          {error && <p style={styles.error}>{error}</p>}

          {view && (
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
                    <th style={styles.thNum}>Az&deg;</th>
                    <th style={styles.thNum}>El&deg;</th>
                    <th style={styles.thNum}>Range km</th>
                  </tr>
                </thead>
                <tbody>
                  {satellites.map((satellite) => (
                    <tr key={satellite.sv}>
                      <td style={styles.td}>{satellite.sv}</td>
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
  mapFrame: { height: 560, border: "1px solid #d1d5db", borderRadius: 6, overflow: "hidden" },
  hint: { fontSize: 12, color: "#6b7280", marginTop: 6 },
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
  placeholder: { color: "#6b7280", fontSize: 14 },
  error: { color: "#b91c1c", fontSize: 13, whiteSpace: "pre-wrap" },
  footnote: { fontSize: 12, color: "#6b7280", marginTop: 12 },
};
