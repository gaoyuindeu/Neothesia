"""Train the association model of the score reader: does a stack of fingering digits belong
to a chord? Pairs come from score_reader_eval with SCORE_READER_PAIRS (run_pairs.sh), on
training pieces read with exact heads and digits (PDF text layer): a pair is right when the
stack's digits are fingers of the chord's notes in the score.

usage: train_assoc.py <pairs dir> <out assoc.bin> [epochs]
"""
import glob
import json
import os
import random
import struct
import sys

import numpy as np
import torch
import torch.nn as nn

pairs_dir, out_path = sys.argv[1], sys.argv[2]
EPOCHS = int(sys.argv[3]) if len(sys.argv) > 3 else 40

pieces = []
for f in sorted(glob.glob(os.path.join(pairs_dir, "*.jsonl"))):
    rows = [json.loads(l) for l in open(f, encoding="utf-8") if l.strip()]
    rows = [r for r in rows if r["known"]]
    if not rows:
        continue
    # One right chord per stack: of several (a neighbour with the same fingers), the one
    # most in line with the stack
    groups = {}
    for r in rows:
        r["y"] = int(r["matched"] == r["size"])
        groups.setdefault((r["page"], r["stack"]), []).append(r)
    for g in groups.values():
        pos = [r for r in g if r["y"]]
        if len(pos) > 1:
            best = min(pos, key=lambda r: r["features"][1])
            for r in pos:
                r["y"] = int(r is best)
    pieces.append((os.path.basename(f), groups))

# Split by piece: the versions of a piece (prefix o_ / r_ / v_: exact detections, detector on
# the MuseScore PDF, on the Verovio engraving) on the same side
base = lambda f: f.split("_", 1)[1] if f[:2] in ("o_", "r_", "v_", "s_", "w_", "t_", "u_") else f
names = sorted({base(f) for f, _ in pieces})
random.Random(0).shuffle(names)
val_names = set(names[: max(1, len(names) // 8)])
val = [p for p in pieces if base(p[0]) in val_names]
train = [p for p in pieces if base(p[0]) not in val_names]


def flatten(ps):
    X, Y, G = [], [], []
    for gi, (_, groups) in enumerate(ps):
        for key, g in groups.items():
            for r in g:
                X.append(r["features"])
                Y.append(r["y"])
                G.append((gi, key))
    return np.array(X, np.float32), np.array(Y, np.float32), G


Xt, Yt, _ = flatten(train)
Xv, Yv, Gv = flatten(val)
print(f"train pairs {len(Xt)} (right {Yt.mean():.2f}), val pairs {len(Xv)}")
mean, std = Xt.mean(0), Xt.std(0) + 1e-6

torch.manual_seed(0)
net = nn.Sequential(nn.Linear(Xt.shape[1], 32), nn.ReLU(), nn.Linear(32, 16), nn.ReLU(), nn.Linear(16, 1))
opt = torch.optim.Adam(net.parameters(), lr=3e-3, weight_decay=1e-5)
xt = torch.from_numpy((Xt - mean) / std)
yt = torch.from_numpy(Yt)[:, None]
xv = torch.from_numpy((Xv - mean) / std)
loss_fn = nn.BCEWithLogitsLoss()


def stack_accuracy(p):
    # Per stack: is the most likely chord the right one (stacks with a right chord)
    best, right = {}, {}
    for i, key in enumerate(Gv):
        if key not in best or p[i] > p[best[key]]:
            best[key] = i
        if Yv[i]:
            right[key] = i
    hits = sum(best[k] == right[k] for k in right)
    return hits / max(len(right), 1), len(right)


for epoch in range(EPOCHS):
    perm = torch.randperm(len(xt))
    for b in range(0, len(xt), 512):
        idx = perm[b:b + 512]
        opt.zero_grad()
        loss = loss_fn(net(xt[idx]), yt[idx])
        loss.backward()
        opt.step()
    if epoch % 10 == 9 or epoch == EPOCHS - 1:
        with torch.no_grad():
            pv = torch.sigmoid(net(xv))[:, 0].numpy()
        acc, n = stack_accuracy(pv)
        vloss = loss_fn(torch.from_numpy(np.log(pv / (1 - pv + 1e-9) + 1e-9))[:, None], torch.from_numpy(Yv)[:, None]).item()
        print(f"epoch {epoch + 1}: train loss {loss.item():.3f} val stacks right {acc:.3f} of {n}")

# Baseline: the hand made score (feature 17, lower is better)
acc, n = stack_accuracy(-Xv[:, 17])
print(f"hand made score: val stacks right {acc:.3f} of {n}")

with open(out_path, "wb") as f:
    f.write(struct.pack("<I", Xt.shape[1]))
    f.write(mean.astype("<f4").tobytes())
    f.write(std.astype("<f4").tobytes())
    layers = [m for m in net if isinstance(m, nn.Linear)]
    f.write(struct.pack("<I", len(layers)))
    for m in layers:
        f.write(struct.pack("<II", m.in_features, m.out_features))
        f.write(m.weight.detach().numpy().astype("<f4").tobytes())
        f.write(m.bias.detach().numpy().astype("<f4").tobytes())
print("wrote", out_path)
