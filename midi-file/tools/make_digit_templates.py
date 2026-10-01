"""Write the digit templates used by Neothesia's fingering reader.

Glyphs 1-5 of free fonts (Edwin: OFL, DejaVu: Bitstream Vera license, GNU FreeFont: GPL
with font exception, Leland fingering glyphs: OFL), upright and slanted both ways, cropped
to the ink and scaled to 20x28.

Format: b"NTDT", u16 count, u8 width, u8 height, then per template:
u8 digit, f32 aspect (ink width / height, little endian), width*height u8 gray values.

usage: make_digit_templates.py <out.bin> <folder with the font files>

Fonts: Edwin-Roman/Italic.otf (github.com/musescore/MuseScore fonts/edwin), DejaVuSerif(-Italic).ttf
and DejaVuSans(-Oblique).ttf (dejavu-fonts releases), FreeSerif(Italic).otf and
FreeSans(Oblique).otf (ftp.gnu.org/gnu/freefont).
"""
import os
import struct
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

FONTS = sys.argv[2] if len(sys.argv) > 2 else "fonts"
TEXT_FONTS = [os.path.join(FONTS, f) for f in (
    "Edwin-Roman.otf", "Edwin-Italic.otf", "DejaVuSerif.ttf", "DejaVuSerif-Italic.ttf",
    "DejaVuSans.ttf", "DejaVuSans-Oblique.ttf", "FreeSerif.otf", "FreeSerifItalic.otf",
    "FreeSans.otf", "FreeSansOblique.otf")]
LELAND = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "assets", "fonts", "Leland.otf")
TW, TH = 20, 28
SHEARS = (-0.22, 0.0, 0.22)


def entries(d, im):
    for k in SHEARS:
        w, h = im.size
        sheared = im.transform((w + int(abs(k) * h) + 2, h), Image.AFFINE,
                               (1, k, -int(abs(k) * h) if k > 0 else 0, 0, 1, 0), Image.BILINEAR)
        m = np.asarray(sheared) > 127
        ys, xs = np.nonzero(m)
        if len(ys) == 0:
            continue
        crop = m[ys.min():ys.max() + 1, xs.min():xs.max() + 1]
        aspect = crop.shape[1] / crop.shape[0]
        small = Image.fromarray((crop * 255).astype(np.uint8)).resize((TW, TH), Image.BILINEAR)
        yield d, aspect, np.asarray(small, dtype=np.uint8)


out = []
for path in TEXT_FONTS:
    font = ImageFont.truetype(path, 80)
    for d in range(1, 6):
        im = Image.new("L", (140, 140), 0)
        ImageDraw.Draw(im).text((30, 10), str(d), fill=255, font=font)
        out.extend(entries(d, im))
font = ImageFont.truetype(LELAND, 120)
for d in range(1, 6):
    im = Image.new("L", (180, 200), 0)
    ImageDraw.Draw(im).text((40, 30), chr(0xED10 + d), fill=255, font=font)
    out.extend(entries(d, im))

with open(sys.argv[1], "wb") as f:
    f.write(b"NTDT" + struct.pack("<HBB", len(out), TW, TH))
    for d, aspect, pixels in out:
        f.write(struct.pack("<Bf", d, aspect))
        f.write(pixels.tobytes())
print(len(out), "templates")
