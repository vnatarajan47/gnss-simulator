"use client";

import type { Satellite } from "@/lib/types";

/**
 * Polar sky plot: azimuth is the angle (0 deg = north, clockwise), elevation
 * is the radius (90 deg at the centre, the horizon at the rim).
 *
 * Plain SVG rather than a charting library -- the projection is two lines of
 * trigonometry and the plot needs no axes, scales, or interaction.
 */

const SIZE = 440;
const CENTRE = SIZE / 2;
const RADIUS = SIZE / 2 - 30;

/** Elevation rings to draw, in degrees. */
const RINGS = [0, 30, 60];

const COMPASS = [
  { label: "N", azimuth: 0 },
  { label: "E", azimuth: 90 },
  { label: "S", azimuth: 180 },
  { label: "W", azimuth: 270 },
];

/** Project an azimuth/elevation pair to SVG coordinates. */
function project(azimuthDeg: number, elevationDeg: number) {
  const radius = RADIUS * (1 - elevationDeg / 90);
  const angle = (azimuthDeg * Math.PI) / 180;
  return {
    x: CENTRE + radius * Math.sin(angle),
    y: CENTRE - radius * Math.cos(angle),
  };
}

/** Warm at the horizon, cool at the zenith -- reads as "how usable is this". */
function elevationColour(elevationDeg: number): string {
  const hue = 12 + (elevationDeg / 90) * 190;
  return `hsl(${hue} 72% 45%)`;
}

export default function SkyPlot({
  satellites,
  elevationMaskDeg,
}: {
  satellites: Satellite[];
  elevationMaskDeg: number;
}) {
  const maskRadius = RADIUS * (1 - elevationMaskDeg / 90);

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

      {satellites.map((satellite) => {
        const at = project(satellite.azimuth, satellite.elevation);
        return (
          <g key={satellite.sv}>
            <circle
              cx={at.x}
              cy={at.y}
              r={9}
              fill={elevationColour(satellite.elevation)}
            />
            <text
              x={at.x}
              y={at.y + 0.5}
              fontSize={9}
              fill="#ffffff"
              fontWeight={600}
              textAnchor="middle"
              dominantBaseline="middle"
            >
              {satellite.prn}
            </text>
          </g>
        );
      })}
    </svg>
  );
}
