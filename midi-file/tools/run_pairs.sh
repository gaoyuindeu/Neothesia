#!/usr/bin/env bash
# (stack, chord) pairs with labels for training the association model.
# usage: run_pairs.sh <list.json> <detections cache> <out dir> [jobs]
here="$(cd "$(dirname "$0")" && pwd)"
EVAL=${EVAL:-$here/../../target/release/examples/score_reader_eval.exe}
list=$1 cache=$2 out=$3 jobs=${4:-4}
mkdir -p "$out"
PYTHONIOENCODING=utf-8 "${PY:-python}" -c "
import json
for t in json.load(open(r'$(cygpath -w "$list")', encoding='utf-8')):
    print(t['dir'] + '\t' + t['name'])
" | tr -d '\r' | xargs -P "$jobs" -d '\n' -I{} bash -c '
    dir=$(printf "%s" "{}" | cut -f1); name=$(printf "%s" "{}" | cut -f2)
    SCORE_READER_PAIRS="$(cygpath -w "'"$out"'/$name.jsonl")" SCORE_READER_DETECTIONS="$(cygpath -w "'"$here"'/page_cache/'"$cache"'/$name")" \
        "'"$EVAL"'" "'"$here"'/$dir/$name.mxl" "'"$here"'/${PDF_DIR:-$dir}/$name.pdf" >> "'"$out"'/results.txt" 2>/dev/null
'
