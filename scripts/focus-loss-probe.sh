#!/usr/bin/env bash
# Usage: [CASES="connect card"] scripts/focus-loss-probe.sh     (NO_STEAL=1: control run, expect FAILs)
#
# WARNING: steals the OS foreground for a few seconds per case. Keystrokes typed meanwhile go to the
# probe's off-screen windows; the window that had the foreground gets it back at the end
# (unless something else took it meanwhile, or it was a manifold window).
#
# Checks in the real exe (spec 2026-10-07-link-gestures, final review I2 and Task 2 M3) that losing
# window focus mid-drag ends the drag: winit's `WindowEvent::Focused(false)` reaches
# `App::on_window_focus`, so the button-up that went to another window cannot act when it arrives.
# Each case is one headless instance on its own fresh profile seeded with two about:blank tabs, a
# (left) and b.
#   connect: a MOUSEDOWN on a's right handle and MOUSEMOVEs halfway to b, then SLEEP 1:
#      `connect_drag` is a (the drag survives idle frames with the button held). The steal (below);
#      then, polled, `connect_drag` is null and there are no links. A late MOUSEUP over b, then a
#      MOUSEDOWN/MOUSEUP click on b: still no link. (With NO_STEAL=1 the drag survives and the late
#      MOUSEUP links a -> b.)
#   card: a MOUSEDOWN on a's centre and MOUSEMOVEs until a's centre is over b, then SLEEP 1:
#      `dragging` is a and a moved. The steal; then, polled, `dragging` is null and a's document pos
#      was saved (`mirror_pos`, null for the seed's (0,0)). MOUSEMOVEs down and away with the button
#      still "down" (egui keeps it down across a focus loss): a stays put. Then back over b and a late
#      MOUSEUP: a and b are still in no group (a drop of a tab on a tab groups them) and a hasn't
#      moved. (With NO_STEAL=1 the drag survives, a follows the pointer and the late MOUSEUP groups
#      them.)
# The steal: scripts/foreground.ps1, in one process, gives the instance's (off-screen) window the OS
# foreground and checks GetForegroundWindow, then at once takes it for a throwaway off-screen form,
# holds it 1 s, hands it back and closes the form. Refused: that case prints SKIP. EXPORTSTATE has
# no window-focus field, so the app's side of the focus change is seen only through the drag state.
# Runs BelowNormal; kills only its own instances. Build first: cargo build -p manifold.
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
fg_ps1="$(cygpath -w "$repo_root/scripts/foreground.ps1")"
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

# The case's working dir; set by `launch`.
dir=""
cmd=""
x() { echo "EXPORTSTATE $(cygpath -w "$dir/$1.json")"; }
py() { python -P -c "$1" "$(cygpath -w "$dir/$2.json")" </dev/null 2>&1 | tr -d '\r'; }
# kv <line> <key>: the value of key=value in a helper's output line.
kv() { printf '%s\n' "$1" | tr ' ' '\n' | sed -n "s/^$2=//p"; }
status=0
check() { # name want got
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else echo "FAIL: $1: got [$3], wanted [$2]"; status=1; fi
}
field() { py "import json,sys; print(json.load(open(sys.argv[1], encoding='utf-8'))['$2'])" "$1"; }
nlinks() { py 'import json,sys; print(len(json.load(open(sys.argv[1], encoding="utf-8"))["links"]))' "$1"; }

# launch <case>: a fresh profile seeded with the two tabs, and one instance on it at BelowNormal.
launch() {
    dir="$out/$1"
    local profile="$dir/appdata"
    seed_settings "$profile"
    cat >"$profile/seed.json" <<'EOF'
{"nodes": [
 {"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [0.0, 0.0]},
 {"id": "t2", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [400.0, 0.0]}
]}
EOF
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null || return 1
    cmd="$dir/cmd"
    : >"$cmd"
    launch_headless "$exe" "$profile" "$cmd" "$dir/log"
    inst=$!
    winpid="$launched_winpid" # the steal needs the Windows pid; launch_headless looked it up
    send WAITTABS "SLEEP 1"
    [ -n "$winpid" ] || { echo "FAIL: $1: no Windows pid for the instance"; status=1; return 1; }
}
quit() { send QUIT; wait "$inst" 2>/dev/null; inst=""; }
dump_log() { echo "--- log ($1)"; tail -n 20 "$dir/log"; }

# The two cards (a the left one) from <state>: "count a b ax ay hx hy bx by" (a's centre, a's right handle, b's centre).
cards() {
    py "import json,sys; d=json.load(open(sys.argv[1], encoding='utf-8')); c=sorted(d['cards'], key=lambda c: c['screen'][0]); a,b=c[0]['id'],c[1]['id']; r=d['link_handles'][a]['right']; print(len(c), a, b, *[round(v) for v in (*c[0]['screen'], (r[0]+r[2])/2, (r[1]+r[3])/2, *c[1]['screen'])])" "$1"
}

# steal <case>: the foreground cycle; returns 1 (after printing SKIP) if Windows refused it.
steal() {
    local cycle hwnd
    # NO_STEAL=1 skips it, as a control: the drag then survives, and the checks after this FAIL.
    [ -n "${NO_STEAL:-}" ] && cycle="OK hwnd=0 before=0 lost=1 (NO_STEAL)" ||
        cycle="$(powershell -NoProfile -ExecutionPolicy Bypass -File "$fg_ps1" -ProcessId "$winpid" -HoldSeconds 1.0 </dev/null 2>&1 | tr -d '\r')"
    case "$cycle" in
    OK*) printf 'INFO: %s: %s\n' "$1" "$cycle" ;;
    *) echo "SKIP: $1: could not take foreground: $cycle"; return 1 ;;
    esac
    hwnd="$(kv "$cycle" hwnd)"
    check "$1: the instance's window had the foreground just before the steal" "$hwnd" "$(kv "$cycle" before)"
    check "$1: and lost it to the form" "1" "$(kv "$cycle" lost)"
}

# poll_cleared <prefix> <field>: EXPORTSTATEs until <field> is null (Focused(false) is a window message;
# commands come through stdin). Prints the last state's name.
poll_cleared() {
    local i
    for i in $(seq 8); do
        send "$(x "$1_$i")"
        wait_file "$dir/$1_$i.json" >&2
        [ "$(field "$1_$i" "$2")" = "None" ] && break
        sleep 0.25
    done
    echo "$1_$i"
}

case_connect() {
    launch connect || return
    send "$(x s0)"
    wait_file "$dir/s0.json"
    read -r ncards a b ax ay hx hy bx by <<<"$(cards s0)"
    check "connect: the session has the two seeded tabs" "2" "$ncards"

    # Press a's right handle and drag halfway to b: over the canvas, button still down. Synthetic
    # input, so the window needs no focus for this; it gets the foreground just before it loses it.
    send "MOUSEMOVE $ax $ay" "MOUSEMOVE $hx $hy" "MOUSEMOVE $hx $hy" "MOUSEDOWN $hx $hy"
    for i in 1 2 3; do send "MOUSEMOVE $((hx + (bx - hx) * i / 6)) $((hy + (by - hy) * i / 6))"; done
    send "SLEEP 1" "$(x s1)"
    wait_file "$dir/s1.json"
    check "connect: the handle press started a connect-drag from a" "$a" "$(field s1 connect_drag)"
    check "connect: no links yet" "0" "$(nlinks s1)"

    steal connect || { quit; return; }
    local s2
    s2="$(poll_cleared s2 connect_drag)"
    check "connect: losing focus cancelled the connect-drag" "None" "$(field "$s2" connect_drag)"
    check "connect: and linked nothing" "0" "$(nlinks "$s2")"

    # The button-up that went elsewhere, arriving late over b: with the drag still live (NO_STEAL) this links a -> b.
    send "MOUSEMOVE $bx $by" "MOUSEUP $bx $by" "SLEEP 1" "$(x s3)"
    wait_file "$dir/s3.json"
    check "connect: a late button-up over b links nothing" "0 None" "$(nlinks s3) $(field s3 connect_drag)"
    # Then an ordinary click on b. (Not a discriminating check alone: a press-release without movement on a live
    # connect-drag is a click, which cancels it too.)
    send "MOUSEDOWN $bx $by" "MOUSEUP $bx $by" "SLEEP 1" "$(x s4)"
    wait_file "$dir/s4.json"
    check "connect: a click on b after the focus loss links nothing" "0 None" "$(nlinks s4) $(field s4 connect_drag)"
    quit
}

case_card() {
    launch card || return
    send "$(x c0)"
    wait_file "$dir/c0.json"
    read -r ncards a b ax ay _ _ bx by <<<"$(cards c0)"
    check "card: the session has the two seeded tabs" "2" "$ncards"
    # a's screen rect, rounded; its saved document pos; both cards' groups.
    rect_of() { py "import json,sys; print(*[round(v) for v in [c for c in json.load(open(sys.argv[1], encoding='utf-8'))['cards'] if c['id']=='$a'][0]['rect']])" "$1"; }
    pos_of() { py "import json,sys; print(json.load(open(sys.argv[1], encoding='utf-8'))['mirror_pos']['$a'])" "$1"; }
    groups() { py "import json,sys; g=json.load(open(sys.argv[1], encoding='utf-8'))['mirror_groups']; print(g['$a'], g['$b'])" "$1"; }
    check "card: no groups to start with" "None None" "$(groups c0)"

    # Press a's centre and drag until its centre is over b (a release now would group them).
    send "MOUSEMOVE $ax $ay" "MOUSEMOVE $ax $ay" "MOUSEDOWN $ax $ay"
    for i in 1 2 3 4 5 6; do send "MOUSEMOVE $((ax + (bx - ax) * i / 6)) $((ay + (by - ay) * i / 6))"; done
    send "SLEEP 1" "$(x c1)"
    wait_file "$dir/c1.json"
    check "card: the press started a card drag of a" "$a" "$(field c1 dragging)"
    [ "$(rect_of c0)" != "$(rect_of c1)" ] && moved=1 || moved=0
    check "card: a follows the pointer" "1" "$moved"

    steal card || { quit; return; }
    local c2
    c2="$(poll_cleared c2 dragging)"
    check "card: losing focus ended the card drag" "None" "$(field "$c2" dragging)"
    [ "$(pos_of "$c2")" != "$(pos_of c0)" ] && saved=1 || saved=0
    check "card: a's pos was saved where it was released ($(pos_of "$c2"))" "1" "$saved"
    check "card: and grouped nothing" "None None" "$(groups "$c2")"

    # The button-up went elsewhere, so the button is still down for the app: the pointer moves on, down and away.
    for i in 1 2 3; do send "MOUSEMOVE $bx $((by + 60 * i))"; done
    send "SLEEP 1" "$(x c3)"
    wait_file "$dir/c3.json"
    check "card: a stays put while the stale button is down" "$(rect_of "$c2")" "$(rect_of c3)"
    # Back over b and the late button-up: with the drag still live (NO_STEAL) this drops a on b, grouping them.
    send "MOUSEMOVE $bx $by" "MOUSEUP $bx $by" "SLEEP 1" "$(x c4)"
    wait_file "$dir/c4.json"
    check "card: the late button-up over b grouped nothing" "None None" "$(groups c4)"
    check "card: a did not move" "$(rect_of "$c2")" "$(rect_of c4)"
    check "card: no drag left" "None" "$(field c4 dragging)"
    quit
}

for c in ${CASES:-connect card}; do
    before=$status
    "case_$c"
    [ $status -ne $before ] && dump_log "$c"
done
exit $status
