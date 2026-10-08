# Sourced by the probes and tests that run manifold.exe in the background.
#
# A native exe started from bash (`"$exe" ... &`) has an MSYS pid ($!) and a
# separate Windows pid, which taskkill and Get-Process need. Right after the
# launch `ps -W` can list the MSYS pid without its Windows pid yet, so every
# lookup here retries (every 0.25 s, 20 tries by default: 5 s). Safe under
# `set -euo pipefail`.

# _winpid_retry <awk condition on $1 (pid) / $2 (ppid)> <pid> [tries]: the
# WINPID column of the first `ps -W` row matching, retried. Warns on stderr and
# fails if none shows up.
_winpid_retry() {
    local cond=$1 p=$2 tries=${3:-20} w=""
    for _ in $(seq "$tries"); do
        # No `exit` in awk: under pipefail a SIGPIPE'd ps would fail the lookup.
        w="$(ps -W | awk -v p="$p" "!found && ($cond) { print \$4; found = 1 }")"
        [ -n "$w" ] && { echo "$w"; return 0; }
        sleep 0.25
    done
    echo "WARNING: no Windows pid for bash pid $p after $tries tries" >&2
    return 1
}

# winpid_of <msys pid> [tries]: the Windows pid of the exe that `$!` names.
winpid_of() { _winpid_retry '$1 == p' "$@"; }

# winpid_of_child <msys pid> [tries]: the Windows pid of the manifold process
# whose parent is <msys pid> (a coproc's subshell runs the exe as its child).
winpid_of_child() { _winpid_retry '$2 == p && $0 ~ /manifold/' "$@"; }

# below_normal <msys pid>: sets the instance BelowNormal and prints its Windows
# pid. Warns on stderr and fails if there is no Windows pid or the priority
# could not be set (the pid, if found, is still printed).
below_normal() {
    local w
    w="$(winpid_of "$1")" || return 1
    echo "$w"
    powershell -NoProfile -Command "(Get-Process -Id $w -ErrorAction Stop).PriorityClass = 'BelowNormal'" \
        </dev/null >/dev/null 2>&1 || { echo "WARNING: could not set Windows pid $w BelowNormal" >&2; return 1; }
}

# kill_tree <msys pid>: for EXIT traps. If that instance is still running,
# taskkill /F /T its Windows pid (its own tree only, never by image name).
# Always succeeds.
kill_tree() {
    local w
    kill -0 "$1" 2>/dev/null || return 0
    if w="$(winpid_of "$1")"; then taskkill //F //T //PID "$w" >/dev/null 2>&1; else kill "$1" 2>/dev/null; fi
    return 0
}
