#!/usr/bin/env bash
# The score reader on the MuseScore test PDFs with oracle detections (make_oracle.py).
# usage: run_oracle.sh <kind: heads|both|digits> <out file>
here="$(cd "$(dirname "$0")" && pwd)"
EVAL=${EVAL:-$here/../../target/release/examples/score_reader_eval.exe}
kind=$1 out=$2
: > "$out"
PYTHONIOENCODING=utf-8 "${PY:-python}" -c "
import json
for t in json.load(open(r'$(cygpath -w "$here")/test_set.json', encoding='utf-8')):
    print(t['dir'] + '\t' + t['name'])
" | tr -d '\r' | while IFS=$'\t' read -r dir name; do
    SCORE_READER_DETECTIONS="$(cygpath -w "$here/page_cache/oracle_$kind/$name")" \
        "$EVAL" "$here/$dir/$name.mxl" "$here/$dir/$name.pdf" >> "$out" 2>>"$out.err"
done
