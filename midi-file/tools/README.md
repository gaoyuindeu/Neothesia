# Fingering from printed scores: data and models

The scripts expect to sit in the data folder (copy them next to the data); they need
Python with PyTorch (CUDA), PyMuPDF, verovio, resvg-py and onnx.

## Score reader (`src/musicxml/score_reader/`, the "Fingering from PDF" button)

`score_reader/detector.onnx` finds fingering digits 1-5 and note heads on a page. The
reader then aligns the found heads with the notes of the score, staff by staff, and gives
each digit to its note.

1. `find_fingered.py <PDMX dir>`: finds piano scores with printed fingering in
   [PDMX](https://zenodo.org/records/14648209) (`PDMX.csv` and `mxl.tar.gz`, CC-BY 4.0
   dataset of public domain / CC0 MuseScore scores). Writes `fingered_piano.csv`.
2. `extract_pdfs.py`: copies their MXL files and streams `pdf.tar.gz` for their PDFs into
   `fingered/`.
3. `make_test_set.py`: chooses the evaluation pieces (spread over eras, kinds and
   fingering density), `test_set.json`. They are left out of the training data.
4. `build_dataset.py <pages>`: renders the PDF pages at 300 dpi and reads the fingering
   digits from the PDF text layer (the font and size whose digit count matches the score's
   `<fingering>` count).
5. `build_heads.py <pages>`: adds the note heads, from the music font glyphs of the text
   layer (SMuFL black / half / whole heads; Bravura in MuseScore 3 uses its alternate
   codepoints U+F4BE / F4BD / F4BC).
6. `render_verovio.py`: engraves pieces again with Verovio (other font, spacing and
   fingering placement), labelled from Verovio's bounding boxes.
7. `prepare_detector_data.py`: index of both sets of pages, `detector_index.json`.
8. `ARCH=fast train_detector.py detector_index.json <out> 35000`: trains the detector
   (CenterNet-like heat maps) and exports `detector.onnx`; keeps a checkpoint every 5000
   steps.
9. `ARCH=fast EVERY=1 PAGES=2 head_eval.py <model.pt> ...`: note head precision / recall
   of checkpoints on the evaluation pieces, with the worst pages (catches a model that
   fails on one engraving style).

Evaluation of the whole reader: `cargo run --release -p midi-file --example
score_reader_eval -- <score with fingering> <its PDF>` strips the fingering from the
score, reads it back from the PDF and compares. `make_scan.py <in.pdf> <out.pdf>` makes
scan-like copies of the test PDFs. `run_reader_gpu.sh <variant> <model.pt> <arch> <out>`
runs it on all evaluation pieces with the detector of a checkpoint run by PyTorch
(`detect_pages.py`), and `summarize.py <out> <out>` sums up.

The app runs the detector on the GPU (wgpu compute shaders, `score_reader/gpu.rs`) and on
the CPU (rten) without one; `NEOTHESIA_DETECT_CPU=1` forces the CPU. Development
variables: `SCORE_READER_TIMING`, `SCORE_READER_DEBUG`, `NEOTHESIA_DETECT_THRESHOLD`.

## Audiveris path (`omr.rs`, `omr_fingering.rs`)

`src/musicxml/digits.onnx` (digit detector) and `src/musicxml/digit_templates.bin`
(templates of the fallback reader) are made with `train_digits.py <pages> <out> [steps]`
and `make_digit_templates.py <out.bin> <fonts dir>` (free fonts: Edwin, DejaVu, GNU
FreeFont, Leland). Evaluation: `cargo run --release -p midi-file --example
omr_fingering_eval -- <score with fingering> <its PDF> [work dir]`.
