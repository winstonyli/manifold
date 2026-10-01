"""Top functions by self and inclusive samples in a samply profile.

  python -P scripts/samply-top.py <profile.json.gz> [N] [thread-name-substring] [exe-with-pdb]

Reads the profile and its `.syms.json` sidecar (`--unstable-presymbolicate`),
which maps each library's rvas to symbol names, and prints, for the busiest
threads, the N heaviest self-time functions and the N heaviest inclusive
ones. Frames the sidecar can't name stay `lib+0xrva`.
"""

import bisect
import collections
import gzip
import json
import sys

path = sys.argv[1]
top = int(sys.argv[2]) if len(sys.argv) > 2 else 25
want = sys.argv[3] if len(sys.argv) > 3 else ""

with gzip.open(path, "rt", encoding="utf-8") as f:
    prof = json.load(f)
with open(path.replace(".json.gz", ".json.syms.json"), encoding="utf-8") as f:
    syms = json.load(f)
strings = syms["string_table"]

# lib name -> (sorted rvas, [(size, symbol)])
tables = {}
for entry in syms["data"]:
    rows = sorted(entry["symbol_table"], key=lambda r: r["rva"])
    tables[entry["debug_name"]] = ([r["rva"] for r in rows], rows)
libs = prof["libs"]


def name_of(thread, frame):
    ft = thread["frameTable"]
    func = ft["func"][frame]
    res = thread["funcTable"]["resource"][func]
    addr = ft["address"][frame]
    lib = None
    if res is not None and res >= 0:
        lib = thread["resourceTable"]["lib"][res]
    if lib is not None and lib >= 0 and addr is not None and addr >= 0:
        dbg = libs[lib]["debugName"]
        if dbg in tables:
            rvas, rows = tables[dbg]
            i = bisect.bisect_right(rvas, addr) - 1
            if i >= 0 and addr < rows[i]["rva"] + rows[i]["size"]:
                return strings[rows[i]["symbol"]]
        return f"{dbg}+{addr:#x}"
    return thread["stringArray"][thread["funcTable"]["name"][func]]


exe = sys.argv[4] if len(sys.argv) > 4 else None
resolved = {}


def resolve(names):
    """`fun_<rva>` -> function name, with llvm-symbolizer against `exe`'s PDB."""
    todo = sorted({n for n in names if n.startswith("fun_") and n not in resolved})
    if not exe or not todo:
        return
    import subprocess

    addrs = "\n".join(f"0x{0x140000000 + int(n[4:], 16):x}" for n in todo)
    out = subprocess.run(
        ["llvm-symbolizer", "--obj=" + exe, "--no-inlines", "--demangle", "--dia"],
        input=addrs, capture_output=True, text=True, check=False,
    ).stdout
    # One block per address: name, location, blank line. `--dia` is needed:
    # the default PDB reader returns no function names.
    for n, block in zip(todo, out.split("\n\n")):
        resolved[n] = block.strip("\n").partition("\n")[0].strip() or n


def shown(n):
    return resolved.get(n, n)


threads = [t for t in prof["threads"] if want in t.get("name", "")]
threads.sort(key=lambda t: len(t["samples"]["stack"]), reverse=True)
for t in threads[:3]:
    stacks = [s for s in t["samples"]["stack"] if s is not None]
    if not stacks:
        continue
    st = t["stackTable"]
    memo = {}

    def frame_name(s):
        if s not in memo:
            memo[s] = name_of(t, st["frame"][s])
        return memo[s]

    selfc, incl = collections.Counter(), collections.Counter()
    for s in stacks:
        selfc[frame_name(s)] += 1
        seen = set()
        while s is not None:
            n = frame_name(s)
            if n not in seen:
                incl[n] += 1
                seen.add(n)
            s = st["prefix"][s]
    total = len(stacks)
    print(f"\n== thread {t.get('name')!r} tid {t.get('tid')}: {total} samples")
    resolve([n for n, _ in selfc.most_common(top)] + [n for n, _ in incl.most_common(top)])
    print("-- self")
    for n, c in selfc.most_common(top):
        print(f"{100 * c / total:5.1f}%  {shown(n)[:110]}")
    print("-- inclusive")
    for n, c in incl.most_common(top):
        print(f"{100 * c / total:5.1f}%  {shown(n)[:110]}")
