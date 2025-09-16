#!/usr/bin/env bash
set -euo pipefail

HOST=${HOST:-http://localhost:8080}
KEY=${KEY:-demo}
BODY='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"ping"}],"stream":false}'
REDIS_CTN=${REDIS_CTN:-strange_khorana}
DB=${DB:-0}
KEYNAME="quota:${KEY}"

echo "== Cleaning ${KEYNAME} on Redis (db ${DB}) =="
docker exec -it "$REDIS_CTN" redis-cli -n "$DB" DEL "$KEYNAME" >/dev/null || true

echo "== Sending 7 requests sequential with X-Api-Key: ${KEY} =="
for i in {1..7}; do
  code=$(curl -s -o /dev/null -w "%{http_code}" \
    -H "Content-Type: application/json" \
    -H "X-Api-Key: ${KEY}" \
    -d "$BODY" "$HOST/v1/chat/completions")
  cnt=$(docker exec -it "$REDIS_CTN" redis-cli -n "$DB" GET "$KEYNAME" 2>/dev/null | tr -d '\r')
  ttl=$(docker exec -it "$REDIS_CTN" redis-cli -n "$DB" TTL "$KEYNAME" 2>/dev/null | tr -d '\r')
  echo "$i) HTTP $code | redis $KEYNAME count=${cnt:-0} ttl=${ttl:--}"
done

echo "== Wait for the window to expire (ttl)... =="
ttl=$(docker exec -it "$REDIS_CTN" redis-cli -n "$DB" TTL "$KEYNAME" 2>/dev/null | tr -d '\r')
echo "TTL actual: $ttl s"
