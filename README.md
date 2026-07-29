# thinrouter

A **thin** OpenAI-compatible LLM API proxy in Rust, inspired by [9router](https://github.com/decolua/9router) / [9router-go](https://github.com/luqman-v1/9router-go).

| | |
|--|--|
| **GitHub repo** | [fallrising/pokercase](https://github.com/fallrising/pokercase) |
| **Crate / binary name** | `thinrouter` |
| **Goals & progress** | [docs/GOALS_AND_PROGRESS.md](./docs/GOALS_AND_PROGRESS.md) |
| **Client setup** | [docs/CLIENTS.md](./docs/CLIENTS.md) |

> **Naming:** the GitHub repository is **pokercase**; the product binary and local data dir are **thinrouter** (`~/.thinrouter`). This is intentional documentation, not a rename of either side.

**In scope:** `/v1/chat/completions`, `/v1/messages`, `/v1/responses`, routes + fallback/RR, **OAuth/session token import** (personal subscriptions), optional token-saver, web admin, TUI, SQLite.

**Architecture:** Layer A = this gateway; Layer B = your multi-agent “team” apps calling only Layer A. See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md).

**Out of scope (for now):** MITM, cloud sync, 9router.db compatibility, media APIs.

## Quick start

```bash
cargo run -- serve
# admin:  http://127.0.0.1:20128/admin
# proxy:  http://127.0.0.1:20128/v1
# claude:    http://127.0.0.1:20128/v1/messages
# responses: http://127.0.0.1:20128/v1/responses
# data:      ~/.thinrouter/thinrouter.db
```

1. Open **Connections** → either:
   - **Import OAuth / session token** (personal subscription; no API key), or  
   - Add OpenAI-compatible `base_url` + API key (optional path).
2. Open **Routes** → map a public model name to one or more targets.
3. Optionally create a **gateway API Key**. If none exist, `/v1` is open (bootstrap).
4. Point clients at the proxy (see [docs/CLIENTS.md](./docs/CLIENTS.md)).

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
| `route-name` | Named route targets (`fallback` or `round_robin`) |
| `connection/model` | Direct to that connection + upstream model |
| `connection` | Connection’s `default_model` |

## CLI

```bash
thinrouter serve --host 127.0.0.1 --port 20128
thinrouter serve --data-dir ./data --admin-token secret --secrets-key passphrase
thinrouter doctor
thinrouter tui
```

| Flag / env | Purpose |
|------------|---------|
| `THINROUTER_HOST` / `--host` | Bind host |
| `THINROUTER_PORT` / `--port` | Bind port |
| `THINROUTER_DATA_DIR` / `--data-dir` | SQLite directory |
| `THINROUTER_ADMIN_TOKEN` / `--admin-token` | Protect `/admin` (login UI + `x-admin-token` / cookie) |
| `THINROUTER_SECRETS_KEY` / `--secrets-key` | Encrypt connection API keys at rest |
| `THINROUTER_SSE_STALL_SECS` / `--sse-stall-secs` | Abort SSE if no chunk (default 90) |
| `THINROUTER_TOKEN_SAVER` / `--token-saver` | Truncate tool / huge message content |
| `THINROUTER_TOKEN_SAVER_MAX_CHARS` | Max chars kept per tool-like block (default 2000) |
| `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` | Upstream egress proxy |

## Admin JSON API

Same operations as the UI under `/admin/api/*` (cookie or `x-admin-token` when configured):

- `GET/POST /admin/api/connections` · `PUT/DELETE .../{id}` · `POST .../{id}/test`
- `POST /admin/api/connections/oauth/import` — personal subscription tokens
- `GET/POST /admin/api/routes` · `GET/PUT/DELETE .../{id}`
- `GET/POST /admin/api/keys` · `DELETE .../{id}`
- `GET /admin/api/usage` · `.../daily` · `.../export.csv` · `GET /admin/api/stats`

Multi-target route:

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

## Docker

```bash
docker compose up --build -d
# http://127.0.0.1:20128/admin
```

## Design notes

- Single process: proxy + web UI + SQLite (+ optional TUI).
- Hot path: API key auth → resolve model → ordered fallback / RR **before** SSE headers committed → byte passthrough (or Anthropic translate).
- Cooldown after 429 / auth / 5xx; SSE stall timeout.
- Own schema (`~/.thinrouter`), not 9router.db compatible.

## License

MIT. Inspired by upstream MIT projects; not affiliated with 9Router.
