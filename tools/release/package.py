#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""Packages an assembled release folder (see release.sh's assemble) as

  deb   a Debian package, with dpkg-deb
  rpm   an RPM package, written here (no rpmbuild, and symlinks, which cargo-generate-rpm lacks)
  app   a macOS application bundle, and a .zip of it
  win   the Windows installer (installer.nsi), with NSIS's makensis

Python 3 standard library only. Everything is deterministic: the same folder and --epoch
give the same bytes.

  package.py deb --src DIR --out FILE --arch amd64|arm64 --version V --epoch SECONDS --readme FILE
  package.py rpm --src DIR --out FILE --arch x86_64|aarch64 --version V --epoch SECONDS --readme FILE
  package.py win --src DIR --out FILE --version V
  package.py app --src DIR --app-dir DIR/robocode-s.app --zip FILE --arch aarch64|x86_64 --version V --epoch SECONDS
"""
import argparse
import gzip
import hashlib
import io
import lzma
import os
import plistlib
import re
import shutil
import stat
import struct
import subprocess
import sys
import time
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))

MAINTAINER = "Nick Hofstede <nick.hofstede@gmail.com>"
HOMEPAGE = "https://github.com/nhofsted/robocode-s"
SUMMARY = "Robocode battle engine and viewer"
DESCRIPTION = (
    "robocode-s is a Robocode reimplementation: a Rust battle engine with bit-exact\n"
    "classic compatibility, a battlefield viewer (robocode-s-gui) and a command line\n"
    "(robocode-s), robots out of process in a sandbox. It runs classic Java robots;\n"
    "the sample robots and the robot host are included. The Java runtime for the\n"
    "robots is downloaded on the first battle."
)
LIBDIR = "usr/lib/robocode-s"
DOCDIR = "usr/share/doc/robocode-s"
BUNDLE_ID = "io.github.nhofsted.robocode-s"

# The shared libraries the GUI loads at run time (winit, wgpu, the PulseAudio client is
# pure Rust): without them it has no window or no graphics. The command line needs none.
DEB_RECOMMENDS = [
    "libvulkan1", "mesa-vulkan-drivers", "libegl1", "libxkbcommon0", "libxkbcommon-x11-0",
    "libx11-6", "libx11-xcb1", "libxcb1", "libxcursor1", "libxi6",
    "libwayland-client0", "libwayland-egl1",
]
RPM_RECOMMENDS = [
    "vulkan-loader", "mesa-vulkan-drivers", "mesa-libEGL", "libxkbcommon", "libxkbcommon-x11",
    "libX11", "libX11-xcb", "libxcb", "libXcursor", "libXi",
    "libwayland-client", "libwayland-egl",
]


def read(path):
    with open(path, "rb") as f:
        return f.read()


class Entry:
    def __init__(self, path, kind, mode, data=None, target=None):
        self.path, self.kind, self.mode, self.data, self.target = path, kind, mode, data, target

    @property
    def size(self):
        if self.kind == "f":
            return len(self.data)
        if self.kind == "l":
            return len(self.target)
        return 0


def linux_entries(src, readme, version, epoch, with_changelog):
    """The files of the Linux packages: the archive's tree under /usr/lib/robocode-s (without
    share/ and the documents), links in /usr/bin, the desktop integration and the documents."""
    out = {}

    def add(e):
        out[e.path] = e

    def addfile(path, data, mode=0o644):
        add(Entry(path, "f", mode, data=data))

    skip = {"share", "README.txt", "LICENSE", "THIRD-PARTY-NOTICES.txt"}
    for base, dirs, files in os.walk(src):
        dirs.sort()
        rel = os.path.relpath(base, src)
        if rel == ".":
            dirs[:] = [d for d in dirs if d not in skip]
            files = [f for f in files if f not in skip]
            rel = ""
        target = LIBDIR + ("/" + rel.replace(os.sep, "/") if rel else "")
        add(Entry(target, "d", 0o755))
        for f in sorted(files):
            p = os.path.join(base, f)
            exe = rel == "" and f in ("robocode-s", "robocode-s-gui")
            addfile(target + "/" + f, read(p), 0o755 if exe else 0o644)
    for prog in ("robocode-s", "robocode-s-gui"):
        add(Entry("usr/bin/" + prog, "l", 0o777, target="../lib/robocode-s/" + prog))
    addfile("usr/share/applications/robocode-s.desktop", read(os.path.join(src, "share/robocode-s.desktop")))
    addfile("usr/share/icons/hicolor/256x256/apps/robocode-s.png", read(os.path.join(src, "share/robocode-s.png")))
    addfile("usr/share/mime/packages/robocode-s.xml", read(os.path.join(HERE, "robocode-s-mime.xml")))
    add(Entry(DOCDIR, "d", 0o755))
    addfile(DOCDIR + "/README", read(readme))
    addfile(DOCDIR + "/LICENSE", read(os.path.join(src, "LICENSE")))
    addfile(DOCDIR + "/THIRD-PARTY-NOTICES.txt", read(os.path.join(src, "THIRD-PARTY-NOTICES.txt")))
    if with_changelog:
        date = time.strftime("%a, %d %b %Y %H:%M:%S +0000", time.gmtime(epoch))
        text = (
            f"robocode-s ({version}) unstable; urgency=medium\n\n"
            f"  * Release {version}; the changes are listed at\n"
            f"    {HOMEPAGE}/releases\n\n"
            f" -- {MAINTAINER}  {date}\n"
        )
        addfile(DOCDIR + "/changelog.Debian.gz", gzip_bytes(text.encode(), epoch))
        addfile(DOCDIR + "/copyright", read(os.path.join(HERE, "debian-copyright")))
    return [out[k] for k in sorted(out)]


def gzip_bytes(data, epoch):
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb", mtime=epoch, compresslevel=9, filename="") as g:
        g.write(data)
    return buf.getvalue()


# ---------------------------------------------------------------- deb

def make_deb(a):
    entries = linux_entries(a.src, a.readme, a.version, a.epoch, True)
    stage = a.out + ".stage"
    shutil.rmtree(stage, ignore_errors=True)
    for e in entries:
        p = os.path.join(stage, e.path)
        if e.kind == "d":
            os.makedirs(p, exist_ok=True)
            os.chmod(p, e.mode)
        else:
            os.makedirs(os.path.dirname(p), exist_ok=True)
            if e.kind == "l":
                os.symlink(e.target, p)
            else:
                with open(p, "wb") as f:
                    f.write(e.data)
                os.chmod(p, e.mode)
    for e in entries:  # parents' modes, after their children were made
        if e.kind == "d":
            os.chmod(os.path.join(stage, e.path), e.mode)
    # Parent directories dpkg-deb includes (usr, usr/share, ...): normal modes
    for base, dirs, files in os.walk(stage):
        os.chmod(base, 0o755)
    kib = -(-sum(e.size for e in entries) // 1024)
    os.makedirs(os.path.join(stage, "DEBIAN"))
    desc = "\n".join(" " + l if l else " ." for l in DESCRIPTION.split("\n"))
    control = (
        "Package: robocode-s\n"
        f"Version: {a.version}\n"
        f"Architecture: {a.arch}\n"
        f"Maintainer: {MAINTAINER}\n"
        f"Installed-Size: {kib}\n"
        "Depends: libc6 (>= 2.31)\n"
        f"Recommends: {', '.join(DEB_RECOMMENDS)}\n"
        "Section: games\n"
        "Priority: optional\n"
        f"Homepage: {HOMEPAGE}\n"
        f"Description: {SUMMARY}\n{desc}\n"
    )
    with open(os.path.join(stage, "DEBIAN/control"), "w") as f:
        f.write(control)
    with open(os.path.join(stage, "DEBIAN/md5sums"), "w") as f:
        for e in entries:
            if e.kind == "f":
                f.write(f"{hashlib.md5(e.data).hexdigest()}  {e.path}\n")
    for base in (stage, os.path.join(stage, "DEBIAN")):
        os.chmod(base, 0o755)
    for n in ("control", "md5sums"):
        os.chmod(os.path.join(stage, "DEBIAN", n), 0o644)
    for base, dirs, files in os.walk(stage):
        for n in dirs + files:
            os.utime(os.path.join(base, n), (a.epoch, a.epoch), follow_symlinks=False)
    env = dict(os.environ, SOURCE_DATE_EPOCH=str(a.epoch))
    if os.path.exists(a.out):
        os.remove(a.out)
    subprocess.run(["dpkg-deb", "--root-owner-group", "-Zxz", "-z9", "--build", stage, a.out], check=True, env=env,
                   stdout=subprocess.DEVNULL)
    shutil.rmtree(stage)


# ---------------------------------------------------------------- rpm

T_INT16, T_INT32, T_STRING, T_BIN, T_STRARRAY, T_I18N = 3, 4, 6, 7, 8, 9


class Header:
    """An RPM header: tags with typed values, in the on-disk form with a region tag."""

    def __init__(self, region_tag):
        self.region_tag = region_tag
        self.items = {}

    def add(self, tag, typ, value):
        self.items[tag] = (typ, value)

    def pack(self):
        store = bytearray()
        index = []
        for tag in sorted(self.items):
            typ, value = self.items[tag]
            align = {T_INT16: 2, T_INT32: 4}.get(typ, 1)
            while len(store) % align:
                store.append(0)
            off = len(store)
            if typ == T_INT16:
                count = len(value)
                store += struct.pack(">%dH" % count, *value)
            elif typ == T_INT32:
                count = len(value)
                store += struct.pack(">%dI" % count, *value)
            elif typ in (T_STRING, T_I18N):
                count = 1
                store += value.encode() + b"\0"
            elif typ == T_BIN:
                count = len(value)
                store += value
            elif typ == T_STRARRAY:
                count = len(value)
                for s in value:
                    store += s.encode() + b"\0"
            else:
                raise ValueError(typ)
            index.append((tag, typ, off, count))
        # The region tag: its data is a trailing index entry that points back at the index.
        trailer_off = len(store)
        store += struct.pack(">IIiI", self.region_tag, T_BIN, -(len(index) + 1) * 16, 16)
        index.insert(0, (self.region_tag, T_BIN, trailer_off, 16))
        out = bytearray(b"\x8e\xad\xe8\x01\0\0\0\0")
        out += struct.pack(">II", len(index), len(store))
        for tag, typ, off, count in index:
            out += struct.pack(">IIiI", tag, typ, off, count)
        out += store
        return bytes(out)


def cpio_newc(entries_with_ino):
    out = bytearray()

    def pad(n):
        while len(out) % 4:
            out.append(0)

    for ino, name, mode, mtime, data in entries_with_ino:
        nameb = name.encode() + b"\0"
        out += b"070701"
        for v in (ino, mode, 0, 0, 1, mtime, len(data), 0, 0, 0, 0, len(nameb), 0):
            out += b"%08X" % v
        out += nameb
        pad(0)
        out += data
        pad(0)
    nameb = b"TRAILER!!!\0"
    out += b"070701" + b"".join(b"%08X" % v for v in (0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, len(nameb), 0))
    out += nameb
    while len(out) % 4:
        out.append(0)
    return bytes(out)


def make_rpm(a):
    entries = linux_entries(a.src, a.readme, a.version, a.epoch, False)
    dirindex_of = {}
    dirnames = []
    basenames = []
    dirindexes = []
    for e in entries:
        full = "/" + e.path
        d, b = os.path.split(full)
        d += "/"
        if d not in dirindex_of:
            dirindex_of[d] = len(dirnames)
            dirnames.append(d)
        dirindexes.append(dirindex_of[d])
        basenames.append(b)
    modes, sizes, flags, digests, links, mtimes = [], [], [], [], [], []
    payload_entries = []
    for i, e in enumerate(entries):
        if e.kind == "d":
            mode = stat.S_IFDIR | e.mode
            data = b""
            digest = ""
            link = ""
        elif e.kind == "l":
            mode = stat.S_IFLNK | e.mode
            data = e.target.encode()
            digest = ""
            link = e.target
        else:
            mode = stat.S_IFREG | e.mode
            data = e.data
            digest = hashlib.sha256(data).hexdigest()
            link = ""
        fl = 0
        if e.path.startswith(DOCDIR + "/"):
            fl = 2 if not e.path.endswith(("LICENSE", "THIRD-PARTY-NOTICES.txt")) else 2 | 128
        modes.append(mode)
        sizes.append(len(data))
        flags.append(fl)
        digests.append(digest)
        links.append(link)
        mtimes.append(a.epoch)
        payload_entries.append((i + 1, "./" + e.path, mode, a.epoch, data))
    cpio = cpio_newc(payload_entries)
    payload = lzma.compress(cpio, format=lzma.FORMAT_XZ, check=lzma.CHECK_CRC32, preset=6)

    n = len(entries)
    h = Header(63)  # RPMTAG_HEADERIMMUTABLE
    h.add(100, T_STRARRAY, ["C"])
    h.add(1000, T_STRING, "robocode-s")
    h.add(1001, T_STRING, a.version)
    h.add(1002, T_STRING, "1")
    h.add(1004, T_I18N, SUMMARY)
    h.add(1005, T_I18N, DESCRIPTION)
    h.add(1006, T_INT32, [a.epoch])
    h.add(1007, T_STRING, "localhost")
    h.add(1009, T_INT32, [sum(sizes)])
    h.add(1014, T_STRING, "EPL-2.0")
    h.add(1015, T_STRING, MAINTAINER)
    h.add(1016, T_I18N, "Amusements/Games")
    h.add(1020, T_STRING, HOMEPAGE)
    h.add(1021, T_STRING, "linux")
    h.add(1022, T_STRING, a.arch)
    h.add(1028, T_INT32, sizes)
    h.add(1030, T_INT16, [m & 0xFFFF for m in modes])
    h.add(1033, T_INT16, [0] * n)
    h.add(1034, T_INT32, mtimes)
    h.add(1035, T_STRARRAY, digests)
    h.add(1036, T_STRARRAY, links)
    h.add(1037, T_INT32, flags)
    h.add(1039, T_STRARRAY, ["root"] * n)
    h.add(1040, T_STRARRAY, ["root"] * n)
    h.add(1064, T_STRING, "4.18.0")
    h.add(1095, T_INT32, [1] * n)
    h.add(1096, T_INT32, list(range(1, n + 1)))
    h.add(1097, T_STRARRAY, [""] * n)
    h.add(1116, T_INT32, dirindexes)
    h.add(1117, T_STRARRAY, basenames)
    h.add(1118, T_STRARRAY, dirnames)
    h.add(1124, T_STRING, "cpio")
    h.add(1125, T_STRING, "xz")
    h.add(1126, T_STRING, "6")
    h.add(5011, T_INT32, [8])  # file digests are SHA-256
    # Requires: the rpm features used, and the C library.
    LESS, GREATER, EQUAL, RPMLIB = 2, 4, 8, 1 << 24
    req = [
        ("glibc", GREATER | EQUAL, "2.31"),
        ("rpmlib(CompressedFileNames)", RPMLIB | LESS | EQUAL, "3.0.4-1"),
        ("rpmlib(FileDigests)", RPMLIB | LESS | EQUAL, "4.6.0-1"),
        ("rpmlib(PayloadFilesHavePrefix)", RPMLIB | LESS | EQUAL, "4.0-1"),
        ("rpmlib(PayloadIsXz)", RPMLIB | LESS | EQUAL, "5.2-1"),
    ]
    h.add(1049, T_STRARRAY, [r[0] for r in req])
    h.add(1048, T_INT32, [r[1] for r in req])
    h.add(1050, T_STRARRAY, [r[2] for r in req])
    h.add(5046, T_STRARRAY, RPM_RECOMMENDS)  # RPMTAG_RECOMMENDNAME
    h.add(5048, T_INT32, [0] * len(RPM_RECOMMENDS))  # RPMTAG_RECOMMENDFLAGS
    h.add(5047, T_STRARRAY, [""] * len(RPM_RECOMMENDS))  # RPMTAG_RECOMMENDVERSION
    header = h.pack()

    sig = Header(62)  # RPMTAG_HEADERSIGNATURES
    sig.add(269, T_STRING, hashlib.sha1(header).hexdigest())
    sig.add(273, T_STRING, hashlib.sha256(header).hexdigest())
    sig.add(1000, T_INT32, [len(header) + len(payload)])
    sig.add(1004, T_BIN, hashlib.md5(header + payload).digest())
    sig.add(1007, T_INT32, [len(cpio)])
    sigb = sig.pack()
    sigb += b"\0" * (-len(sigb) % 8)

    name = f"robocode-s-{a.version}-1".encode()
    lead = struct.pack(">4sBBhh66shh16s", b"\xed\xab\xee\xdb", 3, 0, 0, 1 if a.arch == "x86_64" else 12,
                       name, 1, 5, b"\0" * 16)
    with open(a.out, "wb") as f:
        f.write(lead + sigb + header + payload)


# ---------------------------------------------------------------- macOS app

def icns(png):
    """An .icns holding the 256-pixel PNG as 'ic08' (256x256) and 'ic13' (128x128 at 2x)."""
    chunks = b"".join(t + struct.pack(">I", 8 + len(png)) + png for t in (b"ic08", b"ic13"))
    return b"icns" + struct.pack(">I", 8 + len(chunks)) + chunks


def make_app(a):
    app = a.app_dir
    shutil.rmtree(app, ignore_errors=True)
    macos, res = os.path.join(app, "Contents/MacOS"), os.path.join(app, "Contents/Resources")
    os.makedirs(macos)
    os.makedirs(res)
    for prog in ("robocode-s-gui", "robocode-s"):
        shutil.copy(os.path.join(a.src, prog), os.path.join(macos, prog))
        os.chmod(os.path.join(macos, prog), 0o755)
    for item in ("lib", "robots"):
        shutil.copytree(os.path.join(a.src, item), os.path.join(res, item))
    for f in ("LICENSE", "THIRD-PARTY-NOTICES.txt", "README.txt"):
        shutil.copy(os.path.join(a.src, f), os.path.join(res, f))
    icon = read(os.path.join(ROOT, "web/static/favicon-256.png"))
    with open(os.path.join(res, "robocode-s.icns"), "wb") as f:
        f.write(icns(icon))
    doc = lambda name, exts: {
        "CFBundleTypeName": name,
        "CFBundleTypeExtensions": exts,
        "CFBundleTypeRole": "Viewer",
        "CFBundleTypeIconFile": "robocode-s",
        "LSHandlerRank": "Default",
    }
    plist = {
        "CFBundleName": "robocode-s",
        "CFBundleDisplayName": "robocode-s",
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleExecutable": "robocode-s-gui",
        "CFBundleIconFile": "robocode-s",
        "CFBundlePackageType": "APPL",
        "CFBundleSignature": "????",
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleShortVersionString": a.version,
        "CFBundleVersion": a.version,
        "LSMinimumSystemVersion": "11.0" if a.arch == "aarch64" else "10.13",
        "LSApplicationCategoryType": "public.app-category.games",
        "NSHighResolutionCapable": True,
        "NSHumanReadableCopyright": "EPL-2.0. Copyright 2001-2026 Mathew A. Nelson, Nick Hofstede and contributors.",
        "CFBundleDocumentTypes": [
            doc("robocode-s battle recording", ["rsr", "rec"]),
            doc("robocode-s website replay track", ["track"]),
        ],
    }
    with open(os.path.join(app, "Contents/Info.plist"), "wb") as f:
        plistlib.dump(plist, f, sort_keys=True)
    with open(os.path.join(app, "Contents/PkgInfo"), "wb") as f:
        f.write(b"APPL????")
    # The zip: fixed order and times, modes kept (the programs stay executable).
    date = time.gmtime(max(a.epoch, 315532800))[:6]
    parent = os.path.dirname(app)
    top = os.path.basename(app)
    with zipfile.ZipFile(a.zip, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        top_dir = zipfile.ZipInfo(top + "/", date)
        top_dir.external_attr = (stat.S_IFDIR | 0o755) << 16 | 0x10
        z.writestr(top_dir, b"")
        for base, dirs, files in os.walk(app):
            dirs.sort()
            for d in dirs:
                p = os.path.join(base, d)
                zi = zipfile.ZipInfo(os.path.relpath(p, parent).replace(os.sep, "/") + "/", date)
                zi.external_attr = (stat.S_IFDIR | 0o755) << 16 | 0x10
                z.writestr(zi, b"")
            for fn in sorted(files):
                p = os.path.join(base, fn)
                zi = zipfile.ZipInfo(os.path.relpath(p, parent).replace(os.sep, "/"), date)
                zi.compress_type = zipfile.ZIP_DEFLATED
                zi.external_attr = (stat.S_IFREG | (0o755 if os.access(p, os.X_OK) else 0o644)) << 16
                z.writestr(zi, read(p))


# ---------------------------------------------------------------- Windows installer

def find_makensis():
    import glob

    found = os.environ.get("MAKENSIS") or shutil.which("makensis")
    if not found:
        for pattern in ("~/.local/opt/nsis-*/makensis.exe", "~/.local/opt/nsis-*/makensis"):
            hits = sorted(glob.glob(os.path.expanduser(pattern)))
            if hits:
                found = hits[-1]
                break
    return found


def make_installer(a):
    nsis = find_makensis()
    if not nsis:
        sys.exit(
            "package.py: makensis not found (NSIS 3: apt install nsis; or unpack the Windows zip of "
            "nsis.sourceforge.io into ~/.local/opt/nsis-3.11, which is run through WSL interop; "
            "or set MAKENSIS)"
        )
    windows_tool = nsis.lower().endswith(".exe")

    def native(path):
        path = os.path.abspath(path)
        if not windows_tool:
            return path
        return subprocess.run(["wslpath", "-w", path], check=True, capture_output=True, text=True).stdout.strip()

    work = a.out + ".work"
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(work)
    root = native(a.src)
    bs = "\\"
    install, uninstall, dirs = [], [], []
    for base, ds, files in os.walk(a.src):
        ds.sort()
        rel = os.path.relpath(base, a.src)
        rel = "" if rel == "." else rel.replace("/", bs)
        out_dir = "$INSTDIR" + (bs + rel if rel else "")
        if rel:
            dirs.append(out_dir)
        if not files:
            continue
        install.append(f'SetOutPath "{out_dir}"')
        for f in sorted(files):
            src = root + (bs + rel if rel else "") + bs + f
            install.append(f'File "{src}"')
            uninstall.append(f'Delete "{out_dir}{bs}{f}"')
    # Deepest folders first; RMDir removes only empty folders, so a robot the user added stays.
    for d in sorted(dirs, key=lambda d: -d.count(bs)):
        uninstall.append(f'RMDir "{d}"')
    ilist, ulist = os.path.join(work, "install.nsh"), os.path.join(work, "uninstall.nsh")
    for path, lines in ((ilist, install), (ulist, uninstall)):
        with open(path, "w", encoding="utf-8-sig") as f:  # a BOM tells NSIS the file is Unicode
            f.write("\n".join(lines) + "\n")
    numbers = [int(x) for x in re.findall(r"\d+", a.version.split("-")[0])][:4]
    viversion = ".".join(str(n) for n in (numbers + [0, 0, 0, 0])[:4])
    out_tmp = os.path.join(work, "robocode-s-installer.exe")
    cmd = [
        nsis, "/V2", "/INPUTCHARSET", "UTF8",
        f"/DVERSION={a.version}", f"/DVIVERSION={viversion}", f"/DSRC={root}",
        f"/DOUT={native(out_tmp)}", f"/DINSTALL_LIST={native(ilist)}", f"/DUNINSTALL_LIST={native(ulist)}",
        f"/DICON={native(os.path.join(ROOT, 'crates/gui/assets/robocode-s.ico'))}",
        native(os.path.join(HERE, "installer.nsi")),
    ]
    subprocess.run(cmd, check=True, stdout=sys.stderr)
    shutil.move(out_tmp, a.out)
    shutil.rmtree(work)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("kind", choices=["deb", "rpm", "app", "win"])
    p.add_argument("--src", required=True)
    p.add_argument("--arch")
    p.add_argument("--version", required=True)
    p.add_argument("--epoch", type=int)
    p.add_argument("--out")
    p.add_argument("--readme")
    p.add_argument("--app-dir")
    p.add_argument("--zip")
    a = p.parse_args()
    {"deb": make_deb, "rpm": make_rpm, "app": make_app, "win": make_installer}[a.kind](a)


if __name__ == "__main__":
    sys.exit(main())
