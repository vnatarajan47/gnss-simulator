"use client";

import { useEffect } from "react";
import {
  CircleMarker,
  MapContainer,
  TileLayer,
  useMap,
  useMapEvents,
} from "react-leaflet";
import type { LatLngBoundsExpression } from "leaflet";
import "leaflet/dist/leaflet.css";

import type { Observer } from "@/lib/types";

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

/** Roughly the continental US. */
const CONUS_BOUNDS: LatLngBoundsExpression = [
  [24.4, -125.0],
  [49.5, -66.9],
];

/**
 * Re-fit CONUS once the container has settled at its real size, and on every
 * later resize.
 *
 * `MapContainer`'s own `bounds` prop is not enough: Leaflet fits against
 * whatever size the div reports at mount, which is frequently stale or zero,
 * leaving the map zoomed all the way out. The explicit `center`/`zoom` below
 * guarantees a sane starting view; this refines it to the exact bounds.
 */
function FitConus() {
  const map = useMap();

  useEffect(() => {
    // One frame after mount the flex layout has resolved, so the container
    // reports its real size and fitBounds picks the right zoom.
    //
    // Deliberately one-shot. Re-fitting from a ResizeObserver looks tempting
    // but the observer fires on Leaflet's own layout writes, and the resulting
    // continuous re-fit swallows map clicks.
    const frame = requestAnimationFrame(() => {
      map.invalidateSize();
      map.fitBounds(CONUS_BOUNDS, { padding: [8, 8] });
    });
    return () => cancelAnimationFrame(frame);
  }, [map]);

  return null;
}

function ClickHandler({ onSelect }: { onSelect: (lat: number, lon: number) => void }) {
  useMapEvents({
    click(event) {
      onSelect(event.latlng.lat, event.latlng.lng);
    },
  });
  return null;
}

export default function MapPanel({
  observer,
  onSelect,
}: {
  observer: Observer;
  onSelect: (lat: number, lon: number) => void;
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
      <FitConus />
      <ClickHandler onSelect={onSelect} />
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
