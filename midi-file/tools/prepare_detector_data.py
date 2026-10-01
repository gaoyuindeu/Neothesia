"""Index of training pages for train_detector.py: PDMX pages (train_pages3/labels2.json)
and the Verovio engravings of the same training pieces (verovio_train/)."""
import json
import os

import fitz

here = os.path.dirname(os.path.abspath(__file__))
items = []
# Scores labelled "Piano" in PDMX that are other instruments: left out
non_piano = {n.split("_")[0] for n in json.load(open(os.path.join(here, "non_piano.json"), encoding="utf-8"))}

pdmx = json.load(open(os.path.join(here, "train_pages3", "labels2.json")))
for image, lab in pdmx.items():
    if image.rsplit("_p", 1)[0] in non_piano:
        continue
    objects = [[d - 1, x0, y0, x1, y1] for d, x0, y0, x1, y1 in lab["digits"]]
    objects += [[5 + k, x0, y0, x1, y1] for k, x0, y0, x1, y1 in lab["heads"]]
    items.append({"image": os.path.join(here, "train_pages3", image), "piece": image.split("_")[0], "objects": objects})

vdir = os.path.join(here, "verovio_train")
vlab = json.load(open(os.path.join(vdir, "labels.json")))
os.makedirs(os.path.join(vdir, "pages"), exist_ok=True)
docs = {}
for key, lab in vlab.items():
    name, page = key.rsplit("_p", 1)
    if name.split("_")[0] in non_piano:
        continue
    doc = docs.setdefault(name, fitz.open(os.path.join(vdir, name + ".pdf")))
    if int(page) >= len(doc):
        continue
    images = doc[int(page)].get_images()
    if not images:
        continue
    path = os.path.join(vdir, "pages", f"{name.split('_')[0]}_p{page}.png")
    if not os.path.exists(path):
        with open(path, "wb") as f:
            f.write(doc.extract_image(images[0][0])["image"])
    objects = [[d - 1, x0, y0, x1, y1] for d, x0, y0, x1, y1, _ in lab["digits"]]
    objects += [[5 + h[5], h[1], h[2], h[3], h[4]] for h in lab["heads"]]
    items.append({"image": path, "piece": name.split("_")[0], "objects": objects})

json.dump(items, open(os.path.join(here, "detector_index.json"), "w"))
print("pages", len(items), "digits", sum(sum(o[0] < 5 for o in it["objects"]) for it in items),
      "heads", sum(sum(o[0] >= 5 for o in it["objects"]) for it in items))
