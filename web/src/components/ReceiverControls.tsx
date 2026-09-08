"use client";

import { useEffect, useState } from "react";

import { isWithinCoverage, normaliseLongitude } from "@/lib/coverage";
import type { Observer } from "@/lib/types";

/**
 * Numeric entry for the receiver position.
 *
 * The map is the primary control, but it cannot be the only one once coverage
 * is global: at world zoom a pixel is tens of kilometres, Web Mercator will not
 * draw the poles at all, and "the site I actually care about" is usually a
 * known lat/lon rather than a place to aim at. Typing coordinates is also the
 * only way to reproduce someone else's view exactly.
 *
 * Fields are held as strings while being edited. Parsing on every keystroke
 * and writing the number straight back would fight the user over an
 * intermediate state -- deleting the last digit of "40" leaves "4", and a
 * half-typed minus sign is not a number at all. The parsed value is pushed up
 * only when it is a usable coordinate; otherwise the field is marked invalid
 * and the receiver stays where it was.
 */
export default function ReceiverControls({
  observer,
  onChange,
}: {
  observer: Observer;
  onChange: (observer: Observer) => void;
}) {
  const [lat, setLat] = useState(() => observer.lat.toFixed(4));
  const [lon, setLon] = useState(() => observer.lon.toFixed(4));
  const [alt, setAlt] = useState(() => String(observer.altM));

  // The map writes to the same state, so the fields have to follow it. Compared
  // numerically rather than as text, so this does not stamp on a value the user
  // is in the middle of typing.
  useEffect(() => {
    if (Number(lat) !== observer.lat) setLat(observer.lat.toFixed(4));
    if (Number(lon) !== observer.lon) setLon(observer.lon.toFixed(4));
    if (Number(alt) !== observer.altM) setAlt(String(observer.altM));
    // Only the observer drives this; including the field state would undo edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [observer.lat, observer.lon, observer.altM]);

  function commit(next: { lat?: string; lon?: string; alt?: string }) {
    const nextLat = Number(next.lat ?? lat);
    const nextLon = Number(next.lon ?? lon);
    const nextAlt = Number(next.alt ?? alt);

    if (!isWithinCoverage(nextLat, nextLon) || !Number.isFinite(nextAlt)) return;

    onChange({
      lat: nextLat,
      lon: normaliseLongitude(nextLon),
      altM: nextAlt,
    });
  }

  const latValid = isWithinCoverage(Number(lat), 0);
  const lonValid = Number.isFinite(Number(lon)) && lon.trim() !== "";
  const altValid = Number.isFinite(Number(alt)) && alt.trim() !== "";

  return (
    <div style={styles.row}>
      <Field
        label="Latitude"
        value={lat}
        valid={latValid}
        hint="-90 to 90"
        onChange={(value) => {
          setLat(value);
          commit({ lat: value });
        }}
      />
      <Field
        label="Longitude"
        value={lon}
        valid={lonValid}
        hint="-180 to 180"
        onChange={(value) => {
          setLon(value);
          commit({ lon: value });
        }}
      />
      <Field
        label="Height (m)"
        value={alt}
        valid={altValid}
        hint="above the WGS-84 ellipsoid"
        onChange={(value) => {
          setAlt(value);
          commit({ alt: value });
        }}
      />
    </div>
  );
}

function Field({
  label,
  value,
  valid,
  hint,
  onChange,
}: {
  label: string;
  value: string;
  valid: boolean;
  hint: string;
  onChange: (value: string) => void;
}) {
  return (
    <label style={styles.field}>
      <span style={styles.label}>{label}</span>
      <input
        type="text"
        inputMode="decimal"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        aria-label={label}
        aria-invalid={!valid}
        title={hint}
        style={{
          ...styles.input,
          borderColor: valid ? "#d1d5db" : "#b91c1c",
        }}
      />
    </label>
  );
}

const styles: Record<string, React.CSSProperties> = {
  row: { display: "flex", gap: 8, marginTop: 8, flexWrap: "wrap" },
  field: { display: "flex", flexDirection: "column", gap: 2, flex: "1 1 100px" },
  label: { fontSize: 11, color: "#6b7280", textTransform: "uppercase", letterSpacing: 0.4 },
  input: {
    fontSize: 13,
    padding: "4px 6px",
    borderRadius: 4,
    borderWidth: 1,
    borderStyle: "solid",
    fontFamily: "inherit",
    fontVariantNumeric: "tabular-nums",
    width: "100%",
    boxSizing: "border-box",
  },
};
