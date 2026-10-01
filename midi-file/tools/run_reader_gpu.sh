#!/usr/bin/env bash
# The score reader on the evaluation pieces with the detector run on the GPU (PyTorch):
# the prepared pages are cached per variant, detections redone per model.
# usage: run_reader_gpu.sh <variant> <model.pt> <arch full|fast> <out file>
#   variant: musescore | verovio | scan_musescore | scan_verovio
here="$(cd "$(dirname "$0")" && pwd)"
# Data: DATA (default: this folder) holds test_set.json, the pieces and page_cache/
data=${DATA:-$here}
EVAL=${EVAL:-$here/../../target/release/examples/score_reader_eval.exe}
PY=${PY:-python}
variant=$1 model=$2 arch=$3 out=$4
cache="$data/page_cache/$variant"
list=$(PYTHONIOENCODING=utf-8 "$PY" -c "
import json
for t in json.load(open(r'$(cygpath -w "$data")/test_set.json', encoding='utf-8')):
    print(t['dir'] + '\t' + t['name'])
" | tr -d '\r')
pdf_of() {
    case $variant in
        musescore) echo "$data/$1/$2.pdf" ;;
        verovio) echo "$data/verovio/$2.pdf" ;;
        scan_musescore) echo "$data/test_scans/musescore/$2.pdf" ;;
        scan_verovio) echo "$data/test_scans/verovio/$2.pdf" ;;
    esac
}
# 1. Prepared pages (once per variant)
while IFS=$'\t' read -r dir name; do
    pdf=$(pdf_of "$dir" "$name")
    [ -f "$pdf" ] || continue
    [ -f "$cache/$name/0.json" ] && continue
    SCORE_READER_PAGES_OUT="$(cygpath -w "$cache/$name")" "$EVAL" "$data/$dir/$name.mxl" "$pdf" > /dev/null 2>&1
done <<< "$list"
# 2. Detections on the GPU
"$PY" "$here/detect_pages.py" "$model" "$arch" "$(cygpath -w "$cache")"/* 2>/dev/null
# 3. The rest of the reader with these detections
: > "$out"
while IFS=$'\t' read -r dir name; do
    pdf=$(pdf_of "$dir" "$name")
    [ -f "$pdf" ] || continue
    SCORE_READER_DETECTIONS="$(cygpath -w "$cache/$name")" "$EVAL" "$data/$dir/$name.mxl" "$pdf" >> "$out" 2>>"$out.err"
done <<< "$list"
