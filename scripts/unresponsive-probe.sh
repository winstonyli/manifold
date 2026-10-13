#!/usr/bin/env bash
# Usage: scripts/unresponsive-probe.sh
#
# Checks "not responding" on WebView2 (wave 3b, survey E1). A seeded tab is opened, its engine host is suspended
# (NtSuspendProcess on the instance's own child, found by parent pid), and 1000 characters are typed into the page:
# the app's writer for it blocks and its queue fills. EXPORTSTATE must then read the card `unresponsive` with a
# "Not responding" toast, while the app keeps answering commands. `RESTARTVIEW id` must bring the tab back live and
# responsive and end the suspended host. Last, the new host is suspended too and `QUIT` must end the app within 10 s:
# closing a stuck tab never waits on it.
# One headless instance on a fresh isolated APPDATA. Build first:
# cargo build -j 4 -p manifold -p manifold-view-webview2.
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
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"
# Not under the repo: a path with a space (this one has "Winston Li") stops the app reading its commands.
out="$(mktemp -d)"
inst=""
cleanup() {
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "${KEEP:-}" ] || rm -rf "$out"
}
trap cleanup EXIT

# Suspends the WebView2 engine hosts whose parent is Windows pid $1 and prints their pids.
cat >"$out/suspend.ps1" <<'PS'
param([int]$Parent)
Add-Type -Namespace P -Name N -MemberDefinition '[DllImport("ntdll.dll")] public static extern int NtSuspendProcess(IntPtr h);'
Get-CimInstance Win32_Process -Filter "ParentProcessId=$Parent AND Name='manifold-engine-webview2.exe'" | ForEach-Object {
    $null = [P.N]::NtSuspendProcess((Get-Process -Id $_.ProcessId).Handle)
    $_.ProcessId
}
PS
suspend_engine() {
    powershell -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$out/suspend.ps1")" -Parent "$1" </dev/null | tr -d '\r'
}
# card FILE ID FIELD: that card's field in an exported state ("" if absent).
card() {
    python -P -c 'import json,sys; c = [x for x in json.load(open(sys.argv[1], encoding="utf-8"))["cards"] if x["id"] == sys.argv[2]]; print(str(c[0].get(sys.argv[3], "")).lower() if c else "")' "$(cygpath -w "$1")" "$2" "$3" </dev/null
}
# toast FILE: the exported toast ("" if none).
toast() {
    python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["toast"] or "")' "$(cygpath -w "$1")" </dev/null
}
st() { cygpath -w "$out/$1.json"; }

status=0
profile="$out/appdata"
seed_settings "$profile" webview2
echo '{"nodes": [{"id": "t1", "type": "application/vnd.manifold.tab", "url": "about:blank", "view_kind": "webview2", "pos": [0.0, 0.0]}]}' >"$profile/seed.json"
"$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null
cmd="$profile/cmd"
: >"$cmd"
launch_headless "$exe" "$profile" "$cmd" "$profile/log"
inst=$!
w="$launched_winpid"
[ -n "$w" ] || { echo "FAIL: no Windows pid for the instance"; exit 1; }

send WAITTABS "SLEEP 4" "EXPORTSTATE $(st live)"
wait_file "$out/live.json" || exit 1
# The import may give the node a new id (sleep-probe.sh reads it the same way): the one card's id.
id="$(python -P -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["cards"][0]["id"])' "$(cygpath -w "$out/live.json")" </dev/null)"
check "the tab is live" "true" "$(card "$out/live.json" "$id" loaded)"

# Into Tab view, so typing reaches the page; then suspend its engine.
send GRAPHDOWN GRAPHOPEN "SLEEP 4" "EXPORTSTATE $(st open)"
wait_file "$out/open.json" || exit 1
first="$(suspend_engine "$w")"
[ -n "$first" ] || { echo "FAIL: no engine host to suspend"; exit 1; }
# The stall reads after UNRESPONSIVE_AFTER (2 s) of a blocked write, so wait 3 s; then back in Graph view before exporting
# (the cards are the graph's), and within the toast's 4 s.
send "TYPE $(printf 'a%.0s' $(seq 1000))" "SLEEP 3" ESCAPE "SLEEP 1" "EXPORTSTATE $(st stuck)" "SCREENSHOT $(cygpath -w "$out/stuck.png")"
wait_file "$out/stuck.json" || exit 1
check "the card reads unresponsive" "true" "$(card "$out/stuck.json" "$id" unresponsive)"
case "$(toast "$out/stuck.json")" in
"Not responding: "*) echo "PASS: the toast says not responding" ;;
*) echo "FAIL: toast: got [$(toast "$out/stuck.json")]"; status=1 ;;
esac

send "RESTARTVIEW $id" WAITTABS "SLEEP 4" "EXPORTSTATE $(st restarted)"
wait_file "$out/restarted.json" || exit 1
check "restarted: live" "true" "$(card "$out/restarted.json" "$id" loaded)"
check "restarted: responsive" "false" "$(card "$out/restarted.json" "$id" unresponsive)"
gone="$(powershell -NoProfile -Command "if (Get-Process -Id $first -ErrorAction SilentlyContinue) { 'alive' } else { 'gone' }" </dev/null | tr -d '\r')"
check "the suspended host was ended" "gone" "$gone"

# A second stuck engine, then quit: exit must not wait on it.
send GRAPHDOWN GRAPHOPEN "SLEEP 4" "EXPORTSTATE $(st open2)"
wait_file "$out/open2.json" || exit 1
[ -n "$(suspend_engine "$w")" ] || { echo "FAIL: no second engine host to suspend"; exit 1; }
send "TYPE $(printf 'b%.0s' $(seq 1000))" "SLEEP 3" ESCAPE "SLEEP 1" "EXPORTSTATE $(st stuck2)"
wait_file "$out/stuck2.json" || exit 1
check "the second host is stuck too" "true" "$(card "$out/stuck2.json" "$id" unresponsive)"
send QUIT
quit_at=$(date +%s)
for _ in $(seq 60); do kill -0 "$inst" 2>/dev/null || break; sleep 0.25; done
if kill -0 "$inst" 2>/dev/null; then
    echo "FAIL: still running 15 s after QUIT with a stuck tab"
    status=1
else
    check "QUIT with a stuck tab ends within 10 s" "yes" "$([ $(($(date +%s) - quit_at)) -le 10 ] && echo yes || echo no)"
    inst=""
    echo "quit took about $(($(date +%s) - quit_at)) s"
fi
echo "screenshot: $out/stuck.png (KEEP=1 keeps it)"
exit $status
