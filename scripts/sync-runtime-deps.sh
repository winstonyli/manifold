#!/usr/bin/env bash
# Recopies Servo's ANGLE DLLs and CEF's runtime files from their build.rs
# output directories into target/debug/, where manifold-engine-servo.exe and
# manifold-engine-cef.exe (via its isolated cef-runtime/ subdirectory) actually
# load them from. Cargo does not do this itself, and — as this project has
# hit repeatedly, see README.md's "Manual runtime-file placement" section —
# the fingerprint hash directory these live under can change even on a
# rebuild that touched neither Cargo.toml nor Cargo.lock, silently staling
# any earlier manual copy.
#
# Safe to run any time (no-op if everything's already current); run it
# after every `cargo build` that touches manifold-engine-servo or
# manifold-engine-cef. Not a build.rs: a build script only sees its own
# crate's OUT_DIR, not a sibling crate's, and there is no reliable
# cross-crate coordination here without mozangle/cef-dll-sys themselves
# cooperating (they don't) - a plain script is simpler and more honest
# than fighting Cargo's model.
set -euo pipefail
shopt -s nullglob

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_debug="$repo_root/target/debug"

if [ ! -d "$target_debug" ]; then
    echo "sync-runtime-deps: $target_debug does not exist - build first" >&2
    exit 1
fi

# Finds the most-recently-*written* match of a glob (relative to a fixed,
# possibly space-containing base directory kept quoted throughout - only
# the glob suffix itself, a literal pattern with no spaces, is left
# unquoted for expansion). More reliable than the containing fingerprint
# directory's own mtime, which a `cargo build` touches for its own
# bookkeeping on every invocation regardless of whether that candidate's
# code actually re-ran (see README.md/project memory on this).
# Takes one or more globs: newer cargo nightlies put build-script output
# under build/<crate>/<hash>/, older ones under build/<crate>-<hash>/, and
# a target dir built by both has each.
find_newest() {
    local base="$1" glob
    local f newest="" newest_time=0 t
    shift
    for glob in "$@"; do
        for f in "$base"/$glob; do
            [ -e "$f" ] || continue
            t=$(stat -c '%Y' "$f")
            if [ "$t" -gt "$newest_time" ]; then
                newest_time="$t"
                newest="$f"
            fi
        done
    done
    printf '%s' "$newest"
}

changed=0

sync_file() {
    local src="$1" dst="$2" label="$3"
    if [ -z "$src" ]; then
        echo "sync-runtime-deps: no candidate found for $label, skipping" >&2
        return
    fi
    if [ -f "$dst" ] && cmp -s "$src" "$dst"; then
        echo "sync-runtime-deps: $label already current"
    else
        cp "$src" "$dst"
        echo "sync-runtime-deps: recopied $label"
        changed=1
    fi
}

# --- Servo's ANGLE DLLs ---
mozangle_egl=$(find_newest "$target_debug/build" "mozangle-*/out/libEGL.dll" "mozangle/*/out/libEGL.dll")
mozangle_glesv2=$(find_newest "$target_debug/build" "mozangle-*/out/libGLESv2.dll" "mozangle/*/out/libGLESv2.dll")
sync_file "$mozangle_egl" "$target_debug/libEGL.dll" "libEGL.dll"
sync_file "$mozangle_glesv2" "$target_debug/libGLESv2.dll" "libGLESv2.dll"

# --- CEF's full runtime, isolated in its own subdirectory (see
# process_manager.rs's engine_binary_path doc comment for why) ---
cef_out_dir=$(find_newest "$target_debug/build" "cef-dll-sys-*/out/cef_windows_x86_64" "cef-dll-sys/*/out/cef_windows_x86_64")
cef_runtime_dir="$target_debug/cef-runtime"
if [ -n "$cef_out_dir" ]; then
    mkdir -p "$cef_runtime_dir"
    for f in "$cef_out_dir"/*; do
        name="$(basename "$f")"
        if [ -d "$f" ]; then
            # Directories (e.g. locales/) - just sync wholesale; these are
            # part of the CEF distribution itself and don't change between
            # rebuilds of this project the way the DLLs above do.
            if [ ! -d "$cef_runtime_dir/$name" ] || [ -z "$(ls -A "$cef_runtime_dir/$name" 2>/dev/null)" ]; then
                cp -r "$f" "$cef_runtime_dir/"
                echo "sync-runtime-deps: copied cef-runtime/$name/"
                changed=1
            fi
        else
            sync_file "$f" "$cef_runtime_dir/$name" "cef-runtime/$name"
        fi
    done
else
    echo "sync-runtime-deps: no cef-dll-sys output directory found, skipping CEF runtime" >&2
fi

# manifold-engine-cef.exe itself is rebuilt every time manifold-engine-cef changes,
# unlike the DLLs above - always resync it, cmp-gated same as everything
# else.
sync_file "$target_debug/manifold-engine-cef.exe" "$cef_runtime_dir/manifold-engine-cef.exe" "cef-runtime/manifold-engine-cef.exe"

if [ "$changed" -eq 0 ]; then
    echo "sync-runtime-deps: everything already current, nothing to do"
fi
