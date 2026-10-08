"""The headless smoke test's python checks (scripts/headless-smoke-test.sh), one subcommand each.

Usage: python -P scripts/smoke_eval.py <state|query|nine|border|converted> ARGS... </dev/null

Each branch is the heredoc it replaced, unchanged and at module level, so the expressions it `eval`s see the
same names. The subcommand is popped off sys.argv first, so sys.argv[1], [2]... are the heredoc's arguments.
`query` imports scripts/export_state.py through PYTHONPATH, which the smoke test exports.
"""
import json
import os
import sys
from urllib.parse import parse_qs, urlparse

cmd = sys.argv.pop(1)

if cmd == "state":
    # state NAME EXPR [url]: EXPR over the EXPORTSTATE file; prints it (or the focused URL), exit 0 if truthy.
    s = json.load(open(sys.argv[1], encoding="utf-8"))
    url = s["focused_url"] or ""
    q = parse_qs(urlparse(url).query).get("q", [""])[0]
    row = os.environ.get("ROW", "")
    v = eval(sys.argv[2])
    print(url if sys.argv[3] == "url" else v)
    sys.exit(0 if v else 1)
elif cmd == "query":
    # query FILE EXPR: prints EXPR over the cards (see `query` in the smoke test).
    from export_state import *

    s = json.load(open(sys.argv[1], encoding="utf-8"))
    cards = {c["id"]: c for c in s["cards"]}
    tabs = sorted(i for i, c in cards.items() if card_is(c, TAB))
    groups = sorted(i for i, c in cards.items() if card_is(c, GROUP))
    W, H, TOP = 816, 639, 40  # RESIZE below; the graph toolbar's height, rounded up

    def pt(i, x, y):
        if not (0 <= x < W and TOP <= y < H):
            sys.stderr.write(f"query: {x:.0f},{y:.0f} ({label(cards[i])} {i}) is off the graph canvas\n")
        return "%.0f %.0f" % (x, y)

    xy = lambda i: pt(i, *cards[i]["screen"])
    lt = lambda i: pt(i, cards[i]["rect"][0] + 3, cards[i]["rect"][1] + 3)
    r = [cards[i]["rect"] for i in tabs]
    bbox = "%.0f %.0f %.0f %.0f" % (max(0, min(x[0] for x in r) - 20), max(TOP, min(x[1] for x in r) - 20),
                                    min(W - 1, max(x[2] for x in r) + 20), min(H - 1, max(x[3] for x in r) + 20)) if r else ""
    env = os.environ
    print(eval(sys.argv[2]))
elif cmd == "nine":
    # nine FILE: writes a group "Nine" of nine tabs, for IMPORTGROUP.
    tab = lambda i: {"id": f"t{i}", "type": "application/vnd.manifold.tab", "url": "manifold://welcome", "engine": "webview2", "group": "g"}
    json.dump({"nodes": [{"id": "g", "type": "application/vnd.manifold.group", "name": "Nine"}] + [tab(i) for i in range(9)]}, open(sys.argv[1], "w"))
elif cmd == "border":
    # border FILE PNG ID RGB: exit 0 if a pixel on ID's left edge, at mid-height, is within 40 of RGB.
    from PIL import Image

    cards = {c["id"]: c for c in json.load(open(sys.argv[1], encoding="utf-8"))["cards"]}
    img = Image.open(sys.argv[2]).convert("RGB")
    left, top, _, bottom = cards[sys.argv[3]]["rect"]
    y = round((top + bottom) / 2)
    want = eval(sys.argv[4])
    near = min(sum((a - b) ** 2 for a, b in zip(img.getpixel((x, y)), want)) ** 0.5 for x in range(round(left) - 1, round(left) + 3))
    sys.exit(0 if near < 40 else 1)
elif cmd == "converted":
    # converted FILE NAME KIND: exit 0 if a KIND node's file:// url ends in /NAME and no tab is there.
    nodes = json.load(open(sys.argv[1], encoding="utf-8"))["nodes"]
    KINDS = {"json": "application/json", "pdf": "application/pdf"}
    at = lambda n: n.get("url", "").startswith("file://") and n.get("url", "").endswith("/" + sys.argv[2])
    sys.exit(0 if any(n["type"] == KINDS[sys.argv[3]] and at(n) for n in nodes) and not any(n["type"] == "application/vnd.manifold.tab" and at(n) for n in nodes) else 1)
else:
    sys.exit(f"smoke_eval: unknown subcommand {cmd!r}")
