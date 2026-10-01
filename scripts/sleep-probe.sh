#!/usr/bin/env bash
# Usage: [CASES="manual idle scroll dirty history"] [ENGINES="webview2 cef"] scripts/sleep-probe.sh
#
# Checks "Sleep" per engine. Sleep is two-phase: `App::request_sleep` asks
# the page for its state (`QueryPageState`) and `finish_sleep` sleeps on the
# reply. Cases:
#   manual  the menu action (headless `SLEEPNODE id`): tab, asleep, woken.
#   idle    idle auto-sleep (`App::sweep_idle_tabs`, headless `IDLESWEEP
#           secs`, run twice 3 s apart: the first sweep starts the clock):
#           tab, asleep, woken.
#   scroll  a tall page (/tall) scrolled with PageDown in Tab view, slept
#           from Graph view and woken: the page must come back at the
#           position it had (|before - after| <= 2 px, before > 0).
#   history two more pages, then sleep and wake; Back twice and Forward once
#           must retrace them (the host keeps the list, `nav_history`).
#   dirty   /tall?dirty=1 has an input changed from its default: idle sleep
#           must leave it awake, manual Sleep must still sleep it.
# Opening a card starts the engine again and brings the tab back, under the
# same id. One headless instance on a fresh isolated APPDATA per case and
# engine, tab at a local page (scripts/probe_server.py, which logs the page's
# scroll reports), state read back through EXPORTSTATE.
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

# The card kinds and the focused card's id in an exported state, "id kind".
cards() {
    python -P -c 'import json,sys; c = json.load(open(sys.argv[1], encoding="utf-8"))["cards"]; print(" ".join(x["id"] + ":" + x["kind"] for x in c))' "$(cygpath -w "$1")" </dev/null
}

status=0
for case in ${CASES:-manual idle scroll dirty}; do
for engine in ${ENGINES:-webview2 cef}; do
    profile="$out/appdata-$case-$engine"
    mkdir -p "$profile/manifold"
    page_log="$out/$case-$engine-page.log"
    : >"$page_log"
    python -P "$repo_root/scripts/probe_server.py" "$(cygpath -w "$out/port-$case-$engine")" "$(cygpath -w "$page_log")" 300 </dev/null >/dev/null 2>&1 &
    srv=$!
    for _ in $(seq 50); do [ -s "$out/port-$case-$engine" ] && break; sleep 0.1; done
    port="$(cat "$out/port-$case-$engine")"
    case "$case" in
    scroll) path=/tall ;;
    dirty) path="/tall?dirty=1" ;;
    *) path=/ ;;
    esac
    echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"$engine\"}" >"$profile/manifold/settings.json"
    echo "{\"nodes\": [{\"id\": \"t1\", \"kind\": \"tab\", \"url\": \"http://127.0.0.1:$port$path\", \"engine\": \"$engine\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

    cmd="$profile/cmd"
    : >"$cmd"
    # `-n +1`: plain `tail -f` starts with only the last 10 lines.
    APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log" 2>&1 &
    inst=$!
    st() { cygpath -w "$out/$case-$engine-$1.json"; }
    {
        echo WAITTABS
        echo "SLEEP 4"
        echo "EXPORTSTATE $(st live)"
    } >>"$cmd"
    for _ in $(seq 60); do [ -s "$out/$case-$engine-live.json" ] && break; sleep 0.5; done
    id="$(cards "$out/$case-$engine-live.json" | cut -d: -f1)"
    {
        case "$case" in
        manual | idle)
            if [ "$case" = manual ]; then
                echo "SLEEPNODE $id"
            else
                echo "IDLESWEEP 9999"
                echo "SLEEP 3"
                echo "IDLESWEEP 2"
            fi
            echo "SLEEP 1"
            echo "EXPORTSTATE $(st asleep)"
            # Opening the card wakes it; the open is a click on the card in Graph view.
            echo GRAPHOPEN
            echo WAITTABS
            echo "SLEEP 4"
            echo "EXPORTSTATE $(st woken)"
            ;;
        scroll)
            # Into Tab view, focus the page, scroll it, back to Graph view.
            echo GRAPHDOWN
            echo GRAPHOPEN
            echo "SLEEP 6"
            echo "CLICK 300 200"
            for _ in 1 2 3; do echo "KEYPAGE PageDown"; done
            echo "SLEEP 1"
            echo ESCAPE
            echo "SLEEP 2"
            echo "SLEEPNODE $id"
            echo "SLEEP 2"
            echo GRAPHOPEN
            echo WAITTABS
            echo "SLEEP 6"
            ;;
        history)
            # Two more pages in Tab view, then sleep, wake and go Back twice.
            echo GRAPHDOWN
            echo GRAPHOPEN
            echo "SLEEP 6"
            for page in p2 p3; do
                echo "ADDRESSBAR http://127.0.0.1:$port/$page"
                echo "SLEEP 1"
                echo ENTER
                echo "SLEEP 3"
            done
            echo "EXPORTSTATE $(st pre)"
            echo ESCAPE
            echo "SLEEP 2"
            echo "SLEEPNODE $id"
            echo "SLEEP 2"
            echo GRAPHOPEN
            echo WAITTABS
            echo "SLEEP 5"
            echo "EXPORTSTATE $(st woken)"
            echo GOBACK
            echo "SLEEP 3"
            echo "EXPORTSTATE $(st back1)"
            echo GOBACK
            echo "SLEEP 3"
            echo "EXPORTSTATE $(st back2)"
            echo GOFORWARD
            echo "SLEEP 3"
            echo "EXPORTSTATE $(st fwd)"
            ;;
        dirty)
            echo "IDLESWEEP 9999"
            echo "SLEEP 3"
            echo "IDLESWEEP 2"
            echo "SLEEP 2"
            echo "EXPORTSTATE $(st stay)"
            echo "SLEEPNODE $id"
            echo "SLEEP 2"
            echo "EXPORTSTATE $(st slept)"
            ;;
        esac
        echo QUIT
    } >>"$cmd"
    wait "$inst" 2>/dev/null
    inst=""
    kill "$srv" 2>/dev/null
    srv=""

    case "$case" in
    manual | idle)
        got="$(cards "$out/$case-$engine-live.json") | $(cards "$out/$case-$engine-asleep.json") | $(cards "$out/$case-$engine-woken.json")"
        want="$id:Tab | $id:Unloaded | $id:Tab"
        what="tab, asleep, woken again"
        ;;
    scroll)
        # A: the last scroll report before the page's second load (the
        # position it was slept at); B: the last one after that load (where
        # the woken page ended up). B must come after the reload, or a page
        # that never restored would pass with B = A.
        read -r a b <<<"$(awk '/^load/ { loads++; next } /^scroll/ { sub("y=", "", $2); if (loads < 2) a = $2; else b = $2 } END { print (a == "" ? "none" : a), (b == "" ? "none" : b) }' "$page_log")"
        got="before=$a after=$b"
        want="$got"
        if [ "$a" = none ] || [ "$b" = none ] || [ "$a" -le 0 ] || [ $((a > b ? a - b : b - a)) -gt 2 ]; then
            want="before > 0, |before - after| <= 2"
        fi
        what="scroll kept across sleep ($got)"
        ;;
    history)
        url_of() {
            python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["focused_url"].rsplit("/", 1)[-1] or "/")' "$(cygpath -w "$out/$case-$engine-$1.json")" </dev/null
        }
        got="woken=$(url_of woken) back=$(url_of back1) back=$(url_of back2) forward=$(url_of fwd)"
        want="woken=p3 back=p2 back=/ forward=p2"
        what="Back and Forward after sleep and wake ($got)"
        ;;
    dirty)
        got="$(cards "$out/$case-$engine-stay.json") | $(cards "$out/$case-$engine-slept.json")"
        want="$id:Tab | $id:Unloaded"
        what="idle sleep skipped the dirty tab, manual Sleep did not"
        ;;
    esac
    if [ "$got" = "$want" ]; then
        echo "PASS: $case $engine: $what"
    else
        echo "FAIL: $case $engine: got [$got], wanted [$want]"
        [ "$case" = scroll ] && sed 's/^/    page: /' "$page_log"
        status=1
    fi
done
done
exit $status
