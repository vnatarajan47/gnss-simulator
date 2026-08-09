"use client";

import { useEffect } from "react";
import {
  CircleMarker,
  MapContainer,
  Polygon,
  Rectangle,
  TileLayer,
  useMap,
  useMapEvents,
} from "react-leaflet";
import type { LatLngBoundsExpression, LatLngExpression } from "leaflet";
import "leaflet/dist/leaflet.css";

import type { Observer } from "@/lib/types";
import { CONUS, isWithinConus } from "@/lib/coverage";

/**
 * Click-to-select map of the continental US.
 *
 * Leaflet rather than Mapbox GL: Mapbox needs an access token and an account
 * before a single tile renders, which is a poor trade when phase 1 only needs
 * a raster basemap and a click handler. Revisit at phase 3 -- terrain masking
 * wants elevation tiles, and MapLibre + Terrain-RGB is the natural successor.
 *
 * Must be loaded with `ssr: false`: Leaflet touches `window` at import time.
 */

const CONUS_BOUNDS: LatLngBoundsExpression = [
  [CONUS.south, CONUS.west],
  [CONUS.north, CONUS.east],
];

/**
 * Outer ring covering the whole world, with CONUS punched out as a hole.
 *
 * Leaflet renders a two-ring polygon with the even-odd rule, so the second
 * ring becomes a hole. That dims everything outside the supported region in
 * one shape, rather than stitching four rectangles around it.
 */
const WORLD_RING: LatLngExpression[] = [
  [-89.9, -179.9],
  [-89.9, 179.9],
  [89.9, 179.9],
  [89.9, -179.9],
];

const CONUS_HOLE: LatLngExpression[] = [
  [CONUS.south, CONUS.west],
  [CONUS.south, CONUS.east],
  [CONUS.north, CONUS.east],
  [CONUS.north, CONUS.west],
];

/**
 * Frame CONUS once the container has settled at its real size.
 *
 * Deliberately one-shot. Re-fitting from a ResizeObserver looks tempting but
 * the observer fires on Leaflet's own layout writes, and the resulting
 * continuous re-fit swallows map clicks.
 */
function FitConus() {
  const map = useMap();
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      map.invalidateSize();
      map.fitBounds(CONUS_BOUNDS, { padding: [8, 8] });
    });
    return () => cancelAnimationFrame(frame);
  }, [map]);
  return null;
}

function ClickHandler({
  onSelect,
  onRejected,
}: {
  onSelect: (lat: number, lon: number) => void;
  onRejected: () => void;
}) {
  useMapEvents({
    click(event) {
      const { lat, lng } = event.latlng;
      if (isWithinConus(lat, lng)) {
        onSelect(lat, lng);
      } else {
        onRejected();
      }
    },
  });
  return null;
}

export default function MapPanel({
  observer,
  onSelect,
  onRejected,
}: {
  observer: Observer;
  onSelect: (lat: number, lon: number) => void;
  onRejected: () => void;
}) {
  return (
    <MapContainer
      // Deterministic starting view, refined to the exact bounds by FitConus.
      center={[39.5, -98.35]}
      zoom={4}
      style={{ height: "100%", width: "100%" }}
      scrollWheelZoom
    >
      <TileLayer
        attribution='&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
        url="https://tile.openstreetmap.org/{z}/{x}/{y}.png"
      />

      {/* Dim the unsupported region. `interactive: false` lets clicks fall
          through to the map, where ClickHandler decides whether to accept. */}
      <Polygon
        positions={[WORLD_RING, CONUS_HOLE]}
        pathOptions={{
          fillColor: "#1f2937",
          fillOpacity: 0.55,
          stroke: false,
          interactive: false,
        }}
      />

      {/* Outline the clickable region. */}
      <Rectangle
        bounds={CONUS_BOUNDS}
        pathOptions={{
          color: "#2563eb",
          weight: 2,
          fill: false,
          dashArray: "5 4",
          interactive: false,
        }}
      />

      <FitConus />
      <ClickHandler onSelect={onSelect} onRejected={onRejected} />

      {/* A CircleMarker, not a Marker: Leaflet's default marker icon is loaded
          from a relative asset path that bundlers rewrite incorrectly, and a
          vector marker sidesteps that entirely. */}
      <CircleMarker
        center={[observer.lat, observer.lon]}
        radius={7}
        pathOptions={{ color: "#b91c1c", fillColor: "#ef4444", fillOpacity: 0.9 }}
      />
    </MapContainer>
  );
}
