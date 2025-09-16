// server.js - Mock OpenAI with first-byte delay (forces 504 in the gateway)
import http from 'http';
import { randomUUID } from 'crypto';

const PORT = Number(process.env.PORT ?? 4000);
// Set this > TIMEOUT_SECS*1000 in the gateway to trigger a 504, e.g. 5000 if TIMEOUT_SECS=2
const LATENCY_MS = Number(process.env.LATENCY_MS ?? 0);

const MOCK_REPLY = "Hi, I am a mock OpenAI server. I can help you test your gateway without spending credits.";
const TOKENS = Array.from(MOCK_REPLY.match(/\s*[^\s]+/g) ?? []);

function logSafe(req, payload, stream) {
  const sanitizedHeaders = {
    ...req.headers,
    authorization: req.headers.authorization ? '[redacted]' : undefined,
  };
  console.log('--- Incoming /v1/chat/completions ---');
  console.log('Headers:', sanitizedHeaders);
  console.log('Body:', JSON.stringify(payload, null, 2));
  console.log(`Simulating first-byte latency: ${LATENCY_MS} ms (stream=${stream})`);
}

const server = http.createServer((req, res) => {
  if (req.method === 'POST' && req.url === '/v1/chat/completions') {
    let body = '';

    req.on('data', (chunk) => { body += chunk; });

    req.on('end', () => {
      let payload = {};
      try {
        payload = body.length ? JSON.parse(body) : {};
      } catch {
        res.writeHead(400, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ error: 'invalid_json', message: 'Could not parse request body.' }));
        return;
      }

      const model = payload.model ?? 'gpt-4o-mini';
      const created = Math.floor(Date.now() / 1000);
      const id = `mock-${randomUUID()}`;
      const stream = Boolean(payload.stream);

      logSafe(req, payload, stream);

      // Do not send ANYTHING before this delay.
      setTimeout(() => {
        if (stream) {
          // ---- STREAMING (SSE) ----
          console.log('[mock] Sending headers SSE after delay…');
          res.writeHead(200, {
            'Content-Type': 'text/event-stream',
            'Cache-Control': 'no-cache',
            Connection: 'keep-alive',
          });

          TOKENS.forEach((token, index) => {
            const chunk = {
              id,
              object: 'chat.completion.chunk',
              created,
              model,
              choices: [{
                index: 0,
                delta: index === 0 ? { role: 'assistant', content: token } : { content: token },
                finish_reason: null,
              }],
            };
            setTimeout(() => {
              try { res.write(`data: ${JSON.stringify(chunk)}\n\n`); } catch {}
            }, index * 120);
          });

          const endDelay = TOKENS.length * 120 + 120;
          setTimeout(() => {
            try {
              const finalChunk = {
                id,
                object: 'chat.completion.chunk',
                created,
                model,
                choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
              };
              res.write(`data: ${JSON.stringify(finalChunk)}\n\n`);
              res.write('data: [DONE]\n\n');
              res.end();
            } catch {}
          }, endDelay);

          return;
        }

        // ---- NO-STREAM ----
        const promptTokens = Array.isArray(payload.messages)
          ? payload.messages.reduce(
              (acc, msg) => acc + (msg.content?.split(/\s+/).filter(Boolean).length ?? 0),
              0,
            )
          : 0;

        const responseBody = {
          id,
          object: 'chat.completion',
          created,
          model,
          choices: [{
            index: 0,
            message: { role: 'assistant', content: MOCK_REPLY },
            finish_reason: 'stop',
          }],
          usage: {
            prompt_tokens: promptTokens,
            completion_tokens: TOKENS.length,
            total_tokens: promptTokens + TOKENS.length,
          },
        };

        console.log('[mock] Sending headers JSON after delay…');
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify(responseBody));
      }, LATENCY_MS);
    });

    req.on('error', (err) => {
      console.error('Error receiving request:', err);
      if (!res.headersSent) {
        res.writeHead(500, { 'Content-Type': 'application/json' });
      }
      res.end(JSON.stringify({ error: 'request_error', message: 'There was a problem reading the request.' }));
    });

    return;
  }

  res.writeHead(404, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ error: 'not_found', message: 'Route not found in the OpenAI mock.' }));

});

server.listen(PORT, () => {
  console.log(`Mock OpenAI API listening on http://localhost:${PORT} (LATENCY_MS=${LATENCY_MS}ms)`);
});
