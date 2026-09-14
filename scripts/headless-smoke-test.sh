#!/usr/bin/env bash
# Smoke test for browser-ui's --headless mode (see README.md's "Headless
# mode" section for the stdin text-command protocol this drives). Not a
# full test framework, and not content-level assertions — there is no
# state-query debug command yet, only screenshots — so this catches
# crashes/total breakage (process exits non-zero, a screenshot never gets
# produced or comes back suspiciously small) rather than verifying exact
# pixels. Exercises the address-bar autocomplete flow end to end: launch,
# type a query, screenshot the suggestion dropdown, click a suggestion,
# screenshot the result; arrow-key highlight + Enter-accept; Escape
# dismissing the dropdown without leaving Tab view; quit.
#
# Deliberately NOT covered: clicking an open-tab suggestion (accepting it
# focuses another tab). That needs a second tab, which needs clicking a
# graph-view node — and a node's on-screen position depends on the
# force-directed layout's physics settling, not a fixed coordinate the way
# chrome buttons are. There's no position-query debug command yet to make
# that click reliable, so it's left to manual verification for now (see
# project memory for the one-off manual pass that confirmed it works).
#
# Run after `cargo build -p browser-ui`. Exits 0 if every check passed.
set -uo pipefail # not -e: a failed check should still let later checks run

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exe="$repo_root/target/debug/browser-ui.exe"
out_dir="${HEADLESS_TEST_OUT:-$repo_root/target/headless-test-output}"

if [ ! -x "$exe" ]; then
    echo "headless-smoke-test: $exe not found — build first (cargo build -p browser-ui)" >&2
    exit 1
fi

mkdir -p "$out_dir"
rm -f "$out_dir"/*.png

# `SCREENSHOT`'s path travels over stdin, not argv — MSYS/Git-Bash's
# automatic POSIX-to-Windows path translation for a spawned native exe
# only rewrites its command-line *arguments*, not bytes a script writes
# into the child's stdin afterward. `$out_dir` as-is (`/c/Users/...`)
# isn't a valid Windows path and silently fails to save; `cygpath -w`
# converts it once, up front, into the `C:\Users\...` form the Rust side
# actually needs.
out_dir_win="$(cygpath -w "$out_dir")"

# A stray engine-host-servo.exe left over from an earlier crashed/killed
# run collides with this run's own root tab on the deterministic "tab-0"
# named pipe, silently leaving the app with zero tabs (see project
# memory) — always start from a verified-clean process slate. The extra
# settle time after killing matters: a just-killed process's named pipe
# can take a moment to actually release (documented in project memory as
# a real, if rare, race), and a fresh spawn attempted too soon can fail
# with a plain "Access is denied" rather than a pipe-already-exists error.
taskkill //F //IM browser-ui.exe >/dev/null 2>&1 || true
taskkill //F //IM engine-host-servo.exe >/dev/null 2>&1 || true
sleep 2

fail=0
pass() { echo "PASS: $1"; }
failed() {
    echo "FAIL: $1"
    fail=1
}

check_screenshot() {
    local path="$1" label="$2" min_bytes="${3:-2000}"
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
    taskkill //F //IM engine-host-servo.exe >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 4 # startup: root tab spawn + first Servo frame

# Pin a known window size so the click coordinates below (measured against
# this exact size during manual verification) stay valid regardless of
# whatever winit's platform-default window size happens to be.
send "RESIZE 816 639"
sleep 1

send "ADDRESSBAR weather"
sleep 3 # debounce (~200ms) + real DuckDuckGo suggestion fetch over the network
send "SCREENSHOT $out_dir_win\\dropdown.png"
sleep 1
check_screenshot "$out_dir/dropdown.png" "autocomplete dropdown"

# First suggestion row's position below the address bar, at the 816x639
# size pinned above — re-measure (via a manual SCREENSHOT) if chrome
# layout constants (TAB_STRIP_HEIGHT_POINTS/TOOLBAR_HEIGHT_POINTS in
# main.rs) ever change.
send "CLICK 283 107"
sleep 3 # navigation + first paint
send "SCREENSHOT $out_dir_win\\after_click.png"
sleep 1
check_screenshot "$out_dir/after_click.png" "post-click navigation"

# Arrow-key highlight + Enter-accept: two ARROWDOWNs should land on the
# 3rd suggestion row, and ENTER should navigate to *that* row's query, not
# the literally-typed text — no coordinates involved, fully deterministic.
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

# Escape dismisses the dropdown without leaving Tab view (as opposed to a
# second Escape / one with no dropdown showing, which leaves to Graph
# view) — just confirms no crash and a reasonable frame, same limitation
# as every other check here (no state-query command to assert the
# dropdown is actually gone, or that view_mode is still Tab).
send "ADDRESSBAR pancake recipe"
sleep 3
send "ESCAPE"
sleep 1
send "SCREENSHOT $out_dir_win\\escape_dismiss.png"
sleep 1
check_screenshot "$out_dir/escape_dismiss.png" "Escape dismiss"

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
