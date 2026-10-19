#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""Writes THIRD-PARTY-NOTICES.txt for a release archive.

usage: notices.py OUTPUT [TARGET...]

The Rust part lists every crate the two programs (robocode-s-cli, robocode-s-gui)
link, for the union of the given targets (default: the release targets), with its
version, license expression and repository, and the license files the crates
themselves ship, each distinct text once with the crates it applies to. It reads
`cargo metadata`, so the crates must be downloaded (cargo does that). The rest
(classic Robocode and the Java libraries, the samples, the assets, the Temurin
JRE) is notices-header.txt, with the license texts in licenses/.
"""
import collections
import hashlib
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PROGRAMS = ("robocode-s-cli", "robocode-s-gui")
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)
LICENSE_FILE = re.compile(r"^(licen[sc]e|copying|unlicen[sc]e|notice)([-_. ].*)?$", re.I)


def metadata(target):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked",
         "--filter-platform", target],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def linked(meta):
    """Ids of the packages the programs link (normal dependencies; not dev or build)."""
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    by_name = {p["name"]: p["id"] for p in meta["packages"] if p["id"] in meta["workspace_members"]}
    todo = [by_name[n] for n in PROGRAMS]
    seen = set(todo)
    while todo:
        for dep in nodes[todo.pop()]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    return seen


def licenses(name):
    return open(os.path.join(HERE, "licenses", name), encoding="utf-8").read().strip()


def main():
    out = sys.argv[1]
    targets = sys.argv[2:] or TARGETS
    crates = {}  # (name, version) -> package
    for target in targets:
        meta = metadata(target)
        ids = linked(meta)
        members = set(meta["workspace_members"])
        for p in meta["packages"]:
            if p["id"] in ids and p["id"] not in members:
                crates[(p["name"], p["version"])] = p

    texts = collections.OrderedDict()  # hash -> (text, [crate labels])
    lines = []
    for (name, version), p in sorted(crates.items()):
        label = f"{name} {version}"
        lines.append(
            f"  {label}: {p.get('license') or 'see the license files'}"
            + (f" ({p['repository']})" if p.get("repository") else "")
        )
        folder = os.path.dirname(p["manifest_path"])
        files = [os.path.join(folder, f) for f in sorted(os.listdir(folder)) if LICENSE_FILE.match(f)]
        # License files in a folder of their own (the fonts of epaint_default_fonts).
        for sub in ("licenses", "LICENSES", "fonts"):
            d = os.path.join(folder, sub)
            if os.path.isdir(d):
                files += [os.path.join(d, f) for f in sorted(os.listdir(d))
                          if f.lower().endswith(".txt") and (sub != "fonts" or "icense" in f or f.startswith(("OFL", "UFL", "Hack")))]
        found = False
        for path in files:
            if os.path.isfile(path):
                text = open(path, encoding="utf-8", errors="replace").read().strip()
                if not text:
                    continue
                found = True
                key = hashlib.sha256(re.sub(r"\s+", " ", text).encode()).hexdigest()
                texts.setdefault(key, (text, []))[1].append(label)
        if not found:
            # Nothing shipped: the standard text of the license, to be read with the
            # crate's repository for the copyright line.
            expr = p.get("license") or ""
            name_ = next((n for n in ("MIT", "Apache-2.0", "BSL-1.0") if n in expr), None)
            if name_ is None:
                print(f"warning: {label} ships no license file ({expr})", file=sys.stderr)
            else:
                text = licenses(name_ + ".txt")
                if name_ == "MIT":
                    text = f"MIT License. Copyright (c) the authors of {name} ({p.get('repository')})\n\n" + text
                key = hashlib.sha256((name_ + str(p.get("repository"))).encode()).hexdigest()
                texts.setdefault(key, (text, []))[1].append(label)

    rule = "-" * 78
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(open(os.path.join(HERE, "notices-header.txt"), encoding="utf-8").read())
        f.write(f"{len(crates)} crates, with the license each declares:\n\n")
        f.write("\n".join(lines))
        f.write("\n\n\n6. License texts: Java libraries and classic Robocode\n")
        f.write("-----------------------------------------------------\n\n")
        for title, name in (("Eclipse Public License 1.0", "EPL-1.0.txt"),
                            ("BSD 3-Clause License (protobuf-java, asm, picocontainer: the copyright "
                             "lines above)", "BSD-3-Clause.txt"),
                            ("Apache License 2.0 (kotlin-stdlib, annotations)", "Apache-2.0.txt")):
            f.write(f"{rule}\n{title}\n{rule}\n\n{licenses(name)}\n\n")
        f.write("\n7. License texts: Rust libraries\n")
        f.write("--------------------------------\n\n")
        f.write("The license files the crates ship, each distinct text once, with the crates\n"
                "it comes from.\n\n")
        for text, labels in sorted(texts.values(), key=lambda t: t[1][0]):
            f.write(f"{rule}\nUsed by: {', '.join(labels)}\n{rule}\n\n{text}\n\n")
    print(f"{out}: {len(crates)} crates, {len(texts)} distinct license texts")


if __name__ == "__main__":
    main()
