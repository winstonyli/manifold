#!/usr/bin/env bash
# Usage: scripts/gate.sh LOG [full|light|ui]    (scripts/gate-tier.sh says which tier a change needs)
#
# The pre-merge gate: sequential, so load from other sessions is the only thing that stretches it.
#   full  (default): two-instance-test x3 + headless-smoke-test x1 + launch-forms-probe + link-probe. Use for any
#                    change to sync, the store, the mirror, session save/restore or the engines. ~FULL_MIN min on a
#                    quiet machine (measured DATE).
#   light:           two-instance-test x1 + headless-smoke-test x1 + launch-forms-probe + link-probe. Allowed when
#                    only UI code that writes the store or reaches an engine changed (gestures, menus), or scripts:
#                    ~LIGHT_MIN min quiet. The repeats in `full` exist to catch flaky ordering in sync, which
#                    UI-only changes cannot reach.
#   ui:              `cargo test` of the three CI crates + link-probe. For changes to drawing, input and view-state
#                    modules that import no store, sync or network module (the list is in gate-tier.sh), and docs:
#                    ~UI_MIN min quiet.
# Build first (`cargo build -j 8 -p manifold -p manifold-engine-webview2`; `-p manifold` alone leaves the WebView2 host
# old): every probe stage runs the existing exes. Launch detached and low priority:
#   cmd //c start "" //b //belownormal bash scripts/gate.sh LOG [mode]
# Progress and verdicts go to LOG; each stage's own output to LOG.two-N.log, LOG.smoke.log, LOG.launch-forms.log,
# LOG.link.log and LOG.unit.log. After each stage LOG gets its TIMING lines (see scripts/gate-timing.sh) and at the end
# the slowest ten; LOG.cpu holds the machine's CPU load sampled every few seconds while it ran. Running the runs in
# parallel was tried (2026-10-07) and was worse: 53 min, all four runs failed on timing waits under load.
cd "$(dirname "$0")/.." || exit 1
out="$1"
mode="${2:-full}"
case "$mode" in full) runs=3 ;; light) runs=1 ;; ui) runs=0 ;; *) echo "mode must be full, light or ui" >&2; exit 2 ;; esac
[ -n "$out" ] || { echo "usage: scripts/gate.sh LOG [full|light|ui]" >&2; exit 2; }
cpu() { powershell -NoProfile -Command "[math]::Round((Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue,1)" 2>/dev/null | tr -d '\r'; }
defender() { powershell -NoProfile -Command "(Get-MpComputerStatus).RealTimeProtectionEnabled" 2>/dev/null | tr -d '\r'; }

# The CPU sampler runs until the stop file appears (the EXIT trap makes it) or its own 90-minute limit.
stop="$out.cpu-stop"
rm -f "$stop" "$out.cpu"
powershell -NoProfile -File "$(cygpath -aw scripts/cpu-sampler.ps1)" -Out "$(cygpath -aw "$out.cpu")" -Stop "$(cygpath -aw "$stop")" \
    </dev/null >/dev/null 2>&1 &
sampler=$!
end_sampler() {
    touch "$stop"
    for _ in $(seq 20); do kill -0 "$sampler" 2>/dev/null || break; sleep 0.5; done
    rm -f "$stop"
}
trap end_sampler EXIT

# stage NAME LOGFILE CMD...: runs CMD with its output in LOGFILE, then writes the stage's verdict line, its own TIMING
# line (gate/NAME) and the TIMING lines CMD printed, each with the CPU load around it.
stage() {
    local name=$1 log=$2 t0=$SECONDS s0 rc pf=""
    shift 2
    s0=$(date +%s)
    echo "GATE $name start $(date +%H:%M:%S)" >>"$out"
    "$@" >"$log" 2>&1
    rc=$?
    grep -qE 'PASS|FAIL' "$log" && pf=" pass=$(grep -c PASS "$log") fail=$(grep -c FAIL "$log")"
    echo "GATE $name exit=$rc end $(date +%H:%M:%S) took=$((SECONDS - t0))s last: $(grep -v -e '^TIMING ' -e '^[[:space:]]*$' "$log" | tail -1)$pf" >>"$out"
    printf 'TIMING gate/%s %d %d\n' "$name" $((SECONDS - t0)) "$s0" >>"$out"
    bash scripts/gate-timing.sh collect "$name" "$log" "$out.cpu" >>"$out"
}

echo "GATE start ($mode) $(date +%H:%M:%S) cpu=$(cpu)% defender_rt=$(defender)" >>"$out"
if [ "$mode" = ui ]; then
    stage unit "$out.unit.log" cargo test -j 8 -p manifold -p ipc-protocol -p manifold-engine-webview2
fi
for i in $(seq "$runs"); do
    stage "two-$i" "$out.two-$i.log" bash scripts/two-instance-test.sh
done
if [ "$mode" != ui ]; then
    stage smoke "$out.smoke.log" bash scripts/headless-smoke-test.sh
    stage launch-forms "$out.launch-forms.log" bash scripts/launch-forms-probe.sh
fi
stage link "$out.link.log" bash scripts/link-probe.sh
echo "GATE slowest steps (seconds, cpu at start, mean cpu):" >>"$out"
bash scripts/gate-timing.sh top "$out" 10 >>"$out"
echo "GATE done $(date +%H:%M:%S) cpu=$(cpu)% defender_rt=$(defender)" >>"$out"
