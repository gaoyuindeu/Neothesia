"""Find PDMX piano scores with lots of printed fingering (ground truth for PDF import tests).

Reads PDMX.csv and streams mxl.tar.gz once; writes fingered_piano.csv sorted by how much
of the score is fingered.
"""
import csv
import io
import re
import sys
import tarfile
import zipfile

csv.field_size_limit(10**9)
here = sys.argv[1] if len(sys.argv) > 1 else "."

candidates = {}
with open(f"{here}/PDMX.csv", encoding="utf-8") as f:
    for row in csv.DictReader(f):
        if row["license_conflict"] != "False":
            continue
        tracks = row["tracks"].split("-")
        # Piano only (program 0), one or two tracks (one part with two staves, or two parts)
        if not tracks or any(t != "0" for t in tracks) or len(tracks) > 2:
            continue
        key = row["mxl"].lstrip("./")
        candidates[key] = row
print("piano candidates:", len(candidates), file=sys.stderr)

fingering = re.compile(rb"<fingering[ >]")
note = re.compile(rb"<note[ >]")
found = []
with tarfile.open(f"{here}/mxl.tar.gz", "r|gz") as tar:
    for i, member in enumerate(tar):
        if not member.isfile():
            continue
        name = member.name.lstrip("./")
        row = candidates.get(name)
        if row is None:
            continue
        data = tar.extractfile(member).read()
        try:
            z = zipfile.ZipFile(io.BytesIO(data))
            xml = max((z.read(n) for n in z.namelist() if not n.startswith("META-INF")), key=len)
        except Exception:
            continue
        nf = len(fingering.findall(xml))
        if nf < 50:
            continue
        nn = len(note.findall(xml))
        found.append((nf / max(nn, 1), nf, nn, name, row["pdf"], row["title"], row["composer_name"], row["rating"], row["license"]))

found.sort(reverse=True)
with open(f"{here}/fingered_piano.csv", "w", newline="", encoding="utf-8") as f:
    w = csv.writer(f)
    w.writerow(["share", "fingerings", "notes", "mxl", "pdf", "title", "composer", "rating", "license"])
    w.writerows(found)
print("scores with >= 50 fingerings:", len(found), file=sys.stderr)
