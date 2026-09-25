# Sourced by drive.sh and headless-smoke-test.sh.
#
# require_fresh_exe <path to manifold.exe>: fails, naming the files, if any
# source the exe was built from (cargo's dep-info, manifold.d beside it) is
# newer than the exe. `cargo check`/`test`/`clippy` don't rebuild it, so a
# probe could otherwise screenshot an old build. ALLOW_STALE=1 skips this.
require_fresh_exe() {
    local exe=$1 dep=${1%.exe}.d newer
    [ -n "${ALLOW_STALE:-}" ] && return 0
    [ -f "$dep" ] || return 0
    # One make rule, `<exe>: <src> <src> ...`, with spaces in paths escaped.
    newer=$(sed -e '1s/^.*\.exe: //' -e 's/\\ /\x01/g' "$dep" | tr ' ' '\n' | tr '\001' ' ' |
        grep -v '^$' | cygpath -u -f - |
        while IFS= read -r f; do if [ "$f" -nt "$exe" ]; then echo "  $f"; fi; done)
    if [ -n "$newer" ]; then
        echo "$exe is older than:" >&2
        echo "$newer" >&2
        echo "Build first (cargo build -p manifold -j 12), or set ALLOW_STALE=1." >&2
        return 1
    fi
}
