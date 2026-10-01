"""Choose the evaluation pieces: the 20 already used plus a spread over eras / kinds and
fingering density. Writes test_set.json [{"name", "dir", "era", "fingerings"}]."""
import csv
import glob
import json
import os
import random

here = os.path.dirname(os.path.abspath(__file__))
ERAS = {
    "baroque": ["bach", "handel", "händel", "scarlatti", "couperin", "rameau", "purcell", "telemann", "pachelbel", "vivaldi"],
    "classical": ["mozart", "haydn", "clementi", "beethoven", "kuhlau", "diabelli", "dussek", "benda", "hummel"],
    "romantic": ["chopin", "schumann", "liszt", "brahms", "mendelssohn", "grieg", "tchaikovsky", "tschaikowsky", "schubert", "burgm", "field", "rachmaninoff", "rachmaninov"],
    "modern": ["bartók", "bartok", "debussy", "satie", "prokofiev", "kabalevsky", "ravel", "scriabin", "gurlitt", "tansman", "shostakovich"],
    "etude": ["czerny", "lemoine", "hanon", "beyer", "duvernoy", "bertini", "heller", "köhler", "kohler", "germer", "cramer", "schmitt", "études", "etude", "exercise", "scale"],
}
WANT = {"baroque": 12, "classical": 10, "romantic": 15, "modern": 10, "etude": 15, "other": 20}


def era(title):
    t = title.lower()
    for e, keys in ERAS.items():
        if any(k in t for k in keys):
            return e
    return "other"


def stems():
    """file stem -> (dir, pdf name in the csv)"""
    out = {}
    for d in ("fingered", "fingered15"):
        for mxl in glob.glob(os.path.join(here, d, "*.mxl")):
            stem = os.path.basename(mxl)[:-4]
            if os.path.exists(mxl[:-4] + ".pdf"):
                out[stem] = d
    return out


available = stems()
# csv rows in the order the stems were numbered
rows50 = list(csv.DictReader(open(os.path.join(here, "fingered_piano.csv"), encoding="utf-8")))
old = {r["pdf"] for r in rows50}
rows15 = [r for r in csv.DictReader(open(os.path.join(here, "fingered_piano15.csv"), encoding="utf-8")) if r["pdf"] not in old]
pieces = []
for prefix, rows in (("", rows50), ("n", rows15)):
    for i, r in enumerate(rows):
        key = f"{prefix}{i:03d}_"
        stem = next((s for s in available if s.startswith(key)), None)
        if stem:
            pieces.append(dict(name=stem, dir=available[stem], era=era(r["title"] + " " + r["composer"]),
                               fingerings=int(r["fingerings"])))

existing = {os.path.basename(p.rstrip("/\\")) for p in glob.glob(os.path.join(here, "omr_work", "*", ""))}
chosen = [p for p in pieces if p["name"] in existing]
counts = {e: sum(p["era"] == e for p in chosen) for e in WANT}
rng = random.Random(7)
for e, n in WANT.items():
    pool = sorted([p for p in pieces if p["era"] == e and p not in chosen], key=lambda p: p["fingerings"])
    need = max(0, n - counts[e])
    if need and pool:
        # Evenly over the fingering counts
        step = len(pool) / need
        picks = [pool[min(int(k * step + rng.random() * step), len(pool) - 1)] for k in range(need)]
        for p in picks:
            if p not in chosen:
                chosen.append(p)
json.dump(chosen, open(os.path.join(here, "test_set.json"), "w", encoding="utf-8"), ensure_ascii=False, indent=1)
from collections import Counter
print(len(chosen), Counter(p["era"] for p in chosen), sum(p["fingerings"] for p in chosen))
