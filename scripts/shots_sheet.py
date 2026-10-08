"""chrome-shots.sh's last step: crops each view's PNG to its top CROP_PX rows and tiles them into sheet.png.

Usage: python -P scripts/shots_sheet.py OUT CROP_PX "VIEW..." </dev/null
"""
import os, sys
out, crop_px, views = sys.argv[1], int(sys.argv[2]), sys.argv[3].split()
try:
    from PIL import Image
except ImportError:
    print("Pillow not installed: no crops or sheet")
    sys.exit(0)
imgs = []
for v in views:
    p = os.path.join(out, v + ".png")
    if not os.path.exists(p):
        continue
    im = Image.open(p).convert("RGB")
    im.crop((0, 0, im.width, min(crop_px, im.height))).save(os.path.join(out, "crop", v + ".png"))
    imgs.append(im)
if imgs:
    cols = 3
    tw = 640
    th = int(imgs[0].height * tw / imgs[0].width)
    rows = (len(imgs) + cols - 1) // cols
    sheet = Image.new("RGB", (cols * tw, rows * th), (30, 30, 30))
    for i, im in enumerate(imgs):
        sheet.paste(im.resize((tw, th)), ((i % cols) * tw, (i // cols) * th))
    sheet.save(os.path.join(out, "sheet.png"))
    print(f"{len(imgs)} views: {out}")
