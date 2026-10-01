# Fingering from printed scores: data and models

`src/musicxml/digits.onnx` (the fingering digit detector) and `src/musicxml/digit_templates.bin`
(templates of the fallback reader) are made with these scripts.

1. `find_fingered.py <PDMX dir>`: finds piano scores with printed fingering in
   [PDMX](https://zenodo.org/records/14648209) (`PDMX.csv` and `mxl.tar.gz`, CC-BY 4.0
   dataset of public domain / CC0 MuseScore scores). Writes `fingered_piano.csv`.
2. `extract_pdfs.py`: copies their MXL files and streams `pdf.tar.gz` for their PDFs into
   `fingered/`.
3. `build_dataset.py <out>`: renders the PDF pages at 300 dpi and reads the fingering
   digits from the PDF text layer (the font and size whose digit count matches the score's
   `<fingering>` count). Pieces used for evaluation are left out.
4. `train_digits.py <pages> <out> [steps]`: trains the detector (PyTorch, CUDA) and exports
   `digits.onnx`.
5. `make_digit_templates.py <out.bin> <fonts dir>`: templates for the fallback reader,
   from free fonts (Edwin, DejaVu, GNU FreeFont, Leland).

Evaluation: `cargo run --release -p midi-file --example omr_fingering_eval -- <score with
fingering> <its PDF> [work dir]` strips the fingering from the score, reads the PDF with
Audiveris and this reader, writes the fingering back and compares it with the original.
