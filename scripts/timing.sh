# Sourced by the scripts whose steps the gate times. A step is the run of commands between two marks.
#   timing_mark NAME   ends the open step, if any, printing   TIMING <name> <seconds> <start epoch>   and opens NAME
#   timing_mark        ends the open step and opens none (call it at the end and first in the EXIT trap)
# NAME is one word and must not contain PASS or FAIL (gate.sh counts those words). Needs bash 5 (EPOCHREALTIME).
# Call it only from the top level of a script, never inside a $(...) or a pipeline: the open step is shell state.
_timing_name=""
_timing_t0=0
timing_mark() {
    local now=${EPOCHREALTIME/[.,]/} d
    if [ -n "$_timing_name" ]; then
        d=$((now - _timing_t0))
        printf 'TIMING %s %d.%03d %d\n' "$_timing_name" $((d / 1000000)) $((d % 1000000 / 1000)) $((_timing_t0 / 1000000))
    fi
    _timing_name="${1:-}"
    _timing_t0=$now
    return 0
}
