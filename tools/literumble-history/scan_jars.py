"""Read robot metadata from every downloaded jar, without extracting or running anything.

Usage: python3 -I scan_jars.py ARCHIVE_ROOT OUT_JSON

Expects ARCHIVE_ROOT/strangeautomata/ (jars + listing.json from
fetch_strangeautomata.py) and ARCHIVE_ROOT/archiveorg/extracted/ (+ zip-entries.json
from extract_archiveorg.py). Writes one record per jar file to OUT_JSON.
"""

import datetime
import hashlib
import json
import os
import re
import statistics
import sys
import zipfile

TODAY = datetime.datetime(2026, 10, 26, 23, 59, 59)
MIN_TIME = datetime.datetime(2001, 1, 1)
PROPS_DATE = re.compile(r"^#\s*\w{3} (\w{3}) +(\d{1,2}) (\d{2}):(\d{2}):(\d{2}) (?:(\S+) )?(\d{4})\s*$")
MONTHS = {m: i for i, m in enumerate(
    "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split(), 1)}


def plausible(dt):
    return dt is not None and MIN_TIME <= dt <= TODAY


def parse_properties(text):
    """Minimal java.util.Properties reader (continuations, escapes, = : or space)."""
    props, comments = {}, []
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i].lstrip()
        i += 1
        if not line or line[0] in "#!":
            if line:
                comments.append(line)
            continue
        while line.endswith("\\") and not line.endswith("\\\\") and i < len(lines):
            line = line[:-1] + lines[i].lstrip()
            i += 1
        m = re.match(r"((?:\\.|[^=:\s\\])*)\s*[=:\s]\s*(.*)$", line)
        key, val = (m.group(1), m.group(2)) if m else (line, "")
        props[unescape(key)] = unescape(val).strip()
    return props, comments


def unescape(s):
    def rep(m):
        c = m.group(1)
        if c.startswith("u"):
            try:
                return chr(int(c[1:], 16))
            except ValueError:
                return c
        return {"t": "\t", "n": "\n", "r": "\r", "f": "\f"}.get(c, c)
    return re.sub(r"\\(u[0-9a-fA-F]{4}|.)", rep, s)


def props_saved_at(comments):
    for c in comments:
        m = PROPS_DATE.match(c)
        if m:
            mon, day, hh, mm, ss, tz, year = m.groups()
            try:
                return (datetime.datetime(int(year), MONTHS[mon], int(day), int(hh), int(mm), int(ss))
                        .isoformat(), tz)
            except (KeyError, ValueError):
                return None, None
    return None, None


def iso(dt):
    return dt.isoformat() if dt else None


def scan(path):
    rec = {"size": os.path.getsize(path)}
    with open(path, "rb") as f:
        data = f.read()
    rec["sha256"] = hashlib.sha256(data).hexdigest()
    if not data.startswith(b"PK"):
        head = data[:200].lstrip().lower()
        kind = "html/error page" if head.startswith((b"<", b"<!")) else "unknown content"
        rec.update(error=f"NotAZip: {kind}", robots=[], teams=[])
        return rec
    try:
        with zipfile.ZipFile(path) as z:
            infos = z.infolist()
            class_times, all_times, robots, teams = [], [], [], []
            for info in infos:
                try:
                    dt = datetime.datetime(*info.date_time)
                except ValueError:
                    dt = None
                if plausible(dt):
                    all_times.append(dt)
                    if info.filename.endswith(".class"):
                        class_times.append(dt)
                low = info.filename.lower()
                if low.endswith(".properties") or low.endswith(".team"):
                    if info.file_size > 1_000_000:
                        continue
                    text = z.read(info).decode("latin-1")
                    props, comments = parse_properties(text)
                    saved, tz = props_saved_at(comments)
                    if low.endswith(".team"):
                        teams.append({"file": info.filename, "members": props.get("team.members"),
                                      "version": props.get("team.version"),
                                      "author": props.get("team.author.name")})
                    elif "robot.classname" in props:
                        robots.append({"file": info.filename,
                                       "classname": props.get("robot.classname"),
                                       "version": props.get("robot.version"),
                                       "author": props.get("robot.author.name"),
                                       "robocodeVersion": props.get("robocode.version"),
                                       "codesize": props.get("robot.codesize"),
                                       "propertiesSavedAt": saved, "propertiesTz": tz})
            rec["entries"] = len(infos)
            rec["classCount"] = sum(1 for i in infos if i.filename.endswith(".class"))
            rec["classMtimeMax"] = iso(max(class_times)) if class_times else None
            rec["classMtimeMedian"] = (iso(sorted(class_times)[len(class_times) // 2])
                                       if class_times else None)
            rec["anyMtimeMax"] = iso(max(all_times)) if all_times else None
            rec["bogusClassMtimes"] = rec["classCount"] - len(class_times)
            rec["robots"] = robots
            rec["teams"] = teams
            rec["error"] = None
    except (zipfile.BadZipFile, OSError, EOFError, NotImplementedError, RuntimeError) as e:
        rec["error"] = f"{type(e).__name__}: {e}"
        rec["robots"], rec["teams"] = [], []
    return rec


def scan_retry(path, attempts=3):
    """/mnt drives under WSL sometimes fail a read with ENOMEM; try again."""
    for _ in range(attempts):
        rec = scan(path)
        if not (rec.get("error") or "").startswith("OSError"):
            break
    return rec


def main():
    root, out = sys.argv[1], sys.argv[2]
    records = []
    sa_dir = os.path.join(root, "strangeautomata")
    with open(os.path.join(sa_dir, "listing.json")) as f:
        listing = {e["name"]: e for e in json.load(f)["jars"]}
    for name in sorted(listing):
        path = os.path.join(sa_dir, listing[name].get("localName", name))
        if not os.path.exists(path):
            records.append({"source": "strangeautomata", "path": path, "fileName": name,
                            "error": "missing", "robots": [], "teams": [],
                            "archiveLastModified": listing[name]["lastModified"]})
            continue
        rec = {"source": "strangeautomata", "path": path, "fileName": name,
               "archiveLastModified": listing[name]["lastModified"]}
        rec.update(scan_retry(path))
        records.append(rec)
    ao_dir = os.path.join(root, "archiveorg", "extracted")
    zip_entries = {}
    zpath = os.path.join(root, "archiveorg", "zip-entries.json")
    if os.path.exists(zpath):
        with open(zpath) as f:
            zip_entries = json.load(f)
    for dirpath, _, files in os.walk(ao_dir):
        for name in sorted(files):
            if not name.lower().endswith(".jar"):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, ao_dir).replace(os.sep, "/")
            rec = {"source": "archiveorg", "path": path, "fileName": name,
                   "zipEntryMtime": zip_entries.get(rel, {}).get("mtime")}
            rec.update(scan_retry(path))
            records.append(rec)
    with open(out, "w") as f:
        json.dump(records, f, indent=0)
    print(f"{len(records)} jars scanned "
          f"({sum(r['source'] == 'strangeautomata' for r in records)} strangeautomata, "
          f"{sum(r['source'] == 'archiveorg' for r in records)} archiveorg), "
          f"{sum(1 for r in records if r.get('error'))} errors")


if __name__ == "__main__":
    main()
