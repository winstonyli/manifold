#!/usr/bin/env bash
# Usage: scripts/failed-spawn-probe.sh
#
# Checks the failed card a failed spawn leaves (spec failed-spawn-cards):
#   1. With no engine host to start (a copy of the exe on its own, so every
#      engine fails to launch) the empty session's startup Root spawn fails and
#      leaves one card: a tab, not loaded. (This proves the Root card, not a
#      New card: NEWTAB spawns the same url, so it takes the twin path below.)
#   2. A NEWTAB failing for the same url, engine and group leaves still one
#      card, under the same id (the twin is refreshed, not duplicated).
#   3. Retry (GRAPHRIGHT GRAPHOPEN: the same `RetryMissingFile` the card's menu
#      runs) keeps that card and its id.
# One headless instance, an empty session, state read back through EXPORTSTATE.
# Build first: cargo build -p manifold.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
require_fresh_exe "$exe" || exit 1
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

# The failure is forced by running a copy of the exe with no engine host beside it. A build that embeds the WebView2
# host (release.sh) would still extract it and succeed, so the folder it extracts into is also made unusable: a file
# stands where `engine-hosts` should be. This dev build embeds nothing and fails on the missing sibling alone; the
# file changes nothing for it.
mkdir -p "$out/bin"
cp "$exe" "$out/bin/manifold.exe"
exe="$out/bin/manifold.exe"
profile="$out/appdata"
seed_settings "$profile"
mkdir -p "$profile/manifold/manifold/data"
echo "not a folder" >"$profile/manifold/manifold/data/engine-hosts"
echo '{"nodes": []}' >"$profile/seed.json"
"$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

cmd="$profile/cmd"
: >"$cmd"
launch_headless "$exe" "$profile" "$cmd" "$profile/log"
inst=$!
{
    echo NEWTAB
    echo WAITTABS
    echo "SLEEP 2"
    echo "EXPORTSTATE $(cygpath -w "$out/s1.json")"
    echo NEWTAB
    echo WAITTABS
    echo "SLEEP 2"
    echo "EXPORTSTATE $(cygpath -w "$out/s2.json")"
    echo GRAPHRIGHT
    echo GRAPHOPEN
    echo "SLEEP 2"
    echo "EXPORTSTATE $(cygpath -w "$out/s3.json")"
    echo QUIT
} >>"$cmd"
wait "$inst" 2>/dev/null
inst=""

# Prints "<card count> <id of the first card> <its label>" for one EXPORTSTATE file.
summary() {
    python -P -c 'import json,sys; from export_state import label; c=json.load(open(sys.argv[1], encoding="utf-8"))["cards"]; print(len(c), c[0]["id"] if c else "-", label(c[0]) if c else "-")' "$(cygpath -w "$1")" </dev/null 2>&1
}
status=0
read -r n1 id1 label1 <<<"$(summary "$out/s1.json")"
read -r n2 id2 _ <<<"$(summary "$out/s2.json")"
read -r n3 id3 _ <<<"$(summary "$out/s3.json")"
check "a failed startup Root spawn leaves one card" "1" "$n1"
check "it is a tab that is not loaded" "application/vnd.manifold.tab" "$label1"
check "a New spawn failing for the same intent stays one card" "1" "$n2"
check "and it is the same card" "$id1" "$id2"
check "Retry keeps the card count" "1" "$n3"
check "Retry keeps the card id" "$id1" "$id3"
exit $status
