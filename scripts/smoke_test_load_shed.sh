#!/usr/bin/env bash
set -euo pipefail

HOST=${HOST:-http://localhost:8080}
KEY=${KEY:-stress}
REQUESTS=${REQUESTS:-40}
PARALLEL=${PARALLEL:-20}
BODY='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"ping"}],"stream":false}'

echo "== Make sure you have MAX_CONCURRENCY low (e.g. 1) before running this smoke =="
seq 1 "$REQUESTS" | xargs -I{} -P "$PARALLEL" curl -s -o /dev/null -w "%{http_code}\n" \
  -H "Content-Type: application/json" \
  -H "X-Api-Key: ${KEY}" \
  -d "$BODY" \
  "$HOST/v1/chat/completions" | sort | uniq -c

echo "== Expect to see HTTP 503 (load shed) along with some 200s =="
