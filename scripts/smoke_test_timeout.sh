#!/usr/bin/env bash
set -euo pipefail

HOST=${HOST:-http://localhost:8080}
KEY=${KEY:-timeout-test}
BODY='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"ping"}],"stream":false}'

echo "== Make a request waiting 504 (make sure the upstream takes longer > TIMEOUT_SECS) =="
curl -s -D - -o /dev/null \
  -H "Content-Type: application/json" \
  -H "X-Api-Key: ${KEY}" \
  -d "$BODY" \
  "$HOST/v1/chat/completions"
