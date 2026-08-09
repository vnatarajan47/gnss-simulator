"use client";

import { CONSTELLATIONS, type ConstellationStatus } from "@/lib/coverage";

/** Which constellations are switched on, and what the others are waiting for. */

const SWATCH: Record<ConstellationStatus, { bg: string; fg: string; label: string }> = {
  active: { bg: "#dcfce7", fg: "#166534", label: "on" },
  available: { bg: "#f3f4f6", fg: "#6b7280", label: "off" },
  unsupported: { bg: "#f3f4f6", fg: "#9ca3af", label: "n/a" },
};

export default function ConstellationStatus() {
  return (
    <section style={styles.wrapper}>
      <h2 style={styles.heading}>Constellations</h2>
      <ul style={styles.list}>
        {CONSTELLATIONS.map((constellation) => {
          const swatch = SWATCH[constellation.status];
          return (
            <li key={constellation.code} style={styles.item} title={constellation.note}>
              <span
                style={{
                  ...styles.badge,
                  background: swatch.bg,
                  color: swatch.fg,
                  textDecoration:
                    constellation.status === "unsupported" ? "line-through" : "none",
                }}
              >
                {constellation.name}
              </span>
              <span style={{ ...styles.state, color: swatch.fg }}>{swatch.label}</span>
            </li>
          );
        })}
      </ul>
      <p style={styles.footnote}>
        GPS only in phase 1. Hover for why — the core already propagates Galileo,
        BeiDou and QZSS, but only GPS is validated.
      </p>
    </section>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrapper: { marginBottom: 12 },
  heading: {
    fontSize: 11,
    textTransform: "uppercase",
    color: "#6b7280",
    margin: "0 0 6px",
    letterSpacing: 0.4,
  },
  list: {
    listStyle: "none",
    display: "flex",
    flexWrap: "wrap",
    gap: 6,
    margin: 0,
    padding: 0,
  },
  item: { display: "flex", alignItems: "center", gap: 4, cursor: "help" },
  badge: {
    fontSize: 12,
    padding: "2px 7px",
    borderRadius: 10,
    fontWeight: 600,
  },
  state: { fontSize: 10 },
  footnote: { fontSize: 11, color: "#9ca3af", margin: "6px 0 0" },
};
