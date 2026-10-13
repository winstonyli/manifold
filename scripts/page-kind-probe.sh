#!/usr/bin/env bash
# Usage: [ENGINES="webview2 cef"] scripts/page-kind-probe.sh
#
# Checks each engine's page-kind detection (`EngineToHost::JsonBody` /
# `RedirectableContent`, see `ipc_protocol::PAGE_KIND_SCRIPT`): one headless
# instance per engine and case on a fresh isolated APPDATA restores a tab at
# a local URL (scripts/probe_server.py) that answers with a real
# `application/json` or `image/png`, and the tab must turn into a Json or
# Image graph node (read back through EXPORTSTATE, as the card's `type`). A
# plain HTML page must stay a live tab.
# Build first: cargo build -p manifold -p manifold-view-cef -p manifold-view-webview2.
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
# The inline python below reads EXPORTSTATE cards through scripts/export_state.py.
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
srv=""
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "$srv" ] && kill "$srv" 2>/dev/null
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

python -P "$repo_root/scripts/probe_server.py" "$(cygpath -w "$out/port")" "$(cygpath -w "$out/keys.log")" 600 </dev/null >/dev/null 2>&1 &
srv=$!
for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
port="$(cat "$out/port")"

status=0
# path -> the card it must become, as export_state.py's `label` spells it (the type; `;loaded` on a live tab)
for engine in ${ENGINES:-webview2 cef}; do
    for case in "/data.json application/json" "/pixel.png image/png" "/ application/vnd.manifold.tab;loaded"; do
        path="${case% *}"
        want="${case#* }"
        name="$engine${path//[\/.]/-}"
        profile="$out/appdata-$name"
        seed_settings "$profile" "$engine"
        echo "{\"nodes\": [{\"id\": \"t1\", \"type\": \"application/vnd.manifold.tab\", \"url\": \"http://127.0.0.1:$port$path\", \"view_kind\": \"$engine\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
        "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

        cmd="$profile/cmd"
        : >"$cmd"
        launch_headless "$exe" "$profile" "$cmd" "$profile/log"
        inst=$!
        {
            echo WAITTABS
            # The detection runs when the load finishes, after the tab spawns.
            echo "SLEEP ${SETTLE:-6}"
            echo "EXPORTSTATE $(cygpath -w "$out/state-$name.json")"
            echo QUIT
        } >>"$cmd"
        wait "$inst" 2>/dev/null
        inst=""

        got="$(python -P -c 'import json,sys; from export_state import label; print(" ".join(label(c) for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]))' "$(cygpath -w "$out/state-$name.json")" </dev/null 2>&1)"
        if [ "$got" = "$want" ]; then
            echo "PASS: $engine $path becomes $want"
        else
            echo "FAIL: $engine $path: cards [$got], wanted [$want]"
            status=1
        fi
    done
done
exit $status
