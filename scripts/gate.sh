#!/usr/bin/env bash
# Usage: scripts/gate.sh LOG [full|light]
#
# The pre-merge gate: sequential, so load from other sessions is the only thing that stretches it.
#   full  (default): two-instance-test x3 + headless-smoke-test x1 + launch-forms-probe + link-probe. Use for any
#                    change to sync, the store, the mirror, session save/restore or the engines. ~7 min on a
#                    quiet machine, ~12 under load (measured 2026-10-08).
#   light:           two-instance-test x1 + headless-smoke-test x1 + launch-forms-probe + link-probe. Allowed when
#                    only UI code changed (drawing, gestures, menus, scripts, docs): ~4 min quiet. The repeats in `full` exist to catch flaky
#                    ordering in sync, which UI-only changes cannot reach.
# Build first (`cargo build -j 8 -p manifold`): every stage runs the existing exe. Launch detached and low priority:
#   cmd //c start "" //b //belownormal bash scripts/gate.sh LOG [mode]
# Progress and verdicts go to LOG; each run's own output to LOG.two-N.log, LOG.smoke.log, LOG.launch-forms.log and LOG.link.log. Running the runs in
# parallel was tried (2026-10-07) and was worse: 53 min, all four runs failed on timing waits under load.
cd "$(dirname "$0")/.." || exit 1
out="$1"
mode="${2:-full}"
case "$mode" in full) runs=3 ;; light) runs=1 ;; *) echo "mode must be full or light" >&2; exit 2 ;; esac
[ -n "$out" ] || { echo "usage: scripts/gate.sh LOG [full|light]" >&2; exit 2; }
cpu() { powershell -NoProfile -Command "[math]::Round((Get-Counter '\Processor(_Total)\% Processor Time' -SampleInterval 2 -MaxSamples 2).CounterSamples[-1].CookedValue,1)" 2>/dev/null | tr -d '\r'; }
defender() { powershell -NoProfile -Command "(Get-MpComputerStatus).RealTimeProtectionEnabled" 2>/dev/null | tr -d '\r'; }
echo "GATE start ($mode) $(date +%H:%M:%S) cpu=$(cpu)% defender_rt=$(defender)" >> "$out"
for i in $(seq "$runs"); do
  echo "GATE two-instance run $i start $(date +%H:%M:%S) cpu=$(cpu)%" >> "$out"
  bash scripts/two-instance-test.sh > "$out.two-$i.log" 2>&1
  rc=$?
  echo "GATE two-instance run $i exit=$rc end $(date +%H:%M:%S) last: $(tail -1 "$out.two-$i.log")" >> "$out"
done
echo "GATE smoke start $(date +%H:%M:%S) cpu=$(cpu)%" >> "$out"
bash scripts/headless-smoke-test.sh > "$out.smoke.log" 2>&1
rc=$?
echo "GATE smoke exit=$rc end $(date +%H:%M:%S) last: $(tail -1 "$out.smoke.log") pass=$(grep -c PASS "$out.smoke.log") fail=$(grep -c FAIL "$out.smoke.log")" >> "$out"
for probe in launch-forms link; do
  echo "GATE $probe start $(date +%H:%M:%S) cpu=$(cpu)%" >> "$out"
  bash "scripts/$probe-probe.sh" > "$out.$probe.log" 2>&1
  rc=$?
  echo "GATE $probe exit=$rc end $(date +%H:%M:%S) last: $(tail -1 "$out.$probe.log") pass=$(grep -c PASS "$out.$probe.log") fail=$(grep -c FAIL "$out.$probe.log")" >> "$out"
done
echo "GATE done $(date +%H:%M:%S) cpu=$(cpu)% defender_rt=$(defender)" >> "$out"
