#!/usr/bin/env bash
# Test corpus for crates/dsd's comparison tests (with FFmpeg):
#   crates/dsd/tests/reference/make_reference.sh <output dir>
# then:
#   SONKORE_DSD_CORPUS=<dir> cargo test -p sonkore-dsd
#
# The corpus comes from tests/mkdsf.py and tests/mkdst.py (numpy, or the
# small stand-in in pyshim/ when numpy is missing).
set -euo pipefail
out="$(mkdir -p "${1:?output dir}" && cd "$1" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../../.." && pwd)"
tests="$root/tests"

if ! python3 -c "import numpy" 2>/dev/null; then
    export PYTHONPATH="$here/pyshim${PYTHONPATH:+:$PYTHONPATH}"
fi
cd "$out"
[ -f dsd64.dsf ] || python3 "$tests/mkdsf.py" dsd64.dsf 1 2822400
[ -f dsd128.dsf ] || python3 "$tests/mkdsf.py" dsd128.dsf 1 5644800
[ -f dsd256.dsf ] || python3 "$tests/mkdsf.py" dsd256.dsf 1 11289600
[ -f dsd512.dsf ] || python3 "$tests/mkdsf.py" dsd512.dsf 1 22579200
[ -f dsd64.dff ] || python3 "$tests/mkdst.py" dsd64.dsf dsd64.dff plain
[ -f dsd64-dst.dff ] || python3 "$tests/mkdst.py" dsd64.dsf dsd64-dst.dff
[ -f dsd64-dst-mixed.dff ] || python3 "$tests/mkdst.py" dsd64.dsf dsd64-dst-mixed.dff mixed
[ -f dsd128.dff ] || python3 "$tests/mkdst.py" dsd128.dsf dsd128.dff plain
[ -f dsd128-dst.dff ] || python3 "$tests/mkdst.py" dsd128.dsf dsd128-dst.dff
# The encoder checked against an independent decoder.
cmp <(ffmpeg -nostdin -loglevel error -i dsd64.dff -f f32le -) <(ffmpeg -nostdin -loglevel error -i dsd64-dst.dff -f f32le -)

echo "corpus in $out"
