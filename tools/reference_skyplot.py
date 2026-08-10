#!/usr/bin/env python3
"""Independent reference implementation of the gnss-core math.

This exists to cross-check `crates/gnss-core` -- it is deliberately *not* a
port of the Rust code. Where the Rust has a choice, this makes the other one:

  * RINEX parsing:  fixed-column slicing here, the `rinex` crate there.
  * Kepler's eqn:   fixed-point iteration here, Newton-Raphson there.
  * Elevation:      asin(up / range) here, atan2(up, horizontal) there.
  * ENU:            explicit 3x3 rotation matrix here, inlined dot products
                    there.
  * DOP:            explicit n-by-4 design matrix and an SVD pseudo-inverse
                    here; the normal matrix accumulated in place and inverted
                    by Gauss-Jordan there.

Agreement between two implementations that share no code is meaningful
evidence; agreement between a port and its original is not.

Sources:
  IS-GPS-200N Table 20-IV (satellite position from broadcast ephemeris)
  NIMA TR8350.2 (WGS-84 ellipsoid)

Usage:
    python3 tools/reference_skyplot.py --emit-vectors     # JSON test vectors
    python3 tools/reference_skyplot.py --report           # human-readable

`--emit-vectors` needs numpy, for the SVD. Nothing else here does, and the
generated JSON is checked in, so running the Rust test suite never needs it.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import sys
from pathlib import Path

# ---------------------------------------------------------------- constants

MU_GPS = 3.986005e14            # m^3/s^2, WGS-84 value used by GPS
OMEGA_E = 7.2921151467e-5       # rad/s
C = 299792458.0                 # m/s
WGS84_A = 6378137.0
WGS84_F = 1.0 / 298.257223563
WGS84_B = WGS84_A * (1.0 - WGS84_F)
WGS84_E2 = WGS84_F * (2.0 - WGS84_F)
SECONDS_PER_WEEK = 604800.0
GPS_EPOCH_UNIX = 315964800.0

# (unix seconds when it took effect, cumulative GPS-UTC offset)
LEAPS = [
    (362793600, 1), (394329600, 2), (425865600, 3), (489024000, 4),
    (567993600, 5), (631152000, 6), (662688000, 7), (709948800, 8),
    (741484800, 9), (773020800, 10), (820454400, 11), (867715200, 12),
    (915148800, 13), (1136073600, 14), (1230768000, 15), (1341100800, 16),
    (1435708800, 17), (1483228800, 18),
]


def unix_to_gps_seconds(unix_s: float) -> float:
    offset = 0
    for effective, value in LEAPS:
        if unix_s >= effective:
            offset = value
    return unix_s - GPS_EPOCH_UNIX + offset


def iso_to_unix(iso: str) -> float:
    return dt.datetime.fromisoformat(iso).replace(tzinfo=dt.timezone.utc).timestamp()


# ------------------------------------------------------------------ parsing

# GPS broadcast-orbit field layout, RINEX 3.0x, in transmission order.
GPS_ORBIT_FIELDS = [
    ["iode", "crs", "delta_n", "m0"],
    ["cuc", "e", "cus", "sqrt_a"],
    ["toe", "cic", "omega0", "cis"],
    ["i0", "crc", "omega", "omega_dot"],
    ["idot", "l2_codes", "week", "l2p_flag"],
    ["accuracy", "health", "tgd", "iodc"],
    ["transmission_time", "fit_interval", "spare1", "spare2"],
]


def _f(text: str) -> float:
    """RINEX floats use D or E for the exponent, and pad with blanks."""
    text = text.strip().replace("D", "E").replace("d", "e")
    return float(text) if text else 0.0


def parse_rinex_nav(path: Path) -> list[dict]:
    """Fixed-column parse of a RINEX 3 GPS navigation file."""
    lines = path.read_text().splitlines()

    end = next(i for i, l in enumerate(lines) if "END OF HEADER" in l)
    body = lines[end + 1:]

    records = []
    i = 0
    while i < len(body):
        line = body[i]
        if not line or line[0] != "G":
            i += 1
            continue

        rec = {"prn": int(line[1:3])}
        # Epoch (Time of Clock) then af0/af1/af2.
        rec["toc_iso"] = (
            f"{int(line[4:8]):04d}-{int(line[9:11]):02d}-{int(line[12:14]):02d}"
            f"T{int(line[15:17]):02d}:{int(line[18:20]):02d}:{int(line[21:23]):02d}"
        )
        rec["af0"] = _f(line[23:42])
        rec["af1"] = _f(line[42:61])
        rec["af2"] = _f(line[61:80])

        for row, names in enumerate(GPS_ORBIT_FIELDS):
            cont = body[i + 1 + row]
            for col, name in enumerate(names):
                start = 4 + col * 19
                rec[name] = _f(cont[start:start + 19])

        records.append(rec)
        i += 8

    return records


def select_ephemeris(records, prn: int, gps_seconds: float, max_age=7200.0):
    """Nearest-ToE selection, matching gnss-core's default strategy."""
    best, best_age = None, None
    for rec in records:
        if rec["prn"] != prn:
            continue
        if int(rec["health"]) != 0:
            continue
        toe_abs = rec["week"] * SECONDS_PER_WEEK + rec["toe"]
        age = gps_seconds - toe_abs
        if abs(age) > max_age:
            continue
        if best_age is None or abs(age) < abs(best_age):
            best, best_age = rec, age
    return best, best_age


# -------------------------------------------------------------- propagation

def kepler_eccentric_anomaly(m: float, e: float, tol=1e-13, max_iter=200) -> float:
    """Fixed-point iteration E <- M + e*sin(E).

    Deliberately a different algorithm from the Rust side's Newton-Raphson.
    Linear convergence, but with e ~ 0.01 it still converges in a few passes.
    """
    ecc_anom = m
    for _ in range(max_iter):
        nxt = m + e * math.sin(ecc_anom)
        if abs(nxt - ecc_anom) < tol:
            return nxt
        ecc_anom = nxt
    raise RuntimeError("Kepler iteration did not converge")


def sat_position_ecef(eph: dict, gps_seconds: float) -> tuple[float, float, float]:
    """IS-GPS-200 Table 20-IV, written out longhand."""
    a = eph["sqrt_a"] ** 2
    toe_abs = eph["week"] * SECONDS_PER_WEEK + eph["toe"]
    tk = gps_seconds - toe_abs

    n0 = math.sqrt(MU_GPS / a ** 3)
    n = n0 + eph["delta_n"]
    mk = eph["m0"] + n * tk

    ek = kepler_eccentric_anomaly(mk, eph["e"])

    # True anomaly, via the half-angle-free form.
    sin_vk = math.sqrt(1.0 - eph["e"] ** 2) * math.sin(ek)
    cos_vk = math.cos(ek) - eph["e"]
    vk = math.atan2(sin_vk, cos_vk)

    phi_k = vk + eph["omega"]
    s2, c2 = math.sin(2 * phi_k), math.cos(2 * phi_k)

    uk = phi_k + eph["cus"] * s2 + eph["cuc"] * c2
    rk = a * (1.0 - eph["e"] * math.cos(ek)) + eph["crs"] * s2 + eph["crc"] * c2
    ik = eph["i0"] + eph["idot"] * tk + eph["cis"] * s2 + eph["cic"] * c2

    xp = rk * math.cos(uk)
    yp = rk * math.sin(uk)

    omega_k = (eph["omega0"]
               + (eph["omega_dot"] - OMEGA_E) * tk
               - OMEGA_E * eph["toe"])

    x = xp * math.cos(omega_k) - yp * math.cos(ik) * math.sin(omega_k)
    y = xp * math.sin(omega_k) + yp * math.cos(ik) * math.cos(omega_k)
    z = yp * math.sin(ik)
    return x, y, z


# ---------------------------------------------------------------- geodesy

def geodetic_to_ecef(lat_deg, lon_deg, alt_m):
    lat, lon = math.radians(lat_deg), math.radians(lon_deg)
    n = WGS84_A / math.sqrt(1.0 - WGS84_E2 * math.sin(lat) ** 2)
    return (
        (n + alt_m) * math.cos(lat) * math.cos(lon),
        (n + alt_m) * math.cos(lat) * math.sin(lon),
        (n * (1.0 - WGS84_E2) + alt_m) * math.sin(lat),
    )


def look_angles(sat_ecef, lat_deg, lon_deg, alt_m):
    """Az/el via an explicit ENU rotation matrix and asin for elevation."""
    obs = geodetic_to_ecef(lat_deg, lon_deg, alt_m)
    d = [sat_ecef[i] - obs[i] for i in range(3)]

    lat, lon = math.radians(lat_deg), math.radians(lon_deg)
    sl, cl = math.sin(lat), math.cos(lat)
    so, co = math.sin(lon), math.cos(lon)

    rot = [
        [-so,        co,       0.0],
        [-sl * co,  -sl * so,  cl],
        [ cl * co,   cl * so,  sl],
    ]
    east, north, up = [sum(rot[r][c] * d[c] for c in range(3)) for r in range(3)]

    rng = math.sqrt(east ** 2 + north ** 2 + up ** 2)
    elevation = math.degrees(math.asin(up / rng))
    azimuth = math.degrees(math.atan2(east, north)) % 360.0
    return azimuth, elevation, rng


def apparent_position(eph, gps_seconds, obs_ecef):
    """Light-time solution plus Sagnac de-rotation."""
    pos = sat_position_ecef(eph, gps_seconds)
    tau = math.dist(pos, obs_ecef) / C
    for _ in range(2):
        pos_tx = sat_position_ecef(eph, gps_seconds - tau)
        theta = -OMEGA_E * tau
        pos = (
            pos_tx[0] * math.cos(theta) - pos_tx[1] * math.sin(theta),
            pos_tx[0] * math.sin(theta) + pos_tx[1] * math.cos(theta),
            pos_tx[2],
        )
        tau = math.dist(pos, obs_ecef) / C
    return pos


# ------------------------------------------------------------------- driver

NAV_FILE = Path(__file__).resolve().parent.parent / "data" / "BRDC00WRD_R_20250010000_01D_GN.rnx"

# Observers chosen to span CONUS: corners plus the middle.
OBSERVERS = [
    ("denver",   39.7392, -104.9903, 1609.0),
    ("seattle",  47.6062, -122.3321,   53.0),
    ("miami",    25.7617,  -80.1918,    2.0),
    ("boston",   42.3601,  -71.0589,   43.0),
    ("sandiego", 32.7157, -117.1611,   19.0),
]

EPOCHS = ["2025-01-01T12:00:00", "2025-01-01T18:30:00", "2025-01-01T03:15:00"]


def compute(records, lat, lon, alt, iso, mask=5.0):
    gps_s = unix_to_gps_seconds(iso_to_unix(iso))
    obs = geodetic_to_ecef(lat, lon, alt)

    out = []
    for prn in range(1, 33):
        eph, age = select_ephemeris(records, prn, gps_s)
        if eph is None:
            continue
        pos = apparent_position(eph, gps_s, obs)
        az, el, rng = look_angles(pos, lat, lon, alt)
        if el < mask:
            continue
        out.append({
            "prn": prn, "azimuth": az, "elevation": el,
            "range_km": rng / 1000.0, "ephemeris_age_s": age,
        })
    out.sort(key=lambda s: s["prn"])
    return gps_s, out


def dop(satellites):
    """Dilution of precision from a list of computed satellite dicts.

    Deliberately routed through the singular value decomposition rather than
    through an explicit matrix inverse. For a full-rank A,

        pinv(A) = (A^T A)^-1 A^T   =>   pinv(A) @ pinv(A).T = (A^T A)^-1 = Q

    so this reaches the same Q by an orthogonal factorisation instead of by
    forming the normal matrix and eliminating -- a different algorithm with
    different conditioning, which is the point of a cross-check.

    Returns None when the geometry admits no solution, matching the Rust.
    """
    import numpy as np

    if len(satellites) < 4:
        return None

    rows = []
    for s in satellites:
        az = math.radians(s["azimuth"])
        el = math.radians(s["elevation"])
        # Unit vector from receiver to satellite, in east/north/up.
        e = math.cos(el) * math.sin(az)
        n = math.cos(el) * math.cos(az)
        u = math.sin(el)
        rows.append([-e, -n, -u, 1.0])

    a = np.array(rows, dtype=float)

    # Rank check off the singular values, not off a pivot threshold: a
    # coplanar or coincident set is rank-deficient and has no solution.
    sv = np.linalg.svd(a, compute_uv=False)
    if sv[-1] <= sv[0] * 1e-12:
        return None

    pinv = np.linalg.pinv(a)
    q = pinv @ pinv.T
    east, north, up, time = (q[0][0], q[1][1], q[2][2], q[3][3])

    return {
        "gdop": math.sqrt(east + north + up + time),
        "pdop": math.sqrt(east + north + up),
        "hdop": math.sqrt(east + north),
        "vdop": math.sqrt(up),
        "tdop": math.sqrt(time),
        "satellites": len(satellites),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit-vectors", action="store_true",
                    help="write JSON test vectors to stdout")
    ap.add_argument("--report", action="store_true",
                    help="print a human-readable sky view")
    args = ap.parse_args()

    if not NAV_FILE.exists():
        print(f"missing {NAV_FILE}", file=sys.stderr)
        return 1
    records = parse_rinex_nav(NAV_FILE)

    if args.report:
        print(f"parsed {len(records)} GPS ephemeris records from {NAV_FILE.name}")
        for name, lat, lon, alt in OBSERVERS[:1]:
            for iso in EPOCHS[:1]:
                gps_s, sats = compute(records, lat, lon, alt, iso)
                print(f"\n{name} @ {iso}Z  (GPS {gps_s:.1f} s) "
                      f"-- {len(sats)} satellites above 5 deg")
                print(f"{'PRN':>4} {'az':>9} {'el':>8} {'range km':>11} {'age s':>8}")
                for s in sats:
                    print(f"G{s['prn']:02d} {s['azimuth']:9.4f} {s['elevation']:8.4f} "
                          f"{s['range_km']:11.3f} {s['ephemeris_age_s']:8.0f}")
                d = dop(sats)
                if d is None:
                    print("\nno DOP solution (fewer than 4 satellites, "
                          "or degenerate geometry)")
                else:
                    print(f"\nGDOP {d['gdop']:.4f}  PDOP {d['pdop']:.4f}  "
                          f"HDOP {d['hdop']:.4f}  VDOP {d['vdop']:.4f}  "
                          f"TDOP {d['tdop']:.4f}")
        return 0

    if args.emit_vectors:
        vectors = []
        for name, lat, lon, alt in OBSERVERS:
            for iso in EPOCHS:
                gps_s, sats = compute(records, lat, lon, alt, iso)
                vectors.append({
                    "observer": name,
                    "lat": lat, "lon": lon, "alt": alt,
                    "epoch_iso": iso,
                    "unix_seconds": iso_to_unix(iso),
                    "gps_seconds": gps_s,
                    "elevation_mask_deg": 5.0,
                    "satellites": sats,
                    "dop": dop(sats),
                })
        json.dump({"source": NAV_FILE.name, "cases": vectors}, sys.stdout, indent=1)
        return 0

    ap.print_help()
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
