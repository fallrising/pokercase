# Goals & Progress

> Last updated: 2026-07-28  
> Repo: [fallrising/pokercase](https://github.com/fallrising/pokercase)  
> Working name: **thinrouter** · Architecture: [ARCHITECTURE.md](./ARCHITECTURE.md)

## Mission (updated)

**Personal multi-tool AI gateway** for someone with **subscription seats (no paid API keys)**:

1. **Layer A (this crate):** provide stable service APIs (`/v1/*`) over personal OAuth/session tokens + optional OpenAI-compatible nodes; routes, fallback, token-saver, usage.
2. **Layer B (separate later):** assemble multi-agent / “team” workflows that **only** call Layer A.

Success: Cursor + Claude Code + Codex + custom agents all point at one local base URL; credentials stay in thinrouter; business logic stays outside.

---

## Product shape

```
CLI / IDE / Layer-B agents  →  /v1/*  →  OAuth imports | OpenAI-compatible
Browser                     →  /admin
CLI                         →  serve | doctor | tui
Data                        →  ~/.thinrouter/thinrouter.db
```

---

## Progress

Legend: ✅ done · 🟨 partial · ❌ not started · 🚫 deferred

### Core proxy

| Item | Status |
|------|--------|
| Chat Completions stream/non-stream | ✅ |
| Anthropic `/v1/messages` | ✅ |
| OpenAI `/v1/responses` | ✅ |
| Models / routes / fallback / RR | ✅ |
| SSE stall + client-disconnect cancel (pre-SSE) | ✅ |
| HTTP(S)_PROXY env for upstream | ✅ |
| Token-saver rewrite (opt-in) | ✅ |
| Usage events + daily aggregate + CSV | ✅ |

### Credentials

| Item | Status | Notes |
|------|--------|--------|
| API key connections | ✅ | optional path |
| **OAuth import** (paste access/refresh) | ✅ | `POST /admin/api/connections/oauth/import` + UI |
| Browser OAuth / PKCE per provider | 🟨 | scaffold only — Codex/Claude/Copilot/Cursor/Kiro full flows next |
| Auto token refresh | ❌ | needs per-provider refresh |
| Provider-specific request executors | ❌ | some OAuth upstreams need non-OpenAI shapes |

### Admin / ops

| Item | Status |
|------|--------|
| Web CRUD + multi-target + test + login | ✅ |
| TUI / Docker / release workflow | ✅ |
| Real subscription e2e (your tokens) | 🟨 | user-side |

### Layer B assembler

| Item | Status |
|------|--------|
| Separate multi-agent orchestrator | ❌ | design only in ARCHITECTURE.md |

---

## Backlog (priority for *your* use case)

### P0 — Unblock personal subscriptions

1. [ ] Import + smoke **your** Codex / Claude / Cursor / Copilot / Kiro tokens end-to-end  
2. [ ] Per-provider **refresh** where tokens expire  
3. [ ] Provider-specific forward quirks (headers, base paths) as needed when import alone fails  

### P1 — Protocol & cost

4. [x] OpenAI Responses API  
5. [x] Token-saver (tool_result truncate, opt-in)  
6. [ ] Deeper Anthropic tools/stream edge cases  
7. [ ] Stronger token-saver (RTK-like filters, configurable rules)  

### P2 — Engineering (recommended)

8. [x] Disconnect cancel during upstream connect  
9. [x] HTTP proxy env  
10. [x] Usage daily + CSV  
11. [ ] Usage charts (visual)  
12. [ ] Model alias short names  

### P3 — Layer B

13. [ ] Minimal assembler (parallel team call script/crate using only `/v1`)  
14. [ ] Shared task/run state for multi-agent  

### Explicitly later / careful

- Full browser OAuth UX like 9router (after import path is proven)  
- Cloud sync, MITM, 9router.db compatibility — **no**

---

## How to continue

1. Read [ARCHITECTURE.md](./ARCHITECTURE.md) + this file.  
2. Import a real subscription token in Admin → Connections.  
3. Point one client at `/v1` and log failures (status + body) to drive provider executors.  
4. Update this checklist after each milestone; commit per phase.
