"""Train the page detector: fingering digits 1-5 and note heads (black, half, whole).

CenterNet-like: one heat map per class (peaks at object centers) at half resolution, or a
quarter with ARCH=fast (the model in the app), and
the box size. Input: binarized page at ~21 px between staff lines.

usage: train_detector.py <index.json> <out dir> [steps]
index.json: [{"image": path, "objects": [[class, x0, y0, x1, y1], ...]}, ...]
class 0-4: digit 1-5, 5: black head, 6: half head, 7: whole head.
Writes <out>/detector.onnx (input "page" 1x1xHxW, outputs "heat" 1x8xH/2xW/2 sigmoid,
"size" 1x2xH/2xW/2 box width / height in px / 32), <out>/model.pt and a checkpoint
<out>/model_<step>.pt every 5000 steps.
"""
import json
import math
import os
import random
import sys
import time

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F
from PIL import Image, ImageFilter

INDEX, OUT = sys.argv[1], sys.argv[2]
STEPS = int(sys.argv[3]) if len(sys.argv) > 3 else 30000
CROP = 384
# "fast": stem at half resolution, output at 1/4 (about 5x less compute)
ARCH = os.environ.get("ARCH", "full")
STRIDE = 4 if ARCH == "fast" else 2
NCLS = 8
PASTE = float(os.environ.get("PASTE", "0.5"))
os.makedirs(OUT, exist_ok=True)


class Pages(torch.utils.data.Dataset):
    def __init__(self, items, length, seed):
        self.items, self.length, self.seed = items, length, seed
        self.cache = {}

    def __len__(self):
        return self.length

    def page(self, path):
        if path not in self.cache:
            if len(self.cache) > 64:
                self.cache.clear()
            self.cache[path] = Image.open(path).convert("L")
        return self.cache[path]

    def __getitem__(self, i):
        rng = random.Random(self.seed * 1_000_003 + i)
        item = rng.choice(self.items)
        page = self.page(item["image"])
        objects = item["objects"]
        digits = [o for o in objects if o[0] < 5]
        heads = [o for o in objects if o[0] >= 5]
        s = rng.uniform(0.7, 1.4)
        size = int(CROP / s) + 8
        r = rng.random()
        pool = digits if (r < 0.55 and digits) else heads if (r < 0.8 and heads) else None
        if pool:
            _, x0, y0, x1, y1 = rng.choice(pool)
            cx = (x0 + x1) / 2 + rng.uniform(-size / 2.5, size / 2.5)
            cy = (y0 + y1) / 2 + rng.uniform(-size / 2.5, size / 2.5)
        else:
            cx, cy = rng.uniform(0, page.width), rng.uniform(0, page.height)
        left, top = int(cx - size / 2), int(cy - size / 2)
        im = Image.new("L", (size, size), 255)
        sx0, sy0 = max(left, 0), max(top, 0)
        sx1, sy1 = min(left + size, page.width), min(top + size, page.height)
        if sx1 > sx0 and sy1 > sy0:
            im.paste(page.crop((sx0, sy0, sx1, sy1)), (sx0 - left, sy0 - top))

        objects = list(objects)
        # Copy-paste of fingering digits, also onto staff lines
        if digits and rng.random() < PASTE:
            a0 = np.asarray(im).copy()
            for _ in range(rng.randint(1, 4)):
                c, x0, y0, x1, y1 = rng.choice(digits)
                patch = np.asarray(page.crop((x0, y0, x1 + 1, y1 + 1)))
                ph, pw = patch.shape
                if pw >= size or ph >= size:
                    continue
                px, py = rng.randrange(0, size - pw), rng.randrange(0, size - ph)
                nb = [c, left + px, top + py, left + px + pw - 1, top + py + ph - 1]
                if any(nb[1] <= b[3] + 4 and b[1] <= nb[3] + 4 and nb[2] <= b[4] + 4 and b[2] <= nb[4] + 4 for b in objects):
                    continue
                a0[py:py + ph, px:px + pw] = np.minimum(a0[py:py + ph, px:px + pw], patch)
                objects.append(nb)
            im = Image.fromarray(a0)
        angle = rng.uniform(-1.2, 1.2)
        im = im.rotate(angle, resample=Image.BILINEAR, fillcolor=255)
        im = im.resize((int(size * s), int(size * s)), Image.BILINEAR)
        off = ((im.width - CROP) // 2, (im.height - CROP) // 2)
        im = im.crop((off[0], off[1], off[0] + CROP, off[1] + CROP))

        # Scan-like degradation, then binarization
        r = rng.random()
        if r < 0.3:
            im = im.filter(ImageFilter.MinFilter(3))
        elif r < 0.4:
            im = im.filter(ImageFilter.MaxFilter(3))
        a = np.asarray(im, dtype=np.float32)
        if rng.random() < 0.6:
            a = np.asarray(Image.fromarray(a.astype(np.uint8)).filter(ImageFilter.GaussianBlur(rng.uniform(0.3, 1.3))), dtype=np.float32)
            a = a + np.random.default_rng(rng.randrange(1 << 30)).normal(0, rng.uniform(0, 25), a.shape)
        ink = (a < rng.uniform(100, 170)).astype(np.float32)
        if rng.random() < 0.3:
            noise = np.random.default_rng(rng.randrange(1 << 30)).random(ink.shape) < rng.uniform(0, 0.003)
            ink = np.maximum(ink, noise.astype(np.float32))

        H = W = CROP // STRIDE
        heat = np.zeros((NCLS, H, W), np.float32)
        wh = np.zeros((2, H, W), np.float32)
        mask = np.zeros((H, W), np.float32)
        cos, sin = math.cos(math.radians(-angle)), math.sin(math.radians(-angle))
        for c, x0, y0, x1, y1 in objects:
            bx, by = (x0 + x1) / 2 - cx, (y0 + y1) / 2 - cy
            rx, ry = bx * cos - by * sin, bx * sin + by * cos
            px, py = (rx * s + CROP / 2) / STRIDE, (ry * s + CROP / 2) / STRIDE
            bw, bh = (x1 - x0) * s, (y1 - y0) * s
            if not (0 <= px < W and 0 <= py < H):
                continue
            sigma = max(min(bw, bh) / STRIDE / 6, 1.0)
            ix, iy = int(px), int(py)
            # Only around the object: the gaussian is ~0 beyond 3 sigma
            r = int(3 * sigma) + 1
            x0, x1, y0, y1 = max(ix - r, 0), min(ix + r + 1, W), max(iy - r, 0), min(iy + r + 1, H)
            gx = np.arange(x0, x1)[None, :]
            gy = np.arange(y0, y1)[:, None]
            g = np.exp(-((gx - px) ** 2 + (gy - py) ** 2) / (2 * sigma ** 2))
            heat[c, y0:y1, x0:x1] = np.maximum(heat[c, y0:y1, x0:x1], g)
            heat[c, iy, ix] = 1.0
            wh[:, iy, ix] = (bw / 32, bh / 32)
            mask[iy, ix] = 1.0
        return torch.from_numpy(ink[None]), torch.from_numpy(heat), torch.from_numpy(wh), torch.from_numpy(mask)


def block(cin, cout, stride=1):
    return nn.Sequential(
        nn.Conv2d(cin, cout, 3, stride, 1, bias=False), nn.BatchNorm2d(cout), nn.ReLU(inplace=True),
        nn.Conv2d(cout, cout, 3, 1, 1, bias=False), nn.BatchNorm2d(cout), nn.ReLU(inplace=True))


class FastDetector(nn.Module):
    def __init__(self, c=(24, 32, 64, 96, 128)):
        super().__init__()
        self.stem = nn.Sequential(nn.Conv2d(1, c[0], 3, 2, 1, bias=False), nn.BatchNorm2d(c[0]), nn.ReLU(inplace=True))  # 1/2
        self.e1 = block(c[0], c[1], 2)     # 1/4
        self.e2 = block(c[1], c[2], 2)     # 1/8
        self.e3 = block(c[2], c[3], 2)     # 1/16
        self.e4 = block(c[3], c[4], 2)     # 1/32
        self.d3 = block(c[4] + c[3], c[3])
        self.d2 = block(c[3] + c[2], c[2])
        self.d1 = block(c[2] + c[1], c[1])
        self.heat = nn.Conv2d(c[1], NCLS, 1)
        self.size = nn.Conv2d(c[1], 2, 1)
        self.heat.bias.data.fill_(-2.19)

    def forward(self, page):
        x1 = self.e1(self.stem(page))
        x2 = self.e2(x1)
        x3 = self.e3(x2)
        x4 = self.e4(x3)
        up = lambda t: F.interpolate(t, scale_factor=2.0, mode="nearest")
        y = self.d3(torch.cat([up(x4), x3], 1))
        y = self.d2(torch.cat([up(y), x2], 1))
        y = self.d1(torch.cat([up(y), x1], 1))
        return torch.sigmoid(self.heat(y)), self.size(y)


class Detector(nn.Module):
    def __init__(self, c=(16, 32, 64, 96, 128)):
        super().__init__()
        self.e0 = block(1, c[0])
        self.e1 = block(c[0], c[1], 2)
        self.e2 = block(c[1], c[2], 2)
        self.e3 = block(c[2], c[3], 2)
        self.e4 = block(c[3], c[4], 2)
        self.d3 = block(c[4] + c[3], c[3])
        self.d2 = block(c[3] + c[2], c[2])
        self.d1 = block(c[2] + c[1], c[1])
        self.heat = nn.Conv2d(c[1], NCLS, 1)
        self.size = nn.Conv2d(c[1], 2, 1)
        self.heat.bias.data.fill_(-2.19)

    def forward(self, page):
        x0 = self.e0(page)
        x1 = self.e1(x0)
        x2 = self.e2(x1)
        x3 = self.e3(x2)
        x4 = self.e4(x3)
        up = lambda t: F.interpolate(t, scale_factor=2.0, mode="nearest")
        y = self.d3(torch.cat([up(x4), x3], 1))
        y = self.d2(torch.cat([up(y), x2], 1))
        y = self.d1(torch.cat([up(y), x1], 1))
        return torch.sigmoid(self.heat(y)), self.size(y)


def focal(pred, target):
    pred = pred.clamp(1e-4, 1 - 1e-4)
    pos = target.eq(1).float()
    neg = 1 - pos
    pos_loss = torch.log(pred) * (1 - pred) ** 2 * pos
    neg_loss = torch.log(1 - pred) * pred ** 2 * (1 - target) ** 4 * neg
    return -(pos_loss.sum() + neg_loss.sum()) / pos.sum().clamp(min=1)


@torch.no_grad()
def evaluate(model, loader, dev):
    model.eval()
    stats = {"digits": [0, 0, 0], "heads": [0, 0, 0]}  # tp, fp, fn
    for ink, heat, wh, mask in loader:
        ph, _ = model(ink.to(dev))
        ph = ph.float().cpu()
        peaks = (ph == F.max_pool2d(ph, 3, 1, 1)) & (ph > 0.4)
        for b in range(ph.shape[0]):
            for group, classes in (("digits", range(0, 5)), ("heads", range(5, 8))):
                truth = [t for t in torch.nonzero(heat[b] == 1).tolist() if t[0] in classes]
                found = [f for f in torch.nonzero(peaks[b]).tolist() if f[0] in classes]
                used = set()
                for c, y, x in found:
                    hit = next((k for k, (tc, ty, tx) in enumerate(truth)
                                if k not in used and tc == c and abs(ty - y) <= 2 and abs(tx - x) <= 2), None)
                    if hit is None:
                        stats[group][1] += 1
                    else:
                        used.add(hit)
                        stats[group][0] += 1
                stats[group][2] += len(truth) - len(used)
    for g, (tp, fp, fn) in stats.items():
        print(f"  val {g}: precision {tp / max(tp + fp, 1):.3f} recall {tp / max(tp + fn, 1):.3f}", flush=True)
    model.train()


def export(model):
    model.eval().cpu()
    torch.onnx.export(model, torch.zeros(1, 1, 512, 512), os.path.join(OUT, "detector.onnx"),
                      input_names=["page"], output_names=["heat", "size"], opset_version=17, dynamo=False,
                      dynamic_axes={"page": {2: "h", 3: "w"}, "heat": {2: "h2", 3: "w2"}, "size": {2: "h2", 3: "w2"}})
    print("exported", flush=True)


def main():
    items = json.load(open(INDEX, encoding="utf-8"))
    groups = sorted({it["piece"] for it in items})
    random.Random(0).shuffle(groups)
    val_groups = set(groups[: max(1, len(groups) // 12)])
    train = [it for it in items if it["piece"] not in val_groups]
    val = [it for it in items if it["piece"] in val_groups]
    print(f"train pages {len(train)}, val pages {len(val)}", flush=True)
    batch = 12
    loader = torch.utils.data.DataLoader(Pages(train, STEPS * batch, 1), batch_size=batch, num_workers=6,
                                         persistent_workers=True)
    val_loader = torch.utils.data.DataLoader(Pages(val, 240, 2), batch_size=batch, num_workers=2)
    dev = torch.device("cuda")
    model = (FastDetector() if ARCH == "fast" else Detector()).to(dev)
    print("parameters", sum(p.numel() for p in model.parameters()), flush=True)
    opt = torch.optim.AdamW(model.parameters(), lr=2e-3, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, max_lr=2e-3, total_steps=STEPS, pct_start=0.05)
    t0 = time.time()
    for step, (ink, heat, wh, mask) in enumerate(loader):
        ink, heat, wh, mask = ink.to(dev), heat.to(dev), wh.to(dev), mask.to(dev)
        with torch.autocast("cuda", dtype=torch.bfloat16):
            ph, ps = model(ink)
        lh = focal(ph.float(), heat)
        ls = (F.l1_loss(ps.float(), wh, reduction="none") * mask[:, None]).sum() / mask.sum().clamp(min=1)
        loss = lh + 0.5 * ls
        opt.zero_grad(set_to_none=True)
        loss.backward()
        opt.step()
        sched.step()
        if step % 500 == 0:
            print(f"step {step} loss {loss.item():.3f} heat {lh.item():.3f} size {ls.item():.3f} "
                  f"{time.time() - t0:.0f}s", flush=True)
        if step % 5000 == 4999 or step == STEPS - 1:
            evaluate(model, val_loader, dev)
            torch.save(model.state_dict(), os.path.join(OUT, "model.pt"))
            torch.save(model.state_dict(), os.path.join(OUT, f"model_{step + 1}.pt"))
    export(model)


if __name__ == "__main__":
    main()
