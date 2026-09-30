#!/bin/sh
# Fetch the data and binaries, run every engine, then write results/results.md. PYTHON selects the interpreter.
set -e
cd "$(dirname "$0")"
PY=${PYTHON:-python3}
$PY fetch.py
if [ -f samples.json ]; then $PY bench.py samples --check; else $PY bench.py samples; fi
for e in strato tantivy typesense meilisearch; do $PY bench.py run $e --throughput; done
$PY bench.py run typesense --variant buckets --throughput
$PY bench.py run meilisearch --variant popfirst --throughput
$PY report.py
