#!/usr/bin/env bash
# Usage: scripts/link-probe.sh
#
# Checks links end to end (spec 2026-10-06-links) through the real exe:
#   1. LINK a b between two tabs creates one link, a -> b, in EXPORTSTATE.
#   2. After a restart on the same profile the link is still there, same id.
#   3. UNLINK b a (the other order) removes it, and a second restart still
#      has none (the removal was saved).
#   4. In run 3, LINK again, then a CLICK on the curve's middle (from
#      EXPORTSTATE `edges`, clear of both cards) deletes it (spec
#      2026-10-07-link-gestures).
#   5. A MOUSEDOWN on a's right handle (`link_handles`), six MOUSEMOVEs and a
#      MOUSEUP on b links a to b, without moving a.
#   6. UNLINK, then the same drag released 15 points left of b's left edge
#      (inside the drop tolerance) links a to b again; EXPORTSTATE while the
#      button is down lists handles on b too (every card shows them).
#   7. Run 4 still has that link.
#   8. In run 4, a drag from the curve near b's end (EXPORTSTATE `near_to`) onto
#      the third tab c retargets the link in place, a -> c (same id; mid-drag
#      `connect_drag` is a and `retarget` the link); a drag from the c end
#      with Esc leaves it as it was. A second link a -> b dragged from its b
#      end onto c (already joined to a) is deleted, and a drag of the a -> c
#      link's end released on empty canvas deletes it. Mid-drag EXPORTSTATE's
#      `retarget_preview` says retarget (over c), refused (over the fixed a),
#      delete (over the duplicate's c, and over empty canvas). Run 5 has no link.
# One headless instance per run, on a fresh isolated profile seeded with two
# about:blank tabs (import gives them fresh ids, so the first run reads them
# from EXPORTSTATE), state read back through EXPORTSTATE. Runs BelowNormal.
# Build first: cargo build -p manifold.
set -u
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=timing.sh
. "$repo_root/scripts/timing.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
require_fresh_exe "$exe" || exit 1
timing_mark setup
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    timing_mark
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

profile="$out/appdata"
seed_settings "$profile"
cat >"$profile/seed.json" <<'EOF'
{"nodes": [
 {"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [0.0, 0.0]},
 {"id": "t2", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [400.0, 0.0]},
 {"id": "t3", "type": "application/vnd.manifold.tab", "url": "about:blank", "engine": "webview2", "pos": [0.0, 400.0]}
]}
EOF
"$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null || exit 1

cmd="$profile/cmd"
# start <log>: one instance on the profile, at BelowNormal, its tabs up.
start() {
    : >"$cmd"
    launch_headless "$exe" "$profile" "$cmd" "$out/$1"
    inst=$!
    send WAITTABS "SLEEP 1"
}
stop() { send QUIT; wait "$inst" 2>/dev/null; inst=""; }
x() { echo "EXPORTSTATE $(cygpath -w "$out/$1.json")"; }
py() { python -P -c "$1" "$(cygpath -w "$out/$2.json")" </dev/null 2>&1 | tr -d '\r'; }

timing_mark run1-link
start log1
send "$(x s0)"
wait_file "$out/s0.json"
# a, b, c by position (the seed's ids are replaced on import): a top left, b top right, c below a.
read -r ncards a b c <<<"$(py 'import json,sys; c=json.load(open(sys.argv[1], encoding="utf-8"))["cards"]; print(len(c), *[x["id"] for x in sorted(c, key=lambda x: (round(x["screen"][1] / 50), x["screen"][0]))])' s0)"
send "LINK $a $b" "SLEEP 1" "$(x s1)"
stop
timing_mark run2-restart-unlink
start log2
send "$(x s2)" "UNLINK $b $a" "SLEEP 1" "$(x s3)"
stop
timing_mark run3-gestures
start log3
send "$(x s4)" "LINK $a $b" "SLEEP 1" "$(x s5)"
wait_file "$out/s5.json"
# The link line's middle on screen (EXPORTSTATE edges), clear of both cards: a click there deletes it.
read -r lx ly <<<"$(py 'import json,sys; e=[e for e in json.load(open(sys.argv[1], encoding="utf-8"))["edges"] if e["kind"]=="link"][0]["screen"]; print(round(e[0]), round(e[1]))' s5)"
send "MOUSEMOVE $lx $ly" "CLICK $lx $ly" "SLEEP 1" "$(x s6)"
wait_file "$out/s6.json"
# a's centre, a's right handle and b's centre, then a drag from the handle onto b in six steps.
read -r ax ay hx hy bx by <<<"$(py "import json,sys; d=json.load(open(sys.argv[1], encoding='utf-8')); c={x['id']: x['screen'] for x in d['cards']}; r=d['link_handles']['$a']['right']; print(*[round(v) for v in (*c['$a'], (r[0]+r[2])/2, (r[1]+r[3])/2, *c['$b'])])" s6)"
send "MOUSEMOVE $ax $ay" "MOUSEMOVE $hx $hy" "MOUSEMOVE $hx $hy" "MOUSEDOWN $hx $hy"
for i in 1 2 3 4 5 6; do send "MOUSEMOVE $((hx + (bx - hx) * i / 6)) $((hy + (by - hy) * i / 6))"; done
send "MOUSEUP $bx $by" "SLEEP 1" "$(x s7)"
wait_file "$out/s7.json"
# The same drag, released 15 points left of b's left edge: within the drop tolerance of b, off every card.
read -r nx ny <<<"$(py "import json,sys; d=json.load(open(sys.argv[1], encoding='utf-8')); r=[c for c in d['cards'] if c['id']=='$b'][0]['rect']; print(round(r[0]) - 15, round((r[1]+r[3])/2))" s7)"
send "UNLINK $a $b" "SLEEP 1" "MOUSEMOVE $ax $ay" "MOUSEMOVE $hx $hy" "MOUSEMOVE $hx $hy" "MOUSEDOWN $hx $hy"
for i in 1 2 3 4 5 6; do send "MOUSEMOVE $((hx + (nx - hx) * i / 6)) $((hy + (ny - hy) * i / 6))"; done
send "SLEEP 1" "$(x s7m)"
wait_file "$out/s7m.json"
send "MOUSEUP $nx $ny" "SLEEP 1" "$(x s7t)"
stop
timing_mark run4-persists
start log4
send "$(x s8)"
wait_file "$out/s8.json"
# drag_to X0 Y0 X1 Y1: hover (X0,Y0), press there and move to (X1,Y1) in six steps; the button stays down.
drag_to() {
    local i
    send "MOUSEMOVE $1 $2" "MOUSEMOVE $1 $2" "MOUSEDOWN $1 $2"
    for i in 1 2 3 4 5 6; do send "MOUSEMOVE $(($1 + ($3 - $1) * i / 6)) $(($2 + ($4 - $2) * i / 6))"; done
    send WAITIDLE
}
# edge_pt FILE KEY [TO]: the first link edge's (with TO: the first running to card TO) KEY point (screen, rounded);
# card_pt FILE ID: a card's centre.
edge_pt() { py "import json,sys; e=[e for e in json.load(open(sys.argv[1], encoding='utf-8'))['edges'] if e['kind']=='link' and ('${3:-}' in ('', e['to']))][0]['$2']; print(round(e[0]), round(e[1]))" "$1"; }
card_pt() { py "import json,sys; c=[c for c in json.load(open(sys.argv[1], encoding='utf-8'))['cards'] if c['id']=='$2'][0]['screen']; print(round(c[0]), round(c[1]))" "$1"; }
# Retarget (spec link-gestures, "Dragging a link's end"). The link is a -> b. Grabbed near b's end (t = 0.8) and
# dropped on c, its b end moves to c: same Link node, now a -> c. Mid-drag a is the fixed end.
read -r gx gy <<<"$(edge_pt s8 near_to)"
read -r cx cy <<<"$(card_pt s8 $c)"
drag_to "$gx" "$gy" "$cx" "$cy"
send "$(x s9m)"
wait_file "$out/s9m.json"
send "MOUSEUP $cx $cy" WAITIDLE "$(x s9)"
wait_file "$out/s9.json"
# Esc mid-drag: the c end pulled toward b, Esc, then the release on b: the link stays a -> c.
read -r gx gy <<<"$(edge_pt s9 near_to)"
read -r bx by <<<"$(card_pt s9 $b)"
drag_to "$gx" "$gy" "$bx" "$by"
# Over the fixed end's own card (a) the preview says the release is refused.
read -r fx fy <<<"$(card_pt s9 $a)"
send "MOUSEMOVE $fx $fy" WAITIDLE "$(x s10f)"
wait_file "$out/s10f.json"
send ESCAPE WAITIDLE "$(x s10m)"
wait_file "$out/s10m.json"
send "MOUSEUP $bx $by" WAITIDLE "$(x s10)"
wait_file "$out/s10.json"
# A second link a -> b, its b end dragged onto c: a already joins c, so the dragged link is deleted, a -> c stays.
send "LINK $a $b" "SLEEP 1" "$(x s10b)"
wait_file "$out/s10b.json"
read -r gx gy <<<"$(edge_pt s10b near_to $b)"
read -r cx cy <<<"$(card_pt s10b $c)"
drag_to "$gx" "$gy" "$cx" "$cy"
send "$(x s10dm)"
wait_file "$out/s10dm.json"
send "MOUSEUP $cx $cy" WAITIDLE "$(x s10d)"
wait_file "$out/s10d.json"
# Released on empty canvas (b's column, c's row, clear of every card): the dragged link is deleted.
read -r gx gy <<<"$(edge_pt s10d near_from)"
read -r _ ey <<<"$(card_pt s10d $c)"
drag_to "$gx" "$gy" "$bx" "$ey"
send "$(x s11m)"
wait_file "$out/s11m.json"
send "MOUSEUP $bx $ey" WAITIDLE "$(x s11)"
wait_file "$out/s11.json"
stop
start log5
send "$(x s12)"
stop

timing_mark verdicts
# Prints "<link count> <first link's id> <its from> <its to>" for one EXPORTSTATE file.
summary() {
    py 'import json,sys; l=json.load(open(sys.argv[1], encoding="utf-8"))["links"]; print(len(l), *((l[0]["id"], l[0]["from"], l[0]["to"]) if l else ("-", "-", "-")))' "$1"
}
status=0
read -r n1 id1 from1 to1 <<<"$(summary s1)"
read -r n2 id2 from2 to2 <<<"$(summary s2)"
read -r n3 _ <<<"$(summary s3)"
read -r n4 _ <<<"$(summary s4)"
# a's rect, rounded, for "the card did not move"; and one top-level EXPORTSTATE field.
rect_of() { py "import json,sys; print(*[round(v) for v in [c for c in json.load(open(sys.argv[1], encoding='utf-8'))['cards'] if c['id']=='$a'][0]['rect']])" "$1"; }
field() { py "import json,sys; print(json.load(open(sys.argv[1], encoding='utf-8'))['$2'])" "$1"; }
read -r n5 _ <<<"$(summary s5)"
read -r n6 _ <<<"$(summary s6)"
read -r n7 _ from7 to7 <<<"$(summary s7)"
read -r n7t id7t from7t to7t <<<"$(summary s7t)"
read -r n8 id8 _ <<<"$(summary s8)"
# Whether the link line's middle (s5) lies inside either card's rect, and how many cards list handles in s7m.
clear_of_cards() { py "import json,sys; d=json.load(open(sys.argv[1], encoding='utf-8')); x,y=[e for e in d['edges'] if e['kind']=='link'][0]['screen']; print('clear' if not any(r[0]<=x<=r[2] and r[1]<=y<=r[3] for r in (c['rect'] for c in d['cards'])) else 'on a card')" "$1"; }
handle_cards() { py "import json,sys; d=json.load(open(sys.argv[1], encoding='utf-8')); print(sorted(k for k,v in d['link_handles'].items() if v) == sorted(c['id'] for c in d['cards']))" "$1"; }
check "the session has the three seeded tabs" "3" "$ncards"
check "LINK creates one link" "1" "$n1"
check "it runs a -> b" "$a $b" "$from1 $to1"
check "the link persists across a restart" "1" "$n2"
check "with the same id and ends" "$id1 $a $b" "$id2 $from2 $to2"
check "UNLINK (ends in the other order) removes it" "0" "$n3"
check "the removal persists across a restart" "0" "$n4"
check "LINK again" "1" "$n5"
check "the link curve's middle is clear of both cards" "clear" "$(clear_of_cards s5)"
check "a click on the link curve's middle removes it" "0" "$n6"
check "a drag from a's handle onto b links a -> b" "1 $a $b" "$n7 $from7 $to7"
check "the card dragged from did not move" "$(rect_of s6)" "$(rect_of s7)"
check "the connect-drag ended" "None" "$(field s7 connect_drag)"
check "every card lists handles mid-drag" "True" "$(handle_cards s7m)"
check "a release 15 points outside b links a -> b" "1 $a $b" "$n7t $from7t $to7t"
check "the dragged link persists across a restart" "1 $id7t" "$n8 $id8"
read -r n9 id9 from9 to9 <<<"$(summary s9)"
read -r n10 id10 from10 to10 <<<"$(summary s10)"
read -r n10b _ <<<"$(summary s10b)"
read -r n10d id10d from10d to10d <<<"$(summary s10d)"
read -r n11 _ <<<"$(summary s11)"
read -r n12 _ <<<"$(summary s12)"
check "grabbed near b's end, a is the fixed end of the connect-drag" "$a" "$(field s9m connect_drag)"
check "and the link being retargeted is the one that was drawn" "$id8" "$(field s9m retarget)"
check "dropped on c, the link keeps its id and runs a -> c" "1 $id8 $a $c" "$n9 $id9 $from9 $to9"
check "over a valid card (c) the preview says retarget" "retarget" "$(field s9m retarget_preview)"
check "over the fixed card (a) the preview says refused" "refused" "$(field s10f retarget_preview)"
check "over the already-joined card (c) the preview says delete" "delete" "$(field s10dm retarget_preview)"
check "over empty canvas the preview says delete" "delete" "$(field s11m retarget_preview)"
check "the retarget ended" "None None" "$(field s9 connect_drag) $(field s9 retarget)"
check "Esc ends the drag mid-way" "None None" "$(field s10m connect_drag) $(field s10m retarget)"
check "Esc then a release on b leaves a -> c" "1 $id8 $a $c" "$n10 $id10 $from10 $to10"
check "a second link a -> b is made" "2" "$n10b"
check "its b end dropped on c (already joined to a) deletes it, a -> c stays" "1 $id8 $a $c" "$n10d $id10d $from10d $to10d"
check "a release on empty canvas deletes the dragged link" "0" "$n11"
check "the deletion persists across a restart" "0" "$n12"
if [ $status -ne 0 ]; then
    for l in log1 log2 log3 log4 log5; do echo "--- $l"; tail -n 20 "$out/$l"; done
fi
timing_mark
exit $status
