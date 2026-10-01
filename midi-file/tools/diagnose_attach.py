"""Why fingered notes did not get their digit (part 3 of the reader, digits to notes): for each
note whose finger was missed or wrong, the printed digit that is its finger and what happened to
it. Input: per piece <name>.json (SCORE_READER_LOST) and <name>.jsonl (SCORE_READER_PAIRS).

usage: diagnose_attach.py <dir> [examples per reason]
"""
import collections
import glob
import json
import os
import sys

d_in = sys.argv[1]
show = int(sys.argv[2]) if len(sys.argv) > 2 else 0
center = lambda b: ((b[0] + b[2]) / 2, (b[1] + b[3]) / 2)
same = lambda a, b: all(abs(x - y) < 1.5 for x, y in zip(a, b))

reasons = collections.Counter()
examples = collections.defaultdict(list)
for f in sorted(glob.glob(os.path.join(d_in, "*.json"))):
    name = os.path.basename(f)[:-5]
    data = json.load(open(f, encoding="utf-8"))
    pairs_path = os.path.join(d_in, name + ".jsonl")
    pairs = [json.loads(l) for l in open(pairs_path, encoding="utf-8")] if os.path.exists(pairs_path) else []
    detected = [s for s in data["stages"] if s["stage"] == "detected"]
    after_numbers = [s for s in data["stages"] if s["stage"] == "numbers"]
    for l in data["lost"]:
        if l["kind"] not in ("no digit", "other digit") or not l["head"]:
            continue
        page, h = l["head"]
        il = (h[3] - h[1]) / 0.8
        hx, hy = center(h)
        cand = [g for g in detected if g["page"] == page and g["digit"] == l["finger"]
                and abs(center(g["box"])[0] - hx) < 1.5 * il and abs(center(g["box"])[1] - hy) < 12 * il]
        if not cand:
            reasons["not printed near the note"] += 1
            continue
        g = min(cand, key=lambda g: abs(center(g["box"])[0] - hx) + 0.2 * abs(center(g["box"])[1] - hy))
        dx = (center(g["box"])[0] - hx) / il
        dy = (center(g["box"])[1] - hy) / il
        info = f"{name[:30]} p{page} head {[round(v) for v in h]} finger {l['finger']} digit at dx {dx:+.1f} dy {dy:+.1f}"
        if not any(s["page"] == page and same(s["box"], g["box"]) for s in after_numbers):
            r = "dropped as a measure number"
        else:
            final = [x for x in data["digits"] if x["page"] == page and same(x["box"], g["box"])]
            if not final:
                r = "dropped as tuplet number / wedge"
            else:
                fin = final[0]
                mine = [p for p in pairs if p["page"] == page and any(same(b, g["box"]) for b in p["boxes"])]
                right = [p for p in mine if any(same(hh, h) for hh in p["heads"])]
                if fin["to"] and any(same(fin["to"], hh) for p in right for hh in p["heads"]):
                    r = "right chord, another note of it"
                elif not mine:
                    r = "attached beside a head" if fin["attached"] else "in no stack with a candidate"
                elif not right:
                    sizes = {p["size"] for p in mine}
                    r = "right chord not a candidate" + (" (stack of several)" if max(sizes) > 1 else "")
                elif fin["to"]:
                    r = "right chord a candidate, went to another chord"
                else:
                    r = "right chord a candidate, left alone"
        reasons[r] += 1
        if len(examples[r]) < show:
            examples[r].append(info)

total = sum(reasons.values())
for r, n in reasons.most_common():
    print(f"{n:5d} ({n / total:.0%})  {r}")
    for e in examples[r]:
        print("        " + e)
