"""Stream pdf.tar.gz from Zenodo and keep only the PDFs listed in fingered_piano.csv
(and copy their MXL files next to them), into ./fingered/<n>_<title>.{mxl,pdf}."""
import csv
import os
import re
import shutil
import sys
import tarfile
import urllib.request

here = os.path.dirname(os.path.abspath(__file__))
out = os.path.join(here, "fingered")
os.makedirs(out, exist_ok=True)

rows = list(csv.DictReader(open(os.path.join(here, "fingered_piano.csv"), encoding="utf-8")))
wanted = {}
for i, row in enumerate(rows):
    safe = re.sub(r"[^\w\-]+", "_", row["title"])[:50].strip("_")
    wanted[row["pdf"].lstrip("./")] = f"{i:03d}_{safe}"

# MXL files from the local archive
mxl_names = {row["mxl"].lstrip("./"): wanted[row["pdf"].lstrip("./")] for row in rows}
with tarfile.open(os.path.join(here, "mxl.tar.gz"), "r|gz") as tar:
    for member in tar:
        name = member.name.lstrip("./")
        if name in mxl_names:
            with open(os.path.join(out, mxl_names[name] + ".mxl"), "wb") as f:
                shutil.copyfileobj(tar.extractfile(member), f)
print("mxl done", file=sys.stderr)

url = "https://zenodo.org/records/14648209/files/pdf.tar.gz?download=1"
got = 0
with urllib.request.urlopen(url) as response:
    with tarfile.open(fileobj=response, mode="r|gz") as tar:
        for member in tar:
            name = member.name.lstrip("./")
            if name in wanted:
                with open(os.path.join(out, wanted[name] + ".pdf"), "wb") as f:
                    shutil.copyfileobj(tar.extractfile(member), f)
                got += 1
                print(f"{got}/{len(wanted)} {wanted[name]}", file=sys.stderr, flush=True)
                if got == len(wanted):
                    break
print("pdf done", got, file=sys.stderr)
