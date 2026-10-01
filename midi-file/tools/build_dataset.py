"""Training pages for the fingering digit detector.

For every PDMX piece in fingered/ (MXL with fingering + the PDF made from it), the PDF pages
are rendered at 300 dpi and the fingering digits are read from the PDF's text layer: the
font + size whose count of digits 1-5 is closest to the score's number of <fingering>
elements is the fingering style; other digits (measure numbers, tuplets, tempo) are not
labelled and teach the detector what is not fingering.

Pieces used for evaluation (omr_work/) are left out.

usage: build_dataset.py <out dir>
out/<piece>_p<n>.png (gray) and out/labels.json {image: [[digit, x0, y0, x1, y1], ...]}
"""
import glob
import io
import json
import os
import re
import sys
import zipfile
from collections import Counter

import fitz

here = os.path.dirname(os.path.abspath(__file__))
out = sys.argv[1]
os.makedirs(out, exist_ok=True)
# Evaluation pieces (make_test_set.py) never go into training
held_out = {p["name"] for p in json.load(open(os.path.join(here, "test_set_all.json"), encoding="utf-8"))}
# Scores labelled "Piano" in PDMX that are other instruments (screen_piano.py, checked by eye)
held_out |= set(json.load(open(os.path.join(here, "non_piano.json"), encoding="utf-8")))
print("held out:", sorted(held_out), file=sys.stderr)

DPI = 300
scale = DPI / 72
labels = {}
stats = Counter()
for mxl in sorted(glob.glob(os.path.join(here, "fingered", "*.mxl")) + glob.glob(os.path.join(here, "fingered15", "*.mxl"))):
    name = os.path.basename(mxl)[:-4]
    pdf = mxl[:-4] + ".pdf"
    if name in held_out or not os.path.exists(pdf):
        stats["skipped"] += 1
        continue
    z = zipfile.ZipFile(mxl)
    xml = max((z.read(n) for n in z.namelist() if not n.startswith("META-INF")), key=len)
    truth = len(re.findall(rb"<fingering[^>]*>\s*[1-5]", xml))

    doc = fitz.open(pdf)
    chars = []  # (page, style, digit, bbox)
    for pi, page in enumerate(doc):
        for b in page.get_text("rawdict")["blocks"]:
            for l in b.get("lines", []):
                for s in l["spans"]:
                    style = (s["font"], round(s["size"], 1))
                    for ch in s["chars"]:
                        if ch["c"] in "12345":
                            chars.append((pi, style, int(ch["c"]), ch["bbox"]))
    counts = Counter(c[1] for c in chars)
    if not counts:
        stats["no text"] += 1
        continue
    style, n = min(counts.items(), key=lambda kv: abs(kv[1] - truth))
    if abs(n - truth) > max(5, 0.15 * truth):
        stats["style unclear"] += 1
        continue

    for pi, page in enumerate(doc):
        boxes = [[d] + [round(v * scale) for v in bbox] for p, st, d, bbox in chars if p == pi and st == style]
        pix = page.get_pixmap(dpi=DPI, colorspace=fitz.csGRAY)
        image = f"{name.split('_')[0]}_p{pi}.png"
        pix.save(os.path.join(out, image))
        labels[image] = boxes
        stats["pages"] += 1
        stats["digits"] += len(boxes)
    stats["pieces"] += 1

json.dump(labels, open(os.path.join(out, "labels.json"), "w"))
print(dict(stats), file=sys.stderr)
