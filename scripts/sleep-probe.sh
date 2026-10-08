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
#   crash   as history, but the engine host is killed instead of slept: the app
#           restarts the tab and Back and Forward must still work.
#   restart as history, but the app is quit and started again on the same
#           profile (`tab-history.json`): Back and Forward must still work.
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
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
require_fresh_exe "$exe" || exit 1
# The inline python below reads EXPORTSTATE cards through scripts/export_state.py.
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"
# Seconds from a sleep request to the tab being asleep. WebView2 sleeps at
# once, but a CEF tab's sleep waits for its last cookies: the host's jar pull
# answers only after `EMPTY_PULL_TIMEOUT` (2 s, manifold-engine-cef
# jar_cookies.rs) on an empty jar, and the app holds the page state up to
# `COOKIE_HOLD` (2 s, tabs.rs) for it, so CEF sleeps ~2 s after the request.
# A 1-2 s wait exported (or woke) a CEF tab before it slept, and the dirty
# case's "idle sleep left it awake" check passed on CEF without proving it.
asleep_wait=4
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

# The cards in an exported state, "id:label" each (export_state.py's `label`: the type, `;loaded` on a live tab).
cards() {
    python -P -c 'import json,sys; from export_state import label; c = json.load(open(sys.argv[1], encoding="utf-8"))["cards"]; print(" ".join(x["id"] + ":" + label(x) for x in c))' "$(cygpath -w "$1")" </dev/null
}
live=application/vnd.manifold.tab\;loaded
asleep=application/vnd.manifold.tab

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
    seed_settings "$profile" "$engine"
    echo "{\"nodes\": [{\"id\": \"t1\", \"type\": \"application/vnd.manifold.tab\", \"url\": \"http://127.0.0.1:$port$path\", \"engine\": \"$engine\", \"pos\": [0.0, 0.0]}]}" >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

    cmd="$profile/cmd"
    : >"$cmd"
    # `-n +1`: plain `tail -f` starts with only the last 10 lines.
    APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log" 2>&1 &
    inst=$!
    st() { cygpath -w "$out/$case-$engine-$1.json"; }
    back_and_forward() {
        echo GOBACK
        echo "SLEEP 3"
        echo "EXPORTSTATE $(st back1)"
        echo GOBACK
        echo "SLEEP 3"
        echo "EXPORTSTATE $(st back2)"
        echo GOFORWARD
        echo "SLEEP 3"
        echo "EXPORTSTATE $(st fwd)"
    }
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
            echo "SLEEP $asleep_wait"
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
            echo "SLEEP $asleep_wait"
            echo GRAPHOPEN
            echo WAITTABS
            echo "SLEEP 6"
            ;;
        history | crash | restart)
            # Two more pages in Tab view, then sleep (history) or kill the engine host (crash), then Back twice.
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
            if [ "$case" = crash ]; then
                # Kill the engine host from outside, between command batches; the app restarts the tab at its URL.
                # Only this instance's own host (its child by pid): other manifolds may be running.
                for _ in $(seq 120); do [ -s "$out/$case-$engine-pre.json" ] && break; sleep 0.5; done
                if w="$(winpid_of "$inst")"; then
                    for h in $(powershell -NoProfile -Command "(Get-CimInstance Win32_Process -Filter \"ParentProcessId=$w AND Name='manifold-engine-$engine.exe'\").ProcessId" </dev/null | tr -d '\r'); do
                        taskkill //F //T //PID "$h" >/dev/null 2>&1
                    done
                fi
                sleep 8
            elif [ "$case" = restart ]; then
                # Quit here; the second instance below restores the tab.
                echo "SLEEP 2"
            else
                echo ESCAPE
                echo "SLEEP 2"
                echo "SLEEPNODE $id"
                echo "SLEEP $asleep_wait"
                echo GRAPHOPEN
                echo WAITTABS
                echo "SLEEP 5"
            fi
            if [ "$case" != restart ]; then
                echo "EXPORTSTATE $(st woken)"
                back_and_forward
            fi
            ;;
        dirty)
            echo "IDLESWEEP 9999"
            echo "SLEEP 3"
            echo "IDLESWEEP 2"
            echo "SLEEP $asleep_wait"
            echo "EXPORTSTATE $(st stay)"
            echo "SLEEPNODE $id"
            echo "SLEEP $asleep_wait"
            echo "EXPORTSTATE $(st slept)"
            ;;
        esac
        echo QUIT
    } >>"$cmd"
    wait "$inst" 2>/dev/null
    inst=""
    if [ "$case" = restart ]; then
        # A second instance on the same profile: the restored tab has a new engine.
        : >"$cmd"
        APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log2" 2>&1 &
        inst=$!
        {
            echo WAITTABS
            echo "SLEEP 3"
            echo GRAPHDOWN
            echo GRAPHOPEN
            echo "SLEEP 6"
            echo "EXPORTSTATE $(st woken)"
            back_and_forward
            echo QUIT
        } >>"$cmd"
        wait "$inst" 2>/dev/null
        inst=""
    fi
    kill "$srv" 2>/dev/null
    srv=""

    case "$case" in
    manual | idle)
        got="$(cards "$out/$case-$engine-live.json") | $(cards "$out/$case-$engine-asleep.json") | $(cards "$out/$case-$engine-woken.json")"
        want="$id:$live | $id:$asleep | $id:$live"
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
    history | crash | restart)
        url_of() {
            python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["focused_url"].rsplit("/", 1)[-1] or "/")' "$(cygpath -w "$out/$case-$engine-$1.json")" </dev/null
        }
        got="woken=$(url_of woken) back=$(url_of back1) back=$(url_of back2) forward=$(url_of fwd)"
        want="woken=p3 back=p2 back=/ forward=p2"
        what="Back and Forward after sleep and wake ($got)"
        ;;
    dirty)
        got="$(cards "$out/$case-$engine-stay.json") | $(cards "$out/$case-$engine-slept.json")"
        want="$id:$live | $id:$asleep"
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
