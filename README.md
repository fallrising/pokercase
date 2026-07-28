# thinrouter

A **thin** OpenAI-compatible LLM API proxy in Rust, inspired by [9router](https://github.com/decolua/9router) / [9router-go](https://github.com/luqman-v1/9router-go).

**GitHub repo:** [fallrising/pokercase](https://github.com/fallrising/pokercase)  
**Goals & progress:** [docs/GOALS_AND_PROGRESS.md](./docs/GOALS_AND_PROGRESS.md)

**In scope:** `/v1/chat/completions` (stream + non-stream), model routes with fallback, web admin for connections / routes / keys, local SQLite.

**Out of scope (for now):** OAuth subscription providers, MITM, RTK, Claude `/v1/messages`, media endpoints.

## Quick start

```bash
cargo run -- serve
# admin:  http://127.0.0.1:20128/admin
# proxy:  http://127.0.0.1:20128/v1
# data:   ~/.thinrouter/thinrouter.db
```

1. Open **Connections** → add an OpenAI-compatible upstream (`base_url` + `api_key` + default model).
2. Open **Routes** → map a public model name (e.g. `cheap`) to that connection.
3. Optionally create an **API Key**. If none exist, `/v1` is open (bootstrap mode).
4. Call the proxy:

```bash
curl -s http://127.0.0.1:20128/v1/chat/completions \
  -H "Authorization: Bearer YOUR_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "cheap",
    "messages": [{"role":"user","content":"hi"}],
    "stream": false
  }'
```

Model resolution:

| Client `model` | Behavior |
|----------------|----------|
| `route-name` | Use named route targets (ordered fallback) |
| `connection/model` | Direct to that connection + upstream model |
| `connection` | Connection’s `default_model` |

## CLI

```bash
thinrouter serve --host 127.0.0.1 --port 20128
thinrouter serve --data-dir ./data --admin-token secret
thinrouter doctor
```

Env vars: `THINROUTER_HOST`, `THINROUTER_PORT`, `THINROUTER_DATA_DIR`, `THINROUTER_ADMIN_TOKEN`.

## Admin JSON API

Same operations as the UI under `/admin/api/*` (optional header `x-admin-token` if configured):

- `GET/POST /admin/api/connections`
- `POST /admin/api/connections/{id}/test`
- `GET/POST /admin/api/routes` (multi-target fallback via JSON)
- `GET/POST /admin/api/keys`
- `GET /admin/api/usage`

Example multi-target route:

```bash
curl -s http://127.0.0.1:20128/admin/api/routes \
  -H 'Content-Type: application/json' \
  -d '{
    "public_model": "cheap",
    "strategy": "fallback",
    "targets": [
      {"connection_id": "UUID1", "model_override": "gpt-4o-mini"},
      {"connection_id": "UUID2", "model_override": null}
    ]
  }'
```

## Design notes

- Single process: proxy + web UI + SQLite.
- Hot path copies Go 9router-go ideas: API key auth, resolve model, ordered fallback **before** SSE headers are committed, byte-stream passthrough for streaming.
- Own schema (`~/.thinrouter`), not 9router.db compatible.

## License

MIT. Inspired by upstream MIT projects; not affiliated with 9Router.
