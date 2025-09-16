#!/usr/bin/env bash
set -euo pipefail

HOST=${HOST:-http://localhost:8080}
KEY=${KEY:-ratelimit}
REQUESTS=${REQUESTS:-50}
PARALLEL=${PARALLEL:-20}
BODY='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"ping"}],"stream":false}'

echo "== Triggering ${REQUESTS} requests in parallel (${PARALLEL}) against ${HOST} =="
seq 1 "$REQUESTS" | xargs -I{} -P "$PARALLEL" curl -s -o /dev/null -w "%{http_code}\n" \
  -H "Content-Type: application/json" \
  -H "X-Api-Key: ${KEY}" \
  -d "$BODY" \
  "$HOST/v1/chat/completions" | sort | uniq -c

echo "== Expect to see a mix (ej. 200 + 429) when RPS/BURST is low =="
