"""Rewrite the table in crates/manifold/src/host_pin.rs: the engine host binaries a release built and uploads.

Usage (run by scripts/release.sh; from anywhere):

    python -P scripts/host_pin.py --version 0.5.0 [--out crates/manifold/src/host_pin.rs]
        [--keep-from <an older host_pin.rs>] --asset cef/windows-x64=target/release/manifold-engine-cef.exe ... < /dev/null

--asset is KIND/PLATFORM=PATH: KIND is servo, cef, webkit or webview2; PLATFORM is windows-x64 or linux-x64. The file
is hashed (SHA-256) and measured; the component installs under the PATH's base name and is fetched from
https://github.com/winstonyli/manifold/releases/download/v<version>/<name>-<platform>. A missing or empty file is
refused. --keep-from copies the older table's entries unchanged (their URLs keep pointing at the older release's
assets: scripts/release.sh --reuse-hosts-from, plan Decision 11); an asset for the same (kind, platform, name) is an
error. Only the text between `// BEGIN TABLE` and `// END TABLE` is replaced, and the SHA-256 of that text is printed
on stdout so the release notes can carry it. The file is left alone if anything is refused.

    python -P scripts/host_pin.py --list <a host_pin.rs> < /dev/null

prints that file's table instead: a `version V` line, then `kind platform file size sha256 url` per component.
"""

import argparse
import hashlib
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_OUT = os.path.join(HERE, "..", "crates", "manifold", "src", "host_pin.rs")
BASE = "https://github.com/winstonyli/manifold/releases/download/"
KINDS = {"servo": "Servo", "cef": "Cef", "webkit": "Webkit", "webview2": "Webview2"}  # store name -> variant
PLATFORMS = ("windows-x64", "linux-x64")

REGION = re.compile(r"(// BEGIN TABLE\n)(.*?)(// END TABLE\n)", re.S)
ENTRY_LINE = re.compile(r'^\s*Entry \{ kind: EngineKind::(\w+), platform: "([^"]+)", components: &\[$')
COMPONENT_LINE = re.compile(
    r'^\s*Component \{ file: "([^"]+)", url: "([^"]+)", size: (\d+), sha256: "([0-9a-f]{64})" \},$')


def die(msg):
    sys.exit(f"host_pin.py: {msg}")


def read_region(path):
    with open(path, encoding="utf-8", newline="") as fh:
        text = fh.read()
    m = REGION.search(text)
    if not m:
        die(f"{path} has no `// BEGIN TABLE` ... `// END TABLE` region")
    return text, m


def parse_entries(region):
    """{(kind name, platform): {file: (url, size, sha256)}} from a region this script rendered."""
    by_variant = {v: k for k, v in KINDS.items()}
    out, cur = {}, None
    for line in region.splitlines():
        if m := ENTRY_LINE.match(line):
            cur = (by_variant[m.group(1)], m.group(2))
            out[cur] = {}
        elif m := COMPONENT_LINE.match(line):
            if cur is None:
                die("a Component line outside an Entry in the --keep-from table")
            out[cur][m.group(1)] = (m.group(2), int(m.group(3)), m.group(4))
    return out


def render(version, entries):
    lines = [
        "/// The app version this table was cut for.",
        f'pub const PIN_VERSION: &str = "{version}";',
        "",
        "#[rustfmt::skip]",
        "const TABLE: &[Entry] = &[",
    ]
    for (kind, platform), files in sorted(entries.items()):
        lines.append(f'    Entry {{ kind: EngineKind::{KINDS[kind]}, platform: "{platform}", components: &[')
        for file, (url, size, sha) in sorted(files.items()):
            lines.append(f'        Component {{ file: "{file}", url: "{url}", size: {size}, sha256: "{sha}" }},')
        lines.append("    ] },")
    lines.append("];")
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--version")
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--keep-from")
    ap.add_argument("--list", metavar="FILE", help="print FILE's table as `kind platform file size sha256 url` lines")
    ap.add_argument("--asset", action="append", default=[], metavar="KIND/PLATFORM=PATH")
    args = ap.parse_args()
    if args.list:
        _, m = read_region(args.list)
        version = re.search(r'PIN_VERSION: &str = "([^"]*)"', m.group(2))
        print(f"version {version.group(1) if version else ''}")
        for (kind, platform), files in sorted(parse_entries(m.group(2)).items()):
            for file, (url, size, sha) in sorted(files.items()):
                print(f"{kind} {platform} {file} {size} {sha} {url}")
        return
    if not args.version:
        die("--version is required")

    entries = {}
    if args.keep_from:
        _, m = read_region(args.keep_from)
        entries = parse_entries(m.group(2))
    for spec in args.asset:
        left, eq, path = spec.partition("=")
        kind, slash, platform = left.partition("/")
        if not eq or not slash or kind not in KINDS or platform not in PLATFORMS:
            die(f"bad --asset {spec!r}: want KIND/PLATFORM=PATH with KIND in {sorted(KINDS)}, PLATFORM in {PLATFORMS}")
        if not os.path.isfile(path):
            die(f"{path}: no such file")
        size = os.path.getsize(path)
        if size == 0:
            die(f"{path}: empty file")
        h = hashlib.sha256()
        with open(path, "rb") as fh:
            while chunk := fh.read(1 << 20):
                h.update(chunk)
        name = os.path.basename(path)
        files = entries.setdefault((kind, platform), {})
        if name in files:
            die(f"{kind}/{platform}/{name} is both kept from {args.keep_from} and given as an asset")
        files[name] = (f"{BASE}v{args.version}/{name}-{platform}", size, h.hexdigest())
        print(f"{kind}/{platform} {name} {size} bytes sha256 {h.hexdigest()}", file=sys.stderr)

    body = render(args.version, entries)
    text, m = read_region(args.out)
    new = text[: m.start(2)] + body + text[m.end(2) :]
    tmp = args.out + ".new"
    with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(new)
    os.replace(tmp, args.out)
    print(f"wrote {os.path.normpath(args.out)} ({sum(len(f) for f in entries.values())} components)", file=sys.stderr)
    print(hashlib.sha256(body.encode("utf-8")).hexdigest())


if __name__ == "__main__":
    main()
