# Helpers the probe scripts share. Source after winpid.sh:
#   # shellcheck source=lib.sh
#   . "$repo_root/scripts/lib.sh"
# `bash scripts/lib-test.sh` checks every helper without the app.

# seed_settings PROFILE [ENGINE] [MORE]: PROFILE/manifold/settings.json that skips the first-launch demo and
# starts tabs on ENGINE (default webview2). MORE is extra JSON members, starting with ", ".
seed_settings() {
    mkdir -p "$1/manifold"
    echo "{\"has_shown_first_launch_demo\": true, \"default_starting_engine\": \"${2:-webview2}\"${3:-}}" >"$1/manifold/settings.json"
}

# launch_headless EXE PROFILE CMDFILE LOG: one `EXE --headless` instance in the background, APPDATA set to
# PROFILE, reading commands as they are appended to CMDFILE, output to LOG, set BelowNormal (winpid.sh's
# below_normal; a failure warns and the run goes on). Sets `launched_winpid` to its Windows pid ("" if the lookup
# failed). The caller empties CMDFILE first (some write commands before launching) and reads `$!` right after:
# the lookup runs in the foreground, so `$!` is still the instance. More environment goes on the call:
# `HTTPS_PROXY=... launch_headless ...`.
launch_headless() {
    # `-n +1`: plain `tail -f` starts with only the last 10 lines.
    APPDATA="$(cygpath -w "$2")" "$1" --headless < <(exec tail -n +1 -f --pid=$$ "$3" 2>/dev/null) >"$4" 2>&1 &
    launched_winpid="$(below_normal "$!")" || echo "WARNING: $1 (pid $!) runs at normal priority" >&2
}

# send LINE...: appends each argument as one command line to "$cmd".
send() { printf '%s\n' "$@" >>"$cmd"; }

# wait_file PATH: until PATH exists and is non-empty, for WAIT_FILE_SECS seconds (default 60). The EXPORT*
# files are replaced atomically, so a non-empty one is whole.
wait_file() {
    local _
    for _ in $(seq $((${WAIT_FILE_SECS:-60} * 4))); do
        [ -s "$1" ] && return 0
        sleep 0.25
    done
    echo "timed out waiting for $1" >&2
    return 1
}

# check NAME WANT GOT: a PASS or FAIL line; a FAIL sets status=1 (the caller starts with status=0).
check() {
    if [ "$2" = "$3" ]; then echo "PASS: $1"; else echo "FAIL: $1: got [$3], wanted [$2]"; status=1; fi
}
