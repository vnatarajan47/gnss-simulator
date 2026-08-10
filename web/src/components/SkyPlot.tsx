"use client";

import { useMemo } from "react";

import type { Satellite } from "@/lib/types";
import {
  CENTRE,
  COMPASS,
  MARKER_RADIUS,
  RADIUS,
  RINGS,
  SIZE,
  elevationColour,
  layoutLabels,
  pointOnCircleTowards,
  pointOnRectTowards,
  project,
  type Point,
  type Rect,
} from "@/lib/skyPlotLayout";
import { trackColour, type TrackSegment } from "@/lib/trackLayout";

/**
 * Polar sky plot: azimuth is the angle (0 deg = north, clockwise), elevation
 * is the radius (90 deg at the centre, the horizon at the rim).
 *
 * Plain SVG rather than a charting library -- the projection is two lines of
 * trigonometry and the plot needs no axes, scales, or interaction. All the
 * geometry, including collision-avoiding label placement, lives in
 * `lib/skyPlotLayout.ts` so it can be tested without a DOM.
 *
 * `tracks` are the arcs swept over a time window. They are pre-segmented, so a
 * satellite that sets and rises again arrives as two entries and is never drawn
 * as one line across the sky -- see `lib/trackLayout.ts`.
 */
export default function SkyPlot({
  satellites,
  elevationMaskDeg,
  tracks = [],
}: {
  satellites: Satellite[];
  elevationMaskDeg: number;
  tracks?: TrackSegment[];
}) {
  const maskRadius = RADIUS * (1 - elevationMaskDeg / 90);
  const placements = useMemo(() => layoutLabels(satellites), [satellites]);

  return (
    <svg
      viewBox={`0 0 ${SIZE} ${SIZE}`}
      width="100%"
      style={{ maxWidth: SIZE, display: "block" }}
      role="img"
      aria-label={`Sky plot showing ${satellites.length} satellites above ${elevationMaskDeg} degrees elevation`}
    >
      {/* Region excluded by the elevation mask. */}
      <circle cx={CENTRE} cy={CENTRE} r={RADIUS} fill="#f3f4f6" />
      <circle cx={CENTRE} cy={CENTRE} r={maskRadius} fill="#ffffff" />

      {RINGS.map((elevation) => (
        <g key={elevation}>
          <circle
            cx={CENTRE}
            cy={CENTRE}
            r={RADIUS * (1 - elevation / 90)}
            fill="none"
            stroke="#d1d5db"
          />
          {/* The 0 deg ring is the rim, already labelled by the compass points. */}
          {elevation > 0 && (
            <text
              x={CENTRE + 3}
              y={CENTRE - RADIUS * (1 - elevation / 90) - 3}
              fontSize={10}
              fill="#9ca3af"
            >
              {elevation}&deg;
            </text>
          )}
        </g>
      ))}

      {/* Radial spokes every 30 deg. */}
      {Array.from({ length: 12 }, (_, i) => i * 30).map((azimuth) => {
        const outer = project(azimuth, 0);
        return (
          <line
            key={azimuth}
            x1={CENTRE}
            y1={CENTRE}
            x2={outer.x}
            y2={outer.y}
            stroke="#e5e7eb"
          />
        );
      })}

      {COMPASS.map(({ label, azimuth }) => {
        const at = project(azimuth, -7);
        return (
          <text
            key={label}
            x={at.x}
            y={at.y}
            fontSize={14}
            fontWeight={600}
            fill="#374151"
            textAnchor="middle"
            dominantBaseline="middle"
          >
            {label}
          </text>
        );
      })}

      {/* Arcs beneath everything: they are context for the current positions,
          which must stay the most legible thing on the plot. A single-sample
          segment gets a dot -- a polyline of one point draws nothing, and
          silently losing a satellite caught at the window edge would be worse
          than a stray pixel. */}
      {tracks.map((segment, index) =>
        segment.points.length === 1 ? (
          <circle
            key={`${segment.sv}-${index}`}
            cx={segment.points[0].x}
            cy={segment.points[0].y}
            r={1.25}
            fill={trackColour(segment.sv)}
          />
        ) : (
          <polyline
            key={`${segment.sv}-${index}`}
            points={segment.points.map((p) => `${p.x},${p.y}`).join(" ")}
            fill="none"
            stroke={trackColour(segment.sv)}
            strokeWidth={1.5}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        ),
      )}

      {/* Leader lines beneath everything else, so markers and label chips sit
          cleanly on top of their own connecting line. */}
      {placements.map(
        ({ satellite, marker, label, leader }) =>
          leader && (
            <LeaderLine key={`leader-${satellite.sv}`} marker={marker} label={label} />
          ),
      )}

      {placements.map(({ satellite, marker, isSbas, label }) => {
        const fill = elevationColour(satellite.elevation);
        return (
          <g key={satellite.sv}>
            {/* Geostationary augmentation satellites get a square marker, so
                they read as a different kind of thing at a glance. */}
            {isSbas ? (
              <rect
                x={marker.x - MARKER_RADIUS}
                y={marker.y - MARKER_RADIUS}
                width={MARKER_RADIUS * 2}
                height={MARKER_RADIUS * 2}
                rx={2}
                fill={fill}
                stroke="#111827"
                strokeWidth={1.25}
              />
            ) : (
              <circle cx={marker.x} cy={marker.y} r={MARKER_RADIUS} fill={fill} />
            )}

            <rect
              x={label.x}
              y={label.y}
              width={label.w}
              height={label.h}
              rx={3}
              fill={fill}
              stroke="#ffffff"
              strokeWidth={1}
            />
            <text
              x={label.x + label.w / 2}
              y={label.y + label.h / 2 + 0.5}
              fontSize={9}
              fill="#ffffff"
              fontWeight={600}
              textAnchor="middle"
              dominantBaseline="middle"
            >
              {label.text}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

/** A thin line from a marker's edge to the near edge of its label chip. */
function LeaderLine({ marker, label }: { marker: Point; label: Rect }) {
  const labelCentre = { x: label.x + label.w / 2, y: label.y + label.h / 2 };
  const start = pointOnCircleTowards(marker, labelCentre, MARKER_RADIUS);
  const end = pointOnRectTowards(label, labelCentre, marker);
  return (
    <line
      x1={start.x}
      y1={start.y}
      x2={end.x}
      y2={end.y}
      stroke="#9ca3af"
      strokeWidth={1}
    />
  );
}
