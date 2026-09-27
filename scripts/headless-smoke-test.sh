#!/usr/bin/env bash
# Smoke test for manifold's --headless mode (see README.md's "Headless
# mode" section for the stdin text-command protocol this drives). Not a
# full test framework and not pixel comparison: it checks each screenshot
# was produced and isn't suspiciously small, and asserts on app state
# (view mode, the focused tab's URL, the suggestion rows and which one is
# highlighted) read back through EXPORTSTATE, so
# it catches wrong behavior as well as crashes. Exercises the address-bar
# autocomplete flow end to end: launch (into Graph view), click the root
# node to enter Tab view, type a query, screenshot the suggestion dropdown,
# click a suggestion, screenshot the result; arrow-key highlight +
# Enter-accept; Escape dismissing the dropdown without leaving Tab view;
# entering and leaving a group in Graph view; tab drops inside a group
# (reparent, the cycle refusal) and ejecting a rectangle-selected tab;
# the overloaded and empty groups' border colors (needs Pillow);
# closing a group holding an image and a nested group, then reopening
# it (REOPENTAB) nested as it was;
# quit. The drop checks click cards where EXPORTSTATE's `cards` says they
# are, after the layout stops moving (`settle`), not at fixed coordinates.
#
# Deliberately NOT covered: clicking an open-tab suggestion (accepting it
# focuses another tab), left to manual verification (see project memory
# for the one-off manual pass that confirmed it works). EXPORTSTATE's
# `cards` could now find the second tab's node to click.
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
. "$repo_root/scripts/fresh-exe.sh"
require_fresh_exe "$exe" || exit 1

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
# one, then for the size to stop changing (a save in progress; an empty
# file counts as not written yet).
wait_for_file() {
    local path="$1" waited=0 prev=-1 now
    while [ ! -s "$path" ] && [ "$waited" -lt 150 ]; do
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

# EXPORTSTATE writes the app's UI state as JSON (see its doc comment in
# debug_control.rs). `state NAME EXPR` exports it to NAME.json and
# evaluates the Python expression EXPR over `s` (that state), `url` (the
# focused tab's URL), `q` (that URL's `q=` search query, or "") and `row`
# (the ROW environment variable, for comparing against a captured value): it
# prints the value, or the URL with a third argument `url`, and succeeds
# if the value is truthy.
state() {
    local path="$out_dir/$1.json"
    rm -f "$path"
    send "EXPORTSTATE $out_dir_win\\$1.json"
    wait_for_file "$path"
    python - "$path" "$2" "${3:-}" <<'EOF'
import json, os, sys
from urllib.parse import parse_qs, urlparse
s = json.load(open(sys.argv[1], encoding="utf-8"))
url = s["focused_url"] or ""
q = parse_qs(urlparse(url).query).get("q", [""])[0]
row = os.environ.get("ROW", "")
v = eval(sys.argv[2])
print(url if sys.argv[3] == "url" else v)
sys.exit(0 if v else 1)
EOF
}

check_state() {
    local name="$1" label="$2" expr="$3" result
    if result=$(state "$name" "$expr" url); then
        pass "$label ($result)"
    else
        failed "$label: \`$expr\` is false (focused tab: ${result:-none})"
    fi
}

# Polls until EXPR holds, for up to ~15 s: suggestions come from
# DuckDuckGo over the network, so a fixed sleep either wastes time or
# (seen about once in 15 runs at 3 s) isn't enough.
wait_state() {
    local name="$1" label="$2" expr="$3" i
    for i in $(seq 1 30); do
        if state "$name" "$expr" >/dev/null; then
            pass "$label"
            return
        fi
        sleep 0.5
    done
    failed "$label: \`$expr\` still false after ~15 s"
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

# Pin a known window size so the click coordinates below (measured against
# this exact size during manual verification) stay valid regardless of
# whatever winit's platform-default window size happens to be.
send "RESIZE 816 639"

# The root tab spawns asynchronously (30 s or more on a loaded machine);
# a fixed sleep here used to race it.
send "WAITTABS"

# The app launches into Graph view; with only the root tab, the layout
# puts it at the window's center (~(407, 332) at 816x639). Clicking it
# enters Tab view, which the address-bar flow below needs. This CLICK is
# the first command after the tab spawns, so it lands only because
# headless mode draws a frame before each pointer command (it missed 3
# runs out of 3 without that). Keep it first: any command in between
# renders a frame of its own and would hide a regression.
send "CLICK 407 332"
sleep 1
check_state tab_view "clicking the just-spawned root node enters Tab view" 's["tab_view"]'

send "ADDRESSBAR weather"
wait_state weather_rows "suggestions for \"weather\" arrive" 's["suggestions"]'
send "SCREENSHOT $out_dir_win\\dropdown.png"
check_screenshot "$out_dir/dropdown.png" "autocomplete dropdown"
first_row=$(state weather_rows 's["suggestions"][0]')

# First suggestion row's position below the address bar, at the 816x639
# size pinned above — re-measure (via a manual SCREENSHOT) if chrome
# layout constants (CHROME_HEIGHT_POINTS in main.rs) ever change. (Already
# had to move this twice — most recently when Tab view's separate tab-strip
# and toolbar bars were merged into one, shrinking chrome height 58->32.)
send "CLICK 283 42"
sleep 3 # navigation + first paint
send "SCREENSHOT $out_dir_win\\after_click.png"
check_screenshot "$out_dir/after_click.png" "post-click navigation"
ROW="$first_row" check_state after_click "clicking the first suggestion searches it" 'q == row'

# Arrow-key highlight + Enter-accept: two ARROWDOWNs highlight the 2nd row
# (the first press highlights the 1st), and ENTER searches that row's
# query. No coordinates involved.
send "ADDRESSBAR sourdough bread"
wait_state sourdough_rows "suggestions for \"sourdough bread\" arrive" 'len(s["suggestions"]) >= 2'
send "ARROWDOWN"
send "ARROWDOWN"
check_state highlighted "two ARROWDOWNs highlight the 2nd row" 's["highlighted"] == 1'
second_row=$(state highlighted 's["suggestions"][1]')
send "ENTER"
sleep 3
send "SCREENSHOT $out_dir_win\\arrow_nav.png"
check_screenshot "$out_dir/arrow_nav.png" "arrow-key navigation"
ROW="$second_row" check_state arrow_nav "Enter searches the highlighted row" 'q == row'

# Escape dismisses the dropdown without leaving Tab view (as opposed to a
# second Escape / one with no dropdown showing, which leaves to Graph
# view), and navigates nowhere.
send "ADDRESSBAR pancake recipe"
wait_state pancake_rows "suggestions for \"pancake recipe\" arrive" 's["suggestions"]'
send "ESCAPE"
send "SCREENSHOT $out_dir_win\\escape_dismiss.png"
check_screenshot "$out_dir/escape_dismiss.png" "Escape dismiss"
ROW="$second_row" check_state escape_dismiss "Escape closes the dropdown, stays in Tab view, navigates nowhere" \
    's["tab_view"] and not s["suggestions"] and q == row'

# Card helpers. `query NAME EXPR` evaluates EXPR over an
# already-exported NAME.json without exporting again: `s` (the state),
# `cards` (by id), `tabs` (live or asleep) and `groups` (sorted ids),
# `xy(id)` and `lt(id)` (a card's center and top-left corner, where its
# eject button sits, as "x y") and `bbox` (every tab's rects' bounds grown
# by 20, as "x0 y0 x1 y1").
query() {
    python - "$out_dir/$1.json" "$2" <<'EOF'
import json, os, sys
s = json.load(open(sys.argv[1], encoding="utf-8"))
cards = {c["id"]: c for c in s["cards"]}
tabs = sorted(i for i, c in cards.items() if c["kind"] in ("Tab", "Unloaded"))
groups = sorted(i for i, c in cards.items() if c["kind"] == "Group")
xy = lambda i: "%.0f %.0f" % tuple(cards[i]["screen"])
lt = lambda i: "%.0f %.0f" % (cards[i]["rect"][0] + 3, cards[i]["rect"][1] + 3)
r = [cards[i]["rect"] for i in tabs]
bbox = "%.0f %.0f %.0f %.0f" % (max(0, min(x[0] for x in r) - 20), max(40, min(x[1] for x in r) - 20),
                                max(x[2] for x in r) + 20, max(x[3] for x in r) + 20) if r else ""
env = os.environ
print(eval(sys.argv[2]))
EOF
}

check_query() {
    if [ "$(query "$1" "bool($3)")" = True ]; then pass "$2"; else failed "$2: \`$3\` is false in $1.json"; fi
}

# Exports until two exports in a row agree on every card's rect, leaving
# the last as NAME.json. After entering a group the camera eases, and
# SLEEP draws no frames (each command draws one), so a fixed sleep can
# read positions mid-ease; a drag there lands on empty canvas.
settle() {
    local prev="" now i
    for i in $(seq 1 40); do
        state "$1" 1 >/dev/null
        now=$(query "$1" 'sorted((i, [round(v, 1) for v in c["rect"]]) for i, c in cards.items())')
        [ "$now" = "$prev" ] && return 0
        prev=$now
        sleep 0.3
    done
    failed "$1: the layout never stopped moving"
}

# Group drill-in: a second Escape (no dropdown now) leaves to Graph view,
# NEWGROUP adds a group card, DOUBLECLICK enters it and the toolbar's back
# button leaves it. The card is found by its reported position: spawn
# jitter comes from the id, and ids are random per launch. DOUBLECLICK
# rather than two CLICKs, which a slow frame can push past egui's
# double-click delay.
send "ESCAPE"
send "NEWGROUP"
sleep 1 # Tab-to-Graph transition, layout settle
send "SCREENSHOT $out_dir_win\group_card.png"
check_screenshot "$out_dir/group_card.png" "group card"
settle group_card
first_group=$(query group_card 'groups[0]')
send "DOUBLECLICK $(query group_card "xy('$first_group')")"
check_state group_in "double-clicking the group card enters the group" 'not s["tab_view"] and s["current_group"]'
send "CLICK 22 17"
check_state group_out "the back button leaves the group" 's["current_group"] is None'

# Tab drops inside a group.

# `drag X0 Y0 X1 Y1 [button]`: a press, six moves and a release with no
# sleep between them (see debug_control.rs's advice on drags).
drag() {
    local i
    send "MOUSEDOWN $1 $2${5:+ $5}"
    for i in 1 2 3 4 5 6; do send "MOUSEMOVE $(($1 + ($3 - $1) * i / 6)) $(($2 + ($4 - $2) * i / 6))"; done
    send "MOUSEUP $3 $4${5:+ $5}"
}

# A two-tab group from a file, entered by double-clicking its card.
cat >"$out_dir/pair.group.json" <<'EOF'
{"nodes": [
 {"id": "g", "kind": "group", "name": "Pair"},
 {"id": "a", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "pos": [-150.0, 0.0], "group": "g"},
 {"id": "b", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "pos": [150.0, 0.0], "group": "g"}]}
EOF
before=$(query group_out 'groups')
send "IMPORTGROUP $out_dir_win\pair.group.json"
send "WAITTABS"
# The imported card lands wherever the jitter puts it, which can be off
# screen at this zoom: the toolbar's Reset View (fit all) button brings
# every card into view (its spot at 816 wide, as measured from a
# SCREENSHOT; re-measure if the Graph toolbar changes).
send "CLICK 326 17"
settle pair_top
pair=$(BEFORE="$before" query pair_top '[g for g in groups if g not in eval(env["BEFORE"])][0]')
send "DOUBLECLICK $(query pair_top "xy('$pair')")"
settle pair_in
check_query pair_in "double-clicking the imported group's card enters it" "s['current_group'] == '$pair' and len(tabs) == 2"
read -r m1 m2 <<<"$(query pair_in '" ".join(tabs)')"

drag $(query pair_in "xy('$m1')") $(query pair_in "xy('$m2')")
settle reparented
check_query reparented "a tab dropped on a sibling tab inside a group is reparented under it" \
    "cards['$m1']['parent'] == '$m2'"
# The pair now overlaps exactly, and which is on top follows spawn order,
# so move whichever the press picks up onto empty canvas first.
read -r x y <<<"$(query reparented "xy('$m1')")"
drag "$x" "$y" $((x - 250)) $((y - 150))
settle apart
drag $(query apart "xy('$m2')") $(query apart "xy('$m1')")
settle cycle
PREV="$(query apart "cards['$m2']['screen']")" check_query cycle "dropping a tab onto its own child is refused" \
    "cards['$m2']['screen'] != eval(env['PREV']) and cards['$m2']['parent'] is None and cards['$m1']['parent'] == '$m2'"

# A right-drag marquee selects both; ejecting one through its button
# takes it out of the selection too (the pair overlaps again, so either
# may be the one ejected).
drag $(query cycle 'bbox') right
settle selected
check_query selected "a right-drag marquee selects both tabs" "s['selected_tabs'] == tabs"
read -r x y <<<"$(query selected "xy('$m1')")"
read -r ex ey <<<"$(query selected "lt('$m1')")"
send "MOUSEMOVE $x $y"
send "MOUSEMOVE $ex $ey"
send "CLICK $ex $ey"
settle ejected
check_query ejected "ejecting a selected tab also drops it from the selection" \
    "len(tabs) == 1 and s['selected_tabs'] == tabs"

# Border colors, read off a screenshot: a group of more than 8 members
# rests on DANGER_COLOR (196, 87, 58), and the empty group from above on
# ACCENT (215, 162, 74). `card_border`'s unit test covers which color
# wins; this checks the member count reaches it and the frame paints it.
# The mouse waits in an empty corner so no card eases toward its hover
# color.
send "CLICK 22 17"
python - "$out_dir/nine.group.json" <<'EOF'
import json, sys
tab = lambda i: {"id": f"t{i}", "kind": "tab", "url": "manifold://welcome", "engine": "webview2", "group": "g"}
json.dump({"nodes": [{"id": "g", "kind": "group", "name": "Nine"}] + [tab(i) for i in range(9)]}, open(sys.argv[1], "w"))
EOF
settle top
before_nine=$(query top 'groups')
send "IMPORTGROUP $out_dir_win\nine.group.json"
send "WAITTABS"
send "CLICK 326 17"
send "MOUSEMOVE 806 629"
settle nine
nine=$(BEFORE="$before_nine" query nine '[g for g in groups if g not in eval(env["BEFORE"])][0]')
empty=$(BEFORE="$before" query nine 'eval(env["BEFORE"])[0]')
send "SCREENSHOT $out_dir_win\borders.png"
check_screenshot "$out_dir/borders.png" "group borders"
# `border NAME ID RGB` succeeds if a pixel on ID's left edge, at
# mid-height, is within 40 of RGB (the stroke is about 1 px wide).
border() {
    python - "$out_dir/$1.json" "$out_dir/borders.png" "$2" "$3" <<'EOF'
import json, sys
from PIL import Image
cards = {c["id"]: c for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]}
img = Image.open(sys.argv[2]).convert("RGB")
left, top, _, bottom = cards[sys.argv[3]]["rect"]
y = round((top + bottom) / 2)
want = eval(sys.argv[4])
near = min(sum((a - b) ** 2 for a, b in zip(img.getpixel((x, y)), want)) ** 0.5 for x in range(round(left) - 1, round(left) + 3))
sys.exit(0 if near < 40 else 1)
EOF
}
if ! python -c "import PIL" 2>/dev/null; then
    echo "SKIP: group border colors (needs Pillow: pip install pillow)"
else
    if border nine "$nine" "(196, 87, 58)"; then pass "a group of 9 has a DANGER_COLOR border"
    else failed "the 9-member group $nine's border isn't DANGER_COLOR (see borders.png)"; fi
    if border nine "$empty" "(215, 162, 74)" && ! border nine "$empty" "(196, 87, 58)"; then pass "an empty group has an ACCENT border"
    else failed "the empty group $empty's border isn't ACCENT (see borders.png)"; fi
fi

# Recently Closed keeps nesting: fill the empty group with an image and
# the Pair group (so `$empty { image, group { tab, tab } }`), close it
# from the top level, and REOPENTAB (Ctrl+Shift+T) brings the whole
# subtree back as one entry, still nested.
send "DOUBLECLICK $(query nine "xy('$empty')")"
settle outer_in
send "ADDIMAGE $(cygpath -w "$repo_root")\crates\manifold\assets\icon\icon-64.png"
send "IMPORTGROUP $out_dir_win\pair.group.json"
send "WAITTABS"
settle outer_built
check_query outer_built "a group can hold an image and a nested group" \
    "s['current_group'] == '$empty' and len(groups) == 1 and [c['kind'] for c in cards.values()].count('Image') == 1"
send "CLICK 22 17"
settle outer_back
send "CLOSEGROUP $empty"
settle outer_closed
check_query outer_closed "closing the outer group removes it" "'$empty' not in groups"
send "REOPENTAB"
send "WAITTABS"
settle reopened
outer=$(BEFORE="$(query outer_closed 'groups')" query reopened '([g for g in groups if g not in eval(env["BEFORE"])] or ["none"])[0]')
if [ "$outer" = none ]; then
    failed "REOPENTAB brought no group back"
else
    send "DOUBLECLICK $(query reopened "xy('$outer')")"
    settle reopened_in
    check_query reopened_in "the reopened group has its image and nested group back" \
        "s['current_group'] == '$outer' and len(groups) == 1 and [c['kind'] for c in cards.values()].count('Image') == 1"
    inner=$(query reopened_in '(groups or ["none"])[0]')
    send "DOUBLECLICK $(query reopened_in "xy('$inner')")"
    settle reopened_inner
    check_query reopened_inner "the nested group comes back with both tabs" \
        "s['current_group'] == '$inner' and len(tabs) == 2"
fi

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
