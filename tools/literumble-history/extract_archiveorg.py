"""Safely extract the archive.org all-robots zip and record entry mtimes.

Usage: python3 -I extract_archiveorg.py ZIP OUT_DIR

Rejects absolute paths, drive letters, backslashes and '..' components.
Writes OUT_DIR/../zip-entries.json: {name: {"size", "mtime"}} for every entry.
Re-runnable: entries already extracted with the right size are skipped.
"""

import json
import os
import shutil
import sys
import zipfile


def safe_name(name):
    if not name or name.startswith(("/", "\\")) or "\\" in name or ":" in name:
        return None
    parts = [p for p in name.split("/") if p not in ("", ".")]
    if not parts or any(p == ".." for p in parts):
        return None
    return parts


def main():
    zpath, out_dir = sys.argv[1], sys.argv[2]
    os.makedirs(out_dir, exist_ok=True)
    root = os.path.realpath(out_dir)
    entries, rejected, written = {}, [], 0
    with zipfile.ZipFile(zpath) as z:
        for info in z.infolist():
            y, mo, d, h, mi, s = info.date_time
            entries[info.filename] = {"size": info.file_size,
                                      "mtime": f"{y:04d}-{mo:02d}-{d:02d}T{h:02d}:{mi:02d}:{s:02d}"}
            parts = safe_name(info.filename)
            if parts is None:
                rejected.append(info.filename)
                continue
            target = os.path.realpath(os.path.join(root, *parts))
            if not target.startswith(root + os.sep):
                rejected.append(info.filename)
                continue
            if info.is_dir():
                os.makedirs(target, exist_ok=True)
                continue
            if os.path.exists(target) and os.path.getsize(target) == info.file_size:
                continue
            os.makedirs(os.path.dirname(target), exist_ok=True)
            with z.open(info) as src, open(target, "wb") as dst:
                shutil.copyfileobj(src, dst)
            written += 1
    with open(os.path.join(os.path.dirname(root), "zip-entries.json"), "w") as f:
        json.dump(entries, f, indent=0)
    print(f"{len(entries)} entries, {written} written, {len(rejected)} rejected")
    for r in rejected:
        print("rejected:", r)


if __name__ == "__main__":
    main()
