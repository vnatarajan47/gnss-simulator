#!/usr/bin/env python3
"""Independent reference implementation of the gnss-core math.

This exists to cross-check `crates/gnss-core` -- it is deliberately *not* a
port of the Rust code. Where the Rust has a choice, this makes the other one:

  * RINEX parsing:  fixed-column slicing here, the `rinex` crate there.
  * Kepler's eqn:   fixed-point iteration here, Newton-Raphson there.
  * Elevation:      asin(up / range) here, atan2(up, horizontal) there.
  * ENU:            explicit 3x3 rotation matrix here, inlined dot products
                    there.
  * DOP:            explicit n-by-k design matrix and an SVD pseudo-inverse
                    here; the normal matrix accumulated in place and inverted
                    by Gauss-Jordan there.

Agreement between two implementations that share no code is meaningful
evidence; agreement between a port and its original is not.

Covers GPS, Galileo, BeiDou and QZSS. They share one algorithm with different
constants, plus one genuine structural difference: BeiDou's geostationary
satellites reach Earth-fixed coordinates through an extra pair of rotations.

Sources:
  IS-GPS-200N Table 20-IV       (satellite position from broadcast ephemeris)
  Galileo OS SIS ICD 2.1 §5.1.1 (same algorithm, GTRF constants)
  BDS-SIS-ICD-B1I 3.0 §5.2.4.12 (same algorithm, plus the GEO transformation)
  IS-QZSS-PNT-006               (GPS-interoperable, GPS constants)
  NIMA TR8350.2                 (WGS-84 ellipsoid)

Usage:
    python3 tools/reference_skyplot.py --emit-vectors        # GPS vectors
    python3 tools/reference_skyplot.py --emit-multi-vectors  # multi-GNSS
    python3 tools/reference_skyplot.py --report              # readable

The `--emit-*` modes need numpy, for the SVD. Nothing else here does, and the
generated JSON is checked in, so running the Rust test suite never needs it.
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import json
import math
import sys
from pathlib import Path

# ---------------------------------------------------------------- constants

MU_GPS = 3.986005e14            # m^3/s^2, WGS-84 value used by GPS
MU_GTRF = 3.986004418e14        # m^3/s^2, used by Galileo and BeiDou
OMEGA_E = 7.2921151467e-5       # rad/s
OMEGA_E_BDS = 7.292115e-5       # rad/s, BeiDou's slightly different value
C = 299792458.0                 # m/s
WGS84_A = 6378137.0
WGS84_F = 1.0 / 298.257223563
WGS84_B = WGS84_A * (1.0 - WGS84_F)
WGS84_E2 = WGS84_F * (2.0 - WGS84_F)
SECONDS_PER_WEEK = 604800.0
GPS_EPOCH_UNIX = 315964800.0

# BeiDou counts weeks from 2006-01-01, which is GPS week 1356, and BDT runs
# 14 s behind GPS time.
BDT_EPOCH_GPS_WEEK = 1356
BDT_TO_GPST_S = 14.0

# BeiDou's GEO transformation: a fixed 5 deg tilt, then the Earth rotation.
BDS_GEO_TILT_RAD = math.radians(-5.0)
# Inclination separating BeiDou's geostationary satellites (broadcast i0 of a
# few degrees, referred to that tilted plane) from its IGSO and MEO ones (~55
# deg). Two orders of magnitude apart, so the exact threshold is immaterial.
BDS_GEO_MAX_INCLINATION_RAD = math.radians(10.0)

# Per-system constants. `time_system` is the clock group used for DOP: a
# receiver estimates one clock offset per group, so satellites that share a
# group share a column of the design matrix.
SYSTEMS = {
    "G": {"name": "GPS",     "mu": MU_GPS,  "omega_e": OMEGA_E,     "time_system": "GPS"},
    "E": {"name": "Galileo", "mu": MU_GTRF, "omega_e": OMEGA_E,     "time_system": "GAL"},
    "C": {"name": "BeiDou",  "mu": MU_GTRF, "omega_e": OMEGA_E_BDS, "time_system": "BDS"},
    "J": {"name": "QZSS",    "mu": MU_GPS,  "omega_e": OMEGA_E,     "time_system": "GPS"},
}

# Column order for the clock unknowns, and therefore which system's clock TDOP
# reports: the lowest one present.
TIME_SYSTEM_ORDER = ["GPS", "GAL", "BDS"]

# Curve-fit half-interval: how far either side of ToE a block stays usable.
FIT_HALF_INTERVAL_S = 7200.0

# All six bits of the health field set. Only QZSS needs this: its health word
# is a per-signal bitfield whose unused-signal bits stay set, so no QZSS record
# in this product ever reads zero and the plain `health == 0` rule would
# discard the constellation outright.
HEALTH_ALL_BITS = 0b111111

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

# Broadcast-orbit field layout, RINEX 3.0x, in transmission order.
#
# One table serves all four systems. The lines carrying the Keplerian elements
# are identical between them; only fields this module never reads differ
# (Galileo's data sources where GPS has L2 codes, BeiDou's AODC where GPS has
# IODC), and `week` and `health` sit in the same slot throughout.
ORBIT_FIELDS = [
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


def read_nav_text(path: Path) -> str:
    """Read a RINEX file, transparently ungzipping it."""
    raw = path.read_bytes()
    if raw[:2] == b"\x1f\x8b":
        raw = gzip.decompress(raw)
    return raw.decode("latin1")


def parse_rinex_nav(path: Path, systems: str = "G") -> list[dict]:
    """Fixed-column parse of a RINEX 3 navigation file.

    `systems` is a string of RINEX constellation codes to keep. Records from
    any other system -- including GLONASS and SBAS, whose records are four
    lines rather than eight -- are skipped by scanning for the next line that
    starts a record, rather than by assuming a fixed stride.
    """
    lines = read_nav_text(path).splitlines()

    end = next(i for i, l in enumerate(lines) if "END OF HEADER" in l)
    body = lines[end + 1:]

    def starts_record(line: str) -> bool:
        return (len(line) >= 3 and line[0].isalpha() and line[0].isupper()
                and line[1:3].isdigit())

    records = []
    i = 0
    while i < len(body):
        line = body[i]
        if not starts_record(line):
            i += 1
            continue

        # Find where this record ends, whatever its length.
        end_of_record = i + 1
        while end_of_record < len(body) and not starts_record(body[end_of_record]):
            end_of_record += 1

        if line[0] in systems and end_of_record - i >= len(ORBIT_FIELDS) + 1:
            rec = {"sys": line[0], "prn": int(line[1:3])}
            rec["sv"] = f"{rec['sys']}{rec['prn']:02d}"
            # Epoch (Time of Clock) then af0/af1/af2.
            rec["toc_iso"] = (
                f"{int(line[4:8]):04d}-{int(line[9:11]):02d}-{int(line[12:14]):02d}"
                f"T{int(line[15:17]):02d}:{int(line[18:20]):02d}:{int(line[21:23]):02d}"
            )
            rec["af0"] = _f(line[23:42])
            rec["af1"] = _f(line[42:61])
            rec["af2"] = _f(line[61:80])

            for row, names in enumerate(ORBIT_FIELDS):
                cont = body[i + 1 + row]
                for col, name in enumerate(names):
                    start = 4 + col * 19
                    rec[name] = _f(cont[start:start + 19])

            records.append(rec)

        i = end_of_record

    return records


def toe_absolute(eph: dict) -> float:
    """Time of ephemeris on the continuous GPS timescale [s].

    Galileo and QZSS week numbers are already on the GPS week count in RINEX 3.
    BeiDou's are BDT weeks from 2006, on a scale running 14 s behind GPS.
    """
    if eph["sys"] == "C":
        week = eph["week"] + BDT_EPOCH_GPS_WEEK
        return week * SECONDS_PER_WEEK + eph["toe"] + BDT_TO_GPST_S
    return eph["week"] * SECONDS_PER_WEEK + eph["toe"]


def is_healthy(eph: dict) -> bool:
    """Whether the broadcast health word marks the satellite usable.

    Zero everywhere except QZSS, whose word is a per-signal bitfield that never
    reads zero in this product; only the all-ones filler is taken to mean the
    satellite itself is unusable.
    """
    health = int(eph["health"])
    if eph["sys"] == "J":
        return health != HEALTH_ALL_BITS
    return health == 0


def select_ephemeris(records, sv: str, gps_seconds: float,
                     max_age=FIT_HALF_INTERVAL_S):
    """Nearest-ToE selection, matching gnss-core's default strategy.

    Ties on |age| -- the same block re-issued -- go to the larger issue of
    data, which is the more recent upload. Galileo makes that reachable rather
    than theoretical: it broadcasts I/NAV and F/NAV records sharing a ToE.
    """
    best, best_age = None, None
    for rec in records:
        if rec["sv"] != sv or not is_healthy(rec):
            continue
        age = gps_seconds - toe_absolute(rec)
        if abs(age) > max_age:
            continue
        if best_age is None or abs(age) < abs(best_age) or (
            abs(age) == abs(best_age) and rec["iode"] > best["iode"]
        ):
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


def is_beidou_geo(eph: dict) -> bool:
    """Whether this record needs BeiDou's geostationary transformation.

    Read off the broadcast inclination rather than a PRN table, so it cannot go
    stale as satellites are launched and retired.
    """
    return eph["sys"] == "C" and abs(eph["i0"]) < BDS_GEO_MAX_INCLINATION_RAD


def sat_position_ecef(eph: dict, gps_seconds: float) -> tuple[float, float, float]:
    """IS-GPS-200 Table 20-IV, written out longhand.

    Galileo, BeiDou and QZSS use the same procedure with their own `mu` and
    Earth rotation rate. BeiDou's geostationary satellites additionally take
    the ICD's alternative route into Earth-fixed coordinates.
    """
    system = SYSTEMS[eph["sys"]]
    mu, omega_e = system["mu"], system["omega_e"]

    a = eph["sqrt_a"] ** 2
    tk = gps_seconds - toe_absolute(eph)

    n0 = math.sqrt(mu / a ** 3)
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

    geo = is_beidou_geo(eph)
    # The GEO node leaves the Earth rotation out; it is applied below instead.
    node_rate = eph["omega_dot"] if geo else eph["omega_dot"] - omega_e
    omega_k = eph["omega0"] + node_rate * tk - omega_e * eph["toe"]

    x = xp * math.cos(omega_k) - yp * math.cos(ik) * math.sin(omega_k)
    y = xp * math.sin(omega_k) + yp * math.cos(ik) * math.cos(omega_k)
    z = yp * math.sin(ik)

    if not geo:
        return x, y, z

    # Rz(omega_e * tk) . Rx(-5 deg), in the ICD's frame-rotation convention.
    cx, sx = math.cos(BDS_GEO_TILT_RAD), math.sin(BDS_GEO_TILT_RAD)
    y1 = cx * y + sx * z
    z1 = -sx * y + cx * z
    phi = omega_e * tk
    cz, sz = math.cos(phi), math.sin(phi)
    return (cz * x + sz * y1, -sz * x + cz * y1, z1)


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
    omega_e = SYSTEMS[eph["sys"]]["omega_e"]
    pos = sat_position_ecef(eph, gps_seconds)
    tau = math.dist(pos, obs_ecef) / C
    for _ in range(2):
        pos_tx = sat_position_ecef(eph, gps_seconds - tau)
        theta = -omega_e * tau
        pos = (
            pos_tx[0] * math.cos(theta) - pos_tx[1] * math.sin(theta),
            pos_tx[0] * math.sin(theta) + pos_tx[1] * math.cos(theta),
            pos_tx[2],
        )
        tau = math.dist(pos, obs_ecef) / C
    return pos


# ------------------------------------------------------------------- driver

DATA = Path(__file__).resolve().parent.parent / "data"

# GPS-only fixture, and the CONUS observers phase 1 was validated against.
GPS_NAV_FILE = DATA / "BRDC00WRD_R_20250010000_01D_GN.rnx"
GPS_OBSERVERS = [
    ("denver",   39.7392, -104.9903, 1609.0),
    ("seattle",  47.6062, -122.3321,   53.0),
    ("miami",    25.7617,  -80.1918,    2.0),
    ("boston",   42.3601,  -71.0589,   43.0),
    ("sandiego", 32.7157, -117.1611,   19.0),
]
GPS_EPOCHS = ["2025-01-01T12:00:00", "2025-01-01T18:30:00", "2025-01-01T03:15:00"]

# Multi-constellation fixture: four hours of GPS, Galileo, BeiDou and QZSS.
MULTI_NAV_FILE = DATA / "BRDC00WRD_R_20262201000_04H_MN.rnx.gz"

# Observers spread over the globe, since the app is no longer CONUS-only.
# McMurdo is deliberate: at 78 deg south the MEO constellations never pass
# overhead, so it is the case where geometry is worst and any latitude-
# dependent sign error would show.
MULTI_OBSERVERS = [
    ("denver",    39.7392, -104.9903, 1609.0),
    ("london",    51.5074,   -0.1278,   11.0),
    ("tokyo",     35.6762,  139.6503,   40.0),
    ("sydney",   -33.8688,  151.2093,   58.0),
    ("nairobi",   -1.2921,   36.8219, 1795.0),
    ("mcmurdo",  -77.8419,  166.6863,   10.0),
]
MULTI_EPOCHS = ["2026-08-08T11:00:00", "2026-08-08T13:00:00"]

# Which system combinations to emit. One system with a non-GPS reference
# clock, two systems, then four constellations across three clocks.
MULTI_SYSTEM_SETS = ["E", "GE", "GECJ"]


def compute(records, lat, lon, alt, iso, systems="G", mask=5.0):
    """Sky view for one observer and instant, over the given systems."""
    gps_s = unix_to_gps_seconds(iso_to_unix(iso))
    obs = geodetic_to_ecef(lat, lon, alt)

    wanted = sorted({r["sv"] for r in records if r["sys"] in systems})

    out = []
    for sv in wanted:
        eph, age = select_ephemeris(records, sv, gps_s)
        if eph is None:
            continue
        pos = apparent_position(eph, gps_s, obs)
        az, el, rng = look_angles(pos, lat, lon, alt)
        if el < mask:
            continue
        out.append({
            "sv": sv,
            "prn": eph["prn"],
            "time_system": SYSTEMS[eph["sys"]]["time_system"],
            "azimuth": az, "elevation": el,
            "range_km": rng / 1000.0, "ephemeris_age_s": age,
        })
    out.sort(key=lambda s: s["sv"])
    return gps_s, out


def dop(satellites):
    """Dilution of precision from a list of computed satellite dicts.

    Deliberately routed through the singular value decomposition rather than
    through an explicit matrix inverse. For a full-rank A,

        pinv(A) = (A^T A)^-1 A^T   =>   pinv(A) @ pinv(A).T = (A^T A)^-1 = Q

    so this reaches the same Q by an orthogonal factorisation instead of by
    forming the normal matrix and eliminating -- a different algorithm with
    different conditioning, which is the point of a cross-check.

    Satellites from different time systems do not share a receiver clock, so
    each system present gets its own column and raises the minimum satellite
    count by one. Returns None where there is no solution, matching the Rust.
    """
    import numpy as np

    systems = sorted({s.get("time_system", "GPS") for s in satellites},
                     key=TIME_SYSTEM_ORDER.index)
    unknowns = 3 + len(systems)
    if len(satellites) < unknowns:
        return None

    rows = []
    for s in satellites:
        az = math.radians(s["azimuth"])
        el = math.radians(s["elevation"])
        # Unit vector from receiver to satellite, in east/north/up.
        e = math.cos(el) * math.sin(az)
        n = math.cos(el) * math.cos(az)
        u = math.sin(el)
        clocks = [0.0] * len(systems)
        clocks[systems.index(s.get("time_system", "GPS"))] = 1.0
        rows.append([-e, -n, -u] + clocks)

    a = np.array(rows, dtype=float)

    # Rank check off the singular values, not off a pivot threshold: a
    # coplanar or coincident set is rank-deficient and has no solution, and so
    # is a system whose satellites cannot be told apart from its own clock.
    sv = np.linalg.svd(a, compute_uv=False)
    if sv[-1] <= sv[0] * 1e-12:
        return None

    pinv = np.linalg.pinv(a)
    q = pinv @ pinv.T
    east, north, up = q[0][0], q[1][1], q[2][2]
    clocks = [q[3 + i][3 + i] for i in range(len(systems))]

    return {
        "gdop": math.sqrt(east + north + up + sum(clocks)),
        "pdop": math.sqrt(east + north + up),
        "hdop": math.sqrt(east + north),
        "vdop": math.sqrt(up),
        # The reference system's clock: the lowest one present.
        "tdop": math.sqrt(clocks[0]),
        "satellites": len(satellites),
        "systems": len(systems),
    }


def emit_cases(nav_file, records, observers, epochs, system_sets):
    cases = []
    for systems in system_sets:
        for name, lat, lon, alt in observers:
            for iso in epochs:
                gps_s, sats = compute(records, lat, lon, alt, iso, systems)
                cases.append({
                    "observer": name,
                    "lat": lat, "lon": lon, "alt": alt,
                    "epoch_iso": iso,
                    "unix_seconds": iso_to_unix(iso),
                    "gps_seconds": gps_s,
                    "elevation_mask_deg": 5.0,
                    "systems": systems,
                    "satellites": sats,
                    "dop": dop(sats),
                })
    return {"source": nav_file.name, "cases": cases}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--emit-vectors", action="store_true",
                    help="write GPS JSON test vectors to stdout")
    ap.add_argument("--emit-multi-vectors", action="store_true",
                    help="write multi-constellation JSON test vectors to stdout")
    ap.add_argument("--report", action="store_true",
                    help="print a human-readable sky view")
    args = ap.parse_args()

    if args.report:
        for nav_file, systems, observers, epochs in (
            (GPS_NAV_FILE, "G", GPS_OBSERVERS, GPS_EPOCHS),
            (MULTI_NAV_FILE, "GECJ", MULTI_OBSERVERS, MULTI_EPOCHS),
        ):
            if not nav_file.exists():
                print(f"missing {nav_file}", file=sys.stderr)
                return 1
            records = parse_rinex_nav(nav_file, systems)
            name, lat, lon, alt = observers[0]
            iso = epochs[0]
            gps_s, sats = compute(records, lat, lon, alt, iso, systems)
            print(f"\nparsed {len(records)} [{systems}] ephemeris records "
                  f"from {nav_file.name}")
            print(f"{name} @ {iso}Z  (GPS {gps_s:.1f} s) "
                  f"-- {len(sats)} satellites above 5 deg")
            print(f"{'SV':>4} {'az':>9} {'el':>8} {'range km':>11} {'age s':>8}")
            for s in sats:
                print(f"{s['sv']:>4} {s['azimuth']:9.4f} {s['elevation']:8.4f} "
                      f"{s['range_km']:11.3f} {s['ephemeris_age_s']:8.0f}")
            d = dop(sats)
            if d is None:
                print("no DOP solution (too few satellites for the number of "
                      "clock unknowns, or degenerate geometry)")
            else:
                print(f"GDOP {d['gdop']:.4f}  PDOP {d['pdop']:.4f}  "
                      f"HDOP {d['hdop']:.4f}  VDOP {d['vdop']:.4f}  "
                      f"TDOP {d['tdop']:.4f}  ({d['systems']} clock(s))")
        return 0

    if args.emit_vectors:
        records = parse_rinex_nav(GPS_NAV_FILE, "G")
        json.dump(emit_cases(GPS_NAV_FILE, records, GPS_OBSERVERS, GPS_EPOCHS, ["G"]),
                  sys.stdout, indent=1)
        return 0

    if args.emit_multi_vectors:
        records = parse_rinex_nav(MULTI_NAV_FILE, "GECJ")
        json.dump(emit_cases(MULTI_NAV_FILE, records, MULTI_OBSERVERS,
                             MULTI_EPOCHS, MULTI_SYSTEM_SETS),
                  sys.stdout, indent=1)
        return 0

    ap.print_help()
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
