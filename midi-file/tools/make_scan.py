"""Make a PDF look scanned: 300 dpi gray pages, slightly rotated, blurred, ink a little
thicker or thinner, paper tint and noise, JPEG compressed.

usage: make_scan.py <in.pdf> <out.pdf> [seed]
"""
import io
import random
import sys

import fitz  # PyMuPDF
import numpy as np
from PIL import Image, ImageFilter

src, dst = sys.argv[1], sys.argv[2]
rng = random.Random(int(sys.argv[3]) if len(sys.argv) > 3 else 1)
nrng = np.random.default_rng(rng.randint(0, 1 << 30))

out = fitz.open()
for page in fitz.open(src):
    pix = page.get_pixmap(dpi=300, colorspace=fitz.csGRAY)
    im = Image.frombytes("L", (pix.width, pix.height), pix.samples)
    im = im.rotate(rng.uniform(-0.6, 0.6), resample=Image.BICUBIC, fillcolor=255, expand=False)
    # Ink thickness of a worn or heavy print
    if rng.random() < 0.5:
        im = im.filter(ImageFilter.MinFilter(3))  # thicker
    im = im.filter(ImageFilter.GaussianBlur(rng.uniform(0.6, 1.2)))
    a = np.asarray(im, dtype=np.float32)
    # Paper tint, uneven light, noise
    h, w = a.shape
    light = np.linspace(0, rng.uniform(-18, 18), w)[None, :]
    a = a * 0.85 + 30 + light + nrng.normal(0, 9, a.shape)
    a = np.clip(a, 0, 255).astype(np.uint8)
    buf = io.BytesIO()
    Image.fromarray(a).save(buf, "JPEG", quality=60)
    rect = page.rect
    p = out.new_page(width=rect.width, height=rect.height)
    p.insert_image(rect, stream=buf.getvalue())
out.save(dst)
