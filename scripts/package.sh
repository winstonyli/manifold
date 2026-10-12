#!/usr/bin/env bash
# Usage: scripts/package.sh --check [path\to\manifold-vX.Y.Z-windows-x64.exe]
#
# The check for the bare-exe release that scripts/release.sh writes (plan
# docs/superpowers/plans/2026-10-11-bare-exe-host-downloads.md). The exe defaults to the newest
# dist/manifold-v*-windows-x64.exe. It is copied ALONE into a temp folder, so nothing but manifold.exe is beside it,
# and asserted on:
#   - it imports no vcruntime/msvcp (llvm-objdump; the check fails without it);
#   - it runs headless with APPDATA redirected to a temp folder (never the real one);
#   - ENGINESTATE cef reads "Not installed", with " · N MB" when crates/manifold/src/host_pin.rs pins a CEF host
#     (release.sh wrote it), and likewise for Servo when it is pinned;
#   - a WebView2 tab loads a local page, so the host was extracted from the embed (no sibling host exists);
#   - the folder still holds only manifold.exe, plus the new engine-hosts/ under the redirected APPDATA.
# Downloads nothing. Build the release with scripts/release.sh.
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
cd "$repo_root"

[ "${1:-}" = "--check" ] || { sed -n '2p' "$0" >&2; exit 2; }
exe_src="${2:-$(ls -t dist/manifold-v*-windows-x64.exe 2>/dev/null | head -1)}"
[ -n "$exe_src" ] && [ -f "$exe_src" ] || { echo "no release exe: run scripts/release.sh first" >&2; exit 1; }

out="$(mktemp -d)"
srv="" inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "$srv" ] && kill "$srv" 2>/dev/null
    rm -rf "$out"
}
trap cleanup EXIT
status=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; status=1; }

mkdir -p "$out/bin"
cp "$exe_src" "$out/bin/manifold.exe"
exe="$out/bin/manifold.exe"
echo "    $(basename "$exe_src"): $(stat -c %s "$exe") bytes"

# The CRT is linked statically, so the exe needs no Visual C++ Redistributable (vcruntime140, msvcp140).
if command -v llvm-objdump >/dev/null 2>&1; then
    dlls="$(llvm-objdump -p "$exe" | sed -n 's/^ *DLL Name: //p' | tr '\n' ' ')"
    echo "    imports: $dlls"
    echo "$dlls" | grep -qiE 'vcruntime|msvcp' && bad "the exe needs the VC++ Redistributable" || ok "the exe needs no VC++ Redistributable"
else
    bad "llvm-objdump not found: cannot verify no vcruntime/msvcp imports; install llvm (scoop install llvm)"
fi

profile="$out/appdata"
mkdir -p "$profile/manifold"
python -P "$repo_root/scripts/probe_server.py" "$(cygpath -w "$out/port")" "$(cygpath -w "$out/page.log")" 120 </dev/null >/dev/null 2>&1 &
srv=$!
for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
port="$(cat "$out/port")"
echo '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2"}' >"$profile/manifold/settings.json"
echo "{\"nodes\": [{\"id\": \"t1\", \"type\": \"application/vnd.manifold.tab\", \"url\": \"http://127.0.0.1:$port/tall\", \"engine\": \"webview2\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
"$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null
cmd="$profile/cmd"
: >"$cmd"
APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log" 2>&1 &
inst=$!
{ echo WAITTABS; echo "SLEEP 6"; echo "ENGINESTATE cef"; echo "ENGINESTATE servo"; echo QUIT; } >>"$cmd"
for _ in $(seq 300); do kill -0 "$inst" 2>/dev/null || break; sleep 0.2; done
kill -0 "$inst" 2>/dev/null && bad "headless manifold did not exit after QUIT within 60 s" || { wait "$inst" 2>/dev/null || true; inst=""; }

# A pinned host (host_pin.rs written by release.sh) makes the state carry the real download size.
pins="$(python -P scripts/host_pin.py --list "$(cygpath -m crates/manifold/src/host_pin.rs)" </dev/null)"
for engine in cef servo; do
    # An engine this build pins no download for (a release made without --with-servo) reads Unavailable.
    want='^engine-state '"$engine"': Unavailable: this build has no [A-Za-z]+ download'
    grep -q "^$engine " <<<"$pins" && want='^engine-state '"$engine"': Not installed · [0-9]+ MB'
    grep -qE "$want" "$profile/log" && ok "$engine reports $(grep -E "^engine-state $engine:" "$profile/log" | head -1)" ||
        { bad "$engine state line missing or wrong (wanted /$want/)"; sed 's/^/    log: /' "$profile/log" | tail -5; }
done
grep -q '^load ' "$out/page.log" 2>/dev/null && ok "a WebView2 tab loaded the local page (host extracted from the embed)" ||
    bad "the local page never reported its load"
[ -f "$profile/manifold/manifold/data/engine-hosts/webview2/manifold-engine-webview2.exe" ] &&
    ok "the WebView2 host was extracted under the redirected APPDATA" || bad "no extracted WebView2 host under the redirected APPDATA"
[ "$(ls "$out/bin")" = "manifold.exe" ] && ok "nothing but manifold.exe sits beside the exe" || bad "extra files beside the exe: $(ls "$out/bin" | tr '\n' ' ')"
exit $status
