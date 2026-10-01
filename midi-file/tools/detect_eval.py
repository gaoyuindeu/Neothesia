"""Detection quality on the held-out pieces: the detector on Audiveris' binarized pages,
compared with the digit boxes read from the PDFs.

usage: detect_eval.py <model.pt> [threshold]
"""
import glob
import io
import json
import re
import sys
import zipfile

import numpy as np
import torch
import torch.nn.functional as F
from PIL import Image

src = open("train_digits.py", encoding="utf-8").read().split("if __name__")[0]
src = src.replace("PAGES, OUT = sys.argv[1], sys.argv[2]", "PAGES, OUT = '', ''").replace(
    "STEPS = int(sys.argv[3]) if len(sys.argv) > 3 else 12000", "STEPS = 1").replace("os.makedirs(OUT, exist_ok=True)", "")
ns = {}
exec(compile(src, "train_digits.py", "exec"), ns)
model = ns["Detector"]()
model.load_state_dict(torch.load(sys.argv[1], map_location="cpu"))
model = model.cuda().eval()
threshold = float(sys.argv[2]) if len(sys.argv) > 2 else 0.35
labels = json.load(open("test_labels/labels.json"))

total = dict(tp=0, fp=0, fn=0, wrong_digit=0)
for key in sorted(labels):
    name, page = key.rsplit("_p", 1)
    omr = glob.glob(f"omr_work/{name}/*.omr")
    if not omr:
        continue
    z = zipfile.ZipFile(omr[0])
    sheet = f"sheet#{int(page) + 1}"
    try:
        binary = np.asarray(Image.open(io.BytesIO(z.read(f"{sheet}/BINARY.png"))).convert("L")) < 128
        xml = z.read(f"{sheet}/{sheet}.xml").decode("utf-8")
    except KeyError:
        continue
    il = float(re.search(r'<interline[^>]*main="([\d.]+)"', xml).group(1))
    scale = 21.0 / il
    t = torch.from_numpy(binary.astype(np.float32))[None, None]
    if abs(scale - 1) > 0.02:
        t = (F.interpolate(t, scale_factor=scale, mode="area") >= 0.35).float()
    H, W = t.shape[2:]
    t = F.pad(t, (0, (16 - W % 16) % 16, 0, (16 - H % 16) % 16))
    with torch.no_grad(), torch.autocast("cuda", dtype=torch.bfloat16):
        heat, size = model(t.cuda())
    heat = heat.float()
    peaks = (heat == F.max_pool2d(heat, 3, 1, 1)) & (heat > threshold)
    found = []
    for c, i, j in torch.nonzero(peaks[0]).tolist():
        found.append((heat[0, c, i, j].item(), c + 1, (j * 2 + 1) / scale, (i * 2 + 1) / scale))
    found.sort(reverse=True)
    kept = []
    for f in found:
        if not any(abs(f[2] - k[2]) < 0.3 * il and abs(f[3] - k[3]) < 0.5 * il for k in kept):
            kept.append(f)
    truth = labels[key]
    used = set()
    stats = dict(tp=0, fp=0, fn=0, wrong_digit=0)
    for score, d, x, y in kept:
        hit = None
        for k, (td, x0, y0, x1, y1) in enumerate(truth):
            if k not in used and x0 - 2 <= x <= x1 + 2 and y0 - 2 <= y <= y1 + 2:
                hit = k
                break
        if hit is None:
            stats["fp"] += 1
        else:
            used.add(hit)
            if truth[hit][0] == d:
                stats["tp"] += 1
            else:
                stats["wrong_digit"] += 1
    stats["fn"] = len(truth) - len(used)
    for k in total:
        total[k] += stats[k]
tp, fp, fn, wd = total["tp"], total["fp"], total["fn"], total["wrong_digit"]
print(f"threshold {threshold}: correct {tp}  wrong digit {wd}  false {fp}  missed {fn}  "
      f"precision {tp / max(tp + fp + wd, 1):.3f}  recall {tp / max(tp + fn + wd, 1):.3f}")
