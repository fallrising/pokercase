# Architecture

> Personal gateway for multi-tool AI teamwork — no paid API keys required long-term;
> personal OAuth / subscription tokens are first-class.

## Two layers

```
┌─────────────────────────────────────────────────────────────┐
│  Layer B — Assembler (AI-native business logic)             │
│  Multi-agent / “team” orchestration, tools, workflows       │
│  Calls only Layer A HTTP APIs (never talks to providers)    │
└────────────────────────────┬────────────────────────────────┘
                             │  http://127.0.0.1:20128/v1/*
                             ▼
┌─────────────────────────────────────────────────────────────┐
│  Layer A — thinrouter (this crate)                          │
│  • OpenAI + Anthropic + Responses surfaces                  │
│  • Auth (gateway keys), routes, fallback / RR, cooldown     │
│  • Token-saver rewrite (optional)                           │
│  • Upstream: OpenAI-compatible base URL  OR  OAuth import   │
│  • Admin UI / TUI / usage                                   │
└────────────────────────────┬────────────────────────────────┘
                             │
          ┌──────────────────┼──────────────────┐
          ▼                  ▼                  ▼
   Personal OAuth      Personal OAuth     Generic OpenAI-
   (Codex/Claude/…)    (Cursor/Kiro/…)    compatible node
```

**Rule:** Layer B must stay replaceable (scripts, agents, other apps).  
All provider secrets and fallback live in Layer A only.

## Layer A surfaces

| Path | Role |
|------|------|
| `POST /v1/chat/completions` | OpenAI Chat Completions |
| `POST /v1/messages` | Anthropic Messages (translate ↔ OpenAI upstream) |
| `POST /v1/responses` | OpenAI Responses (translate ↔ chat completions) |
| `GET /v1/models` | Routes + connections |
| `/admin`, `/admin/api/*` | Config + usage + OAuth import |

## Auth on connections

| `auth_type` | Credential | Notes |
|-------------|------------|--------|
| `api_key` | Bearer API key | Optional; not the personal primary path |
| `oauth_import` | access (+ refresh) tokens | Paste/import from CLI / browser session |

Provider-specific **browser OAuth** (device code / PKCE) lands per-provider later;  
import path unblocks personal subscription use first.

## Request pipeline

```
client → gateway auth → token-saver rewrite (opt)
      → resolve route/targets → skip cooldown → order (fallback|RR)
      → attach credential (api_key | oauth access)
      → forward (cancel if client drops) → SSE stall guard
      → usage log → response (maybe format translate)
```

## Layer B (out of this binary for now)

Examples of what *uses* Layer A:

- Parallel “team” calls: planner on route `brain`, coder on `cheap`, reviewer on `strict`
- Sequential pipelines with shared conversation state
- Tool routers that only know `http://127.0.0.1:20128/v1`

Can live as separate crate/scripts later; not required inside thinrouter MVP of OAuth.

## Explicit non-goals (still)

- Cloud multi-device sync
- Full 9router provider registry UI
- MITM
- Compatibility with `9router.db`
