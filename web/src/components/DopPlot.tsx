"use client";

import { useMemo } from "react";

import {
  HEIGHT,
  MARGIN,
  PLOT_HEIGHT,
  WIDTH,
  dopChartLayout,
  epochAtViewBoxX,
  xForEpoch,
  yForValue,
} from "@/lib/dopChart";
import { formatClock } from "@/lib/interval";
import type { SkySeries } from "@/lib/types";

/**
 * HDOP and VDOP against time, sharing the sky plot's cursor.
 *
 * Both views are driven by the same integer epoch index rather than by a
 * timestamp each converts independently, so the two cursors cannot drift apart.
 *
 * Plain SVG for the same reason the sky plot is: two polylines, two axes and a
 * cursor do not need a charting library, and the parts with real invariants
 * (gaps, scaling) live in `lib/dopChart.ts` where they are unit-tested.
 */
export default function DopPlot({
  series,
  cursor,
  onCursorChange,
}: {
  series: SkySeries;
  cursor: number;
  onCursorChange?: (epochIndex: number) => void;
}) {
  const layout = useMemo(() => dopChartLayout(series), [series]);
  const epochCount = series.epochs.length;
  const cursorX = xForEpoch(cursor, epochCount);
  const atCursor = series.dop[cursor] ?? null;

  /** Let a click anywhere on the plot move the cursor there. */
  const handlePointer = (event: React.MouseEvent<SVGSVGElement>) => {
    if (!onCursorChange || epochCount <= 1) return;
    const box = event.currentTarget.getBoundingClientRect();
    // The SVG scales to its container, so map client pixels back to viewBox
    // units first. The axis inversion itself lives in `dopChart` beside the
    // forward mapping, where the round trip is tested.
    const viewBoxX = ((event.clientX - box.left) / box.width) * WIDTH;
    onCursorChange(epochAtViewBoxX(viewBoxX, epochCount));
  };

  return (
    <figure style={styles.figure}>
      <figcaption style={styles.caption}>
        <span style={styles.title}>Dilution of precision</span>
        <span style={styles.legend}>
          {layout.series.map((entry) => (
            <span key={entry.key} style={styles.legendItem}>
              <span style={{ ...styles.swatch, background: entry.colour }} />
              {entry.label}
              {atCursor && (
                <strong style={styles.legendValue}>
                  {atCursor[entry.key].toFixed(2)}
                </strong>
              )}
            </span>
          ))}
        </span>
      </figcaption>

      <svg
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        width="100%"
        style={{ maxWidth: WIDTH, display: "block", cursor: onCursorChange ? "pointer" : "default" }}
        onClick={handlePointer}
        role="img"
        aria-label={`HDOP and VDOP across the window; at the cursor ${
          atCursor
            ? `HDOP ${atCursor.hdop.toFixed(2)}, VDOP ${atCursor.vdop.toFixed(2)}`
            : "there is no position solution"
        }`}
      >
        <rect
          x={MARGIN.left}
          y={MARGIN.top}
          width={WIDTH - MARGIN.left - MARGIN.right}
          height={PLOT_HEIGHT}
          fill="#fbfbfd"
          stroke="#e5e7eb"
        />

        {layout.yTicks.map((value) => {
          const y = yForValue(value, layout.yMax);
          return (
            <g key={value}>
              <line
                x1={MARGIN.left}
                y1={y}
                x2={WIDTH - MARGIN.right}
                y2={y}
                stroke="#eef0f3"
              />
              <text x={MARGIN.left - 5} y={y + 3} fontSize={9} fill="#9ca3af" textAnchor="end">
                {value % 1 === 0 ? value : value.toFixed(1)}
              </text>
            </g>
          );
        })}

        {layout.xTicks.map((epochIndex) => {
          const x = xForEpoch(epochIndex, epochCount);
          return (
            <text
              key={epochIndex}
              x={x}
              y={HEIGHT - 7}
              fontSize={9}
              fill="#9ca3af"
              textAnchor={
                epochIndex === 0
                  ? "start"
                  : epochIndex === epochCount - 1
                    ? "end"
                    : "middle"
              }
            >
              {formatClock(series.epochs[epochIndex])}
            </text>
          );
        })}

        {/* Cursor beneath the data so it never hides a value. */}
        <line
          x1={cursorX}
          y1={MARGIN.top}
          x2={cursorX}
          y2={MARGIN.top + PLOT_HEIGHT}
          stroke="#111827"
          strokeWidth={1}
          strokeDasharray="3 2"
          opacity={0.55}
        />

        {layout.series.map((entry) =>
          entry.segments.map((segment, index) => (
            <polyline
              key={`${entry.key}-${index}`}
              points={segment.map((p) => `${p.x},${p.y}`).join(" ")}
              fill="none"
              stroke={entry.colour}
              strokeWidth={1.5}
              strokeLinejoin="round"
              strokeLinecap="round"
            />
          )),
        )}

        {/* Dot on each series at the cursor — the counterpart of the sky
            plot's moving markers, driven by the same index. */}
        {atCursor &&
          layout.series.map((entry) => {
            const y = yForValue(atCursor[entry.key], layout.yMax);
            return (
              <circle
                key={entry.key}
                cx={cursorX}
                cy={y}
                r={3.5}
                fill={entry.colour}
                stroke="#ffffff"
                strokeWidth={1.5}
              />
            );
          })}
      </svg>

      <p style={styles.note}>
        {atCursor ? (
          <>
            {atCursor.satellites} satellites in the solution · PDOP{" "}
            {atCursor.pdop.toFixed(2)} · TDOP {atCursor.tdop.toFixed(2)}
          </>
        ) : (
          <span style={styles.warning}>
            No position solution at this instant — fewer than four satellites
            above the mask, or their geometry is degenerate.
          </span>
        )}
        {layout.hasGaps && atCursor && (
          <>
            {" "}
            <span style={styles.warning}>
              Breaks in the lines are epochs with no solution.
            </span>
          </>
        )}
      </p>
    </figure>
  );
}

const styles: Record<string, React.CSSProperties> = {
  figure: { margin: "16px 0 0" },
  caption: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "baseline",
    gap: 12,
    marginBottom: 4,
    flexWrap: "wrap",
  },
  title: { fontSize: 13, fontWeight: 600, color: "#374151" },
  legend: { display: "flex", gap: 12, fontSize: 12, color: "#6b7280" },
  legendItem: { display: "inline-flex", alignItems: "center", gap: 4 },
  legendValue: { fontVariantNumeric: "tabular-nums", color: "#111827", marginLeft: 2 },
  swatch: { width: 10, height: 2.5, borderRadius: 1, display: "inline-block" },
  note: { fontSize: 11, color: "#6b7280", margin: "4px 0 0", lineHeight: 1.5 },
  warning: { color: "#b45309" },
};
