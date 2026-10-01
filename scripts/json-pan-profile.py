"""Inputs for scripts/json-pan-profile.sh (run as `python -P <this> <cmd> ... </dev/null`).

  fixture <out.json>
      A tree 3 levels deep, 12 children per level (1 + 12 + 144 + 1728
      containers, each with a few scalar rows), deterministic. The level's
      default expansion stops at 150 cards, so the script reveals the
      12 x 12 depth-2 cards to bring a few thousand cards into play.
  script <fixture.json> <out.txt> <seconds> [pan|zoom]
      The headless command file: opens the fixture, enters its level,
      selects into every depth-2 card (which reveals it), then for about
      <seconds> (one command per frame, headless renders a frame per command)
      either pans (drag on the background, back and forth; default) or zooms
      (graph view maps plain wheel scroll to zoom, so SCROLL with dy zooms in
      and out around the cursor; dx alone does nothing).
"""

import json
import sys

BRANCH = 12


def leaf(n):
    return {"id": n, "name": f"item {n}", "score": n * 0.37, "tags": ["a", "b"], "ok": n % 2 == 0}


def tree(depth, n=0):
    if depth == 0:
        return leaf(n)
    return {f"k{i}": tree(depth - 1, n * BRANCH + i) for i in range(BRANCH)}


def cmd_fixture(args):
    (out,) = args
    with open(out, "w", encoding="utf-8") as f:
        json.dump(tree(3), f)


def cmd_script(args):
    fixture, out, seconds = args[0], args[1], float(args[2])
    mode = args[3] if len(args) > 3 else "pan"
    lines = ["RESIZE 1400 900", f"ADDFILE {fixture} 500 350", "SLEEP 1", "DOUBLECLICK 500 350", "SLEEP 2"]
    for i in range(BRANCH):
        for j in range(BRANCH):
            lines.append(f"JSONSELECT /k{i}/k{j}/k0")  # reveals (opens) the depth-2 card
    lines.append("SLEEP 2")
    # ~60 frames/s is not guaranteed; the loop is a fixed count, the
    # profile's own timestamps give the real rate.
    frames = int(seconds * 60)
    if mode == "zoom":
        for n in range(frames):
            dy = 12 if (n // 60) % 2 == 0 else -12
            lines.append(f"SCROLL 600 400 0 {dy}")
    else:
        lines.append("MOUSEDOWN 1300 850")
        for n in range(frames):
            step = n % 120
            x = 1300 - 6 * (step if (n // 120) % 2 == 0 else 120 - step)
            lines.append(f"MOUSEMOVE {x} 850")
        lines.append("MOUSEUP 700 850")
    lines.append("QUIT")
    with open(out, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


COMMANDS = {"fixture": cmd_fixture, "script": cmd_script}

if __name__ == "__main__":
    cmd, *rest = sys.argv[1:]
    COMMANDS[cmd](rest)
