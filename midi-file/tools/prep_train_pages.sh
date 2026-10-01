#!/usr/bin/env bash
# Prepared pages of the training pieces (for running the detector on them), then the v5
# detections. usage: prep_train_pages.sh <list.json> <cache name> <model.pt> <arch>
here="$(cd "$(dirname "$0")" && pwd)"
EVAL=${EVAL:-$here/../../target/release/examples/score_reader_eval.exe}
list=$1 cache=$2 model=$3 arch=$4
PYTHONIOENCODING=utf-8 "${PY:-python}" -c "
import json
for t in json.load(open(r'$(cygpath -w "$list")', encoding='utf-8')):
    print(t['dir'] + '\t' + t['name'])
" | tr -d '\r' | xargs -P 6 -d '\n' -I{} bash -c '
    dir=$(printf "%s" "{}" | cut -f1); name=$(printf "%s" "{}" | cut -f2)
    out="'"$here"'/page_cache/'"$cache"'/$name"
    [ -f "$out/0.json" ] && exit 0
    SCORE_READER_PAGES_OUT="$(cygpath -w "$out")" "'"$EVAL"'" "'"$here"'/$dir/$name.mxl" "'"$here"'/${PDF_DIR:-$dir}/$name.pdf" > /dev/null 2>&1
'
"${PY:-python}" "$here/detect_pages.py" "$model" "$arch" "$(cygpath -w "$here/page_cache/$cache")"/* 2>/dev/null
echo done
