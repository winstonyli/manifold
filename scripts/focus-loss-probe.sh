#!/usr/bin/env bash
# Usage: scripts/focus-loss-probe.sh            (NO_STEAL=1: control run, expect FAILs)
#
# WARNING: steals the OS foreground for a few seconds. Keystrokes typed meanwhile go to the
# probe's off-screen windows; the window that had the foreground gets it back at the end
# (unless something else took it meanwhile, or it was a manifold window).
#
# Checks in the real exe (spec 2026-10-07-link-gestures, final review I2) that losing window
# focus mid connect-drag cancels it: winit's `WindowEvent::Focused(false)` reaches
# `App::on_window_focus`, which calls `cancel_connect`, so the button-up that went to another
# window cannot link when it, or the next ordinary click, lands on a card.
#   1. One headless instance on a fresh profile seeded with two about:blank tabs, a and b.
#   2. A MOUSEDOWN on a's right handle and MOUSEMOVEs halfway to b, then SLEEP 1:
#      `connect_drag` is a (the drag survives idle frames with the button held).
#   3. scripts/foreground.ps1, in one process: gives the instance's (off-screen) window the
#      OS foreground and checks GetForegroundWindow, then at once takes it for a throwaway
#      off-screen form, holds it 1 s, hands it back and closes the form. Refused: prints
#      SKIP and exits 0. EXPORTSTATE has no window-focus field, so the app's side of the
#      focus change is seen only through `connect_drag`.
#   4. EXPORTSTATE, polled: `connect_drag` is null and there are still no links.
#   5. A late MOUSEUP over b, then a MOUSEDOWN/MOUSEUP click on b: still no link. (With
#      NO_STEAL=1 the drag survives step 4 and the late MOUSEUP links a -> b.)
# egui keeps its pointer button down across a focus loss (it clears only keys), so the
# "connect-drag without its button down" path can't be what cancels it in step 4.
# Runs BelowNormal; kills only its own instance. Build first: cargo build -p manifold.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
require_fresh_exe "$exe" || exit 1
fg_ps1="$(cygpath -w "$repo_root/scripts/foreground.ps1")"
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && { winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"; [ -n "$winpid" ] && taskkill //F //T //PID "$winpid" >/dev/null 2>&1; }
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

profile="$out/appdata"
mkdir -p "$profile/manifold"
echo '{"has_shown_first_launch_demo": true, "default_starting_engine": "webview2"}' >"$profile/manifold/settings.json"
cat >"$profile/seed.json" <<'EOF'
{"nodes": [
 {"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [0.0, 0.0]},
 {"id": "t2", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [400.0, 0.0]}
]}
EOF
"$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null || exit 1

cmd="$profile/cmd"
send() { printf '%s\n' "$@" >>"$cmd"; }
: >"$cmd"
# `-n +1`: plain `tail -f` starts with only the last 10 lines.
APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$out/log" 2>&1 &
inst=$!
# Retried: right after the launch `ps -W` can list it without its Windows pid yet.
for _ in $(seq 20); do winpid="$(ps -W | awk -v p="$inst" '$1 == p { print $4 }')"; [ -n "$winpid" ] && break; sleep 0.25; done
[ -n "$winpid" ] && powershell -NoProfile -Command "try { (Get-Process -Id $winpid -ErrorAction Stop).PriorityClass = 'BelowNormal' } catch {}" </dev/null >/dev/null 2>&1
send WAITTABS "SLEEP 1"

x() { echo "EXPORTSTATE $(cygpath -w "$out/$1.json")"; }
wait_file() { for _ in $(seq 240); do [ -s "$1" ] && return 0; sleep 0.25; done; echo "timed out waiting for $1" >&2; return 1; }
py() { python -P -c "$1" "$(cygpath -w "$out/$2.json")" </dev/null 2>&1 | tr -d '\r'; }
# kv <line> <key>: the value of key=value in a helper's output line.
kv() { printf '%s\n' "$1" | tr ' ' '\n' | sed -n "s/^$2=//p"; }
status=0
check() { # name want got
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else echo "FAIL: $1: got [$3], wanted [$2]"; status=1; fi
}
field() { py "import json,sys; print(json.load(open(sys.argv[1], encoding='utf-8'))['$2'])" "$1"; }
nlinks() { py 'import json,sys; print(len(json.load(open(sys.argv[1], encoding="utf-8"))["links"]))' "$1"; }
finish() {
    if [ $status -ne 0 ]; then echo "--- log"; tail -n 20 "$out/log"; fi
    exit $status
}

[ -n "$winpid" ] || { echo "FAIL: no Windows pid for the instance"; status=1; finish; }
send "$(x s0)"
wait_file "$out/s0.json" && sleep 0.2
# The two cards (a the left one), a's centre, a's right handle, b's centre.
read -r ncards a b ax ay hx hy bx by <<<"$(py "import json,sys; d=json.load(open(sys.argv[1], encoding='utf-8')); c=sorted(d['cards'], key=lambda c: c['screen'][0]); a,b=c[0]['id'],c[1]['id']; r=d['link_handles'][a]['right']; print(len(c), a, b, *[round(v) for v in (*c[0]['screen'], (r[0]+r[2])/2, (r[1]+r[3])/2, *c[1]['screen'])])" s0)"
check "the session has the two seeded tabs" "2" "$ncards"

# Press a's right handle and drag halfway to b: over the canvas, button still down. Synthetic
# input, so the window needs no focus for this; it gets the foreground just before it loses it.
send "MOUSEMOVE $ax $ay" "MOUSEMOVE $hx $hy" "MOUSEMOVE $hx $hy" "MOUSEDOWN $hx $hy"
for i in 1 2 3; do send "MOUSEMOVE $((hx + (bx - hx) * i / 6)) $((hy + (by - hy) * i / 6))"; done
send "SLEEP 1" "$(x s1)"
wait_file "$out/s1.json" && sleep 0.2
check "the handle press started a connect-drag from a" "$a" "$(field s1 connect_drag)"
check "no links yet" "0" "$(nlinks s1)"

# Foreground to the instance's window, checked, then at once to a throwaway form, and back.
# NO_STEAL=1 skips it, as a control: the drag then survives, and the checks after this FAIL.
[ -n "${NO_STEAL:-}" ] && cycle="OK hwnd=0 before=0 lost=1 (NO_STEAL)" ||
    cycle="$(powershell -NoProfile -ExecutionPolicy Bypass -File "$fg_ps1" -ProcessId "$winpid" -HoldSeconds 1.0 </dev/null 2>&1 | tr -d '\r')"
case "$cycle" in
OK*) printf 'INFO: %s\n' "$cycle" ;;
*) echo "SKIP: could not take foreground: $cycle"; exit 0 ;;
esac
hwnd="$(kv "$cycle" hwnd)"
check "the instance's window had the foreground just before the steal" "$hwnd" "$(kv "$cycle" before)"
check "and lost it to the form" "1" "$(kv "$cycle" lost)"

# Poll until the cancel shows (Focused(false) is a window message; commands come through stdin).
drag=""
for i in $(seq 8); do
    send "$(x s2_$i)"
    wait_file "$out/s2_$i.json" && sleep 0.1
    drag="$(field "s2_$i" connect_drag)"
    [ "$drag" = "None" ] && break
    sleep 0.25
done
check "losing focus cancelled the connect-drag" "None" "$drag"
check "and linked nothing" "0" "$(nlinks "s2_$i")"

# The button-up that went elsewhere, arriving late over b: with the drag still live (NO_STEAL) this links a -> b.
send "MOUSEMOVE $bx $by" "MOUSEUP $bx $by" "SLEEP 1" "$(x s3)"
wait_file "$out/s3.json" && sleep 0.2
check "a late button-up over b links nothing" "0 None" "$(nlinks s3) $(field s3 connect_drag)"
# Then an ordinary click on b. (Not a discriminating check alone: a press-release without movement on a live
# connect-drag is a click, which cancels it too.)
send "MOUSEDOWN $bx $by" "MOUSEUP $bx $by" "SLEEP 1" "$(x s4)"
wait_file "$out/s4.json" && sleep 0.2
check "a click on b after the focus loss links nothing" "0 None" "$(nlinks s4) $(field s4 connect_drag)"
send QUIT
wait "$inst" 2>/dev/null
inst=""
finish
