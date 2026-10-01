"""Oracle detections for the MuseScore test PDFs: note heads and fingering digits read from
the PDF text layer (exact), to find out which part of the score reader loses fingerings.

Writes page_cache/oracle_<kind>/<piece>/<n>.det.json for the score reader
(SCORE_READER_DETECTIONS), kinds:
  heads   true heads + detected digits
  both    true heads + true digits
  digits  detected heads + true digits
Detected objects come from page_cache/musescore (run_reader_gpu.sh musescore ... first).
Also prints, per piece, PDF heads vs score notes.
"""
import json
import os
import re
import zipfile
from collections import Counter

import fitz

here = os.path.dirname(os.path.abspath(__file__))
S = 300 / 72
WHOLE, HALF = {0xE0A2, 0xF4BC}, {0xE0A3, 0xF4BD}


def is_head(cp):
    # SMuFL note heads, Bravura's alternates in MuseScore 3
    return 0xE0A0 <= cp <= 0xE0FF or cp in (0xF4BC, 0xF4BD, 0xF4BE)


def score_notes(mxl):
    z = zipfile.ZipFile(mxl)
    xml = max((z.read(n) for n in z.namelist() if not n.startswith("META-INF")), key=len).decode("utf-8", "replace")
    notes = re.findall(r"<note\b[^>]*>.*?</note>", xml, re.S)
    sounding = [n for n in notes if "<rest" not in n]
    visible = [n for n in sounding if 'print-object="no"' not in n]
    grace = sum("<grace" in n for n in visible)
    cue = sum("<cue" in n for n in visible)
    fing = len(re.findall(r"<fingering[^>]*>\s*[1-5]", xml))
    return len(visible), grace, cue, len(sounding) - len(visible), fing


tests = json.load(open(os.path.join(here, "test_set.json"), encoding="utf-8"))
total = Counter()
for t in tests:
    name = t["name"]
    pdf = os.path.join(here, t["dir"], name + ".pdf")
    mxl = os.path.join(here, t["dir"], name + ".mxl")
    doc = fitz.open(pdf)
    heads, digits = {}, []
    for pi, page in enumerate(doc):
        heads[pi] = []
        for b in page.get_text("rawdict")["blocks"]:
            for l in b.get("lines", []):
                for s in l["spans"]:
                    style = (s["font"], round(s["size"], 1))
                    for ch in s["chars"]:
                        cp = ord(ch["c"])
                        x0, y0, x1, y1 = ch["bbox"]
                        if is_head(cp):
                            cls = 7 if cp in WHOLE else 6 if cp in HALF else 5
                            cx, cy, w = (x0 + x1) / 2 * S, ch["origin"][1] * S, (x1 - x0) * S
                            heads[pi].append([cls, 1.0, cx - w / 2, cy - 0.4 * w, cx + w / 2, cy + 0.4 * w])
                        elif ch["c"] in "12345":
                            digits.append((pi, style, int(ch["c"]) - 1, [v * S for v in (x0, y0, x1, y1)]))
    n_visible, grace, cue, hidden, fing = score_notes(mxl)
    counts = Counter(d[1] for d in digits)
    style = min(counts.items(), key=lambda kv: abs(kv[1] - fing))[0] if counts else None
    true_digits = {pi: [[c, 1.0] + box for p, st, c, box in digits if p == pi and st == style] for pi in heads}
    n_heads = sum(len(h) for h in heads.values())
    n_digits = sum(len(d) for d in true_digits.values())
    total.update(heads=n_heads, notes=n_visible, grace=grace, cue=cue, hidden=hidden, fing=fing, digits=n_digits)
    print(f"{name[:44]:44s} pdf heads {n_heads:5d} score notes {n_visible:5d} (grace {grace}, cue {cue}, hidden {hidden})"
          f"  diff {n_heads - n_visible:+5d} | fingering {fing:4d} pdf digits {n_digits:4d}")

    for pi in heads:
        det_path = os.path.join(here, "page_cache", "musescore", name, f"{pi}.det.json")
        detected = json.load(open(det_path)) if os.path.exists(det_path) else []
        det_heads = [d for d in detected if d[0] >= 5]
        det_digits = [d for d in detected if d[0] < 5]
        for kind, objs in (("heads", heads[pi] + det_digits), ("both", heads[pi] + true_digits[pi]),
                           ("digits", det_heads + true_digits[pi])):
            d = os.path.join(here, "page_cache", f"oracle_{kind}", name)
            os.makedirs(d, exist_ok=True)
            json.dump(objs, open(os.path.join(d, f"{pi}.det.json"), "w"))
print(dict(total))
