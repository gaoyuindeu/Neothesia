import re, sys
def load(path):
    out = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        m = re.match(r"(n?\d{3})_.*fingered (\d+) .*correct (\d+) wrong (\d+) missing (\d+) extra (\d+)", line)
        if m:
            out[m.group(1)] = tuple(map(int, m.groups()[1:]))
    return out
a, b = load(sys.argv[1]), load(sys.argv[2])
keys = sorted(set(a) & set(b))
for name, data in (("Audiveris only", a), ("Audiveris + own digits", b)):
    f = sum(data[k][0] for k in keys); c = sum(data[k][1] for k in keys)
    w = sum(data[k][2] for k in keys); e = sum(data[k][4] for k in keys)
    print(f"{name:24s} pieces {len(keys)}  fingered {f}  correct {c}  recall {100*c/f:.1f}%  precision {100*c/max(c+w+e,1):.1f}%")
