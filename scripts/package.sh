#!/usr/bin/env bash
# Usage: scripts/package.sh [--check]
#
# Builds the Windows release package: dist/manifold-v<version>-windows-x64.zip holding manifold.exe,
# manifold-engine-webview2.exe and the small manifold-engine-cef.exe, flat in one folder, plus README.md and
# PACKAGING.md. CEF's runtime is not in it: the app downloads it on demand (spec
# docs/superpowers/specs/2026-10-02-engine-install-design.md). Refuses a dirty tree, so the zip matches a commit.
#
# --check  unzips the newest zip into a temp folder and asserts on it (no libcef.dll or .pak files; a headless
#          manifold starts, reports CEF as not installed, and a WebView2 tab loads a local page that reports its load). Downloads nothing.
# SKIP_BUILD=1  package whatever target/release holds (for a build you ran yourself, e.g. at BelowNormal priority).
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"
version="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' Cargo.toml | head -1)"
name="manifold-v$version-windows-x64"
zip="$repo_root/dist/$name.zip"

check() {
    [ -f "$zip" ] || { echo "no $zip: run scripts/package.sh first" >&2; exit 1; }
    out="$(mktemp -d)"
    srv="" inst=""
    cleanup() {
        [ -n "$inst" ] && { winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"; [ -n "$winpid" ] && taskkill //F //T //PID "$winpid" >/dev/null 2>&1; }
        [ -n "$srv" ] && kill "$srv" 2>/dev/null
        rm -rf "$out"
    }
    trap cleanup EXIT
    unzip -q "$zip" -d "$out/pkg"
    pkg="$out/pkg/$name"
    status=0
    ok() { echo "PASS: $1"; }
    bad() { echo "FAIL: $1"; status=1; }

    for f in manifold.exe manifold-engine-webview2.exe manifold-engine-cef.exe README.md PACKAGING.md; do
        [ -f "$pkg/$f" ] && ok "zip has $f" || bad "zip lacks $f"
    done
    heavy="$(find "$pkg" \( -iname 'libcef.dll' -o -iname '*.pak' -o -iname 'icudtl.dat' \) | head -1)"
    [ -z "$heavy" ] && ok "zip has no CEF runtime files" || bad "zip carries a CEF runtime file: $heavy"

    profile="$out/appdata"
    mkdir -p "$profile/manifold"
    python -P "$repo_root/scripts/probe_server.py" "$(cygpath -w "$out/port")" "$(cygpath -w "$out/page.log")" 120 </dev/null >/dev/null 2>&1 &
    srv=$!
    for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
    port="$(cat "$out/port")"
    echo '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2"}' >"$profile/manifold/settings.json"
    echo "{\"nodes\": [{\"id\": \"t1\", \"kind\": \"tab\", \"url\": \"http://127.0.0.1:$port/tall\", \"engine\": \"webview2\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$pkg/manifold.exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null
    cmd="$profile/cmd"
    : >"$cmd"
    APPDATA="$(cygpath -w "$profile")" "$pkg/manifold.exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log" 2>&1 &
    inst=$!
    { echo WAITTABS; echo "SLEEP 6"; echo "ENGINESTATE cef"; echo QUIT; } >>"$cmd"
    wait "$inst" 2>/dev/null || true
    inst=""
    grep -q '^engine-state cef: Not installed' "$profile/log" && ok "CEF reports Not installed" ||
        { bad "CEF state line missing or wrong"; sed 's/^/    log: /' "$profile/log" | tail -5; }
    grep -q '^load ' "$out/page.log" 2>/dev/null && ok "a WebView2 tab loaded the local page" || bad "the local page never reported its load"
    exit $status
}

if [ "${1:-}" = "--check" ]; then check; fi

if [ -n "$(git status --porcelain)" ]; then echo "package.sh: the working tree is dirty; commit first" >&2; exit 1; fi
if [ -z "${SKIP_BUILD:-}" ]; then
    cargo build --release -j 12 -p manifold -p manifold-engine-webview2 -p manifold-engine-cef
fi
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name" "$repo_root/dist"
for f in manifold manifold-engine-webview2 manifold-engine-cef; do
    cp "target/release/$f.exe" "$stage/$name/"
done
cp README.md "$stage/$name/"
cat >"$stage/$name/PACKAGING.md" <<EOF
# manifold $version (Windows x64)

Run manifold.exe. Keep the three .exe files in the same folder.

This package carries two engines: WebView2 (needs the WebView2 runtime that Windows 11
includes) and a small CEF host. CEF's runtime (about 172 MB compressed, 423 MB unpacked) is not included. It is an
optional download: pick CEF in the engine picker, or Settings > Engines > Download, and confirm. It installs under
%APPDATA%\\manifold\\engine-hosts\\cef-runtime and can be removed from the same page.

The executables are unsigned, so Windows SmartScreen may warn on first run.
EOF
rm -f "$zip"
if command -v 7z >/dev/null 2>&1; then
    (cd "$stage" && 7z a -tzip -mx=6 "$(cygpath -w "$zip")" "$name" >/dev/null)
else
    powershell -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$stage/$name")' -DestinationPath '$(cygpath -w "$zip")'"
fi
echo "$zip"
ls -l "$zip" | awk '{print $5 " bytes"}'
sha256sum "$zip" | cut -d' ' -f1
