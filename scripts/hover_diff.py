"""Usage: hover_diff.py DIR BAND [y [X0]]

DIR holds rest.png and x<NNNN>.png (the mouse at that x on the top bar, from
scripts/chrome-shots.sh hover_scan). Compares the top BAND rows of each against
rest and prints, per hover, the span of changed columns and how many separate
runs they form. A hover should repaint one widget (one run, the same span for
every x inside it). With `y`, the hovers went down a column instead (files
are y<NNNN>.png), BAND is the width of the strip compared, starting at column X0 (default 0), and runs are
of changed rows. Several runs, or a span that differs between two x inside
one widget, means a hover moved or resized something: layout instability.
"""
import glob
import os
import sys

from PIL import Image, ImageChops

d, band = sys.argv[1], int(sys.argv[2])
column = len(sys.argv) > 3 and sys.argv[3] == "y"
x0 = int(sys.argv[4]) if len(sys.argv) > 4 else 0
rest = Image.open(os.path.join(d, "rest.png")).convert("RGB")


def region(im):
    # Rows of a top band, or (column mode) a left strip transposed so the same run logic applies.
    if column:
        return im.crop((x0, 0, x0 + band, im.height)).transpose(Image.TRANSPOSE)
    return im.crop((0, 0, im.width, band))


rest = region(rest)
changed = {}
for p in sorted(glob.glob(os.path.join(d, "y*.png" if column else "x*.png"))):
    x = int(os.path.basename(p)[1:5])
    im = region(Image.open(p).convert("RGB"))
    diff = ImageChops.difference(im, rest).convert("L").point(lambda v: 255 if v > 12 else 0)
    changed[x] = {c for c, v in enumerate(diff.resize((diff.width, 1), Image.BOX).get_flattened_data()) if v > 0}
# Columns that change in most hovers are the app itself (a clock, a blinking cursor), not the hover.
if len(changed) >= 5:
    counts = {}
    for cols in changed.values():
        for c in cols:
            counts[c] = counts.get(c, 0) + 1
    noise = {c for c, n in counts.items() if n > 0.6 * len(changed)}
    changed = {x: cols - noise for x, cols in changed.items()}
rows = []
for x, cols in changed.items():
    cols = sorted(cols)
    if not cols:
        rows.append((x, None, 0))
        continue
    runs, start, prev = [], cols[0], cols[0]
    for c in cols[1:]:
        if c - prev > 6:
            runs.append((start, prev))
            start = c
        prev = c
    runs.append((start, prev))
    rows.append((x, (cols[0], cols[-1]), len(runs)))
bad = 0
for x, span, n in rows:
    flag = "" if n <= 1 else "  <-- several runs"
    if n > 1:
        bad += 1
    print(f"x={x:4d} " + ("no change" if span is None else f"changed {span[0]}..{span[1]} (w={span[1]-span[0]+1}) runs={n}") + flag)
print(f"{len(rows)} hovers, {bad} with several runs")
