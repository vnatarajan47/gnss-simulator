/**
 * Sky-plot geometry and label placement.
 *
 * Kept separate from the React component so it stays a pure function of its
 * inputs: no JSX, no DOM, no imports beyond types. That makes the layout
 * invariants directly testable (`skyPlotLayout.test.ts`), which matters
 * because phase 2's time-series view will push far more points through here
 * than a single static epoch does.
 *
 * Markers sit at their true projected position, always. Labels do not: two
 * satellites close together in az/el (WAAS's three GEOs sit within ~17 deg of
 * azimuth of each other from CONUS) would otherwise print overlapping,
 * illegible text. Each label is instead placed by a small greedy search that
 * pushes it outward until it clears every marker and every label already
 * placed, with a leader line drawn back whenever it ends up far enough away
 * to need one.
 */

import type { Satellite } from "./types.ts";

export const SIZE = 440;
export const CENTRE = SIZE / 2;
export const RADIUS = SIZE / 2 - 30;

/** Elevation rings to draw, in degrees. */
export const RINGS = [0, 30, 60];

export const COMPASS = [
  { label: "N", azimuth: 0 },
  { label: "E", azimuth: 90 },
  { label: "S", azimuth: 180 },
  { label: "W", azimuth: 270 },
];

/** Marker size: circle radius for the GNSS constellations, square half-side for SBAS. */
export const MARKER_RADIUS = 5;

const LABEL_HEIGHT = 14;
/**
 * Advance width per character at the 9 px label size, deliberately generous.
 * Overestimating widens the collision box, which only ever makes the layout
 * more conservative.
 */
const LABEL_CHAR_WIDTH = 6.4;
const LABEL_PAD_X = 5;

/** Search radii, nearest first. The first ring is "touching" -- no leader line. */
const CANDIDATE_RADII = [14, 20, 28, 38, 50, 64, 80];

/** Below this distance a label reads as paired with its marker; no line needed. */
const LEADER_THRESHOLD = CANDIDATE_RADII[0];

/** Fan the angular search out from the preferred direction: 0, +-30, ..., 180. */
const ANGLE_STEP_DEG = 30;
const ANGLE_OFFSETS_DEG: number[] = [0];
for (let step = ANGLE_STEP_DEG; step < 180; step += ANGLE_STEP_DEG) {
  ANGLE_OFFSETS_DEG.push(step, -step);
}
ANGLE_OFFSETS_DEG.push(180);

export const CANVAS_MARGIN = 4;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Point {
  x: number;
  y: number;
}

/** Project an azimuth/elevation pair to SVG coordinates. */
export function project(azimuthDeg: number, elevationDeg: number): Point {
  const radius = RADIUS * (1 - elevationDeg / 90);
  const angle = (azimuthDeg * Math.PI) / 180;
  return {
    x: CENTRE + radius * Math.sin(angle),
    y: CENTRE - radius * Math.cos(angle),
  };
}

/** Warm at the horizon, cool at the zenith -- reads as "how usable is this". */
export function elevationColour(elevationDeg: number): string {
  const hue = 12 + (elevationDeg / 90) * 190;
  return `hsl(${hue} 72% 45%)`;
}

/**
 * SBAS satellites are geostationary, which is worth showing: they never move,
 * and they sit in a tight clump rather than sweeping across the sky.
 */
export function isGeostationary(satellite: Satellite): boolean {
  return satellite.sv.startsWith("S");
}

export function rectsOverlap(a: Rect, b: Rect): boolean {
  return !(
    a.x + a.w < b.x ||
    b.x + b.w < a.x ||
    a.y + a.h < b.y ||
    b.y + b.h < a.y
  );
}

function withinCanvas(rect: Rect): boolean {
  return (
    rect.x >= CANVAS_MARGIN &&
    rect.y >= CANVAS_MARGIN &&
    rect.x + rect.w <= SIZE - CANVAS_MARGIN &&
    rect.y + rect.h <= SIZE - CANVAS_MARGIN
  );
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

/** Where the segment from `from` toward `to` crosses a circle of `radius` about `from`. */
export function pointOnCircleTowards(from: Point, to: Point, radius: number): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const dist = Math.hypot(dx, dy) || 1;
  return { x: from.x + (dx / dist) * radius, y: from.y + (dy / dist) * radius };
}

/** Where the segment from `centre` toward `from` crosses the boundary of `rect`. */
export function pointOnRectTowards(rect: Rect, centre: Point, from: Point): Point {
  const dx = from.x - centre.x;
  const dy = from.y - centre.y;
  const sx = dx !== 0 ? rect.w / 2 / Math.abs(dx) : Infinity;
  const sy = dy !== 0 ? rect.h / 2 / Math.abs(dy) : Infinity;
  const s = clamp(Math.min(sx, sy), 0, 1);
  return { x: centre.x + s * dx, y: centre.y + s * dy };
}

export interface LabelPlacement {
  satellite: Satellite;
  marker: Point;
  isSbas: boolean;
  label: Rect & { text: string };
  /** True once the label has been pushed far enough to need a connecting line. */
  leader: boolean;
  /** False when every candidate collided and the fallback position was used. */
  placed: boolean;
}

/**
 * Place a label for every satellite.
 *
 * Labels are laid out in array order, so earlier satellites get first claim on
 * the tightest positions. The input is already sorted by constellation and
 * PRN, so the order is stable across epochs -- which matters for phase 2,
 * where a label jumping sides between frames would read as flicker.
 */
export function layoutLabels(satellites: Satellite[]): LabelPlacement[] {
  const markers = satellites.map((satellite) => ({
    satellite,
    point: project(satellite.azimuth, satellite.elevation),
    isSbas: isGeostationary(satellite),
  }));

  // Every marker is an obstacle from the start: a label must clear every
  // OTHER satellite's marker, not just the ones already labelled.
  const markerObstacles: Rect[] = markers.map(({ point }) => ({
    x: point.x - MARKER_RADIUS,
    y: point.y - MARKER_RADIUS,
    w: MARKER_RADIUS * 2,
    h: MARKER_RADIUS * 2,
  }));

  // Keep the compass letters clear so N/E/S/W stay readable.
  const compassObstacles: Rect[] = COMPASS.map(({ azimuth }) => {
    const at = project(azimuth, -7);
    return { x: at.x - 9, y: at.y - 9, w: 18, h: 18 };
  });

  const placedLabels: Rect[] = [];
  const placements: LabelPlacement[] = [];

  markers.forEach(({ satellite, point, isSbas }, index) => {
    // Full identifier, constellation code included: `G05`, `S131`. A bare `5`
    // is ambiguous once more than one constellation is on, and it would not
    // match the SV column in the table.
    const text = satellite.sv;
    const width = LABEL_PAD_X * 2 + text.length * LABEL_CHAR_WIDTH;
    const rectAt = (centre: Point): Rect => ({
      x: centre.x - width / 2,
      y: centre.y - LABEL_HEIGHT / 2,
      w: width,
      h: LABEL_HEIGHT,
    });

    const obstacles = [
      ...compassObstacles,
      ...markerObstacles.filter((_, otherIndex) => otherIndex !== index),
      ...placedLabels,
    ];

    // Search outward starting away from the plot centre: for a low-elevation
    // (near-rim) satellite this fans labels toward the open space near the rim
    // rather than back into the crowded middle of the disk.
    const preferredAngle = Math.atan2(point.y - CENTRE, point.x - CENTRE);

    let chosen: Rect | null = null;
    let chosenRadius = 0;

    searching: for (const radius of CANDIDATE_RADII) {
      for (const offsetDeg of ANGLE_OFFSETS_DEG) {
        const angle = preferredAngle + (offsetDeg * Math.PI) / 180;
        const centre = {
          x: point.x + radius * Math.cos(angle),
          y: point.y + radius * Math.sin(angle),
        };
        const candidate = rectAt(centre);
        if (!withinCanvas(candidate)) continue;
        if (obstacles.some((obstacle) => rectsOverlap(candidate, obstacle))) continue;
        chosen = candidate;
        chosenRadius = radius;
        break searching;
      }
    }

    const placed = chosen !== null;

    if (!chosen) {
      // Every candidate collided. Best effort: take the farthest search ring in
      // the preferred direction, clamped on-canvas, and accept the overlap
      // rather than leave a satellite unlabelled.
      const radius = CANDIDATE_RADII[CANDIDATE_RADII.length - 1];
      const centre = {
        x: clamp(
          point.x + radius * Math.cos(preferredAngle),
          CANVAS_MARGIN + width / 2,
          SIZE - CANVAS_MARGIN - width / 2,
        ),
        y: clamp(
          point.y + radius * Math.sin(preferredAngle),
          CANVAS_MARGIN + LABEL_HEIGHT / 2,
          SIZE - CANVAS_MARGIN - LABEL_HEIGHT / 2,
        ),
      };
      chosen = rectAt(centre);
      chosenRadius = radius;
    }

    placedLabels.push(chosen);
    placements.push({
      satellite,
      marker: point,
      isSbas,
      label: { ...chosen, text },
      leader: chosenRadius > LEADER_THRESHOLD,
      placed,
    });
  });

  return placements;
}
