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


COMMANDS = {"has_kinds": cmd_has_kinds, "new_id": cmd_new_id, "has_id": cmd_has_id, "count_kind": cmd_count_kind}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        print(f"usage: session_asserts.py <{'|'.join(COMMANDS)}> ...", file=sys.stderr)
        sys.exit(2)
    COMMANDS[sys.argv[1]](sys.argv[2:])
