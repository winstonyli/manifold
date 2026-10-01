#!/usr/bin/env bash
# Usage: [ENGINES="webview2 cef"] scripts/sleep-probe.sh
#
# Checks "Sleep" (`App::sleep_tab`, headless `SLEEPNODE id`) per engine: a
# live tab becomes an unloaded card, and opening that card starts the engine
# again and brings the tab back, under the same id. One headless instance on
# a fresh isolated APPDATA per engine, tab at a local page
# (scripts/probe_server.py), state read back through EXPORTSTATE.
# Build first: cargo build -p manifold -p manifold-engine-cef -p manifold-engine-webview2.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
require_fresh_exe "$exe" || exit 1
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
srv=""
inst=""
cleanup() {
    [ -n "$inst" ] && { winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"; [ -n "$winpid" ] && taskkill //F //T //PID "$winpid" >/dev/null 2>&1; }
    [ -n "$srv" ] && kill "$srv" 2>/dev/null
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

python -P "$repo_root/scripts/probe_server.py" "$(cygpath -w "$out/port")" "$(cygpath -w "$out/keys.log")" 600 </dev/null >/dev/null 2>&1 &
srv=$!
for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
port="$(cat "$out/port")"

# The card kinds and the focused card's id in an exported state, "id kind".
cards() {
    python -P -c 'import json,sys; c = json.load(open(sys.argv[1], encoding="utf-8"))["cards"]; print(" ".join(x["id"] + ":" + x["kind"] for x in c))' "$(cygpath -w "$1")" </dev/null
}

status=0
for engine in ${ENGINES:-webview2 cef}; do
    profile="$out/appdata-$engine"
    mkdir -p "$profile/manifold"
    echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"$engine\"}" >"$profile/manifold/settings.json"
    echo "{\"nodes\": [{\"id\": \"t1\", \"kind\": \"tab\", \"url\": \"http://127.0.0.1:$port/\", \"engine\": \"$engine\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

    cmd="$profile/cmd"
    : >"$cmd"
    # `-n +1`: plain `tail -f` starts with only the last 10 lines.
    APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log" 2>&1 &
    inst=$!
    st() { cygpath -w "$out/$engine-$1.json"; }
    {
        echo WAITTABS
        echo "SLEEP 4"
        echo "EXPORTSTATE $(st live)"
    } >>"$cmd"
    for _ in $(seq 60); do [ -s "$out/$engine-live.json" ] && break; sleep 0.5; done
    id="$(cards "$out/$engine-live.json" | cut -d: -f1)"
    {
        echo "SLEEPNODE $id"
        echo "SLEEP 1"
        echo "EXPORTSTATE $(st asleep)"
        # Opening the card wakes it; the open is a click on the card in Graph view.
        echo GRAPHOPEN
        echo WAITTABS
        echo "SLEEP 4"
        echo "EXPORTSTATE $(st woken)"
        echo QUIT
    } >>"$cmd"
    wait "$inst" 2>/dev/null
    inst=""

    got="$(cards "$out/$engine-live.json") | $(cards "$out/$engine-asleep.json") | $(cards "$out/$engine-woken.json")"
    want="$id:Tab | $id:Unloaded | $id:Tab"
    if [ "$got" = "$want" ]; then
        echo "PASS: $engine: tab, asleep, woken again"
    else
        echo "FAIL: $engine: got [$got], wanted [$want]"
        status=1
    fi
done
exit $status
