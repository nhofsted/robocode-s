# literumble-history

Builds a dataset of Robocode robot versions with a best-guess release date,
from two jar archives and the RoboWiki participant-list history.

All downloads are untrusted data. Nothing here runs code from a jar: jars
are read with `zipfile` only (properties files and entry timestamps). Keep the
downloads in their own directories and run the scripts from this directory
with `python3 -I` (stdlib only).

## Regenerate

```sh
A=/mnt/g/robocodes-archive        # any empty directory with room for ~1 GB
mkdir -p $A/strangeautomata $A/archiveorg $A/wiki $A/dataset

# 1. ~6,500 jars from the strangeautomata archive (4 parallel, resume-safe)
python3 -I fetch_strangeautomata.py $A/strangeautomata

# 2. archive.org snapshot of the rumble robots (483 MB), then a safe extract
curl -L -C - -o $A/archiveorg/all-robots-20261010.zip \
  https://archive.org/download/robocode-rumble-robots-20261010/all-robots-20261010.zip
python3 -I extract_archiveorg.py $A/archiveorg/all-robots-20261010.zip $A/archiveorg/extracted

# 3. Full revision history of the participant pages via Special:Export
#    (~5 minutes; resume-safe, re-run to pick up new revisions)
python3 -I fetch_wiki.py $A/wiki

# 4. Read every jar's robot properties and timestamps
python3 -I scan_jars.py $A $A/dataset/jars.json

# 5. Merge into versions.json and summary.md
python3 -I build_dataset.py $A
```

## Outputs (in `$A/dataset/`)

- `jars.json`: one record per jar file (source, path, sha256, size, robot and
  team properties, class-file mtimes, archive dates, read errors).
- `versions.json`: one entry per (robot id, version, jar sha256), plus entries
  for versions that were listed on the wiki but have no jar. See the schema
  in `build_dataset.py` (`main`) and the summary.
- `summary.md`: counts, coverage of the currently listed bots, date-source
  distribution, a hand-check sample and data-quality notes.

## Release-date rules

1. Listed on a wiki participant page: `releaseDate` = first revision that lists
   the exact `id version` (`wikiFirstListed`, high). Flags
   `buildAfterFirstListed` (jar classes newer than the listing by more than a
   day) and `buildLongBeforeFirstListed` (more than two years older).
2. Already listed in a page's first stored revision (the wiki history starts in
   November 2007; earlier history was lost in the wiki move): the jar's build
   time if it is earlier (`buildTime(preHistory)`, medium), otherwise the
   first revision's date (`wikiFirstListed(preHistory)`, low).
3. Not on the wiki: the latest `.class` entry mtime in the jar (`buildTime`,
   medium; falls back to the latest mtime of any entry, `entryMtime`, e.g. for
   team jars holding only a .team file, then to the date in the properties
   file comment, `propertiesComment`), provided it is not after the archive dates. Otherwise
   the earlier of the strangeautomata Last-Modified and the archive.org zip
   entry mtime (low; these are when the archive got the jar, an upper bound).

strangeautomata Last-Modified values are server time at a fixed UTC-7 and
are converted to UTC. Timestamps outside 2001-01-01 .. 2026-10-26 (including the 1980 DOS default)
are ignored. Jar entry times are local time of the author's machine, with no
time zone. Most strangeautomata Last-Modified values are 2021-01-01, a bulk
upload, and are flagged `archiveBulkUploadDate`.
