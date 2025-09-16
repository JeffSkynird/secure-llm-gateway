# Mock OpenAI API

Minimal HTTP server that mimics OpenAI's `/v1/chat/completions` endpoint so you can test `secure-llm-gateway` without using your real API key.

## Usage

```bash
npm install # optional, there are no external dependencies
npm start
```

The server listens by default on `http://localhost:4000`.

In your main project, override OpenAI's base URL to point to the mock, for example with the environment variable:

```bash
export OPENAI_BASE_URL="http://localhost:4000"
```

Then run the gateway and the requests to OpenAI will reach the mock. The server supports both regular responses and streaming (SSE) and returns a sample message.

You can modify `server.js` to customize the text you want to return.
