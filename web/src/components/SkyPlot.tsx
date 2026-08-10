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

/**
 * SBAS satellites are geostationary, which is worth showing: they never move,
 * and they sit in a tight clump rather than sweeping across the sky.
 */
function isGeostationary(satellite: Satellite): boolean {
  return satellite.sv.startsWith("S");
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
        const label = String(satellite.prn);
        // SBAS PRNs are three digits and do not fit the default marker.
        const radius = label.length > 2 ? 12 : 9;
        const fill = elevationColour(satellite.elevation);

        return (
          <g key={satellite.sv}>
            {/* Geostationary augmentation satellites get a square marker, so
                they read as a different kind of thing at a glance rather than
                only by PRN. */}
            {isGeostationary(satellite) ? (
              <rect
                x={at.x - radius}
                y={at.y - radius}
                width={radius * 2}
                height={radius * 2}
                rx={3}
                fill={fill}
                stroke="#111827"
                strokeWidth={1.5}
              />
            ) : (
              <circle cx={at.x} cy={at.y} r={radius} fill={fill} />
            )}
            <text
              x={at.x}
              y={at.y + 0.5}
              fontSize={label.length > 2 ? 8.5 : 9}
              fill="#ffffff"
              fontWeight={600}
              textAnchor="middle"
              dominantBaseline="middle"
            >
              {label}
            </text>
          </g>
        );
      })}
    </svg>
  );
}
