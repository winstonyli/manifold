#!/usr/bin/env bash
# Usage: scripts/gate-timing.sh collect TAG RUNLOG [CPULOG]   prints RUNLOG's TIMING lines as
#            TIMING TAG/name secs start cpu_start=% cpu_mean=%
#        scripts/gate-timing.sh top LOG [N]                  the N (default 10) slowest TIMING lines of LOG
# CPULOG is gate.sh's sampler file, lines "<unix seconds> <total cpu %>". cpu_start is the last sample taken at or
# before the step began, cpu_mean the mean of the samples taken during it; "-" where there is none.
case "${1:-}" in
collect)
    grep '^TIMING ' "$3" | awk -v tag="$2" -v cpulog="${4:-/dev/null}" '
        BEGIN { while ((getline l < cpulog) > 0) { split(l, f, " "); n++; ts[n] = f[1]; cp[n] = f[2] } close(cpulog) }
        {
            s = $4; e = s + $3; start = "-"; sum = 0; cnt = 0
            for (i = 1; i <= n; i++) {
                if (ts[i] <= s) start = cp[i]
                if (ts[i] >= s && ts[i] <= e) { sum += cp[i]; cnt++ }
            }
            printf "TIMING %s/%s %s %s cpu_start=%s cpu_mean=%s\n", tag, $2, $3, s, start, (cnt ? sprintf("%.1f", sum / cnt) : "-")
        }'
    ;;
top)
    grep '^TIMING ' "$2" | sort -g -r -k3,3 | head -n "${3:-10}" | awk '{ printf "%8ss  %-36s %s %s\n", $3, $2, $5, $6 }'
    ;;
*) echo "usage: gate-timing.sh collect TAG RUNLOG [CPULOG] | top LOG [N]" >&2; exit 2 ;;
esac
