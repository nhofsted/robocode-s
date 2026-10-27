"""Download every robot jar from the strangeautomata Robocode archive.

Usage: python3 -I fetch_strangeautomata.py OUT_DIR

Writes OUT_DIR/listing.json (name, localName, url, lastModified, size per jar,
plus any subdirectories seen) and the jars themselves into OUT_DIR. Resume-safe: a jar
already present with the listed size is skipped. At most 4 downloads run at
once; each is retried with backoff.
"""

import concurrent.futures
import html
import json
import os
import re
import sys
import time
import urllib.parse
import urllib.request

BASE = "https://robocode-archive.strangeautomata.com/robots/"
UA = "robocode-s-history/1.0 (contact via github.com/nhofsted/robocodeS)"
ROW = re.compile(
    r'<tr(?: class="d")?><td class="n"><a href="([^"]+)">.*?</a>/?</td>'
    r'<td class="m"[^>]*>([^<]*)</td><td class="s"(?: data-value="(-?\d+)")?[^>]*>[^<]*</td>'
    r'<td class="t">([^<]*)</td></tr>'
)


def get(url, attempts=5):
    for i in range(attempts):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.read()
        except Exception as e:  # noqa: BLE001 - retry anything network-ish
            if i == attempts - 1:
                raise
            time.sleep(2 ** i)
            print(f"retry {url}: {e}", file=sys.stderr)


def parse_listing(text):
    entries, dirs = [], []
    for href, mtime, size, typ in ROW.findall(text):
        href = html.unescape(href)
        if href == "../":
            continue
        if typ == "Directory":
            dirs.append({"name": href, "lastModified": mtime.strip()})
            continue
        name = urllib.parse.unquote(href)
        if "/" in name or name.startswith("."):
            continue
        # Server-local time, no zone in the listing; build_dataset.py converts it
        # (it is UTC-7 all year: new jars then appear 0-1 h after their wiki listing).
        lm = time.strftime("%Y-%m-%dT%H:%M:%S", time.strptime(mtime.strip(), "%Y-%b-%d %H:%M:%S"))
        entries.append({"name": name, "url": BASE + href, "lastModified": lm, "size": int(size)})
    return entries, dirs


def assign_local_names(jars):
    """Names differing only in case collide on case-insensitive disks (NTFS
    under WSL): keep the first as is, store the others as NAME~caseN.jar."""
    seen = {}
    for e in sorted(jars, key=lambda e: e["name"]):
        n = seen.get(e["name"].lower(), 0)
        seen[e["name"].lower()] = n + 1
        e["localName"] = e["name"] if n == 0 else f"{e['name'][:-4]}~case{n + 1}.jar"


def download(entry, out_dir):
    path = os.path.join(out_dir, entry["localName"])
    if os.path.exists(path) and os.path.getsize(path) == entry["size"]:
        return "skip", 0
    data = get(entry["url"])
    if len(data) != entry["size"]:
        print(f"size mismatch {entry['name']}: {len(data)} != {entry['size']}", file=sys.stderr)
    tmp = path + ".part"
    with open(tmp, "wb") as f:
        f.write(data)
    os.replace(tmp, path)
    return "got", len(data)


def main():
    out_dir = sys.argv[1]
    os.makedirs(out_dir, exist_ok=True)
    entries, dirs = parse_listing(get(BASE).decode("utf-8", "replace"))
    for d in dirs:  # report subdirectories and any jars inside them
        sub, _ = parse_listing(get(BASE + d["name"]).decode("utf-8", "replace"))
        d["jarCount"] = sum(1 for e in sub if e["name"].endswith(".jar"))
    jars = [e for e in entries if e["name"].lower().endswith(".jar")]
    assign_local_names(jars)
    with open(os.path.join(out_dir, "listing.json"), "w") as f:
        json.dump({"fetchedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                   "base": BASE, "subdirectories": dirs, "jars": jars}, f, indent=1)
    print(f"{len(jars)} jars listed; subdirs: {dirs}")
    t0, got, skipped, nbytes, failed = time.time(), 0, 0, 0, []
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        futs = {pool.submit(download, e, out_dir): e for e in jars}
        for i, fut in enumerate(concurrent.futures.as_completed(futs), 1):
            try:
                kind, n = fut.result()
                got += kind == "got"
                skipped += kind == "skip"
                nbytes += n
            except Exception as e:  # noqa: BLE001
                failed.append(futs[fut]["name"])
                print(f"FAILED {futs[fut]['name']}: {e}", file=sys.stderr)
            if i % 250 == 0:
                print(f"{i}/{len(jars)} done, {nbytes/1e6:.1f} MB", flush=True)
    print(f"downloaded {got}, skipped {skipped}, failed {len(failed)}, "
          f"{nbytes/1e6:.1f} MB in {time.time()-t0:.0f}s")
    for name in failed:
        print("failed:", name)


if __name__ == "__main__":
    main()
