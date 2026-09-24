#!/usr/bin/env bash
# Smoke test for manifold's --headless mode (see README.md's "Headless
# mode" section for the stdin text-command protocol this drives). Not a
# full test framework and not pixel comparison: it checks each screenshot
# was produced and isn't suspiciously small, and asserts on app state
# (view mode, the focused tab's URL) read back through EXPORTSESSION, so
# it catches wrong behavior as well as crashes. Exercises the address-bar
# autocomplete flow end to end: launch (into Graph view), click the root
# node to enter Tab view, type a query, screenshot the suggestion dropdown,
# click a suggestion, screenshot the result; arrow-key highlight +
# Enter-accept; Escape dismissing the dropdown without leaving Tab view;
# quit.
#
# Deliberately NOT covered: clicking an open-tab suggestion (accepting it
# focuses another tab). That needs a second tab, which needs clicking a
# graph-view node — and a node's on-screen position depends on the
# force-directed layout's physics settling, not a fixed coordinate the way
# chrome buttons are. There's no position-query debug command yet to make
# that click reliable, so it's left to manual verification for now (see
# project memory for the one-off manual pass that confirmed it works).
#
# Run after `cargo build -p manifold`. Exits 0 if every check passed.
set -uo pipefail # not -e: a failed check should still let later checks run

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
out_dir="${HEADLESS_TEST_OUT:-$repo_root/target/headless-test-output}"

if [ ! -x "$exe" ]; then
    echo "headless-smoke-test: $exe not found — build first (cargo build -p manifold)" >&2
    exit 1
fi

mkdir -p "$out_dir"
rm -f "$out_dir"/*.png "$out_dir"/*.json

# `SCREENSHOT`'s path travels over stdin, not argv — MSYS/Git-Bash's
# automatic POSIX-to-Windows path translation for a spawned native exe
# only rewrites its command-line *arguments*, not bytes a script writes
# into the child's stdin afterward. `$out_dir` as-is (`/c/Users/...`)
# isn't a valid Windows path and silently fails to save; `cygpath -w`
# converts it once, up front, into the `C:\Users\...` form the Rust side
# actually needs.
out_dir_win="$(cygpath -w "$out_dir")"

# A fresh profile of its own, so the real %APPDATA%\manifold is never
# read or written (and a real session can't move the root tab the click
# below expects). `has_shown_first_launch_demo` skips the demo scene, so
# the app starts with just the root tab. Other running manifold windows
# are left alone: an engine pipe name they hold makes `spawn_tab` fall
# back to a fresh one, and engines never outlive their manifold.exe (its
# Job Object), so there are no orphans to clear first.
profile="$repo_root/target/headless-test-appdata-$$"
mkdir -p "$profile/manifold"
echo '{"has_shown_first_launch_demo": true}' >"$profile/manifold/settings.json"
export APPDATA="$(cygpath -w "$profile")"

fail=0
pass() { echo "PASS: $1"; }
failed() {
    echo "FAIL: $1"
    fail=1
}

# The app saves a SCREENSHOT on its next drawn frame, which can take
# seconds when the GPU is busy; wait up to the 30 s the app itself gives
# one, then for the size to stop changing (a save in progress).
wait_for_file() {
    local path="$1" waited=0 prev=-1 now
    while [ ! -f "$path" ] && [ "$waited" -lt 150 ]; do
        sleep 0.2
        waited=$((waited + 1))
    done
    while [ -f "$path" ] && now=$(stat -c%s "$path") && [ "$now" != "$prev" ]; do
        prev=$now
        sleep 0.2
    done
}

check_screenshot() {
    local path="$1" label="$2" min_bytes="${3:-2000}"
    wait_for_file "$path"
    if [ ! -f "$path" ]; then
        failed "$label: screenshot not created ($path)"
        return
    fi
    local size
    size=$(stat -c%s "$path" 2>/dev/null || stat -f%z "$path" 2>/dev/null || echo 0)
    if [ "$size" -lt "$min_bytes" ]; then
        failed "$label: screenshot suspiciously small ($size bytes)"
    else
        pass "$label: screenshot looks reasonable ($size bytes)"
    fi
}

# EXPORTSESSION writes the live session (tabs and their URLs, the focused
# tab, `view_mode_tab`), which makes it the protocol's state query. `expr`
# is a Python expression over `d` (the session), `url` (the focused tab's
# URL) and `q` (that URL's `q=` search query, or "").
check_state() {
    local name="$1" label="$2" expr="$3" path="$out_dir/$1.json" result
    send "EXPORTSESSION $out_dir_win\\$name.json"
    wait_for_file "$path"
    if result=$(python - "$path" "$expr" <<'EOF'
import json, sys
from urllib.parse import parse_qs, urlparse
d = json.load(open(sys.argv[1], encoding="utf-8"))
url = next((t["url"] for t in d["tabs"] if t["id"] == d["focused"]), "")
q = parse_qs(urlparse(url).query).get("q", [""])[0]
print(url)
sys.exit(0 if eval(sys.argv[2]) else 1)
EOF
    ); then
        pass "$label ($result)"
    else
        failed "$label: \`$expr\` is false (focused tab: ${result:-none})"
    fi
}

# `coproc` gives a bidirectional pipe to the child — the same shape
# `--headless` is designed for (a driving script writing commands, reading
# nothing back except the exit code), without needing a second process
# just to hold stdin open.
coproc BROWSER { "$exe" --headless; }
browser_pid=$BROWSER_PID

send() { echo "$1" >&"${BROWSER[1]}"; }

cleanup() {
    kill "$browser_pid" 2>/dev/null || true
    wait "$browser_pid" 2>/dev/null
    rm -rf "$profile"
}
trap cleanup EXIT

sleep 4 # startup: root tab spawn + first Servo frame

# Pin a known window size so the click coordinates below (measured against
# this exact size during manual verification) stay valid regardless of
# whatever winit's platform-default window size happens to be.
send "RESIZE 816 639"
sleep 1

# The app now launches straight into Graph view (M5-polish-era change) —
# with only the root tab and no edges, physics has nothing to do but pull
# it toward the world origin, which maps to the window's screen center at
# this pinned size (measured: ~(407, 332) at 816x639 during manual
# verification of the shrink/expand transition). Click it to enter Tab
# view before the address-bar flow below, which assumes Tab view's chrome
# is on screen; the 220ms shrink/expand transition needs a moment to land.
send "CLICK 407 332"
sleep 1
check_state tab_view "clicking the root node enters Tab view" 'd["view_mode_tab"]'

send "ADDRESSBAR weather"
sleep 3 # debounce (~200ms) + real DuckDuckGo suggestion fetch over the network
send "SCREENSHOT $out_dir_win\\dropdown.png"
sleep 1
check_screenshot "$out_dir/dropdown.png" "autocomplete dropdown"

# First suggestion row's position below the address bar, at the 816x639
# size pinned above — re-measure (via a manual SCREENSHOT) if chrome
# layout constants (CHROME_HEIGHT_POINTS in main.rs) ever change. (Already
# had to move this twice — most recently when Tab view's separate tab-strip
# and toolbar bars were merged into one, shrinking chrome height 58->32.)
# The SCREENSHOT above also matters here: it makes the app draw the
# dropdown, and a CLICK sent before any frame has drawn it hits nothing.
send "CLICK 283 42"
sleep 3 # navigation + first paint
send "SCREENSHOT $out_dir_win\\after_click.png"
sleep 1
check_screenshot "$out_dir/after_click.png" "post-click navigation"
# Usually the first row is the typed text itself, but DuckDuckGo's order
# varies (see the arrow-key check below), so only the prefix is certain.
check_state after_click "clicking the first suggestion searches it" 'q.startswith("weather")'

# Arrow-key highlight + Enter-accept: two ARROWDOWNs land on the 2nd
# suggestion row (the first press highlights row 1), and ENTER searches
# that row's query — no coordinates involved. The state check can't tell
# that row from the typed text: DuckDuckGo's order varies between runs,
# and sometimes the typed text itself is the 2nd row. So it only checks
# that Enter searched one of the suggestions; which row shows in the
# screenshot.
send "ADDRESSBAR sourdough bread"
sleep 3
send "ARROWDOWN"
sleep 1
send "ARROWDOWN"
sleep 1
send "ENTER"
sleep 3
send "SCREENSHOT $out_dir_win\\arrow_nav.png"
sleep 1
check_screenshot "$out_dir/arrow_nav.png" "arrow-key navigation"
check_state arrow_nav "Enter searches a suggestion" 'q.startswith("sourdough bread")'

# Escape dismisses the dropdown without leaving Tab view (as opposed to a
# second Escape / one with no dropdown showing, which leaves to Graph
# view). Whether the dropdown itself is gone only shows in the screenshot.
send "ADDRESSBAR pancake recipe"
sleep 3
send "ESCAPE"
sleep 1
send "SCREENSHOT $out_dir_win\\escape_dismiss.png"
sleep 1
check_screenshot "$out_dir/escape_dismiss.png" "Escape dismiss"
check_state escape_dismiss "Escape stays in Tab view and navigates nowhere"     'd["view_mode_tab"] and q != "pancake recipe" and "sourdough" in q'

send "QUIT"
if wait "$browser_pid"; then
    pass "process exited cleanly"
else
    failed "process did not exit cleanly (exit code $?)"
fi

echo
if [ "$fail" -eq 0 ]; then
    echo "All checks passed. Screenshots in $out_dir"
else
    echo "Some checks failed. Screenshots in $out_dir"
fi
exit "$fail"
