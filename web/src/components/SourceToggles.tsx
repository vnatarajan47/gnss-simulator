"use client";

import { SOURCES, type Source, type SourceStatus } from "@/lib/coverage";

/**
 * Click a source to switch it on or off.
 *
 * Only validated sources are switchable. The rest are shown deliberately —
 * knowing *why* SDCM is off is more useful than not seeing it at all — but are
 * rendered as disabled buttons with the reason in the tooltip.
 *
 * The two groups are not cosmetic. Core constellations are global: switching
 * one on adds satellites wherever the receiver is. Augmentation systems are
 * regional, and enabling the wrong one adds nothing at all — so each carries
 * the region it serves, and the group is labelled as regional rather than
 * leaving the user to discover it from an empty plot.
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
  const constellations = SOURCES.filter((s) => s.kind === "constellation");
  const augmentations = SOURCES.filter((s) => s.kind === "augmentation");

  return (
    <section style={styles.wrapper}>
      <Group
        heading="Constellations"
        hint="Global — visible from anywhere."
        sources={constellations}
        enabled={enabled}
        onToggle={onToggle}
      />
      <Group
        heading="Augmentation (SBAS)"
        hint="Regional — each is geostationary over the area it serves, and invisible from the other side of the world."
        sources={augmentations}
        enabled={enabled}
        onToggle={onToggle}
      />
      <p style={styles.footnote}>
        Greyed sources are not selectable — hover for the reason. Only sources
        validated against an independent implementation can be switched on.
      </p>
    </section>
  );
}

function Group({
  heading,
  hint,
  sources,
  enabled,
  onToggle,
}: {
  heading: string;
  hint: string;
  sources: Source[];
  enabled: string[];
  onToggle: (key: string) => void;
}) {
  return (
    <div style={styles.group}>
      <h2 style={styles.heading}>{heading}</h2>
      <div style={styles.row}>
        {sources.map((source) => {
          const isOn = enabled.includes(source.key);
          const switchable = source.status === "supported";
          const palette = PALETTE[source.status];
          const where = source.region ? ` Covers ${source.region}.` : "";

          return (
            <button
              key={source.key}
              type="button"
              onClick={() => switchable && onToggle(source.key)}
              disabled={!switchable}
              aria-pressed={isOn}
              title={
                switchable
                  ? `${source.note}${where} Click to turn ${isOn ? "off" : "on"}.`
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
      <p style={styles.hint}>{hint}</p>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrapper: { marginBottom: 12 },
  group: { marginBottom: 8 },
  heading: {
    fontSize: 11,
    textTransform: "uppercase",
    color: "#6b7280",
    margin: "0 0 6px",
    letterSpacing: 0.4,
  },
  row: { display: "flex", flexWrap: "wrap", gap: 6 },
  hint: { fontSize: 11, color: "#9ca3af", margin: "4px 0 0" },
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
