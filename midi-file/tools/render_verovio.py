"""Engrave the evaluation pieces again with Verovio: other font, spacing and fingering
placement than MuseScore. Pages are rasterized so that staff lines are ~21 px apart (like
a 300 dpi scan), saved as a PDF for the full pipeline, and labelled:

verovio/<name>.pdf
verovio/labels.json {"<name>_p<n>": {"digits": [[d, x0, y0, x1, y1, note id]],
                                     "heads": [[note id, x0, y0, x1, y1]]}}
"""
import io
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
import zipfile

import fitz
import numpy as np
import resvg_py
import verovio
from PIL import Image

here = os.path.dirname(os.path.abspath(__file__))
LIST = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, "test_set.json")
out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(here, "verovio")
os.makedirs(out, exist_ok=True)
SVG = "{http://www.w3.org/2000/svg}"
TARGET_SPACE = 21.0

tests = json.load(open(LIST, encoding="utf-8"))
labels = {}
done = 0
for t in tests:
    name = t["name"]
    z = zipfile.ZipFile(os.path.join(here, t["dir"], name + ".mxl"))
    xml = max((z.read(n) for n in z.namelist() if not n.startswith("META")), key=len).decode("utf-8", "replace")
    tk = verovio.toolkit()
    tk.setOptions({"svgBoundingBoxes": True, "pageHeight": 2970, "pageWidth": 2100, "scale": 100,
                   "breaks": "auto"})
    if not tk.loadData(xml):
        print("load failed", name, file=sys.stderr)
        continue
    mei = tk.getMEI()
    # The same engraving without bounding boxes, for the image (the boxes Verovio puts
    # inside <tspan> make SVG renderers drop the text)
    plain = verovio.toolkit()
    plain.setOptions({"pageHeight": 2970, "pageWidth": 2100, "scale": 100, "breaks": "auto"})
    plain.loadData(xml)
    # fing id -> note id
    fing_note = dict(re.findall(r'<fing xml:id="([^"]+)"[^>]*startid="#([^"]+)"', mei))
    pdf = fitz.open()
    for page_no in range(1, tk.getPageCount() + 1):
        svg = tk.renderToSVG(page_no)
        root = ET.fromstring(svg)
        # Staff space in SVG units: the lines of the first staff
        ys = []
        for g in root.iter(SVG + "g"):
            if "staff" in (g.get("class") or "").split():
                for p in g.findall(SVG + "path"):
                    m = re.match(r"M\s*([\d.]+)\s+([\d.]+)\s+L\s*([\d.]+)\s+([\d.]+)", p.get("d") or "")
                    if m and abs(float(m.group(2)) - float(m.group(4))) < 1:
                        ys.append(float(m.group(2)))
                if len(ys) >= 5:
                    break
        ys = sorted(set(ys))[:5]
        if len(ys) < 5:
            print("no staff", name, page_no, file=sys.stderr)
            continue
        space = (ys[-1] - ys[0]) / 4
        # The inner viewBox maps to the outer width / height in pixels
        inner = root.find(SVG + "svg")
        vb = [float(v) for v in inner.get("viewBox").split()]
        width_px = float(root.get("width").rstrip("px"))
        units_per_px = vb[2] / width_px
        zoom = TARGET_SPACE / (space / units_per_px)
        png = resvg_py.svg_to_bytes(svg_string=plain.renderToSVG(page_no), zoom=zoom, background="#ffffff")
        # Notehead glyphs are placed relative to the page margin group
        margin = re.search(r'class="page-margin" transform="translate\(([-\d.]+),\s*([-\d.]+)\)', svg)
        mx, my = (float(margin.group(1)), float(margin.group(2))) if margin else (0.0, 0.0)
        im = Image.open(io.BytesIO(bytes(png))).convert("L")
        px_per_unit = zoom / units_per_px

        def box(rect):
            x, y = float(rect.get("x")) + mx - vb[0], float(rect.get("y")) + my - vb[1]
            w, h = float(rect.get("width")), float(rect.get("height"))
            return [round(x * px_per_unit), round(y * px_per_unit),
                    round((x + w) * px_per_unit), round((y + h) * px_per_unit)]

        digits, heads = [], []
        for g in root.iter(SVG + "g"):
            cls = (g.get("class") or "").split()
            if "fing" in cls and "bounding-box" not in cls:
                note = fing_note.get(g.get("id"))
                for tb in g.iter(SVG + "g"):
                    if "text" in (tb.get("class") or "").split() and "bounding-box" in (tb.get("class") or "").split():
                        r = tb.find(SVG + "rect")
                        text = "".join(t.text or "" for t in g.iter(SVG + "tspan") if t.text and t.text.strip())
                        ds = [int(c) for c in text if c in "12345"]
                        if r is not None and len(ds) == 1:
                            digits.append([ds[0]] + box(r) + [note])
            if "note" in cls and "bounding-box" not in cls:
                for nh in g.findall(SVG + "g"):
                    if "notehead" in (nh.get("class") or "").split():
                        use = nh.find(SVG + "use")
                        if use is not None:
                            # Glyph origin: left end, on the line / space of the note
                            m = re.search(r"translate\(([-\d.]+),\s*([-\d.]+)\)", use.get("transform") or "")
                            if m is None:
                                continue
                            x = (float(m.group(1)) + mx - vb[0]) * px_per_unit
                            y = (float(m.group(2)) + my - vb[1]) * px_per_unit
                            sp = space * px_per_unit
                            glyph = (use.get("{http://www.w3.org/1999/xlink}href") or use.get("href") or "")[1:5].upper()
                            kind = {"E0A4": 0, "E0A3": 1, "E0A2": 2}.get(glyph, 0)
                            heads.append([g.get("id"), round(x), round(y - 0.5 * sp), round(x + 1.18 * sp), round(y + 0.5 * sp), kind])
        labels[f"{name}_p{page_no - 1}"] = {"digits": digits, "heads": heads}
        buf = io.BytesIO()
        im.save(buf, "PNG")
        png = buf.getvalue()
        w_pt, h_pt = im.width / 300 * 72, im.height / 300 * 72
        p = pdf.new_page(width=w_pt, height=h_pt)
        p.insert_image(p.rect, stream=png)
    pdf.save(os.path.join(out, name + ".pdf"))
    done += 1
    print(done, name, sum(len(labels[k]["digits"]) for k in labels if k.startswith(name + "_p")), file=sys.stderr, flush=True)

json.dump(labels, open(os.path.join(out, "labels.json"), "w"))
print("pieces", done, "pages", len(labels), "digits", sum(len(v["digits"]) for v in labels.values()))
