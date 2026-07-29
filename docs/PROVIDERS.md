# Providers

Personal subscription / free upstreams for **Layer A** (thinrouter).

## Active (implemented enough to use)

| ID | Aliases | Status | Format | Default base |
|----|---------|--------|--------|--------------|
| `codex` | `cx` | **active** | OpenAI Responses | `https://chatgpt.com/backend-api/codex/responses` |
| `claude` | `cc`, `anthropic` | **active** | Anthropic Messages | `https://api.anthropic.com/v1/messages` |
| `cursor` | `cu` | **partial** | Stub (import only) | `https://api2.cursor.sh` |
| `grok` | `xai`, `grok_cli` | **active** | OpenAI Responses | `https://cli-chat-proxy.grok.com/v1` |
| `opencode` | `oc` | **active** | OpenAI Chat | `https://opencode.ai/zen/v1` |
| `agy` | `ag`, `antigravity` | **partial** | Stub (import only) | `https://cloudcode-pa.googleapis.com` |

**Status meaning**

- **active** — import tokens + forward with format conversion (chat ↔ responses / anthropic).
- **partial** — store OAuth/session tokens; full wire protocol (Cursor protobuf, Antigravity Gemini envelope) is **not** built yet. Use when you need them as a *project* and we implement the executor.

### Import

Admin UI → Connections → **Import OAuth**, or:

```bash
curl -s http://127.0.0.1:20128/admin/api/connections/oauth/import \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "my-codex",
    "provider": "codex",
    "access_token": "…",
    "default_model": "gpt-5.3-codex"
  }'
```

List registry: `GET /admin/api/providers`

---

## Wishlist (record only — develop when needed)

| ID | Notes |
|----|--------|
| `github_copilot` | OAuth + copilot-token exchange |
| `kiro` | Free/subscription custom executor |
| `gemini_cli` | Google Gemini CLI OAuth |
| `qwen` | Qwen / DashScope |
| `iflow` | iFlow free tier |
| `vertex` | Vertex free credits |
| `glm` | Zhipu GLM cheap API |
| `minimax` | MiniMax cheap API |
| `deepseek` | DeepSeek API key |
| `openrouter` | OpenRouter multi-model |
| `ollama` | Local Ollama |
| `azure_openai` | Azure deployments |
| `bedrock` | AWS Bedrock |
| `perplexity` | Perplexity |
| `grok_web` | Grok web (non-CLI) |
| `codebuddy` | Tencent CodeBuddy |
| `qoder` | Qoder |
| `mimo_free` | MiMo free |
| `xiaomi_tokenplan` | Xiaomi token plan |

When you need one of these: open an item, implement `ProviderProfile` + forward quirks, move it from wishlist → active/partial.

---

## Forwarding notes

| Provider | Client → Upstream |
|----------|-------------------|
| codex / grok | Chat Completions or Responses → **Responses** body |
| claude | Chat Completions → **Anthropic Messages**; native `/v1/messages` passes through |
| opencode | Chat Completions passthrough (+ `x-opencode-client`) |
| cursor / agy | Import only until executors land |

Token refresh (OAuth refresh_token) is **not** automated yet — re-import when expired.
