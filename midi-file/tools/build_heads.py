"""Labels for the combined detector (digits 1-5 and note heads) on the training pages.

PDMX pages (train_pages3, rendered at 300 dpi by build_dataset.py): note heads are glyphs
of MuseScore's music font in the PDF text layer (SMuFL E0A4 black, E0A3 half, E0A2 whole);
a head is centered on the glyph's baseline.

usage: build_heads.py <pages dir>   (adds "heads" to <pages dir>/labels2.json)
labels2.json {image: {"digits": [[d, x0, y0, x1, y1]], "heads": [[kind, x0, y0, x1, y1]]}}
kind: 0 black, 1 half, 2 whole
"""
import glob
import json
import os
import sys

import fitz

here = os.path.dirname(os.path.abspath(__file__))
pages_dir = sys.argv[1]
labels = json.load(open(os.path.join(pages_dir, "labels.json")))
# Bravura in MuseScore 3 writes its heads at the alternate codepoints F4BE, F4BD, F4BC
HEADS = {0xE0A4: 0, 0xE0A3: 1, 0xE0A2: 2, 0xF4BE: 0, 0xF4BD: 1, 0xF4BC: 2}
scale = 300 / 72

# image prefix -> pdf
pdfs = {}
for d in ("fingered", "fingered15"):
    for pdf in glob.glob(os.path.join(here, d, "*.pdf")):
        pdfs[os.path.basename(pdf).split("_")[0]] = pdf

out = {}
n_heads = 0
docs = {}
for image, digits in labels.items():
    prefix, page = image[:-4].rsplit("_p", 1)
    pdf = pdfs.get(prefix)
    if pdf is None:
        continue
    doc = docs.setdefault(pdf, fitz.open(pdf))
    heads = []
    for b in doc[int(page)].get_text("rawdict")["blocks"]:
        for l in b.get("lines", []):
            for s in l["spans"]:
                for ch in s["chars"]:
                    kind = HEADS.get(ord(ch["c"]))
                    if kind is None:
                        continue
                    x0, _, x1, _ = ch["bbox"]
                    oy = ch["origin"][1]
                    w = x1 - x0
                    h = w * 0.8
                    heads.append([kind, round(x0 * scale), round((oy - h / 2) * scale),
                                  round(x1 * scale), round((oy + h / 2) * scale)])
    out[image] = {"digits": digits, "heads": heads}
    n_heads += len(heads)
json.dump(out, open(os.path.join(pages_dir, "labels2.json"), "w"))
print("pages", len(out), "heads", n_heads)
