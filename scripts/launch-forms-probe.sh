#!/usr/bin/env bash
# Usage: scripts/launch-forms-probe.sh
#
# Launches the exe once in each form its launchers and the session tools use
# (wave 1, M4: argument checking), and checks each exit code:
#   drive.sh                      0   (--import-json/--out, then --headless)
#   --headless --profile-dir --script (json-pan-profile.sh's list)  0
#   --import-json ... --out ...   0
#   --dump <mnf>                  0
#   --check <mnf>                 0
#   --headless ... --hedless      2, stderr names `unknown argument "--hedless"`
# --debug-commands is not launched: it opens a real window (a unit test covers it).
# Every run is headless or a file tool, on a temp profile (never the real
# %APPDATA%\manifold), under `timeout 60` so a hang fails instead of stalling.
# Build first: cargo build -j 8 -p manifold -p manifold-engine-webview2.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
require_fresh_exe "$exe" || exit 1
# Not under the repo: a path with a space stops the app reading its commands.
t="$(mktemp -d)"
trap '[ -n "${KEEP:-}" ] || rm -rf "$t"' EXIT
mkdir -p "$t/p" "$t/bad"
printf '%s' '{"nodes":[{"id":"g","type":"application/vnd.manifold.group","name":"G"}]}' >"$t/session.json"
printf 'QUIT\n' >"$t/quit.txt"
w() { cygpath -w "$1"; }

status=0
# run <name> <expected exit> <cmd...>: the run's stdout and stderr go to $t/<name>.out and .err.
run() {
    local name=$1 want=$2 rc
    shift 2
    timeout 60 "$@" </dev/null >"$t/$name.out" 2>"$t/$name.err"
    rc=$?
    check "$name exits $want" "$want" "$rc"
}

# drive.sh seeds its isolated profile from SEED/session.json, then runs --headless.
SEED="$t" run drive 0 bash "$repo_root/scripts/drive.sh" "$t/quit.txt"
run script 0 "$exe" --headless --profile-dir "$(w "$t/p")" --script "$(w "$t/quit.txt")"
run import 0 "$exe" --import-json "$(w "$t/session.json")" --out "$(w "$t/s.mnf")"
run dump 0 "$exe" --dump "$(w "$t/s.mnf")"
run check 0 "$exe" --check "$(w "$t/s.mnf")"
run refused 2 "$exe" --headless --profile-dir "$(w "$t/bad")" --hedless
check "refused names the argument" "1" "$(grep -c 'unknown argument "--hedless"' "$t/refused.err")"

if [ $status -ne 0 ]; then
    for f in "$t"/*.out "$t"/*.err; do
        [ -s "$f" ] && { echo "--- $(basename "$f")"; tail -n 20 "$f"; }
    done
fi
exit $status
