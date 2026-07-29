# Goals & Progress

> Last updated: 2026-07-28  
> Repo: [fallrising/pokercase](https://github.com/fallrising/pokercase)  
> Working name in code: **thinrouter** (binary / crate); GitHub repo name: **pokercase**

## 1. Mission

Build a **thin, self-owned LLM API proxy** in Rust — inspired by [9router](https://github.com/decolua/9router) and [9router-go](https://github.com/luqman-v1/9router-go), but **not** a full clone.

We deliberately “copy homework” on the hot path (auth → resolve model → fallback → OpenAI-compatible forward → SSE), while keeping a **simple web UI** to configure providers. We do **not** aim for 100+ providers, OAuth subscription hacks, or MITM.

**Success for MVP:** one process, local SQLite, configure upstreams in a browser, point Claude Code / Cursor / curl at `/v1`, and chat works (stream + non-stream).

---

## 2. Product shape

```
CLI tools / IDE  →  /v1/* (proxy)     →  upstream OpenAI-compatible APIs
Browser          →  /admin (config UI)
CLI              →  serve | doctor | tui
Data             →  ~/.thinrouter/thinrouter.db  (own schema, not 9router.db)
```

Single binary, single process: proxy + admin UI + SQLite.

---

## 3. Goals (what we want)

### In scope (target)

| Area | Goal |
|------|------|
| LLM proxy | OpenAI-compatible `POST /v1/chat/completions` (stream + non-stream) |
| Models | `GET /v1/models`; resolve `route` / `connection/model` / bare connection |
| Fallback | Ordered targets; retry next on retryable upstream errors **before** SSE is committed |
| Config UI | Web UI: connections, routes, gateway API keys, usage |
| Config API | JSON under `/admin/api/*` for automation |
| Auth | Gateway API keys for `/v1`; optional admin token |
| Ops | `serve`, `doctor`, health endpoint, graceful shutdown |
| Later | TUI; Claude `/v1/messages` translation; stronger cooldown/locks; Docker/release |

### Out of scope (unless we explicitly reopen)

- Full 9router provider registry (100+)
- OAuth / subscription account flows (Kiro, Cursor, Copilot, etc.)
- MITM proxy
- RTK / Caveman / token-saver pipelines
- Fusion / multi-panel judge
- Media: embeddings, image, TTS, scrape, etc.
- Compatibility with official `9router.db` schema

### Design principles (from Go reference)

1. Propagate request cancellation to upstream.
2. No fallback after SSE response headers are committed.
3. Prefer byte passthrough for streaming; transform only when needed.
4. Cap request body size; mask secrets in logs.
5. Own thin schema; do not bind to upstream dashboard DB.

---

## 4. Progress status

Legend: ✅ done · 🟨 partial · ❌ not started · 🚫 deferred (out of scope)

### Phase 0 — Scaffold

| Item | Status | Notes |
|------|--------|--------|
| Rust binary + clap (`serve`, `doctor`) | ✅ | also `tui` |
| axum HTTP server + `/health` | ✅ | |
| SQLite (WAL) + migrations | ✅ | `~/.thinrouter` |
| Config via flags / env | ✅ | host, port, data_dir, admin_token, secrets_key, sse_stall |
| Repo on GitHub | ✅ | `fallrising/pokercase` |

### Phase 1 — Thin proxy

| Item | Status | Notes |
|------|--------|--------|
| `POST /v1/chat/completions` | ✅ | stream + non-stream |
| `GET /v1/models` | ✅ | |
| Gateway API key auth | ✅ | no keys ⇒ bootstrap open |
| Model resolve (route / `conn/model` / default) | ✅ | unit tests for resolve |
| OpenAI-compatible forward | ✅ | rewrite `model` + Bearer |
| Ordered fallback | ✅ | 401/402/403/408/429/5xx |
| Usage event logging | ✅ | status/latency/error + tokens/cost |
| Real upstream end-to-end | 🟨 | **mock e2e ✅**; real API key still user-side |
| SSE stall timeout | ✅ | `--sse-stall-secs` / env (default 90) |
| Round-robin strategy | ✅ | route `strategy: round_robin` |
| Connection cooldown / model lock | ✅ | in-memory after 429/auth/5xx |

### Phase 1.5 — Web admin

| Item | Status | Notes |
|------|--------|--------|
| Dashboard | ✅ | stats + sample curl + est. cost |
| Connections CRUD (UI) | ✅ | create + edit + delete + prices |
| Routes CRUD (UI) | ✅ | multi-target (5 slots) + edit + delete |
| Multi-target route via API | ✅ | `POST/PUT /admin/api/routes` |
| Multi-target editor in UI | ✅ | ordered slots 1–5 |
| API keys UI | ✅ | create (show secret once) + delete |
| Usage page | ✅ | events + tokens + cost total |
| Connection test API | ✅ | `POST .../connections/{id}/test` |
| Connection test button in UI | ✅ | |
| Admin token login in UI | ✅ | `/admin/login` + cookie |

### Phase 2 — Experience

| Item | Status | Notes |
|------|--------|--------|
| Claude `POST /v1/messages` + format translation | ✅ | non-stream + stream SSE map |
| TUI (`ratatui`) | ✅ | `thinrouter tui` |
| Stronger error classify + backoff | ✅ | cooldown map by status class |
| Usage charts / cost | 🟨 | cost estimate from $/1M rates; no charts |
| Docker image / release binaries | ✅ | Dockerfile + compose + GH release workflow |
| Broader automated tests (mock stream e2e) | ✅ | `tests/e2e_mock.rs` |

### Phase 3 — Explicitly deferred

| Item | Status |
|------|--------|
| OAuth / MITM / RTK / fusion / media | 🚫 |

---

## 5. What works today (user-facing)

1. `cargo run -- serve` → proxy on `http://127.0.0.1:20128/v1`, admin on `/admin`.
2. Add an OpenAI-compatible **connection** (base URL + API key + default model; optional $/1M prices).
3. Create a **route** with one or more targets (`fallback` or `round_robin`).
4. Optionally create a **gateway API key**; optionally set `THINROUTER_ADMIN_TOKEN`.
5. Call OpenAI or Anthropic-shaped APIs (see [CLIENTS.md](./CLIENTS.md)).
6. `thinrouter tui` for terminal overview; Docker via `docker compose up --build`.
7. Optional `THINROUTER_SECRETS_KEY` encrypts connection API keys at rest.

```bash
curl -s http://127.0.0.1:20128/v1/chat/completions \
  -H "Authorization: Bearer YOUR_GATEWAY_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"your-route","messages":[{"role":"user","content":"hi"}]}'
```

---

## 6. Remaining work (backlog)

### P0 — Validate

- [x] Mock end-to-end (stream + non-stream + Anthropic + fallback) — automated
- [ ] End-to-end with a **real** OpenAI-compatible API key (user must supply key)
- [ ] Confirm a real IDE/CLI (Cursor / Claude Code OpenAI mode / curl) against this proxy

### P1 — Protocol / clients

- [x] Claude `/v1/messages` request/response (+ stream) translation
- [x] Document exact settings for each client we care about → [CLIENTS.md](./CLIENTS.md)

### P2 — Admin UX

- [x] Edit connection / route in UI
- [x] Multi-target route editor (ordered list) in UI
- [x] “Test connection” button on Connections page
- [x] Optional simple admin auth for non-loopback binds (token + login cookie)

### P3 — Reliability & ops

- [x] SSE stall detection
- [x] Cooldown / model lock after rate limits
- [x] Mock upstream e2e tests (including streaming)
- [x] Docker + GitHub Release binaries (workflow; run on tag `v*`)
- [x] Align naming: crate `thinrouter` vs repo `pokercase` (documented, not renamed)

### P4 — Nice-to-have

- [x] TUI
- [x] Round-robin
- [x] Encrypt secrets at rest (`THINROUTER_SECRETS_KEY`; not OS keyring)
- [x] Cost estimation from usage (token × connection $/1M rates)
- [ ] Usage charts (visual)
- [ ] OS keyring integration

---

## 7. Reference map (where we copy from)

| Topic | Upstream reference |
|-------|-------------------|
| Request lifecycle | 9router-go `ARCHITECTURE.md`, `handlers/chat/*` |
| Forward + SSE | `forward.go`, `proxy/sse.go`, `executor/openai.go` |
| Auth middleware | `middleware/auth.go` |
| Fallback ideas | `fallback.go`, `errorclassify.go` |
| DB concepts only | 9router-go `DATABASE.md` (we use our own schema) |
| Full product surface | 9router `docs/ARCHITECTURE.md`, `open-sse/` |

We implement **behavior-inspired** code in Rust; we do not vendor upstream source or brand as 9Router.

---

## 8. Layout (code)

```
src/
  main.rs       CLI (serve | doctor | tui)
  config.rs     data dir / listen / secrets / stall
  store.rs      SQLite
  resolve.rs    model → targets + strategy
  proxy.rs      forward + fallback + SSE stall + RR order
  cooldown.rs   in-memory connection cooldown
  claude.rs     Anthropic ↔ OpenAI translate
  secrets.rs    optional AES-GCM at-rest key encrypt
  server.rs     /v1 + /v1/messages + middleware
  admin.rs      /admin/api/*
  web.rs        /admin pages + login
  tui_app.rs    ratatui overview
  templates.rs  minijinja
templates/      HTML + CSS
docs/           goals + clients
tests/          mock e2e
Dockerfile      multi-stage image
```

---

## 9. Changelog of planning milestones

| Date | Milestone |
|------|-----------|
| 2026-07-28 | Architecture plan: thin proxy + web config; Rust; own schema |
| 2026-07-28 | Phase 0–1.5 implemented (proxy + admin UI + API) |
| 2026-07-28 | Published as `fallrising/pokercase` |
| 2026-07-28 | Goals & progress doc |
| 2026-07-28 | Full backlog pass: Claude messages, admin UX, SSE stall, cooldown, RR, mock e2e, Docker/release, TUI, secrets encrypt, cost estimate |

---

## 10. How to continue

1. Read this file + `README.md` + `docs/CLIENTS.md`.
2. Remaining user-side items: real API key smoke + IDE confirmation (P0).
3. Optional polish: usage charts, OS keyring.
4. After meaningful progress, update this file, then commit.

Suggested first session after a break:

```bash
cd thinrouter   # or clone fallrising/pokercase
cargo test
cargo run -- serve
# open http://127.0.0.1:20128/admin
# wire a real key and verify stream + non-stream
# try: cargo run -- tui
```
