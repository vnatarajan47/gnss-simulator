"use client";

import { useEffect } from "react";
import {
  CircleMarker,
  MapContainer,
  TileLayer,
  useMap,
  useMapEvents,
} from "react-leaflet";
import "leaflet/dist/leaflet.css";

import type { Observer } from "@/lib/types";
import { normaliseLongitude } from "@/lib/coverage";

/**
 * Click-to-select world map.
 *
 * Leaflet rather than Mapbox GL: Mapbox needs an access token and an account
 * before a single tile renders, which is a poor trade for a raster basemap and
 * a click handler. Revisit at phase 3 -- terrain masking wants elevation tiles,
 * and MapLibre + Terrain-RGB is the natural successor.
 *
 * Must be loaded with `ssr: false`: Leaflet touches `window` at import time.
 */

/**
 * How far the map lets you scroll vertically.
 *
 * Web Mercator diverges at the poles, so the tiles stop short of them. The
 * bound is on the *map*, not on the computation -- an observer at 89 deg is a
 * perfectly good sky plot, it just cannot be clicked on this projection, which
 * is why the coordinate fields exist.
 */
const MERCATOR_LIMIT = 85;

/** Fit the world once the container has settled at its real size.
 *
 * Deliberately one-shot. Re-fitting from a ResizeObserver looks tempting but
 * the observer fires on Leaflet's own layout writes, and the resulting
 * continuous re-fit swallows map clicks.
 */
function FitWorld() {
  const map = useMap();
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      map.invalidateSize();
    });
    return () => cancelAnimationFrame(frame);
  }, [map]);
  return null;
}

/**
 * Keep the marker in view when the receiver moves from outside the map.
 *
 * Typing coordinates is the only way to reach latitudes Mercator will not
 * draw, and the map silently staying where it was would read as the entry
 * having been ignored.
 */
function FollowObserver({ observer }: { observer: Observer }) {
  const map = useMap();
  useEffect(() => {
    if (!map.getBounds().contains([observer.lat, observer.lon])) {
      map.panTo([observer.lat, observer.lon], { animate: true });
    }
  }, [map, observer.lat, observer.lon]);
  return null;
}

function ClickHandler({
  onSelect,
}: {
  onSelect: (lat: number, lon: number) => void;
}) {
  useMapEvents({
    click(event) {
      const { lat, lng } = event.latlng;
      // Panning past the edge of the world puts you on a repeated copy of it,
      // where Leaflet reports longitudes outside +-180. The geodesy would cope,
      // but the number is shown to the user, so fold it here.
      onSelect(lat, normaliseLongitude(lng));
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
      center={[observer.lat, observer.lon]}
      zoom={2}
      minZoom={1}
      style={{ height: "100%", width: "100%" }}
      scrollWheelZoom
      // Keeps the marker and the basemap on the same copy of the world when
      // the user pans across the date line.
      worldCopyJump
      maxBounds={[
        [-MERCATOR_LIMIT, -Infinity],
        [MERCATOR_LIMIT, Infinity],
      ]}
    >
      <TileLayer
        attribution='&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
        url="https://tile.openstreetmap.org/{z}/{x}/{y}.png"
        noWrap={false}
      />

      <FitWorld />
      <FollowObserver observer={observer} />
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
