"""Small JSON asserts for scripts/two-instance-test.sh, run as a file
(`python -P scripts/session_asserts.py <cmd> ... </dev/null`), beside
compare_sessions.py rather than inline heredoc Python (Global Constraints).

Commands:
  has_kinds <path> <mime:min_count> [<mime:min_count> ...]
      Fails if any listed node type (a MIME type, e.g. text/markdown) has fewer
      than min_count nodes in <path>.
  new_id <before_path> <after_path>
      Prints the id of the one node present in <after_path> but not
      <before_path>. Fails unless there's exactly one.
  has_id <path> <id>
      Fails unless <id> is a node id in <path>.
  count_kind <path> <mime>
      Prints how many nodes of type <mime> (a MIME type) <path> has.
  group_of <path> <id>
      Prints the group of node <id> in <path> ("null" at the top level).
  mirror_agree <a_state> <b_state> <session> <id> [<id> ...]
      Fails unless each <id>'s group is the same in both EXPORTSTATE files'
      `mirror_groups` (what each instance shows) and in the EXPORTSESSION
      <session> (the document). Prints the groups.
  no_cycle <path>
      Fails if following groups from any node in <path> loops.
  kept_ids <before_path> <after_path>
      Fails if a node id in <before_path> is missing from <after_path>.
"""

import json
import sys
from collections import Counter


def load_nodes(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)["nodes"]


def cmd_has_kinds(args):
    path, specs = args[0], args[1:]
    counts = Counter(n["type"] for n in load_nodes(path))
    ok = True
    for spec in specs:
        kind, want = spec.rsplit(":", 1)
        want = int(want)
        got = counts.get(kind, 0)
        if got < want:
            print(f"FAIL: {path} has {got} {kind!r} nodes, want >= {want}")
            ok = False
    if ok:
        print(f"ok: {path} satisfies {specs}")
    sys.exit(0 if ok else 1)


def cmd_new_id(args):
    before_path, after_path = args
    before_ids = {n["id"] for n in load_nodes(before_path)}
    new = [n for n in load_nodes(after_path) if n["id"] not in before_ids]
    if len(new) != 1:
        print(f"FAIL: expected exactly 1 new node between {before_path} and {after_path}, got {len(new)}: {new}")
        sys.exit(1)
    print(new[0]["id"])


def cmd_has_id(args):
    path, wanted = args
    ids = {n["id"] for n in load_nodes(path)}
    if wanted not in ids:
        print(f"FAIL: {wanted!r} not found in {path}")
        sys.exit(1)
    print(f"ok: {wanted!r} found in {path}")


def cmd_count_kind(args):
    path, kind = args
    print(sum(1 for n in load_nodes(path) if n["type"] == kind))


def groups_of(path):
    return {n["id"]: n.get("group") for n in load_nodes(path)}


def cmd_group_of(args):
    path, wanted = args
    groups = groups_of(path)
    if wanted not in groups:
        print(f"FAIL: {wanted!r} not found in {path}")
        sys.exit(1)
    print(groups[wanted] or "null")


def cmd_mirror_agree(args):
    a_state, b_state, session, ids = args[0], args[1], args[2], args[3:]
    mirrors = []
    for path in (a_state, b_state):
        with open(path, encoding="utf-8") as f:
            mirrors.append(json.load(f)["mirror_groups"])
    doc = groups_of(session)
    ok = True
    for i in ids:
        seen = [m.get(i, "<absent>") for m in mirrors] + [doc.get(i, "<absent>")]
        print(f"{i}: A shows {seen[0]}, B shows {seen[1]}, document {seen[2]}")
        if "<absent>" in seen or len({str(s) for s in seen}) != 1:
            ok = False
    sys.exit(0 if ok else 1)


def cmd_no_cycle(args):
    (path,) = args
    groups = groups_of(path)
    for start in groups:
        seen, cur = set(), start
        while cur is not None:
            if cur in seen:
                print(f"FAIL: {path}: a group cycle through {cur!r} (from {start!r})")
                sys.exit(1)
            seen.add(cur)
            cur = groups.get(cur)
    print(f"ok: {path} has no group cycle")


def cmd_kept_ids(args):
    before_path, after_path = args
    lost = {n["id"] for n in load_nodes(before_path)} - {n["id"] for n in load_nodes(after_path)}
    if lost:
        print(f"FAIL: {sorted(lost)} in {before_path} but not {after_path}")
        sys.exit(1)
    print(f"ok: every node of {before_path} is in {after_path}")


COMMANDS = {
    "has_kinds": cmd_has_kinds,
    "new_id": cmd_new_id,
    "has_id": cmd_has_id,
    "count_kind": cmd_count_kind,
    "group_of": cmd_group_of,
    "mirror_agree": cmd_mirror_agree,
    "no_cycle": cmd_no_cycle,
    "kept_ids": cmd_kept_ids,
}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        print(f"usage: session_asserts.py <{'|'.join(COMMANDS)}> ...", file=sys.stderr)
        sys.exit(2)
    COMMANDS[sys.argv[1]](sys.argv[2:])
