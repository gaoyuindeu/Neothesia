"""Accuracy of each part of the score reader with the other parts exact, on the MuseScore test
PDFs (exact digits and heads from the PDF text layer, make_oracle.py):

1. digit detection: detected digits vs text layer digits (fingering style)
2. head detection: detected heads vs text layer heads
(detections: page_cache/<cache>/<piece>/<n>.det.json, score >= 0.35 as in the reader)

usage: component_eval.py <detections cache, e.g. v5_musescore>
"""
import glob
import json
import os
import sys

here = os.path.dirname(os.path.abspath(__file__))
cache = sys.argv[1]
THRESHOLD = 0.35
tests = json.load(open(os.path.join(here, "test_set.json"), encoding="utf-8"))


def match(found, truth, il, same_class):
    """Greedy matching by distance of centers (< 0.6 staff space)"""
    c = lambda o: ((o[2] + o[4]) / 2, (o[3] + o[5]) / 2)
    used = set()
    tp = wrong_class = 0
    for f in sorted(found, key=lambda o: -o[1]):
        fx, fy = c(f)
        best = None
        for k, t in enumerate(truth):
            if k in used:
                continue
            tx, ty = c(t)
            d = ((fx - tx) ** 2 + (fy - ty) ** 2) ** 0.5
            if d < 0.6 * il and (best is None or d < best[0]):
                best = (d, k)
        if best:
            used.add(best[1])
            if not same_class or truth[best[1]][0] == f[0]:
                tp += 1
            else:
                wrong_class += 1
    return tp, wrong_class, len(found), len(truth)


tot = {"digits": [0, 0, 0, 0], "heads": [0, 0, 0, 0]}
for t in tests:
    name = t["name"]
    for oracle in sorted(glob.glob(os.path.join(here, "page_cache", os.environ.get("ORACLE", "oracle_both"), name, "*.det.json"))):
        page = os.path.basename(oracle).split(".")[0]
        det = os.path.join(here, "page_cache", cache, name, f"{page}.det.json")
        meta = os.path.join(here, "page_cache", os.environ.get("META", "musescore"), name, f"{page}.json")
        if not os.path.exists(det) or not os.path.exists(meta):
            continue
        il = json.load(open(meta))["interline"]
        truth = json.load(open(oracle))
        found = [o for o in json.load(open(det)) if o[1] >= THRESHOLD]
        for kind, sel, same in (("digits", lambda o: o[0] < 5, True), ("heads", lambda o: o[0] >= 5, False)):
            r = match([o for o in found if sel(o)], [o for o in truth if sel(o)], il, same)
            tot[kind] = [a + b for a, b in zip(tot[kind], r)]

for kind, (tp, wrong, n_found, n_truth) in tot.items():
    print(f"{kind}: truth {n_truth}, found {n_found}, right {tp} -> recall {tp / n_truth:.1%}, "
          f"precision {tp / n_found:.1%}" + (f", place right but digit wrong {wrong}" if kind == "digits" else ""))
