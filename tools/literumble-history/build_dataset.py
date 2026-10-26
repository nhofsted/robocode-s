"""Combine wiki participant history and scanned jars into versions.json + summary.md.

Usage: python3 -I build_dataset.py ARCHIVE_ROOT

Reads ARCHIVE_ROOT/wiki/*.jsonl (fetch_wiki.py), ARCHIVE_ROOT/dataset/jars.json
(scan_jars.py) and ARCHIVE_ROOT/strangeautomata/listing.json; writes
ARCHIVE_ROOT/dataset/versions.json, wiki-history.json and summary.md.
"""

import collections
import datetime
import json
import os
import re
import sys

# Wiki page title -> short page name used in the dataset.
PAGES = {
    "RoboRumble/Participants": "1v1",
    "RoboRumble/Participants/Melee": "melee",
    "RoboRumble/Participants/GigaRumble": "gigarumble",
    "RoboRumble/Participants/MeleeTop30": "meleetop30",
}
CURRENT_PAGES = ("RoboRumble/Participants", "RoboRumble/Participants/Melee")
ID_RE = re.compile(r"^[A-Za-z_$][\w$]*(\.[A-Za-z_$][\w$]*)+$")
PRE_RE = re.compile(r"<pre>(.*?)</pre>", re.S | re.I)
SA_UTC_OFFSET = datetime.timedelta(hours=7)  # add to listing time to get UTC
WIKI_SLOP = datetime.timedelta(days=1)   # timezones: jar mtimes are local time
LONG_BEFORE = datetime.timedelta(days=730)
SAMPLE_IDS = ["jk.mega.DrussGT", "voidious.Diamond", "kc.serpent.WaveSerpent", "abc.Shadow",
              "pez.rumble.CassiusClay", "ar.horizon.Horizon", "aaa.n.Neuromancer",
              "cs.s2.Seraphim", "voidious.Dookious", "kc.mega.BeepBoop",
              "rz.HawkOnFire", "jk.precise.Wallaby", "abc.tron3.Tron", "pe.SandboxDT",
              "florent.XSeries", "wiki.nano.RaikoNano", "simonton.mini.WeeksOnEnd",
              "darkcanuck.Gaff", "Krabb.sliNk.Garm", "positive.Portia"]


def sa_to_utc(local):
    """strangeautomata listing times are server-local, a fixed UTC-7 (no DST):
    with that offset new jars appear 0-60 min after their wiki listing, in
    summer and winter alike."""
    if not local:
        return None
    t = datetime.datetime.fromisoformat(local.rstrip("Z")) + SA_UTC_OFFSET
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


NOTES = """## Hand check of the sample (2026-10-26)

All sample dates above look plausible against what the wiki and community
remember: DrussGT 0.1 Sep 2007, Diamond 1.0 May 2009, Dookious 0.51 Dec 2005,
CassiusClay 1.9.9.24 Oct 2004, SandboxDT 1.91 Jun 2003, HawkOnFire 0.1 Jun 2003,
BeepBoop 0.1 May 2021, DrussGT 3.1.16 Sep 2026. Wiki listings usually follow
the jar's class times by minutes to hours (the class times are the author's
local time, so a few hours either way is time zone, not error).

## Odd things

- strangeautomata "Last Modified" is server time at a fixed UTC-7 (no DST);
  converted, new jars appear 0-60 min after their wiki listing, so the
  archive polls the participant lists. 6039 of its jars carry 2021-01-01
  (bulk upload) and the archive.org zip's entry times have the same bulk date:
  both are useless as release dates before 2021 (flag archiveBulkUploadDate).
- Eight pairs of strangeautomata file names differ only in case; on a
  case-insensitive disk they collide, so the second is stored as
  NAME~case2.jar. The archive.org zip has only one of each pair.
- Some "jars" are HTML error pages saved under a .jar name (mjb.Lothrik,
  techdude.Carruthers, whind.Strength*, robar.nano.Assertive, ...); one
  (axeBots.SilverSurfer 2.08) is a truncated zip.
- The same jar is often shipped under two names (a robot and its team);
  versions.json has an entry per name.
- The wiki history starts 2007-11-11 (1v1, copied from the old wiki) and
  2009-01-06 (melee); bots on those first revisions are "preHistory".
- Wiki lines with typos (wrong case, "version url" without comma, version
  "1.0.jar") produce noJar entries; most noJar entries are versions whose
  download link (robocoderepository.com, dropbox, personal sites) died before
  any archive copied them.
- robowiki.net's api.php answered 429 for over an hour after ~1000 revisions
  with content; Special:Export served the full history without trouble."""


def ts(s):
    """Parse an ISO timestamp (with or without Z) to a naive UTC-ish datetime."""
    if not s:
        return None
    return datetime.datetime.fromisoformat(s.replace("Z", ""))


def fmt(dt):
    return dt.strftime("%Y-%m-%dT%H:%M:%S") + "Z" if dt else None


def parse_listing(content):
    """Return ({(id, version): url}, unparsed_line_count) for one revision's text."""
    out, bad = {}, 0
    for block in PRE_RE.findall(content or ""):
        for raw in block.splitlines():
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            left, url = (line.rsplit(",", 1) + [None])[:2] if "," in line else (line, None)
            parts = left.split()
            if url is None and len(parts) >= 3 and "://" in parts[-1]:
                url = parts.pop()  # "id version url": the comma was forgotten
            if not parts or not ID_RE.match(parts[0]):
                bad += 1
                continue
            out[(parts[0], " ".join(parts[1:]))] = url.strip() if url else None
    return out, bad


def load_wiki(wiki_dir):
    """Walk each page's revisions and build per-(id, version) listing history."""
    hist = {}
    page_info, current = {}, {}
    for title, short in PAGES.items():
        path = os.path.join(wiki_dir, title.replace("/", "__") + ".jsonl")
        if not os.path.exists(path):
            continue
        revs = []
        with open(path) as f:
            for line in f:
                try:
                    revs.append(json.loads(line))
                except ValueError:
                    pass  # partial line from an interrupted fetch
        revs = [r for r in revs if r.get("content") is not None]
        revs.sort(key=lambda r: (r["timestamp"], r["revid"]))
        if not revs:
            continue
        page_info[title] = {"page": short, "revisions": len(revs),
                            "first": revs[0]["timestamp"], "last": revs[-1]["timestamp"],
                            "unparsedLines": 0}
        prev = set()
        for i, rev in enumerate(revs):
            listed, bad = parse_listing(rev["content"])
            page_info[title]["unparsedLines"] = max(page_info[title]["unparsedLines"], bad)
            now = set(listed)
            for key in now:
                h = hist.setdefault(key, {"firstListed": rev["timestamp"], "firstRevid": rev["revid"],
                                          "firstEditor": rev.get("user"),
                                          "firstComment": rev.get("comment"),
                                          "firstPage": short, "preHistory": False,
                                          "lastListed": None, "pages": set(), "urls": set(),
                                          "spans": 0, "stillListed": set(), "removedAt": None})
                if rev["timestamp"] < h["firstListed"]:
                    h.update(firstListed=rev["timestamp"], firstRevid=rev["revid"],
                             firstEditor=rev.get("user"), firstComment=rev.get("comment"),
                             firstPage=short, preHistory=False)
                if i == 0 and h["firstRevid"] == rev["revid"]:
                    h["preHistory"] = True
                h["pages"].add(short)
                if listed[key]:
                    h["urls"].add(listed[key])
                if key not in prev:
                    h["spans"] += 1
                if h["lastListed"] is None or rev["timestamp"] > h["lastListed"]:
                    h["lastListed"] = rev["timestamp"]
            # The revision that dropped a version: when it left this page (the ranked lists
            # only: GigaRumble and MeleeTop30 lag behind them).
            for key in (prev - now) if title in CURRENT_PAGES else ():
                h = hist[key]
                if h["removedAt"] is None or rev["timestamp"] > h["removedAt"]:
                    h["removedAt"] = rev["timestamp"]
            prev = now
        for key in prev:
            hist[key]["stillListed"].add(short)
        if title in CURRENT_PAGES:
            current[short] = sorted(prev)
    return hist, page_info, current


def pick_robot(jar):
    """Choose the (id, version, author, is_team) a jar stands for, or None.

    Prefer the robot or team whose id matches the jar file name; a team jar
    also contains its members' properties.
    """
    robots, teams = jar.get("robots") or [], jar.get("teams") or []
    stem = jar["fileName"][:-4]
    cands = [(t["file"][:-5].replace("/", "."), (t.get("version") or "").strip(), t.get("author"), True)
             for t in teams]
    cands += [(r["classname"], (r.get("version") or "").strip(), r.get("author"), False) for r in robots]
    if not cands:
        return None
    for c in cands:
        if stem in (f"{c[0]}_{c[1]}", f"{c[0]}_{c[1]}".replace(" ", "_")):
            return c
    for c in cands:
        if stem.startswith(c[0] + "_") or stem == c[0]:
            return c
    return cands[0]


def main():
    root = sys.argv[1]
    out_dir = os.path.join(root, "dataset")
    hist, page_info, current = load_wiki(os.path.join(root, "wiki"))
    with open(os.path.join(out_dir, "jars.json")) as f:
        jars = json.load(f)

    # Days on which strangeautomata received many jars at once (bulk uploads).
    lm_days = collections.Counter(j["archiveLastModified"][:10] for j in jars
                                  if j.get("archiveLastModified"))
    bulk_days = {d for d, n in lm_days.items() if n >= 50}
    zip_days = collections.Counter(j["zipEntryMtime"][:10] for j in jars if j.get("zipEntryMtime"))
    zip_bulk_days = {d for d, n in zip_days.items() if n >= 50}

    # Dedupe by (sha256, id, version): one jar file can be shipped under several
    # names (e.g. a robot and its team in the same jar); each name says which
    # robot or team it stands for.
    by_sha, unreadable, no_robot, multi, mismatch = {}, [], [], [], []
    for j in jars:
        if j.get("error"):
            unreadable.append({"file": j["path"], "error": j["error"]})
            continue
        picked = pick_robot(j)
        if picked is None:
            no_robot.append(j["path"])
            continue
        jid, ver, author, is_team = picked
        if len(j["robots"]) > 1 and not j["teams"]:
            multi.append({"file": j["path"], "robots": [r["classname"] for r in j["robots"]]})
        expected = f"{jid}_{ver}.jar"
        name_ok = j["fileName"] == expected or j["fileName"] == expected.replace(" ", "_")
        if not name_ok:
            mismatch.append({"file": j["path"], "properties": f"{jid} {ver}"})
        e = by_sha.setdefault((j["sha256"], jid, ver), {
            "id": jid, "version": ver, "author": author, "sha256": j["sha256"],
            "size": j["size"], "team": is_team, "jarSources": set(), "files": [],
            "buildTime": j.get("classMtimeMax"), "buildTimeMedian": j.get("classMtimeMedian"),
            "entryMtime": j.get("anyMtimeMax"),
            "propertiesSavedAt": None, "archiveLastModified": None,
            "archiveorgZipMtime": None, "flags": set()})
        if not e["buildTime"] and not e["propertiesSavedAt"]:
            for r in j["robots"]:
                if r["classname"] == jid and r.get("propertiesSavedAt"):
                    e["propertiesSavedAt"] = r["propertiesSavedAt"]
        e["jarSources"].add(j["source"])
        e["files"].append(j["path"])
        if not name_ok:
            e["flags"].add("fileNameMismatch")
        if len(j["robots"]) > 1 and not j["teams"]:
            e["flags"].add("multipleRobots")
        if is_team:
            e["flags"].add("team")
        if j.get("bogusClassMtimes"):
            e["flags"].add("bogusClassMtimes")
        if j.get("archiveLastModified"):
            e["archiveLastModified"] = sa_to_utc(j["archiveLastModified"])
            if j["archiveLastModified"][:10] in bulk_days:
                e["flags"].add("archiveBulkUploadDate")
        if j.get("zipEntryMtime"):
            e["archiveorgZipMtime"] = j["zipEntryMtime"]
            if j["zipEntryMtime"][:10] in zip_bulk_days:
                e["flags"].add("archiveBulkUploadDate")

    by_key = collections.defaultdict(list)
    for e in by_sha.values():
        by_key[(e["id"], e["version"])].append(e)
    conflicts = {k: v for k, v in by_key.items() if len(v) > 1}

    versions = []
    for key in sorted(set(by_key) | set(hist)):
        w = hist.get(key)
        wiki = None
        if w:
            wiki = {"firstListed": w["firstListed"],
                    "lastListed": None if w["stillListed"] else w["lastListed"],
                    "removedAt": None if w["stillListed"] else w["removedAt"],
                    "pages": sorted(w["pages"]), "urls": sorted(w["urls"]),
                    "firstEditor": w["firstEditor"], "firstComment": w["firstComment"],
                    "firstRevid": w["firstRevid"], "firstPage": w["firstPage"],
                    "preHistory": w["preHistory"], "listingSpans": w["spans"],
                    "currentlyListedOn": sorted(w["stillListed"])}
        entries = by_key.get(key) or [None]
        for e in entries:
            v = {"id": key[0], "version": key[1], "author": None, "sha256": None, "size": None,
                 "codesize": None, "jarSources": [], "files": [], "wiki": wiki,
                 "buildTime": None, "buildTimeMedian": None, "buildTimeSource": None,
                 "archiveLastModified": None, "archiveorgZipMtime": None,
                 "releaseDate": None, "dateSource": None, "confidence": None, "flags": set()}
            if e:
                v.update(author=e["author"], sha256=e["sha256"], size=e["size"],
                         jarSources=sorted(e["jarSources"]), files=sorted(e["files"]),
                         buildTimeMedian=e["buildTimeMedian"],
                         archiveLastModified=e["archiveLastModified"],
                         archiveorgZipMtime=e["archiveorgZipMtime"])
                v["flags"] |= e["flags"]
                if e["buildTime"]:
                    v["buildTime"], v["buildTimeSource"] = e["buildTime"], "classMtime"
                elif e["entryMtime"]:  # e.g. a team jar holding only a .team file
                    v["buildTime"], v["buildTimeSource"] = e["entryMtime"], "entryMtime"
                elif e["propertiesSavedAt"]:
                    v["buildTime"], v["buildTimeSource"] = e["propertiesSavedAt"], "propertiesComment"
                if len(entries) > 1:
                    v["flags"].add("conflict")
            else:
                v["flags"].add("noJar")
            date_version(v, w)
            v["flags"] = sorted(v["flags"])
            versions.append(v)
    versions.sort(key=lambda v: (v["id"], v["releaseDate"] or "", v["version"]))

    sources = {
        "strangeautomataJars": sum(1 for j in jars if j["source"] == "strangeautomata"),
        "archiveorgJars": sum(1 for j in jars if j["source"] == "archiveorg"),
        "uniqueJarsBySha256": len({k[0] for k in by_sha}),
        "unreadableJars": len(unreadable),
        "jarsWithoutRobotProperties": len(no_robot),
        "wikiPages": page_info,
        "wikiVersions": len(hist),
        "versionEntries": len(versions),
    }
    out = {"generatedAt": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
           "sources": sources, "versions": versions}
    with open(os.path.join(out_dir, "versions.json"), "w") as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
    write_summary(out_dir, out, current, by_key, conflicts, unreadable, no_robot, multi,
                  mismatch, bulk_days, lm_days)
    print(f"{len(versions)} version entries written")


def date_version(v, w):
    flags = v["flags"]
    build = ts(v["buildTime"])
    uppers = [(ts(v["archiveLastModified"]), "strangeautomataLastModified"),
              (ts(v["archiveorgZipMtime"]), "archiveorgZipMtime")]
    uppers = [u for u in uppers if u[0]]
    if w:
        first = ts(w["firstListed"])
        if build and build > first + WIKI_SLOP:
            flags.add("buildAfterFirstListed")
        if build and build < first - LONG_BEFORE and not w["preHistory"]:
            flags.add("buildLongBeforeFirstListed")
        sa = ts(v["archiveLastModified"])
        if sa and "archiveBulkUploadDate" not in flags and sa < first - WIKI_SLOP:
            flags.add("archivedBeforeFirstListed")
        if w["preHistory"]:
            flags.add("preHistory")
            if build and build < first:
                v.update(releaseDate=fmt(build), dateSource="buildTime(preHistory)",
                         confidence="medium")
            else:
                v.update(releaseDate=w["firstListed"], dateSource="wikiFirstListed(preHistory)",
                         confidence="low")
        else:
            v.update(releaseDate=w["firstListed"], dateSource="wikiFirstListed", confidence="high")
        return
    upper = min(uppers, default=None)
    if build and (upper is None or build <= upper[0] + WIKI_SLOP):
        v.update(releaseDate=fmt(build), dateSource="buildTime", confidence="medium")
        if v["buildTimeSource"] != "classMtime":
            v["dateSource"] = v["buildTimeSource"]
    elif upper:
        if build:
            flags.add("buildAfterArchived")
        v.update(releaseDate=fmt(upper[0]), dateSource=upper[1], confidence="low")
    else:
        v.update(releaseDate=None, dateSource=None, confidence="none")


def write_summary(out_dir, out, current, by_key, conflicts, unreadable, no_robot, multi,
                  mismatch, bulk_days, lm_days):
    vs, s = out["versions"], out["sources"]
    L = ["# Robocode robot versions: release-date dataset", "",
         f"Generated {out['generatedAt']} by tools/literumble-history/build_dataset.py.", "",
         "## Sources", "",
         f"- strangeautomata jars: {s['strangeautomataJars']}",
         f"- archive.org jars: {s['archiveorgJars']}",
         f"- unique jars (sha256): {s['uniqueJarsBySha256']}",
         f"- unreadable jars: {s['unreadableJars']}; jars without robot properties: "
         f"{s['jarsWithoutRobotProperties']}"]
    srcsets = collections.Counter(tuple(v["jarSources"]) for v in vs if v["sha256"])
    for k, n in sorted(srcsets.items()):
        L.append(f"- unique jars found in {' + '.join(k)}: {n}")
    L += [f"- distinct (id, version) with a jar: {len(by_key)}",
          f"- (id, version) claimed by different jars (conflicts): {len(conflicts)}",
          f"- jars with several robots (not teams): {len(multi)}",
          f"- team jars: {sum(1 for v in vs if 'team' in v['flags'])}",
          f"- jar file name disagrees with properties: {len(mismatch)}",
          f"- strangeautomata bulk-upload days (>=50 jars same day): "
          + ", ".join(f"{d} ({lm_days[d]})" for d in sorted(bulk_days)), "",
          "Wiki pages:", ""]
    for title, p in s["wikiPages"].items():
        L.append(f"- {title} ({p['page']}): {p['revisions']} revisions, {p['first']} .. {p['last']}")
    L += ["", f"- (id, version) ever listed on the wiki: {s['wikiVersions']}",
          f"- of those without any jar: {sum(1 for v in vs if 'noJar' in v['flags'])}",
          f"- (id, version) with a jar but never listed on these wiki pages: "
          f"{len({(v['id'], v['version']) for v in vs if v['sha256'] and not v['wiki']})}",
          f"- version entries in versions.json: {len(vs)}", "", "## Currently listed", ""]
    idx = collections.defaultdict(list)
    for v in vs:
        idx[(v["id"], v["version"])].append(v)
    for page, keys in current.items():
        with_jar = sum(1 for k in keys if any(v["sha256"] for v in idx[k]))
        with_date = sum(1 for k in keys if any(v["releaseDate"] for v in idx[k]))
        L.append(f"- {page}: {len(keys)} listed, {with_jar} with a jar, {with_date} with a date")
        missing = [f"{k[0]} {k[1]}" for k in keys if not any(v["sha256"] for v in idx[k])]
        if missing:
            L.append(f"  - no jar: {', '.join(missing[:40])}{' ...' if len(missing) > 40 else ''}")
    L += ["", "## Date sources", "", "| dateSource | confidence | entries |", "|---|---|---|"]
    for (ds, c), n in sorted(collections.Counter((v["dateSource"], v["confidence"]) for v in vs).items(),
                             key=lambda x: -x[1]):
        L.append(f"| {ds} | {c} | {n} |")
    L += ["", "## Flags", "", "| flag | entries |", "|---|---|"]
    for fl, n in collections.Counter(f for v in vs for f in v["flags"]).most_common():
        L.append(f"| {fl} | {n} |")
    L += ["", "## Sample (well-known bots)", "",
          "| id | version | releaseDate | source | conf | build | wiki first | flags |",
          "|---|---|---|---|---|---|---|---|"]
    for sid in SAMPLE_IDS:
        rows = [v for v in vs if v["id"] == sid]
        for v in rows[:1] + rows[-1:] if len(rows) > 1 else rows:
            L.append(f"| {v['id']} | {v['version']} | {v['releaseDate']} | {v['dateSource']} | "
                     f"{v['confidence']} | {v['buildTime']} | "
                     f"{(v['wiki'] or {}).get('firstListed')} | {', '.join(v['flags'])} |")
    L += ["", NOTES, "", "## Conflicts (first 30)", ""]
    for k, es in list(conflicts.items())[:30]:
        L.append(f"- {k[0]} {k[1]}: " + "; ".join(
            f"{e['sha256'][:12]} {e['size']}B {sorted(e['jarSources'])} build {e['buildTime']}" for e in es))
    L += ["", "## Unreadable jars", ""] + [f"- {u['file']}: {u['error']}" for u in unreadable[:50]]
    L += ["", "## Jars without robot properties (first 30)", ""] + [f"- {p}" for p in no_robot[:30]]
    L += ["", "## File name vs properties mismatches (first 30)", ""] + [
        f"- {m['file']} -> {m['properties']}" for m in mismatch[:30]]
    with open(os.path.join(out_dir, "summary.md"), "w") as f:
        f.write("\n".join(L) + "\n")


if __name__ == "__main__":
    main()
