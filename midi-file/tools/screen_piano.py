"""Find scores in PDMX lists that are not piano music (labelled "Piano" anyway): features of
the MusicXML that piano scores do not have. usage: screen_piano.py <list.json> [...]"""
import json
import re
import sys
import zipfile


def features(mxl):
    z = zipfile.ZipFile(mxl)
    xml = max((z.read(n) for n in z.namelist() if not n.startswith("META-INF")), key=len).decode("utf-8", "replace")
    first = re.search(r"<measure\b.*?</measure>", xml, re.S)
    parts = len(re.findall(r"<score-part\b", xml))
    staves = sum(int(s) for s in re.findall(r"<staves>(\d+)</staves>", first.group(0))) if first else 0
    # staves of all parts in the first measures
    total_staves = max(staves, 0) + parts - len(re.findall(r"<staves>", first.group(0) if first else ""))
    fing = re.findall(r"<fingering[^>]*>\s*([^<]*?)\s*</fingering>", xml)
    text = " ".join(re.findall(r"<(?:credit-words|work-title|movement-title|words)[^>]*>([^<]*)", xml)).lower()
    f = {
        "staves": total_staves,
        "string_fret": len(re.findall(r"<string>|<fret>", xml)),
        "bow": len(re.findall(r"<up-bow|<down-bow", xml)),
        "pima": sum(1 for x in fing if x in ("p", "i", "m", "a")),
        "zero": sum(1 for x in fing if x == "0"),
        "guitar_clef": len(re.findall(r"<sign>G</sign>\s*<line>2</line>\s*<clef-octave-change>-1", xml)),
        "words": [w for w in ("guitar", "gitarre", "guitare", "violão", "violao", "lute", "laúd", "violin",
                               "violon", "viola", "cello", "flute", "flöte", "ukulele", "mandolin", "harp")
                  if w in text],
        "fing": len(fing),
    }
    return f


def suspect(f):
    reasons = []
    if f["staves"] < 2:
        reasons.append("one staff")
    for k in ("string_fret", "bow", "pima", "guitar_clef"):
        if f[k]:
            reasons.append(f"{k} {f[k]}")
    if f["zero"] >= 2:
        reasons.append(f"finger 0 x{f['zero']}")
    if f["words"]:
        reasons.append("words " + "/".join(f["words"]))
    return reasons


for lst in sys.argv[1:]:
    items = json.load(open(lst, encoding="utf-8"))
    n = 0
    for t in items:
        name = t["name"] if isinstance(t, dict) else t
        d = t.get("dir", "fingered") if isinstance(t, dict) else "fingered"
        try:
            f = features(f"{d}/{name}.mxl")
        except FileNotFoundError:
            continue
        r = suspect(f)
        if r:
            n += 1
            print(f"{lst}: {name[:50]:50s} fing {f['fing']:4d}  " + "; ".join(r))
    print(f"{lst}: {n} of {len(items)} suspect")
