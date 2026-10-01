"""Train the fingering digit detector (CenterNet-like heatmaps on binarized pages).

usage: train_digits.py <pages dir> <out dir> [steps]
Writes <out>/digits.onnx (input "page": 1x1xHxW, ink = 1; outputs "heat": 1x5xH/2xW/2
sigmoid, "size": 1x2xH/2xW/2 box width/height in pixels / 32) and <out>/model.pt.
"""
import io
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

PAGES, OUT = sys.argv[1], sys.argv[2]
STEPS = int(sys.argv[3]) if len(sys.argv) > 3 else 12000
CROP = 384
PASTE = float(os.environ.get("PASTE", "0.5"))
STRIDE = 2
os.makedirs(OUT, exist_ok=True)


# ---------------------------------------------------------------- data

class Pages(torch.utils.data.Dataset):
    def __init__(self, names, labels, length, seed):
        self.names, self.labels, self.length = names, labels, length
        self.seed = seed
        self.cache = {}

    def __len__(self):
        return self.length

    def page(self, name):
        if name not in self.cache:
            self.cache[name] = Image.open(os.path.join(PAGES, name)).convert("L")
        return self.cache[name]

    def __getitem__(self, i):
        rng = random.Random(self.seed * 1_000_003 + i)
        name = rng.choice(self.names)
        page = self.page(name)
        boxes = self.labels[name]
        s = rng.uniform(0.7, 1.4)
        size = int(CROP / s) + 8
        # Mostly around a digit, sometimes anywhere
        if boxes and rng.random() < 0.75:
            _, x0, y0, x1, y1 = rng.choice(boxes)
            cx = (x0 + x1) / 2 + rng.uniform(-size / 2.5, size / 2.5)
            cy = (y0 + y1) / 2 + rng.uniform(-size / 2.5, size / 2.5)
        else:
            cx, cy = rng.uniform(0, page.width), rng.uniform(0, page.height)
        left, top = int(cx - size / 2), int(cy - size / 2)
        # White outside the page (PIL would fill with black)
        im = Image.new("L", (size, size), 255)
        sx0, sy0 = max(left, 0), max(top, 0)
        sx1, sy1 = min(left + size, page.width), min(top + size, page.height)
        if sx1 > sx0 and sy1 > sy0:
            im.paste(page.crop((sx0, sy0, sx1, sy1)), (sx0 - left, sy0 - top))

        # Copy-paste: fingering of this page put elsewhere, also on staff lines
        if boxes and rng.random() < PASTE:
            a0 = np.asarray(im).copy()
            boxes = list(boxes)
            for _ in range(rng.randint(1, 4)):
                d, x0, y0, x1, y1 = rng.choice(self.labels[name])
                patch = np.asarray(page.crop((x0, y0, x1 + 1, y1 + 1)))
                ph, pw = patch.shape
                if pw >= size or ph >= size:
                    continue
                px, py = rng.randrange(0, size - pw), rng.randrange(0, size - ph)
                nb = [d, left + px, top + py, left + px + pw - 1, top + py + ph - 1]
                if any(nb[1] <= b[3] + 4 and b[1] <= nb[3] + 4 and nb[2] <= b[4] + 4 and b[2] <= nb[4] + 4 for b in boxes):
                    continue
                a0[py:py + ph, px:px + pw] = np.minimum(a0[py:py + ph, px:px + pw], patch)
                boxes.append(nb)
            im = Image.fromarray(a0)
        angle = rng.uniform(-1.2, 1.2)
        im = im.rotate(angle, resample=Image.BILINEAR, fillcolor=255)
        im = im.resize((int(size * s), int(size * s)), Image.BILINEAR)
        off = ((im.width - CROP) // 2, (im.height - CROP) // 2)
        im = im.crop((off[0], off[1], off[0] + CROP, off[1] + CROP))

        # Scan-like degradation, then binarization like the OMR does it
        r = rng.random()
        if r < 0.3:
            im = im.filter(ImageFilter.MinFilter(3))  # bolder
        elif r < 0.4:
            im = im.filter(ImageFilter.MaxFilter(3))  # thinner
        a = np.asarray(im, dtype=np.float32)
        if rng.random() < 0.6:
            a = np.asarray(Image.fromarray(a.astype(np.uint8)).filter(ImageFilter.GaussianBlur(rng.uniform(0.3, 1.3))), dtype=np.float32)
            a = a + np.random.default_rng(rng.randrange(1 << 30)).normal(0, rng.uniform(0, 25), a.shape)
        threshold = rng.uniform(100, 170)
        ink = (a < threshold).astype(np.float32)
        if rng.random() < 0.3:
            noise = np.random.default_rng(rng.randrange(1 << 30)).random(ink.shape) < rng.uniform(0, 0.003)
            ink = np.maximum(ink, noise.astype(np.float32))

        # Targets at 1/STRIDE
        H = W = CROP // STRIDE
        heat = np.zeros((5, H, W), np.float32)
        wh = np.zeros((2, H, W), np.float32)
        mask = np.zeros((H, W), np.float32)
        cos, sin = math.cos(math.radians(-angle)), math.sin(math.radians(-angle))
        pcx, pcy = cx, cy
        for d, x0, y0, x1, y1 in boxes:
            bx, by = (x0 + x1) / 2 - pcx, (y0 + y1) / 2 - pcy
            # rotation about the crop center (PIL rotates counter-clockwise)
            rx, ry = bx * cos - by * sin, bx * sin + by * cos
            px, py = (rx * s + CROP / 2) / STRIDE, (ry * s + CROP / 2) / STRIDE
            bw, bh = (x1 - x0) * s, (y1 - y0) * s
            if not (0 <= px < W and 0 <= py < H):
                continue
            sigma = max(bh / STRIDE / 6, 1.0)
            ix, iy = int(px), int(py)
            ys, xs = np.ogrid[:H, :W]
            g = np.exp(-((xs - px) ** 2 + (ys - py) ** 2) / (2 * sigma ** 2))
            heat[d - 1] = np.maximum(heat[d - 1], g)
            heat[d - 1, iy, ix] = 1.0
            wh[:, iy, ix] = (bw / 32, bh / 32)
            mask[iy, ix] = 1.0
        return torch.from_numpy(ink[None]), torch.from_numpy(heat), torch.from_numpy(wh), torch.from_numpy(mask)


# ---------------------------------------------------------------- model

def block(cin, cout, stride=1):
    return nn.Sequential(
        nn.Conv2d(cin, cout, 3, stride, 1, bias=False), nn.BatchNorm2d(cout), nn.ReLU(inplace=True),
        nn.Conv2d(cout, cout, 3, 1, 1, bias=False), nn.BatchNorm2d(cout), nn.ReLU(inplace=True))


class Detector(nn.Module):
    def __init__(self, c=(16, 32, 64, 96, 128)):
        super().__init__()
        self.e0 = block(1, c[0])           # 1
        self.e1 = block(c[0], c[1], 2)     # 1/2
        self.e2 = block(c[1], c[2], 2)     # 1/4
        self.e3 = block(c[2], c[3], 2)     # 1/8
        self.e4 = block(c[3], c[4], 2)     # 1/16
        self.d3 = block(c[4] + c[3], c[3])
        self.d2 = block(c[3] + c[2], c[2])
        self.d1 = block(c[2] + c[1], c[1])
        self.heat = nn.Conv2d(c[1], 5, 1)
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
    n = pos.sum().clamp(min=1)
    return -(pos_loss.sum() + neg_loss.sum()) / n


# ---------------------------------------------------------------- training

def main():
    labels = json.load(open(os.path.join(PAGES, "labels.json")))
    pieces = sorted({n.split("_")[0] for n in labels})
    random.Random(0).shuffle(pieces)
    val_pieces = set(pieces[: max(1, len(pieces) // 10)])
    train = [n for n in labels if n.split("_")[0] not in val_pieces]
    val = [n for n in labels if n.split("_")[0] in val_pieces]
    print(f"train pages {len(train)}, val pages {len(val)}", flush=True)

    batch = 12
    loader = torch.utils.data.DataLoader(Pages(train, labels, STEPS * batch, 1), batch_size=batch,
                                         num_workers=6, persistent_workers=True)
    val_loader = torch.utils.data.DataLoader(Pages(val, labels, 240, 2), batch_size=batch, num_workers=2)
    dev = torch.device("cuda")
    model = Detector().to(dev)
    print("parameters", sum(p.numel() for p in model.parameters()), flush=True)
    opt = torch.optim.AdamW(model.parameters(), lr=2e-3, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, max_lr=2e-3, total_steps=STEPS, pct_start=0.05)
    scaler = torch.amp.GradScaler()
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
        if step % 200 == 0:
            print(f"step {step} loss {loss.item():.3f} heat {lh.item():.3f} size {ls.item():.3f} "
                  f"{time.time() - t0:.0f}s", flush=True)
        if step % 2000 == 1999 or step == STEPS - 1:
            evaluate(model, val_loader, dev)
            torch.save(model.state_dict(), os.path.join(OUT, "model.pt"))
    export(model)


@torch.no_grad()
def evaluate(model, loader, dev):
    model.eval()
    tp = fp = fn = 0
    for ink, heat, wh, mask in loader:
        ph, _ = model(ink.to(dev))
        ph = ph.float().cpu()
        peaks = (ph == F.max_pool2d(ph, 3, 1, 1)) & (ph > 0.4)
        for b in range(ph.shape[0]):
            truth = torch.nonzero(heat[b] == 1)  # (digit, y, x)
            found = torch.nonzero(peaks[b])
            used = set()
            for d, y, x in found.tolist():
                hit = None
                for k, (td, ty, tx) in enumerate(truth.tolist()):
                    if k not in used and td == d and abs(ty - y) <= 2 and abs(tx - x) <= 2:
                        hit = k
                        break
                if hit is None:
                    fp += 1
                else:
                    used.add(hit)
                    tp += 1
            fn += len(truth) - len(used)
    print(f"  val: precision {tp / max(tp + fp, 1):.3f} recall {tp / max(tp + fn, 1):.3f} ({tp} found)", flush=True)
    model.train()


def export(model):
    model.eval().cpu()
    dummy = torch.zeros(1, 1, 512, 512)
    torch.onnx.export(model, dummy, os.path.join(OUT, "digits.onnx"), input_names=["page"],
                      output_names=["heat", "size"], opset_version=17, dynamo=False,
                      dynamic_axes={"page": {2: "h", 3: "w"}, "heat": {2: "h2", 3: "w2"}, "size": {2: "h2", 3: "w2"}})
    print("exported", flush=True)


if __name__ == "__main__":
    main()
