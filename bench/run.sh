#!/bin/sh
# Fetch the HN data and binaries, run and scale every engine, then write results/hn/results.md. PYTHON selects the interpreter.
set -e
cd "$(dirname "$0")"
PY=${PYTHON:-python3}
$PY fetch.py
if [ -f samples.json ]; then $PY bench.py samples --check; else $PY bench.py samples; fi
for e in completr tantivy typesense meilisearch; do $PY bench.py run $e --throughput; done
$PY bench.py run typesense --variant buckets --throughput
$PY bench.py run meilisearch --variant popfirst --throughput
for e in completr tantivy typesense meilisearch; do $PY bench.py scale $e --sizes 25000,50000,124440; done
$PY report.py
