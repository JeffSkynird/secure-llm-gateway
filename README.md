# 🔐 Secure LLM Gateway — README

A small Rust gateway that sits in front of LLM to add **quotas**, **rate limiting**, **simple circuit-breaker behaviors**, **PII redaction**, **streaming SSE handling**, **metrics** and **distributed tracing**.

| ⚡ Quick Facts | 🔄 Proxy | 🧠 Observability | 🛡️ Safety |
| --- | --- | --- | --- |
| Single binary | `/v1/chat/completions` | Prometheus + OTLP | PII redaction |
| Config via `.env` | Streams SSE | Metrics + Tracing | Quotas + Ratelimits |

> Endpoints:
> - `POST /v1/chat/completions` — Chat proxy (streams SSE).
> - `GET  /metrics` — Prometheus metrics.
> - `GET  /healthz` — Liveness.

---

## ✨ Features

- 🧮 **Redis-backed quotas**: per-tenant (per `X-Api-Key`) counters with TTL windows.
- 🚦 **HTTP rate-limit**: RPS/BURST using `tower-governor` with a custom key extractor (`X-Api-Key` fallback to IP+path).
- 🛑 **Circuit-breaker-lite**: request timeout, global concurrency limit, and load-shedding.
- 🔁 **Streaming bridge**: SSE in → SSE out (OpenAI “Chat Completions” style).
- ⏱️ **First-byte timeout**: the handler waits for the first upstream chunk and returns **504** if it doesn’t arrive in `TIMEOUT_SECS`.
- 🧽 **PII redaction**: redacts email/credit-card-like content in request and streamed deltas.
- 📈 **Telemetry**: Prometheus metrics + OTLP tracing (Jaeger UI).

---

## 🧰 Prerequisites

- **Rust** (stable) with `cargo`.
- **Docker** (for Redis and Jaeger).
- **Node.js** (optional) to run the local mock upstream for testing latency/timeouts.

---

## 🚀 Quick Start

1) **Copy `.env.example`**:

```bash
cp .env.example .env
```

2) **Run Redis (Docker):**
```bash
docker rm -f redis >/dev/null 2>&1 || true
docker run -d --name redis -p 6379:6379 redis:7-alpine
```

3) **(Optional) Start a local mock upstream (OPEN AI MOCK)**
Use the provided mock to test the app and that supports `LATENCY_MS` (first-byte delay) to test timeouts too:
Ensure edit `.env` with that URL.

Dummy Server is located in: ```openaimock/server.js```

Run it:
```bash
# Fast responses
node server.js
# Or slow (to trigger 504)
LATENCY_MS=5000 PORT=4000 node server.js
```

4) **Run the gateway**
```bash
cargo run
```

---

## ✅ Basic Functional Test

### 1) Streaming happy-path
```bash
curl -N http://localhost:8080/v1/chat/completions -H 'Content-Type: application/json' -H 'X-Api-Key: demo' -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hola 👋"}],"stream":true}'
```

### 2) Non-stream (single JSON)
```bash
curl http://localhost:8080/v1/chat/completions -H 'Content-Type: application/json' -H 'X-Api-Key: demo' -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hola 👋"}],"stream":false}'
```

---

## 📊 Quotas (Redis) — 200 → 429 rollover

**Config of interest:** `REDIS_URL`, `DEFAULT_QUOTA`, `QUOTA_WINDOW_SECS`, `TENANT_QUOTAS`.

Sequentially send 7 requests with the same API key (example below assumes `DEFAULT_QUOTA=5` and `QUOTA_WINDOW_SECS=60`; the binary defaults to 120/60s):  
Expected: 200×5 then **429**.

```bash
HOST=http://localhost:8080
BODY='{"model":"gpt-4o-mini","messages":[{"role":"user","content":"ping"}],"stream":false}'
for i in {1..7}; do
  echo -n "$i: "; curl -s -o /dev/null -w "%{http_code}\n" \
    -H "Content-Type: application/json" -H "X-Api-Key: demo" \
    -d "$BODY" "$HOST/v1/chat/completions"
done
```

Inspect Redis:
```bash
docker exec -it redis redis-cli -n 0 GET "quota:demo"
docker exec -it redis redis-cli -n 0 TTL "quota:demo"
```

Tenant overrides:
```bash
# TENANT_QUOTAS=tenantA=5,tenantB=8
for i in {1..7};  do curl -s -o /dev/null -w "%{http_code}\n" -H "X-Api-Key: tenantA" -H "Content-Type: application/json" -d "$BODY" $HOST/v1/chat/completions; done
for i in {1..10}; do curl -s -o /dev/null -w "%{http_code}\n" -H "X-Api-Key: tenantB" -H "Content-Type: application/json" -d "$BODY" $HOST/v1/chat/completions; done
```

Reset just one key (or flush DB cautiously):
```bash
docker exec -it redis redis-cli -n 0 DEL "quota:demo"
# docker exec -it redis redis-cli -n 0 FLUSHDB
```

---

## 🚦 HTTP Rate Limit (RPS/BURST) — 429 under concurrency

**Config:** `RPS=5`, `BURST=10`.

Fire a concurrent burst (this is *not* the Redis quota):
```bash
seq 50 | xargs -I{} -P 20 curl -s -o /dev/null -w "%{http_code}\n" \
  -H "Content-Type: application/json" -H "X-Api-Key: ratelimit" \
  -d "$BODY" "$HOST/v1/chat/completions" | sort | uniq -c
```
You should see mostly **429** with some **200**.

---

## Load Shedding (Concurrency) — 503

**Config:** `MAX_CONCURRENCY` (try `1` temporarily).

```bash
# In .env temporarily: MAX_CONCURRENCY=1 (then restart the gateway)
seq 40 | xargs -I{} -P 20 curl -s -o /dev/null -w "%{http_code}\n" \
  -H "Content-Type: application/json" -H "X-Api-Key: stress" \
  -d "$BODY" "$HOST/v1/chat/completions" | sort | uniq -c
```
Expected: a noticeable portion of **503** (“server overloaded”).

---

## Upstream Timeout — 504 (first-byte timeout)

**Pre-reqs:** run mock with a delay longer than `TIMEOUT_SECS`:

```bash
# .env: TIMEOUT_SECS=2
LATENCY_MS=5000 PORT=4000 node server.js
```

Request (non-stream is easiest to read):
```bash
curl -s -i http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" -H "X-Api-Key: timeout-test" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"ping"}],"stream":false}' | sed -n '1,30p'
```
Expected: **HTTP/1.1 504** `upstream timed out` after ~2s.

## 📈 Metrics (Prometheus)

- Scrape: `GET /metrics`
- Useful counters:
  - `requests_total{route="/v1/chat/completions"}`
  - `http_requests_total{route,model}`
  - `inflight_requests` (gauge)
  - `redactions_total`
  - `quota_block_total{reason="exceeded"}`
  - `cb_events_total{event="timeout" | "load_shed"}`

Examples:
```bash
curl -s http://localhost:8080/metrics | grep -E 'requests_total|cb_events_total|quota_block_total|redactions_total'
```

---

## 🛰️ Tracing (OTLP → Jaeger)

1) **Run Jaeger with OTLP enabled:**
```bash
docker rm -f jaeger >/dev/null 2>&1 || true
docker run -d --name jaeger \
  -e COLLECTOR_OTLP_ENABLED=true \
  -p 16686:16686 -p 4317:4317 -p 4318:4318 \
  jaegertracing/all-in-one:latest
# UI: http://localhost:16686
```

2) **Env for HTTP exporter (matches your code’s `with_http()`)**
```ini
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318   
OTEL_SERVICE_NAME=secure-llm-gateway
```

3) **Run the gateway** and generate a few requests.

4) **See traces in Jaeger UI**
- Service: `secure-llm-gateway`
- Tags filters you can use:
  - `tenant=demo`
  - `model=gpt-4o-mini`
  - `error=true`, `http.status_code=429|503|504`


## 🛎️ Handy One-Liners

```bash
# Basic health
curl -s http://localhost:8080/healthz && echo

# Metrics shortlist
curl -s http://localhost:8080/metrics | grep -E 'requests_total|cb_events_total|quota_block_total'

# Redis peek (adjust container name if different)
docker exec -it redis redis-cli -n 0 --scan --pattern 'quota:*'
docker exec -it redis redis-cli -n 0 GET "quota:demo"
docker exec -it redis redis-cli -n 0 TTL "quota:demo"
```

---

## ⚠️ Limitations

- Currently proxies only the OpenAI Chat Completions endpoint (`/v1/chat/completions`).
- Relies on OpenAI-compatible SSE semantics; other upstreams are not tested.
- PII redaction is regex-based and may produce false positives/negatives.

---

## 🔥 Smoke Tests

Run the scripts from the repo root; start the gateway (`cargo run`) and Redis beforehand.

- `./scripts/smoke_test_quota.sh` — clears the Redis counter and demonstrates the 200 → 429 rollover.
- `./scripts/smoke_test_ratelimit.sh` — fires parallel load to observe the RPS/BURST policy.
- `./scripts/smoke_test_load_shed.sh` — with a low `MAX_CONCURRENCY`, expects 503 responses due to load shedding.
- `./scripts/smoke_test_timeout.sh` — requires a slow upstream to confirm the 504 timeout.

Basic execution (uses the defaults defined within each script):

```bash
./scripts/smoke_test_quota.sh
./scripts/smoke_test_ratelimit.sh
./scripts/smoke_test_load_shed.sh
./scripts/smoke_test_timeout.sh
```

Example overriding environment variables before calling the script:

```bash
HOST=http://localhost:8081 \
KEY=tenantA \
REDIS_CTN=my_redis_container \
./scripts/smoke_test_quota.sh

HOST=http://localhost:8080 \
KEY=ratelimit \
REQUESTS=100 \
PARALLEL=50 \
./scripts/smoke_test_ratelimit.sh

HOST=http://localhost:8080 \
KEY=stress \
REQUESTS=80 \
PARALLEL=40 \
./scripts/smoke_test_load_shed.sh

HOST=http://localhost:8080 \
KEY=timeout-test \
./scripts/smoke_test_timeout.sh
```

For the timeout scenario, start the mock (`openaimock/server.js`) with a `LATENCY_MS` higher than the gateway's `TIMEOUT_SECS` to observe the 504 response.

---

## 🧭 Roadmap

- Extend compatibility to Anthropic (similar SSE contract) and other OpenAI-like APIs.
- Introduce a Cedar-based policy engine for tenant-scoped rules.
- Add a "no storage" mode with hashed identifiers in logs.
