"""Helpers for scripts/json-level-probe.txt (Global Constraints: Python
only as `python -P scripts/json-level-probe.py <cmd> ... </dev/null`,
never `python -`/`python -P -`).

Commands:
  fixture <out_path>
      Writes the probe's JSON file: a paged array (45 users) and a
      nested object (a bbox), deterministic so it can be regenerated.
  doubleclick <export_path> <drop_x> <drop_y>
      Reads an EXPORTSTATE export and prints "DOUBLECLICK x y" for the
      Json card nearest (drop_x, drop_y) (a fresh profile's own demo
      scene can have another Json card already).
"""

import json
import os
import sys

# `-P` keeps this file's directory off sys.path; export_state.py sits beside it.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from export_state import JSON, card_is  # noqa: E402


def cmd_fixture(args):
    (out_path,) = args
    data = {
        "page": 1,
        "users": [{"id": i, "name": f"user {i}", "roles": ["a", "b"]} for i in range(45)],
        "meta": {"bbox": [[-0.12, 51.5], [0.33, 51.7]]},
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(data, f)


def cmd_doubleclick(args):
    # A fresh profile's first-launch demo scene includes its own Json
    # card, so more than one can be live at once (Global Constraints
    # ledger: welcome.html's "Try it" card). Pick the one nearest the
    # ADDFILE drop point (500, 350), not just any Json card.
    export_path, drop_x, drop_y = args
    drop_x, drop_y = float(drop_x), float(drop_y)
    with open(export_path, encoding="utf-8") as f:
        s = json.load(f)
    candidates = [c for c in s["cards"] if card_is(c, JSON)]
    c = min(candidates, key=lambda c: (c["screen"][0] - drop_x) ** 2 + (c["screen"][1] - drop_y) ** 2)
    print("DOUBLECLICK %.0f %.0f" % tuple(c["screen"]))


COMMANDS = {"fixture": cmd_fixture, "doubleclick": cmd_doubleclick}

if __name__ == "__main__":
    cmd, *rest = sys.argv[1:]
    COMMANDS[cmd](rest)
