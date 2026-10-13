#!/usr/bin/env bash
# Usage: [ENGINES="webview2 cef"] scripts/hscroll-probe.sh
#
# Checks horizontal wheel scrolling per engine: a page 6000 px wide
# (scripts/probe_server.py /wide, which logs `hscroll y=<scrollX>` after each
# scroll settles) gets SCROLL with a positive dx (the page must move right,
# scrollX up), then a negative one (back toward 0). The deltas are the host's
# own (`WindowEvent::MouseWheel`), so the sign checked here is the one a real
# touchpad or tilt wheel produces.
# One headless instance on a fresh isolated APPDATA per engine.
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

status=0
for engine in ${ENGINES:-webview2 cef}; do
    profile="$out/appdata-$engine"
    mkdir -p "$profile/manifold"
    page_log="$out/$engine-page.log"
    : >"$page_log"
    python -P "$repo_root/scripts/probe_server.py" "$(cygpath -w "$out/port-$engine")" "$(cygpath -w "$page_log")" 300 </dev/null >/dev/null 2>&1 &
    srv=$!
    for _ in $(seq 50); do [ -s "$out/port-$engine" ] && break; sleep 0.1; done
    port="$(cat "$out/port-$engine")"
    seed_settings "$profile" "$engine"
    echo "{\"nodes\": [{\"id\": \"t1\", \"type\": \"application/vnd.manifold.tab\", \"url\": \"http://127.0.0.1:$port/wide\", \"engine\": \"$engine\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

    cmd="$profile/cmd"
    : >"$cmd"
    launch_headless "$exe" "$profile" "$cmd" "$profile/log"
    inst=$!
    {
        echo WAITTABS
        echo "SLEEP 4"
        echo GRAPHDOWN
        echo GRAPHOPEN
        echo "SLEEP 6"
        echo "SCROLL 300 200 240 0"
        echo "SLEEP 2"
        echo "SCROLL 300 200 -120 0"
        echo "SLEEP 2"
        echo QUIT
    } >>"$cmd"
    wait "$inst" 2>/dev/null
    inst=""
    kill "$srv" 2>/dev/null
    srv=""

    # The settled positions, in order: after the positive dx, after the negative one.
    read -r a b <<<"$(awk '/^hscroll/ { sub("y=", "", $2); n++; if (n == 1) a = $2; else if (n == 2) b = $2 } END { print (a == "" ? "none" : a), (b == "" ? "none" : b) }' "$page_log")"
    got="after +dx=$a after -dx=$b"
    if [ "$a" != none ] && [ "$b" != none ] && [ "$a" -gt 0 ] && [ "$b" -lt "$a" ]; then
        echo "PASS: hscroll $engine: right then back ($got)"
    else
        echo "FAIL: hscroll $engine: wanted +dx > 0 and -dx below it, got [$got]"
        sed 's/^/    page: /' "$page_log"
        status=1
    fi
done
exit $status
