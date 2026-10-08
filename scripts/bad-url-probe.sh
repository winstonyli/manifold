#!/usr/bin/env bash
# Usage: [ENGINES="webview2"] scripts/bad-url-probe.sh
#
# Checks (spec 2026-10-07-wave-0-tooling, BT-5) that an engine host given an initial URL it can't open shows
# about:blank instead of panicking, for two tabs:
#   garbage: stored url "not a url";
#   missing: no url (the host gets ENGINE_INITIAL_URL="").
# Each is one headless instance on a fresh profile seeded with that one tab. Imported tabs sleep, so GRAPHDOWN
# (focus the first card) and GRAPHOPEN wake it. EXPORTSTATE is polled until the tab is loaded and its focused_url
# reads about:blank (or 30 tries); then a host log must exist, hold the host's "opening about:blank" line and no
# "panicked", and the tab must be loaded on about:blank. Requiring about:blank proves the host reached its initial
# navigate (a loaded tab alone doesn't: one read loaded between panics). Hosts log to %TEMP%\manifold-<tab>.log,
# so TEMP and TMP point into the case's own directory. ENGINES defaults to webview2 (servo needs manifold-engine-servo.exe beside the exe). Runs
# BelowNormal; kills only its own instances. Build first: cargo build -p manifold.
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
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT
status=0
cmd=""

# tab_state STATE: "<tab card count> <all loaded> <focused_url>" from one EXPORTSTATE file, e.g.
# "1 True about:blank" ("1 False None" when nothing is focused, "1 True " for an empty url).
tab_state() {
    python -P -c 'import json,sys; s=json.load(open(sys.argv[1], encoding="utf-8")); c=[c for c in s["cards"] if c["type"]=="application/vnd.manifold.tab"]; print(len(c), all(x["loaded"] for x in c), s["focused_url"])' "$(cygpath -w "$1")" </dev/null 2>&1 | tr -d '\r'
}

# run_case ENGINE NAME URL-MEMBER: URL-MEMBER is the seed tab's `, "url": ...` member, or empty for none.
run_case() {
    local dir="$out/$1-$2" profile="$out/$1-$2/appdata" tmp
    seed_settings "$profile" "$1"
    mkdir -p "$dir/tmp"
    tmp="$(cygpath -w "$dir/tmp")"
    echo "{\"nodes\": [{\"id\": \"t1\", \"type\": \"application/vnd.manifold.tab\"$3, \"engine\": \"$1\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null ||
        { check "$1 $2: the seed imports" 0 1; return; }
    cmd="$dir/cmd"
    : >"$cmd"
    TEMP="$tmp" TMP="$tmp" launch_headless "$exe" "$profile" "$cmd" "$dir/log"
    inst=$! # launch_headless set it BelowNormal
    # `SLEEP 4` before GRAPHDOWN is hscroll-probe's settle for the graph's first layout, kept as is: EXPORTSTATE has
    # no field that says the graph will take GRAPHDOWN yet. Every wait after it is a condition with a deadline.
    send WAITTABS "SLEEP 4" GRAPHDOWN GRAPHOPEN
    # Poll until the tab is loaded on about:blank, up to 30 exports. A host that panics never gets there: CRASH_LIMIT
    # respawns it up to 3 times, then the tab fails. The first export waits behind WAITTABS (up to 60 s), hence 120 s
    # per file.
    local n=0 got="" state=""
    while [ "$n" -lt 30 ]; do
        n=$((n + 1))
        state="$dir/s$n.json"
        send "EXPORTSTATE $(cygpath -w "$state")"
        WAIT_FILE_SECS=120 wait_file "$state" || break
        got="$(tab_state "$state")"
        [ "$got" = "1 True about:blank" ] && break
        sleep 2 # the poll interval
    done
    send QUIT
    wait "$inst" 2>/dev/null
    inst=""
    local logs
    logs="$(ls "$dir"/tmp/manifold-*.log 2>/dev/null | wc -l)"
    check "$1 $2: the host wrote a log" yes "$([ "$logs" -gt 0 ] && echo yes || echo no)"
    check "$1 $2: the host fell back to about:blank" yes \
        "$(cat "$dir"/tmp/manifold-*.log 2>/dev/null | grep -q 'opening about:blank' && echo yes || echo no)"
    check "$1 $2: the host did not panic" 0 "$(cat "$dir"/tmp/manifold-*.log 2>/dev/null | grep -c panicked)"
    # "1 True about:blank" splits into "1 True" and "about:blank"; "1 True " (an empty url) into "1 True" and "".
    check "$1 $2: the tab ends loaded (after $n exports)" "1 True" "$(echo "$got" | cut -d' ' -f1-2)"
    check "$1 $2: the tab ends on about:blank" about:blank "$(echo "$got" | cut -d' ' -f3-)"
    [ "$status" -eq 0 ] || { echo "--- $1 $2 host log"; tail -n 15 "$dir"/tmp/manifold-*.log 2>/dev/null; }
}

for engine in ${ENGINES:-webview2}; do
    run_case "$engine" garbage ', "url": "not a url"'
    run_case "$engine" missing ''
done
exit $status
