"use client";

import { SOURCES, type SourceStatus } from "@/lib/coverage";

/**
 * Click a source to switch it on or off.
 *
 * Only validated sources are switchable. The rest are shown deliberately —
 * knowing *why* Galileo is off is more useful than not seeing it at all — but
 * are rendered as disabled buttons with the reason in the tooltip.
 */

const PALETTE: Record<
  SourceStatus,
  { on: React.CSSProperties; off: React.CSSProperties }
> = {
  supported: {
    on: { background: "#166534", color: "#ffffff", borderColor: "#166534" },
    off: { background: "#ffffff", color: "#374151", borderColor: "#9ca3af" },
  },
  unvalidated: {
    on: {},
    off: { background: "#f9fafb", color: "#9ca3af", borderColor: "#e5e7eb" },
  },
  unsupported: {
    on: {},
    off: {
      background: "#f9fafb",
      color: "#9ca3af",
      borderColor: "#e5e7eb",
      textDecoration: "line-through",
    },
  },
};

export default function SourceToggles({
  enabled,
  onToggle,
}: {
  enabled: string[];
  onToggle: (key: string) => void;
}) {
  return (
    <section style={styles.wrapper}>
      <h2 style={styles.heading}>Sources</h2>
      <div style={styles.row}>
        {SOURCES.map((source) => {
          const isOn = enabled.includes(source.key);
          const switchable = source.status === "supported";
          const palette = PALETTE[source.status];

          return (
            <button
              key={source.key}
              type="button"
              onClick={() => switchable && onToggle(source.key)}
              disabled={!switchable}
              aria-pressed={isOn}
              title={
                switchable
                  ? `${source.note} Click to turn ${isOn ? "off" : "on"}.`
                  : `Not selectable — ${source.note}`
              }
              style={{
                ...styles.chip,
                ...(isOn ? palette.on : palette.off),
                cursor: switchable ? "pointer" : "not-allowed",
              }}
            >
              {source.label}
            </button>
          );
        })}
      </div>
      <p style={styles.footnote}>
        Greyed sources are not selectable yet — hover for the reason. Only
        validated sources can be switched on.
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
  row: { display: "flex", flexWrap: "wrap", gap: 6 },
  chip: {
    fontSize: 12,
    fontWeight: 600,
    padding: "3px 10px",
    borderRadius: 12,
    borderWidth: 1,
    borderStyle: "solid",
    fontFamily: "inherit",
    lineHeight: 1.6,
  },
  footnote: { fontSize: 11, color: "#9ca3af", margin: "6px 0 0" },
};
