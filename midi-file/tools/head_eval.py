"""Note head detection on the MuseScore pages of the evaluation pieces (heads read from the
PDF text layer), for several detector checkpoints.

usage: head_eval.py <model.pt> [<model.pt> ...]
"""
import glob
import json
import os
import sys

import fitz
import numpy as np
import torch
import torch.nn.functional as F

here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, "train_detector.py"), encoding="utf-8").read().split("if __name__")[0]
models = sys.argv[1:]
sys.argv = ["x", "detector_index.json", os.path.join(here, "tmp_out"), "1"]
ns = {}
exec(compile(src, "train_detector.py", "exec"), ns)

HEADS = {0xE0A4, 0xE0A3, 0xE0A2, 0xF4BE, 0xF4BD, 0xF4BC}  # F4xx: Bravura in MuseScore 3
tests = json.load(open(os.path.join(here, "test_set.json"), encoding="utf-8"))
pages = []
for t in tests[::int(os.environ.get("EVERY", "3"))]:
    doc = fitz.open(os.path.join(here, t["dir"], t["name"] + ".pdf"))
    for page in list(doc)[:int(os.environ.get("PAGES", "2"))]:
        heads = []
        for b in page.get_text("rawdict")["blocks"]:
            for l in b.get("lines", []):
                for s in l["spans"]:
                    for ch in s["chars"]:
                        if ord(ch["c"]) in HEADS:
                            x0, _, x1, _ = ch["bbox"]
                            heads.append(((x0 + x1) / 2 * 300 / 72, ch["origin"][1] * 300 / 72))
        pix = page.get_pixmap(dpi=300, colorspace=fitz.csGRAY)
        g = np.frombuffer(pix.samples, np.uint8).reshape(pix.height, pix.width)
        pages.append((g, heads, t["name"]))
print("pages", len(pages), "heads", sum(len(h) for _, h, _ in pages))

for name in models:
    m = ns["FastDetector"]() if os.environ.get("ARCH") == "fast" else ns["Detector"]()
    m.load_state_dict(torch.load(name, map_location="cpu"))
    m = m.cuda().eval()
    tp = fp = fn = 0
    worst = []
    for g, truth, piece in pages:
        tp0, fn0 = tp, fn
        ink = torch.from_numpy((g < 140).astype(np.float32))[None, None]
        H, W = ink.shape[2:]
        ink = F.pad(ink, (0, (32 - W % 32) % 32, 0, (32 - H % 32) % 32))
        with torch.no_grad(), torch.autocast("cuda", dtype=torch.bfloat16):
            heat, _ = m(ink.cuda())
        hm = heat.float()[0, 5:8].max(0).values
        peaks = (hm == F.max_pool2d(hm[None], 3, 1, 1)[0]) & (hm > 0.35)
        st = ink.shape[2] // heat.shape[2]
        found = [(j * st + st / 2, i * st + st / 2) for i, j in torch.nonzero(peaks).tolist()]
        used = set()
        for x, y in found:
            hit = next((k for k, (tx, ty) in enumerate(truth) if k not in used and abs(tx - x) < 12 and abs(ty - y) < 10), None)
            if hit is None:
                fp += 1
            else:
                used.add(hit)
                tp += 1
        fn += len(truth) - len(used)
        if truth:
            worst.append(((tp - tp0) / len(truth), piece[:40]))
    worst.sort()
    print("  worst pages:", ", ".join(f"{r:.2f} {n}" for r, n in worst[:5]))
    print(f"{name}: precision {tp / max(tp + fp, 1):.3f} recall {tp / max(tp + fn, 1):.3f}")
