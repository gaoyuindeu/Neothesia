"""Run the detector on the GPU over pages written by the Rust reader
(SCORE_READER_PAGES_OUT), writing <n>.det.json next to them: [[class, score, x0, y0, x1, y1]].
Post-processing as in score_reader/detect.rs.

usage: detect_pages.py <model.pt> <arch full|fast> <dir> [<dir> ...]
"""
import glob
import json
import os
import sys

import numpy as np
import torch
import torch.nn.functional as F
from PIL import Image

here = os.path.dirname(os.path.abspath(__file__))
model_path, arch, dirs = sys.argv[1], sys.argv[2], sys.argv[3:]
os.environ["ARCH"] = arch
sys.argv = ["x", "detector_index.json", os.path.join(here, "tmp_out"), "1"]
ns = {}
exec(compile(open(os.path.join(here, "train_detector.py"), encoding="utf-8").read().split("if __name__")[0],
             "train_detector.py", "exec"), ns)
model = ns["FastDetector"]() if arch == "fast" else ns["Detector"]()
model.load_state_dict(torch.load(model_path, map_location="cpu"))
model = model.cuda().eval()
INTERLINE = 21.0

for d in dirs:
    for png in sorted(glob.glob(os.path.join(d, "*.png"))):
        n = os.path.basename(png)[:-4]
        il = json.load(open(os.path.join(d, n + ".json")))["interline"]
        ink = torch.from_numpy((np.asarray(Image.open(png)) < 128).astype(np.float32))[None, None]
        scale = INTERLINE / max(il, 1.0)
        if abs(scale - 1) >= 0.02:
            ink = (F.interpolate(ink, scale_factor=scale, mode="area") >= 0.35).float()
        H, W = ink.shape[2:]
        ink = F.pad(ink, (0, (32 - W % 32) % 32, 0, (32 - H % 32) % 32))
        with torch.no_grad():
            heat, size = model(ink.cuda())
        heat, size = heat.float(), size.float()
        stride = ink.shape[2] // heat.shape[2]
        peaks = (heat == F.max_pool2d(heat, 3, 1, 1)) & (heat > 0.1)
        found = []
        for c, i, j in torch.nonzero(peaks[0]).tolist():
            v = heat[0, c, i, j].item()
            cx, cy = (j * stride + stride / 2) / scale, (i * stride + stride / 2) / scale
            bw = max(size[0, 0, i, j].item() * 32, 2) / scale
            bh = max(size[0, 1, i, j].item() * 32, 2) / scale
            found.append((v, c, cx - bw / 2, cy - bh / 2, cx + bw / 2, cy + bh / 2))
        found.sort(reverse=True)
        kept = []
        for f in found:
            fx, fy = (f[2] + f[4]) / 2, (f[3] + f[5]) / 2
            clash = any((k[1] >= 5) == (f[1] >= 5)
                        and abs(fx - (k[2] + k[4]) / 2) < (k[4] - k[2]) / 2
                        and abs(fy - (k[3] + k[5]) / 2) < (k[5] - k[3]) / 2 for k in kept)
            if not clash:
                kept.append(f)
        json.dump([[k[1], k[0], k[2], k[3], k[4], k[5]] for k in kept], open(os.path.join(d, n + ".det.json"), "w"))
