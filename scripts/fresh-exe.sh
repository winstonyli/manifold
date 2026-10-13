# Sourced by drive.sh, most probes that run
# target/debug/manifold.exe (`grep -l fresh-exe.sh scripts/*.sh` lists them).
#
# require_fresh_exe <path to manifold.exe>: fails, naming the files, if any
# source the exe was built from (cargo's dep-info, manifold.d beside it) is
# newer than the exe; the same for the WebView2 host beside it (its own
# manifold-engine-webview2.d), which `cargo build -p manifold` does not build,
# so build both. `cargo check`/`test`/`clippy` don't rebuild it, so a
# probe could otherwise screenshot an old build. ALLOW_STALE=1 skips this.
require_fresh_exe() {
    local exe=$1 newer cef host e stale
    [ -n "${ALLOW_STALE:-}" ] && return 0
    # CEF runs from an isolated copy that only sync-runtime-deps.sh refreshes;
    # a stale one speaks an old message numbering and its host crashes.
    cef=$(dirname "$exe")/manifold-engine-cef.exe
    if [ -f "$cef" ] && [ -f "$(dirname "$exe")/cef-runtime/manifold-engine-cef.exe" ] &&
        [ "$cef" -nt "$(dirname "$exe")/cef-runtime/manifold-engine-cef.exe" ]; then
        echo "cef-runtime/manifold-engine-cef.exe is older than the built one: run scripts/sync-runtime-deps.sh, or set ALLOW_STALE=1." >&2
        return 1
    fi
    # The WebView2 host is a separate crate that `-p manifold` does not build; judge it by its own dep-info.
    host=$(dirname "$exe")/manifold-engine-webview2.exe
    stale=0
    for e in "$exe" "$host"; do
        [ "$e" = "$host" ] && [ ! -f "$e" ] && continue
        newer=$(_sources_newer_than "$e")
        if [ -n "$newer" ]; then
            echo "$e is older than:" >&2
            echo "$newer" >&2
            stale=1
        fi
    done
    if [ "$stale" = 1 ]; then
        echo "Build first (cargo build -j 8 -p manifold -p manifold-view-webview2), or set ALLOW_STALE=1." >&2
        return 1
    fi
}

# _sources_newer_than <exe>: lists (indented) the sources in <exe minus .exe>.d that are newer than the exe; prints
# nothing if there is no .d.
_sources_newer_than() {
    local exe=$1 dep=${1%.exe}.d
    [ -f "$dep" ] || return 0
    # One make rule, `<exe>: <src> <src> ...`, with spaces in paths escaped.
    sed -e '1s/^.*\.exe: //' -e 's/\\ /\x01/g' "$dep" | tr ' ' '\n' | tr '\001' ' ' |
        grep -v '^$' | cygpath -u -f - |
        while IFS= read -r f; do if [ "$f" -nt "$exe" ]; then echo "  $f"; fi; done
}
