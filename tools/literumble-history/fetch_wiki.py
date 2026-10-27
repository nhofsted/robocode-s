"""Fetch the full revision history of the RoboRumble participant pages.

Usage: python3 -I fetch_wiki.py OUT_DIR [PAGE ...]

For each page writes OUT_DIR/<page with / replaced by __>.jsonl, one revision
per line (revid, parentid, timestamp, user, comment, content), oldest first.
Resume-safe: an existing file is continued after its last revision.

Revisions come from Special:Export (history with offset/limit), a few hundred
per request. The MediaWiki API (api.php, prop=revisions) gives at most 50
revisions with content per request and robowiki.net answered it with 429 after
about a thousand revisions, for a long time; Special:Export was not limited.
The page list (OUT_DIR/allpages.json) comes from the API, once.
"""

import calendar
import json
import os
import sys
import time
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET

API = "https://robowiki.net/w/api.php"
EXPORT = "https://robowiki.net/w/index.php?title=Special:Export"
UA = "robocode-s-history/1.0 (contact via github.com/nhofsted/robocodeS)"
PACE = 15.0   # seconds between requests
BATCH = 250   # revisions per export request
DEFAULT_PAGES = [
    "RoboRumble/Participants",
    "RoboRumble/Participants/Melee",
    "RoboRumble/Participants/GigaRumble",
    "RoboRumble/Participants/MeleeTop30",
]


def request(url, data=None, attempts=15):
    for i in range(attempts):
        try:
            req = urllib.request.Request(url, data=data, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=300) as r:
                body = r.read()
            time.sleep(PACE)
            return body
        except Exception as e:  # noqa: BLE001
            if i == attempts - 1:
                raise
            print(f"{time.strftime('%H:%M:%S')} retry: {e}", file=sys.stderr, flush=True)
            time.sleep(min(300, 30 * (i + 1)))


def fname(title):
    return title.replace("/", "__") + ".jsonl"


def read_existing(path):
    """Return (last revision, set of revids); drop a partial trailing line."""
    last, ids, good = None, set(), 0
    if os.path.exists(path):
        with open(path, "rb") as f:
            for line in f:
                try:
                    rev = json.loads(line)
                except ValueError:
                    break
                last = rev
                ids.add(rev["revid"])
                good += len(line)
        with open(path, "r+b") as f:
            f.truncate(good)
    return last, ids


def local(tag):
    return tag.rsplit("}", 1)[-1]


def parse_export(body):
    root = ET.fromstring(body)
    revs = []
    for el in root.iter():
        if local(el.tag) != "revision":
            continue
        rev = {"revid": None, "parentid": None, "timestamp": None, "user": None,
               "comment": "", "content": None, "texthidden": False}
        for child in el:
            t = local(child.tag)
            if t == "id":
                rev["revid"] = int(child.text)
            elif t == "parentid":
                rev["parentid"] = int(child.text)
            elif t == "timestamp":
                rev["timestamp"] = child.text
            elif t == "comment":
                rev["comment"] = child.text or ""
            elif t == "contributor":
                for c in child:
                    if local(c.tag) in ("username", "ip"):
                        rev["user"] = c.text
            elif t == "text":
                if child.get("deleted"):
                    rev["texthidden"] = True
                else:
                    rev["content"] = child.text or ""
        revs.append(rev)
    return revs


def one_second_before(stamp):
    secs = calendar.timegm(time.strptime(stamp, "%Y-%m-%dT%H:%M:%SZ")) - 1
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(secs))


def fetch_page(title, out_dir):
    path = os.path.join(out_dir, fname(title))
    last, ids = read_existing(path)
    # offset is exclusive; step back a second so revisions sharing a timestamp
    # are not skipped (duplicates are dropped by revid).
    offset = one_second_before(last["timestamp"]) if last else "1"
    n = 0
    with open(path, "a") as f:
        while True:
            data = urllib.parse.urlencode({"pages": title, "offset": offset, "limit": BATCH,
                                           "action": "submit"}).encode()
            revs = parse_export(request(EXPORT, data))
            new = [r for r in revs if r["revid"] not in ids]
            for rev in sorted(new, key=lambda r: (r["timestamp"], r["revid"])):
                f.write(json.dumps(rev) + "\n")
                ids.add(rev["revid"])
            f.flush()
            n += len(new)
            print(f"{title}: {n} new revisions, up to "
                  f"{max((r['timestamp'] for r in revs), default=offset)}", flush=True)
            if not new:
                break
            offset = one_second_before(max(r["timestamp"] for r in revs))
    print(f"{title}: done, {len(ids)} revisions", flush=True)


def main():
    out_dir = sys.argv[1]
    os.makedirs(out_dir, exist_ok=True)
    allpages = os.path.join(out_dir, "allpages.json")
    if not os.path.exists(allpages):
        q = urllib.parse.urlencode({"action": "query", "list": "allpages", "format": "json",
                                    "apprefix": "RoboRumble/Participants", "aplimit": "500"})
        with open(allpages, "w") as f:
            json.dump(json.loads(request(API + "?" + q))["query"]["allpages"], f, indent=1)
    for title in sys.argv[2:] or DEFAULT_PAGES:
        fetch_page(title, out_dir)


if __name__ == "__main__":
    main()
