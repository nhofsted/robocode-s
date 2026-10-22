#!/usr/bin/env python3
# SPDX-License-Identifier: EPL-2.0
"""The apt and dnf repositories of robocode-s (design §6.9, issue #79).

  repo.py keygen                       prints the gpg commands that make the signing key (runs nothing)
  repo.py pull   [--for-real]          copies the published repository down (aws s3 sync)
  repo.py build  VERSION               adds target/release-dist/VERSION's .deb and .rpm to the local
                                       repository (target/package-repo), keeping the earlier versions,
                                       regenerates and signs the metadata
  repo.py upload [--for-real]          uploads the local repository to S3 and invalidates CloudFront;
                                       only prints the commands without --for-real

The repository tree is what lives under https://rumble.planethofstede.be/packages/ :

  robocode-s.asc  robocode-s.gpg       the public key (armored; binary, for apt's signed-by)
  robocode-s.repo                      for /etc/yum.repos.d/
  robocode-s.sources                   for /etc/apt/sources.list.d/ (deb822)
  apt/dists/stable/{InRelease,Release,Release.gpg}
  apt/dists/stable/main/binary-{amd64,arm64}/Packages(.gz)
  apt/pool/main/r/robocode-s/robocode-s_<version>_<arch>.deb
  rpm/{x86_64,aarch64}/robocode-s-<version>-1.<arch>.rpm
  rpm/{x86_64,aarch64}/repodata/{repomd.xml,repomd.xml.asc,*-primary.xml.gz,*-filelists.xml.gz,*-other.xml.gz}

The metadata is generated here (no dpkg-scanpackages or createrepo needed; dpkg-deb reads the
.deb control files). The .rpm files are signed here as well: package.py writes unsigned ones,
and dnf's gpgcheck wants a header signature, which is added (RPMSIGTAG_RSAHEADER) to the copy in
the repository. Use an RSA key: rpm before 4.19 (RHEL 9, Rocky 9) can't verify ed25519.

Python 3 standard library; gpg, dpkg-deb and (for upload) the AWS CLI are run as programs.
Signing uses ROBOCODES_GPG_KEY (a key id or fingerprint of the signing key in the gpg keyring
of GNUPGHOME or ~/.gnupg).
"""
import argparse
import gzip
import hashlib
import os
import shlex
import shutil
import struct
import subprocess
import sys
import time
from xml.sax.saxutils import escape, quoteattr

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import package as pk  # noqa: E402  (the RPM header writer and its tag constants)

ROOT = os.path.dirname(os.path.dirname(HERE))
DEFAULT_REPO = os.path.join(ROOT, "target", "package-repo")
DEFAULT_DIST = os.path.join(ROOT, "target", "release-dist")
BASE_URL = "https://rumble.planethofstede.be/packages"
STACK = "RobocodeS-Site"
REGION = "eu-west-1"
PROFILE = "robocode-s"  # ADR 0016: the member account, through Identity Center
DEB_ARCH = {"x86_64": "amd64", "aarch64": "arm64"}
ARCHES = ["x86_64", "aarch64"]
POOL = "apt/pool/main/r/robocode-s"
SUITE = "stable"


def die(msg):
    sys.exit("repo.py: " + msg)


def say(msg):
    print("== " + msg, file=sys.stderr)


def sha(algo, data):
    return hashlib.new(algo, data).hexdigest()


def read_bytes(path):
    with open(path, "rb") as f:
        return f.read()


def write_bytes(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def gz(data):
    return gzip.compress(data, 9, mtime=0)


# ------------------------------------------------------------------ gpg

def gpg_key():
    key = os.environ.get("ROBOCODES_GPG_KEY")
    if not key:
        die("ROBOCODES_GPG_KEY (the signing key's id or fingerprint) is not set; `repo.py keygen` says how to make one")
    return key


def gpg_env():
    """The environment for gpg: the data goes to its stdin, so gpg-agent's pinentry asks
    for the key's passphrase on the terminal GPG_TTY names; set it from ours if unset."""
    env = dict(os.environ)
    if "GPG_TTY" not in env:
        for stream in (sys.stderr, sys.stdout, sys.stdin):
            if stream.isatty():
                env["GPG_TTY"] = os.ttyname(stream.fileno())
                break
    return env


def gpg(args, data=None):
    cmd = ["gpg", "--batch", "--yes", "--local-user", gpg_key(), "--digest-algo", "SHA256"] + args
    r = subprocess.run(cmd, input=data, stdout=subprocess.PIPE, check=False, env=gpg_env())
    if r.returncode:
        die("gpg failed: " + " ".join(cmd))
    return r.stdout


def sign_detached(data, armor):
    return gpg((["--armor"] if armor else []) + ["--detach-sign", "--output", "-"], data)


def sign_clear(data):
    return gpg(["--clearsign", "--output", "-"], data)


def export_key(armor):
    cmd = ["gpg", "--batch"] + (["--armor"] if armor else []) + ["--export", gpg_key()]
    out = subprocess.run(cmd, stdout=subprocess.PIPE, check=True).stdout
    if not out:
        die("gpg exports nothing for " + gpg_key())
    return out


# ------------------------------------------------------------------ apt

def deb_field(path, field):
    return subprocess.run(["dpkg-deb", "-f", path, field], stdout=subprocess.PIPE, check=True,
                          text=True).stdout.strip()


def add_debs(src, repo, version):
    """Copies the .deb files of VERSION into the pool, with Debian's file names."""
    n = 0
    for arch in ARCHES:
        f = os.path.join(src, f"robocode-s-{version}-linux-{arch}.deb")
        if not os.path.exists(f):
            say(f"no {os.path.basename(f)}: skipped")
            continue
        got = deb_field(f, "Architecture")
        if got != DEB_ARCH[arch]:
            die(f"{f} has Architecture {got}, expected {DEB_ARCH[arch]}")
        ver = deb_field(f, "Version")
        if ver != version:
            die(f"{f} has Version {ver}, expected {version}")
        dest = os.path.join(repo, POOL, f"robocode-s_{ver}_{got}.deb")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copyfile(f, dest)
        n += 1
    return n


def version_key(v):
    import re
    return [int(p) if p.isdigit() else p for p in re.split(r"(\d+)", v) if p]


def packages_index(repo, deb_arch):
    stanzas = []
    pooldir = os.path.join(repo, POOL)
    names = sorted(os.listdir(pooldir)) if os.path.isdir(pooldir) else []
    for name in sorted((n for n in names if n.endswith(f"_{deb_arch}.deb")),
                       key=lambda n: version_key(n.split("_")[1])):
        path = os.path.join(pooldir, name)
        control = subprocess.run(["dpkg-deb", "-f", path], stdout=subprocess.PIPE, check=True,
                                 text=True).stdout.rstrip("\n")
        data = read_bytes(path)
        stanzas.append(
            control + "\n"
            f"Filename: pool/main/r/robocode-s/{name}\n"
            f"Size: {len(data)}\n"
            f"MD5sum: {sha('md5', data)}\n"
            f"SHA1: {sha('sha1', data)}\n"
            f"SHA256: {sha('sha256', data)}\n")
    return "\n".join(stanzas).encode()


def rfc2822_now():
    t = int(os.environ.get("SOURCE_DATE_EPOCH", time.time()))
    return time.strftime("%a, %d %b %Y %H:%M:%S UTC", time.gmtime(t))


def build_apt(repo):
    dist = os.path.join(repo, "apt", "dists", SUITE)
    files = []  # (path relative to dist)
    for arch in DEB_ARCH.values():
        idx = packages_index(repo, arch)
        rel = f"main/binary-{arch}"
        write_bytes(os.path.join(dist, rel, "Packages"), idx)
        write_bytes(os.path.join(dist, rel, "Packages.gz"), gz(idx))
        files += [f"{rel}/Packages", f"{rel}/Packages.gz"]
    lines = [
        "Origin: robocode-s", "Label: robocode-s", f"Suite: {SUITE}", f"Codename: {SUITE}",
        f"Date: {rfc2822_now()}", f"Architectures: {' '.join(sorted(DEB_ARCH.values()))}",
        "Components: main", "Description: robocode-s packages",
    ]
    for algo, field in (("md5", "MD5Sum"), ("sha1", "SHA1"), ("sha256", "SHA256")):
        lines.append(field + ":")
        for rel in files:
            data = read_bytes(os.path.join(dist, rel))
            lines.append(f" {sha(algo, data)} {len(data):>16} {rel}")
    release = ("\n".join(lines) + "\n").encode()
    write_bytes(os.path.join(dist, "Release"), release)
    write_bytes(os.path.join(dist, "InRelease"), sign_clear(release))
    write_bytes(os.path.join(dist, "Release.gpg"), sign_detached(release, True))


# ------------------------------------------------------------------ rpm

LEAD = 96
T_INT16, T_INT32, T_STRING, T_BIN, T_STRARRAY, T_I18N = pk.T_INT16, pk.T_INT32, pk.T_STRING, pk.T_BIN, pk.T_STRARRAY, pk.T_I18N


def parse_header(buf, off):
    """Returns ({tag: (type, value)}, offset of the end of the header's data)."""
    if buf[off:off + 3] != b"\x8e\xad\xe8":
        die("not an RPM header")
    n, dlen = struct.unpack(">II", buf[off + 8:off + 16])
    store = off + 16 + 16 * n
    data = buf[store:store + dlen]
    items = {}
    for i in range(n):
        tag, typ, o, count = struct.unpack(">IIiI", buf[off + 16 + 16 * i:off + 32 + 16 * i])
        if typ == T_INT16:
            val = list(struct.unpack(">%dH" % count, data[o:o + 2 * count]))
        elif typ == T_INT32:
            val = list(struct.unpack(">%dI" % count, data[o:o + 4 * count]))
        elif typ in (T_STRING, T_I18N):
            val = data[o:data.index(b"\0", o)].decode()
        elif typ == T_BIN:
            val = data[o:o + count]
        elif typ == T_STRARRAY:
            val = []
            p = o
            for _ in range(count):
                e = data.index(b"\0", p)
                val.append(data[p:e].decode())
                p = e + 1
        else:
            die(f"RPM tag {tag}: unsupported type {typ}")
        items[tag] = (typ, val)
    return items, store + dlen


class Rpm:
    def __init__(self, blob):
        self.blob = blob
        self.sig, send = parse_header(blob, LEAD)
        self.hstart = send + (-send % 8)
        self.hdr, self.hend = parse_header(blob, self.hstart)

    def get(self, tag, default=None):
        return self.hdr[tag][1] if tag in self.hdr else default


def sign_rpm(blob):
    """Returns blob with a header signature (RSAHEADER, tag 268) in its signature header."""
    r = Rpm(blob)
    header = blob[r.hstart:r.hend]
    payload = blob[r.hend:]
    sig = pk.Header(62)
    for tag, (typ, val) in r.sig.items():
        if tag not in (62, 268, 1002, 1005):
            sig.add(tag, typ, val)
    sig.add(268, T_BIN, sign_detached(header, False))  # RPMSIGTAG_RSA: the header
    sig.add(1002, T_BIN, sign_detached(header + payload, False))  # RPMSIGTAG_PGP: header and payload
    sigb = sig.pack()
    sigb += b"\0" * (-len(sigb) % 8)
    return blob[:LEAD] + sigb + header + payload


FLAGS = {0: None, 2: "LT", 4: "GT", 8: "EQ", 10: "LE", 12: "GE"}


def entry(name, flags=0, ver=""):
    f = FLAGS.get(flags & 0xF)
    s = f"<rpm:entry name={quoteattr(name)}"
    if f:
        epoch = "0"
        v, _, rel = ver.partition("-")
        s += f" flags=\"{f}\" epoch=\"{epoch}\" ver={quoteattr(v)}"
        if rel:
            s += f" rel={quoteattr(rel)}"
    return s + "/>"


def rpm_metadata(path, href):
    blob = read_bytes(path)
    r = Rpm(blob)
    name, ver, rel, arch = r.get(1000), r.get(1001), r.get(1002), r.get(1022)
    pkgid = sha("sha256", blob)
    dirs, bases, idx = r.get(1118, []), r.get(1117, []), r.get(1116, [])
    modes = r.get(1030, [])
    files = []
    for d, b, m in zip(idx, bases, modes):
        files.append((dirs[d] + b, (m & 0o170000) == 0o040000))
    evr = f'epoch="0" ver="{ver}" rel="{rel}"'
    req = []
    for n, f, v in zip(r.get(1049, []), r.get(1048, []), r.get(1050, [])):
        if not n.startswith("rpmlib("):
            req.append(entry(n, f, v))
    rec = [entry(n) for n in r.get(5046, [])]
    prov = [f'<rpm:entry name="{name}" flags="EQ" {evr}/>',
            f'<rpm:entry name="{name}({arch})" flags="EQ" {evr}/>']
    size_arch = r.sig.get(1007, (0, [0]))[1][0]
    primary_files = "".join(f"<file>{escape(p)}</file>" for p, isdir in files
                            if not isdir and (p.startswith("/usr/bin/") or p.startswith("/etc/")))
    primary = (
        f'<package type="rpm"><name>{name}</name><arch>{arch}</arch><version {evr}/>'
        f'<checksum type="sha256" pkgid="YES">{pkgid}</checksum>'
        f"<summary>{escape(r.get(1004, ''))}</summary><description>{escape(r.get(1005, ''))}</description>"
        f"<packager>{escape(r.get(1015, ''))}</packager><url>{escape(r.get(1020, ''))}</url>"
        f'<time file="{int(os.path.getmtime(path))}" build="{r.get(1006, [0])[0]}"/>'
        f'<size package="{len(blob)}" installed="{r.get(1009, [0])[0]}" archive="{size_arch}"/>'
        f'<location href="{href}"/>'
        f"<format><rpm:license>{escape(r.get(1014, ''))}</rpm:license><rpm:vendor>{escape(r.get(1007, ''))}</rpm:vendor>"
        f"<rpm:group>{escape(r.get(1016, ''))}</rpm:group><rpm:buildhost>{escape(r.get(1007, ''))}</rpm:buildhost>"
        f"<rpm:sourcerpm></rpm:sourcerpm>"
        f'<rpm:header-range start="{r.hstart}" end="{r.hend}"/>'
        f"<rpm:provides>{''.join(prov)}</rpm:provides>"
        f"<rpm:requires>{''.join(req)}</rpm:requires>"
        f"<rpm:recommends>{''.join(rec)}</rpm:recommends>"
        f"{primary_files}</format></package>")
    filelists = (
        f'<package pkgid="{pkgid}" name="{name}" arch="{arch}"><version {evr}/>'
        + "".join(f'<file type="dir">{escape(p)}</file>' if isdir else f"<file>{escape(p)}</file>"
                  for p, isdir in files) + "</package>")
    other = f'<package pkgid="{pkgid}" name="{name}" arch="{arch}"><version {evr}/></package>'
    return primary, filelists, other


def build_rpm_arch(repo, arch):
    d = os.path.join(repo, "rpm", arch)
    if not os.path.isdir(d):
        return
    rpms = sorted((f for f in os.listdir(d) if f.endswith(".rpm")), key=lambda n: version_key(n))
    if not rpms:
        return
    docs = {"primary": [], "filelists": [], "other": []}
    for f in rpms:
        for kind, x in zip(docs, rpm_metadata(os.path.join(d, f), f)):
            docs[kind].append(x)
    n = len(rpms)
    head = '<?xml version="1.0" encoding="UTF-8"?>\n'
    ns = {
        "primary": '<metadata xmlns="http://linux.duke.edu/metadata/common" xmlns:rpm="http://linux.duke.edu/metadata/rpm"',
        "filelists": '<filelists xmlns="http://linux.duke.edu/metadata/filelists"',
        "other": '<otherdata xmlns="http://linux.duke.edu/metadata/other"',
    }
    close = {"primary": "</metadata>", "filelists": "</filelists>", "other": "</otherdata>"}
    rd = os.path.join(d, "repodata")
    if os.path.isdir(rd):
        shutil.rmtree(rd)
    stamp = int(os.environ.get("SOURCE_DATE_EPOCH", time.time()))
    data_xml = ""
    for kind in ("primary", "filelists", "other"):
        xml = (head + ns[kind] + f' packages="{n}">\n' + "\n".join(docs[kind]) + "\n" + close[kind] + "\n").encode()
        z = gz(xml)
        zsha = sha("sha256", z)
        write_bytes(os.path.join(rd, f"{zsha}-{kind}.xml.gz"), z)
        data_xml += (
            f'<data type="{kind}"><checksum type="sha256">{zsha}</checksum>'
            f'<open-checksum type="sha256">{sha("sha256", xml)}</open-checksum>'
            f'<location href="repodata/{zsha}-{kind}.xml.gz"/><timestamp>{stamp}</timestamp>'
            f'<size>{len(z)}</size><open-size>{len(xml)}</open-size></data>\n')
    repomd = (head + '<repomd xmlns="http://linux.duke.edu/metadata/repo" xmlns:rpm="http://linux.duke.edu/metadata/rpm">\n'
              f"<revision>{stamp}</revision>\n{data_xml}</repomd>\n").encode()
    write_bytes(os.path.join(rd, "repomd.xml"), repomd)
    write_bytes(os.path.join(rd, "repomd.xml.asc"), sign_detached(repomd, True))


def add_rpms(src, repo, version):
    n = 0
    for arch in ARCHES:
        f = os.path.join(src, f"robocode-s-{version}-linux-{arch}.rpm")
        if not os.path.exists(f):
            say(f"no {os.path.basename(f)}: skipped")
            continue
        blob = read_bytes(f)
        r = Rpm(blob)
        if r.get(1001) != version or r.get(1022) != arch:
            die(f"{f}: version/arch inside the package are {r.get(1001)}/{r.get(1022)}")
        write_bytes(os.path.join(repo, "rpm", arch, f"robocode-s-{version}-{r.get(1002)}.{arch}.rpm"), sign_rpm(blob))
        n += 1
    return n


# ------------------------------------------------------------------ commands

def base_url(a):
    return (a.base_url or os.environ.get("ROBOCODES_REPO_URL") or BASE_URL).rstrip("/")


def cmd_build(a):
    src = os.path.join(a.dist_dir, a.version)
    if not os.path.isdir(src):
        die(f"{src} does not exist: run release.sh build --version {a.version} first")
    repo = a.repo_dir
    gpg_key()
    os.makedirs(repo, exist_ok=True)
    if a.pull:
        cmd_pull(argparse.Namespace(repo_dir=repo, for_real=True, bucket=a.bucket))
    nd, nr = add_debs(src, repo, a.version), add_rpms(src, repo, a.version)
    if not nd and not nr:
        die(f"no .deb or .rpm of {a.version} in {src}")
    build_apt(repo)
    for arch in ARCHES:
        build_rpm_arch(repo, arch)
    write_bytes(os.path.join(repo, "robocode-s.asc"), export_key(True))
    write_bytes(os.path.join(repo, "robocode-s.gpg"), export_key(False))
    url = base_url(a)
    write_bytes(os.path.join(repo, "robocode-s.repo"), (
        "[robocode-s]\nname=robocode-s\n"
        f"baseurl={url}/rpm/$basearch\nenabled=1\ngpgcheck=1\nrepo_gpgcheck=1\n"
        f"gpgkey={url}/robocode-s.asc\n").encode())
    write_bytes(os.path.join(repo, "robocode-s.sources"), (
        f"Types: deb\nURIs: {url}/apt\nSuites: {SUITE}\nComponents: main\n"
        "Signed-By: /etc/apt/keyrings/robocode-s.gpg\n").encode())
    say(f"repository in {repo}: {nd} .deb and {nr} .rpm added for {a.version}")


def aws_env():
    return {"AWS_PROFILE": os.environ.get("AWS_PROFILE", PROFILE)}


def lookup(a, key, env):
    """The bucket name or distribution id: from the environment, else the stack's outputs."""
    v = os.environ.get(env)
    if v:
        return v
    out = subprocess.run(
        ["aws", "cloudformation", "describe-stacks", "--region", REGION, "--stack-name", STACK,
         "--query", f"Stacks[0].Outputs[?OutputKey=='{key}'].OutputValue", "--output", "text"],
        stdout=subprocess.PIPE, text=True, env={**os.environ, **aws_env()})
    if out.returncode or not out.stdout.strip():
        die(f"can't read {key} of stack {STACK}: set {env}, or AWS_PROFILE to a signed-in profile of the robocode-s account")
    return out.stdout.strip()


def run_or_print(cmds, real):
    for c in cmds:
        print("$ " + " ".join(shlex.quote(x) for x in c))
        if real:
            r = subprocess.run(c, env={**os.environ, **aws_env()})
            if r.returncode:
                die("failed: " + c[0] + " " + c[1])


def cmd_pull(a):
    bucket = (lookup(a, "PackagesBucketName", "ROBOCODES_PACKAGES_BUCKET") if a.for_real
              else os.environ.get("ROBOCODES_PACKAGES_BUCKET", "<packages bucket>"))
    run_or_print([["aws", "s3", "sync", f"s3://{bucket}/packages/", a.repo_dir + "/"]], a.for_real)


def cmd_upload(a):
    repo = a.repo_dir
    if not os.path.exists(os.path.join(repo, "apt", "dists", SUITE, "InRelease")):
        die(f"{repo} has no repository: run `repo.py build VERSION` first")
    real = a.for_real
    bucket = (lookup(a, "PackagesBucketName", "ROBOCODES_PACKAGES_BUCKET") if real
              else os.environ.get("ROBOCODES_PACKAGES_BUCKET", "<packages bucket>"))
    dist = (lookup(a, "DistributionId", "ROBOCODES_DISTRIBUTION_ID") if real
            else os.environ.get("ROBOCODES_DISTRIBUTION_ID", "<distribution id>"))
    dest = f"s3://{bucket}/packages"
    imm, short = "public, max-age=31536000, immutable", "public, max-age=300"

    def sync(sub, includes, ctype, cache):
        c = ["aws", "s3", "sync", f"{repo}/{sub}", f"{dest}/{sub}", "--exclude", "*"]
        for i in includes:
            c += ["--include", i]
        return c + ["--content-type", ctype, "--cache-control", cache]

    def cp(rel, ctype):
        return ["aws", "s3", "cp", f"{repo}/{rel}", f"{dest}/{rel}", "--content-type", ctype,
                "--cache-control", short]

    print(f"# {'UPLOADING' if real else 'DRY RUN (nothing is sent; --for-real uploads)'}: {repo} -> {dest}")
    print(f"# profile: {aws_env()['AWS_PROFILE']} (the robocode-s account, ADR 0016); order: packages, then the metadata that points at them")
    cmds = [
        sync("apt/pool", ["*.deb"], "application/vnd.debian.binary-package", imm),
        sync("rpm", ["*.rpm"], "application/x-rpm", imm),
        sync("rpm", ["*/repodata/*-primary.xml.gz", "*/repodata/*-filelists.xml.gz", "*/repodata/*-other.xml.gz"],
             "application/gzip", imm),
    ]
    for arch in ARCHES:
        for f in ("repomd.xml", "repomd.xml.asc"):
            rel = f"rpm/{arch}/repodata/{f}"
            if os.path.exists(os.path.join(repo, rel)):
                cmds.append(cp(rel, "application/xml" if f.endswith(".xml") else "text/plain"))
    d = f"apt/dists/{SUITE}"
    for arch in DEB_ARCH.values():
        cmds.append(cp(f"{d}/main/binary-{arch}/Packages.gz", "application/gzip"))
        cmds.append(cp(f"{d}/main/binary-{arch}/Packages", "text/plain"))
    cmds += [cp(f"{d}/Release", "text/plain"), cp(f"{d}/Release.gpg", "text/plain"), cp(f"{d}/InRelease", "text/plain"),
             cp("robocode-s.asc", "text/plain"), cp("robocode-s.gpg", "application/octet-stream"),
             cp("robocode-s.repo", "text/plain"), cp("robocode-s.sources", "text/plain")]
    cmds.append(["aws", "cloudfront", "create-invalidation", "--distribution-id", dist, "--paths",
                 "/packages/apt/dists/*", "/packages/rpm/*/repodata/repomd.xml", "/packages/rpm/*/repodata/repomd.xml.asc",
                 "/packages/robocode-s.*"])
    run_or_print(cmds, real)


KEYGEN = """\
# A signing key for the apt and dnf repositories: RSA 4096 (rpm before 4.19, as in RHEL 9 and
# Rocky 9, can't check ed25519 signatures; every apt and dnf can check RSA) with no expiry, so
# nothing has to be re-published on a schedule: the key is the repositories' identity, users
# trust it once. If it ever leaks, revoke it (below) and announce a new one.
#
# These commands change your gpg keyring, which is why this script only prints them.
# They ask for a passphrase: use one, and keep it in a password manager.

gpg --quick-generate-key "robocode-s packages <nick.hofstede@gmail.com>" rsa4096 sign never

# The key's fingerprint (the 40-hex-digit line under "sec"):
gpg --list-secret-keys --keyid-format long "robocode-s packages"
export ROBOCODES_GPG_KEY=<that fingerprint>     # what repo.py build uses; put it in your shell profile

# A revocation certificate, made now, while you still can:
gpg --gen-revoke "$ROBOCODES_GPG_KEY" > robocode-s-revoke.asc

# Back up the secret key and the revocation certificate somewhere other than this machine
# (an encrypted drive, or a password manager's attachment); without the secret key nobody
# can publish an update that existing installations will accept:
gpg --armor --export-secret-keys "$ROBOCODES_GPG_KEY" > robocode-s-secret.asc
#   keep robocode-s-secret.asc and robocode-s-revoke.asc OFF the disk afterwards (shred -u them).
# Restoring: gpg --import robocode-s-secret.asc

# The public key is published by repo.py build (robocode-s.asc and robocode-s.gpg); never the secret one.
"""


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("build", "upload", "pull"):
        s = sub.add_parser(name)
        s.add_argument("--repo-dir", default=os.environ.get("ROBOCODES_PACKAGE_REPO", DEFAULT_REPO))
        s.add_argument("--bucket")
        if name == "build":
            s.add_argument("version")
            s.add_argument("--dist-dir", default=os.environ.get("ROBOCODES_DIST_ROOT", DEFAULT_DIST))
            s.add_argument("--base-url", help=f"default {BASE_URL} (or ROBOCODES_REPO_URL)")
            s.add_argument("--pull", action="store_true", help="sync the published repository down first (needs AWS access)")
        else:
            s.add_argument("--for-real", action="store_true", help="run the commands (default: print them)")
    sub.add_parser("keygen")
    a = p.parse_args()
    if a.cmd == "keygen":
        print(KEYGEN, end="")
    else:
        {"build": cmd_build, "upload": cmd_upload, "pull": cmd_pull}[a.cmd](a)


if __name__ == "__main__":
    main()
