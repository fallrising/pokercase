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
CLI              →  serve | doctor
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
| Rust binary + clap (`serve`, `doctor`) | ✅ | |
| axum HTTP server + `/health` | ✅ | |
| SQLite (WAL) + migrations | ✅ | `~/.thinrouter` |
| Config via flags / env | ✅ | host, port, data_dir, admin_token |
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
| Usage event logging | ✅ | coarse status/latency/error |
| Real upstream end-to-end | 🟨 | smoke with dead upstream only; **real API key not verified yet** |
| SSE stall timeout | ❌ | Go reference has this |
| Round-robin strategy | ❌ | only sequential fallback |
| Connection cooldown / model lock | ❌ | |

### Phase 1.5 — Web admin

| Item | Status | Notes |
|------|--------|--------|
| Dashboard | ✅ | stats + sample curl |
| Connections CRUD (UI) | 🟨 | create + delete; no full edit form |
| Routes CRUD (UI) | 🟨 | single-target create + delete |
| Multi-target route via API | ✅ | `POST /admin/api/routes` with targets[] |
| Multi-target editor in UI | ❌ | documented workaround: JSON API |
| API keys UI | ✅ | create (show secret once) + delete |
| Usage page | ✅ | last N events |
| Connection test API | ✅ | `POST .../connections/{id}/test` |
| Connection test button in UI | ❌ | |
| Admin token login in UI | ❌ | header `x-admin-token` for API only |

### Phase 2 — Experience (planned)

| Item | Status | Notes |
|------|--------|--------|
| Claude `POST /v1/messages` + format translation | ❌ | needed if clients speak Anthropic natively |
| TUI (`ratatui`) | ❌ | connections / routes / recent requests |
| Stronger error classify + backoff | ❌ | align more with Go |
| Usage charts / cost | ❌ | |
| Docker image / release binaries | ❌ | |
| Broader automated tests (mock stream e2e) | 🟨 | resolve tests only |

### Phase 3 — Explicitly deferred

| Item | Status |
|------|--------|
| OAuth / MITM / RTK / fusion / media | 🚫 |

---

## 5. What works today (user-facing)

1. `cargo run -- serve` → proxy on `http://127.0.0.1:20128/v1`, admin on `/admin`.
2. Add an OpenAI-compatible **connection** (base URL + API key + default model).
3. Create a **route** public model name pointing at that connection.
4. Optionally create a **gateway API key**.
5. Call:

```bash
curl -s http://127.0.0.1:20128/v1/chat/completions \
  -H "Authorization: Bearer YOUR_GATEWAY_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"your-route","messages":[{"role":"user","content":"hi"}]}'
```

6. Client `model` values:
   - `route-name` → route targets (fallback order)
   - `connection/model` → direct upstream model
   - `connection` → connection `default_model`

7. Multi-target fallback can be created via admin JSON API (see README).

---

## 6. Remaining work (backlog)

Ordered by practical priority for “daily driver” use:

### P0 — Validate

- [ ] End-to-end with a **real** OpenAI-compatible API key (stream + non-stream)
- [ ] Confirm a real IDE/CLI (Cursor / Claude Code OpenAI mode / curl) against this proxy

### P1 — Protocol / clients

- [ ] Claude `/v1/messages` request/response (+ stream) translation if Anthropic-native clients are required
- [ ] Document exact settings for each client we care about

### P2 — Admin UX

- [ ] Edit connection / route in UI
- [ ] Multi-target route editor (ordered list) in UI
- [ ] “Test connection” button on Connections page
- [ ] Optional simple admin auth for non-loopback binds

### P3 — Reliability & ops

- [ ] SSE stall detection
- [ ] Cooldown / model lock after rate limits
- [ ] Mock upstream e2e tests (including streaming)
- [ ] Docker + GitHub Release binaries
- [ ] Align naming: crate `thinrouter` vs repo `pokercase` (rename or document only)

### P4 — Nice-to-have

- [ ] TUI
- [ ] Round-robin
- [ ] Encrypt secrets at rest / OS keyring
- [ ] Cost estimation from usage

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
  main.rs       CLI
  config.rs     data dir / listen
  store.rs      SQLite
  resolve.rs    model → targets
  proxy.rs      forward + fallback + SSE
  server.rs     /v1 + middleware
  admin.rs      /admin/api/*
  web.rs        /admin pages
  templates.rs  minijinja
templates/      HTML + CSS
docs/           this document
```

---

## 9. Changelog of planning milestones

| Date | Milestone |
|------|-----------|
| 2026-07-28 | Architecture plan: thin proxy + web config; Rust; own schema |
| 2026-07-28 | Phase 0–1.5 implemented (proxy + admin UI + API) |
| 2026-07-28 | Published as `fallrising/pokercase` |
| 2026-07-28 | This goals & progress doc |

---

## 10. How to continue

1. Read this file + `README.md`.
2. Pick the next P0/P1 item from §6.
3. After meaningful progress, update the status tables and backlog checkboxes in this file, then commit.

Suggested first session after a break:

```bash
cd thinrouter   # or clone fallrising/pokercase
cargo run -- serve
# open http://127.0.0.1:20128/admin
# wire a real key and verify stream
```
