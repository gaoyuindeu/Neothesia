#!/usr/bin/env bash
# The score reader on the test pieces with detections already in page_cache/<cache>/
# (run_reader_gpu.sh, make_oracle.py). usage: run_cached.sh <cache> <variant pdfs> <out file>
#   cache: musescore | verovio | ... | oracle_both ...; variant: musescore | verovio | scan_*
here="$(cd "$(dirname "$0")" && pwd)"
EVAL=${EVAL:-$here/../../target/release/examples/score_reader_eval.exe}
cache=$1 variant=$2 out=$3
: > "$out"
PYTHONIOENCODING=utf-8 "${PY:-python}" -c "
import json
for t in json.load(open(r'$(cygpath -w "$here")/test_set.json', encoding='utf-8')):
    print(t['dir'] + '\t' + t['name'])
" | tr -d '\r' | while IFS=$'\t' read -r dir name; do
    case $variant in
        musescore) pdf="$here/$dir/$name.pdf" ;;
        verovio) pdf="$here/verovio/$name.pdf" ;;
        scan_musescore) pdf="$here/test_scans/musescore/$name.pdf" ;;
        scan_verovio) pdf="$here/test_scans/verovio/$name.pdf" ;;
    esac
    SCORE_READER_DETECTIONS="$(cygpath -w "$here/page_cache/$cache/$name")" "$EVAL" "$here/$dir/$name.mxl" "$pdf" >> "$out" 2>>"$out.err"
done
