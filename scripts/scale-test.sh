#!/usr/bin/env bash
# Usage: [ENGINE=cef] scripts/scale-test.sh <tabs>...
#
# What a big session costs at startup. For each <tabs>, one headless
# instance on a fresh isolated APPDATA (never the real %APPDATA%\manifold)
# restores <tabs> tabs of a local page (scripts/page_server.py) and prints:
#   - how long until the restore settled (WAITTABS), checking the focused
#     tab is live and the rest asleep (spec "Lazy tabs");
#   - 15 s later, its process tree: process count, private memory and
#     working set, and private memory per process name.
# ENGINE (default webview2) is the engine every tab uses. Build first:
# cargo build -j 12 -p manifold. Each tab opened costs ~80 MB on WebView2
# and ~180 MB on CEF (README, known limitations).
set -euo pipefail
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
exe="$repo_root/target/debug/manifold.exe"
# shellcheck source=fresh-exe.sh
. "$repo_root/scripts/fresh-exe.sh"
# shellcheck source=winpid.sh
. "$repo_root/scripts/winpid.sh"
# shellcheck source=lib.sh
. "$repo_root/scripts/lib.sh"
require_fresh_exe "$exe"
[ $# -gt 0 ] || { sed -n 2p "$0"; exit 1; }
# The inline python below reads EXPORTSTATE cards through scripts/export_state.py.
export PYTHONPATH="$(cygpath -w "$repo_root/scripts")"
engine="${ENGINE:-webview2}"
out="$repo_root/target/scale-test-$$"
mkdir -p "$out"
srv=""
inst=""
cleanup() {
    local status=$?
    [ -n "$inst" ] && kill_tree "$inst"
    [ -n "$srv" ] && kill "$srv" 2>/dev/null
    rm -rf "$out"
    exit $status
}
trap cleanup EXIT

python -P "$repo_root/scripts/page_server.py" "$out/port" 3600 </dev/null >/dev/null 2>&1 &
srv=$!
for _ in $(seq 50); do [ -s "$out/port" ] && break; sleep 0.1; done
port="$(cat "$out/port")"

# Memory of the process tree rooted at a Windows pid.
tree_memory() {
    ROOT_PID="$1" powershell -NoProfile -Command '
        $root = [int]$env:ROOT_PID
        if ($root -le 0) { "  (no pid)"; exit 1 }
        $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
        $ids = @($root); $i = 0
        while ($i -lt $ids.Count) { $p = $ids[$i]; $ids += @($all | Where-Object { $_.ParentProcessId -eq $p -and $ids -notcontains $_.ProcessId } | ForEach-Object { $_.ProcessId }); $i++ }
        $procs = $ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }
        $mb = { param($g) [math]::Round((($g | Measure-Object PrivateMemorySize64 -Sum).Sum) / 1MB) }
        $names = $procs | Group-Object ProcessName | ForEach-Object { "{0} x{1} {2} MB" -f $_.Name, $_.Count, (& $mb $_.Group) }
        "  {0} processes, {1} MB private, {2} MB working set" -f $procs.Count, (& $mb $procs), [math]::Round((($procs | Measure-Object WorkingSet64 -Sum).Sum) / 1MB)
        $names | ForEach-Object { "    $_" }
    ' | tr -d '\r'
}

for n in "$@"; do
    profile="$out/appdata-$n"
    seed_settings "$profile" "$engine"
    {
        printf '{"nodes": ['
        for i in $(seq 1 "$n"); do
            [ "$i" -gt 1 ] && printf ','
            printf '{"id": "t%d", "type": "application/vnd.manifold.tab", "url": "http://127.0.0.1:%s/p?i=%d", "engine": "%s", "pos": [%d.0, %d.0]}' \
                "$i" "$port" "$i" "$engine" $((i % 10 * 300)) $((i / 10 * 200))
        done
        printf ']}'
    } >"$profile/seed.json"
    "$exe" --import-json "$(cygpath -w "$profile/seed.json")" --out "$(cygpath -w "$profile/manifold/session.mnf")" >/dev/null

    cmd="$profile/cmd"
    : >"$cmd"
    spawned="$profile/spawned.json"
    start=$(date +%s.%N)
    # Commands come from a tail that exits with this script.
    APPDATA="$(cygpath -w "$profile")" "$exe" --headless < <(exec tail -n +1 -f --pid=$$ "$cmd" 2>/dev/null) >"$profile/log" 2>&1 &
    inst=$!
    echo WAITTABS >>"$cmd"
    echo "EXPORTSTATE $(cygpath -w "$spawned")" >>"$cmd"
    for _ in $(seq 1200); do [ -s "$spawned" ] && break; sleep 0.1; done
    [ -s "$spawned" ] || { echo "FAIL: $n tabs: WAITTABS never answered in 120 s"; exit 1; }
    elapsed=$(awk -v a="$start" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }')
    # The tab cards with a running engine, and the sleeping ones.
    read -r live asleep < <(python -P -c 'import json,sys; from export_state import *; c = json.load(open(sys.argv[1], encoding="utf-8"))["cards"]; print(sum(card_is(x, TAB, True) for x in c), sum(card_is(x, TAB, False) for x in c))' "$(cygpath -w "$spawned")" </dev/null | tr -d '\r')
    [ "$live" -eq 1 ] && [ $((live + asleep)) -eq "$n" ] ||
        { echo "FAIL: $n tabs: $live live and $asleep asleep, expected 1 and $((n - 1))"; exit 1; }
    winpid="$(winpid_of "$inst")" || { echo "FAIL: $n tabs: no Windows pid for the instance, so no memory figure"; exit 1; }
    echo "$engine, $n tabs: restored in ${elapsed}s, 1 live and $asleep asleep"
    sleep 15
    tree_memory "$winpid"
    echo QUIT >>"$cmd"
    wait "$inst" 2>/dev/null || true
    inst=""
done
