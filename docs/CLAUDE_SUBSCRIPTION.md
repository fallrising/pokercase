# Claude 订阅如何接到 thinrouter

## 原理（两条账本）

Anthropic 侧大致有两套「付钱方式」：

| 路径 | 凭证 | 计费 |
|------|------|------|
| **Console / API Key** | `sk-ant-api…`，Header `x-api-key` | 按 token 从 API 余额扣费 |
| **claude.ai 订阅（Pro/Max/Team）** | OAuth access token（常以 `sk-ant-oat…` 开头），Header `Authorization: Bearer …` | 走**订阅配额/限速**，不是 API 余额 |

Claude Code CLI 默认走的是 **订阅 OAuth**，不是 Console API Key。

```
浏览器登录 claude.ai
        │
        ▼
 OAuth 授权码 + PKCE
        │
        ▼
 token endpoint 换 access_token + refresh_token
        │
        ▼
 Linux: ~/.claude/.credentials.json   (mode 0600)
 macOS: Keychain
        │
        ▼
 每次请求 POST https://api.anthropic.com/v1/messages
    Authorization: Bearer <access_token>
    Anthropic-Version / Anthropic-Beta / User-Agent=claude-cli/…
```

thinrouter 做的事：

1. 从本机读出（或你粘贴）这个 OAuth access token  
2. 代你发 `/v1/messages`（或把 OpenAI chat 转成 messages）  
3. 到期前用 refresh_token 换新 access（与 CLI 同类）

所以 **不是「破解」**，而是 **复用 Claude Code 已经换到的合法 OAuth 会话**，请求仍打官方 API，**配额与 429 仍算你的订阅**。

官方文档也写明：可用 `claude setup-token` 生成长生命周期 token，再设：

```bash
export CLAUDE_CODE_OAUTH_TOKEN=…
```

该 token 绑定订阅，主要用于模型推理。  
见：[Claude Code Authentication](https://code.claude.com/docs/en/authentication)

---

## 和「这台机器上能用 Claude」的关系

「能用 Claude」可能是：

| 场景 | 是否等于 thinrouter 能代理 |
|------|---------------------------|
| Claude Code CLI 已 `claude` 登录 | 通常 **是**（有 credentials） |
| Claude 网页 / 桌面 App | **否**（会话 cookie 不在 CLI 凭据文件里） |
| Cursor 里选 Claude 模型 | **否**（走 Cursor 自己的协议与账号） |
| 设了 `ANTHROPIC_API_KEY` | 那是 **API 计费**，不是 Pro 订阅 OAuth |

本机若 `claude auth status` 显示未登录，或 `~/.claude/.credentials.json` 里 `accessToken`/`refreshToken` 为空，则 **CLI 订阅会话已失效**，需要重新登录。

检查：

```bash
claude auth status
# 或
python3 -c "import json;print(json.load(open('$HOME/.claude/.credentials.json')))"
```

重新拿到 token：

```bash
# 交互登录（会写回 .credentials.json）
claude
# 在对话里 /login

# 或生成 1 年期 token（不写文件，需自己 export）
claude setup-token
export CLAUDE_CODE_OAUTH_TOKEN='…'
```

---

## thinrouter 导入方式

```bash
# 1) 优先读 ~/.claude/.credentials.json
# 2) 若为空，读环境变量 CLAUDE_CODE_OAUTH_TOKEN
thinrouter import-local --routes --refresh

# 或 Admin UI → Import OAuth → provider=claude → 粘贴 access token
```

转发时 thinrouter 会带 Claude Code 风格的 beta headers，并把客户端的 OpenAI chat 转成 Anthropic messages（或直接 `/v1/messages`）。

---

## 常见失败

| 现象 | 含义 |
|------|------|
| `accessToken` 空 | 未登录 / 已 logout / 会话过期且 refresh 失败 |
| **401** | token 无效 → 再 `/login` 或 `setup-token` |
| **429 rate_limit** | 订阅限流，**整合是成功的**，等配额恢复或降模型 |
| 有 API Key 却扣 API 费 | 环境里 `ANTHROPIC_API_KEY` 优先级更高，先 `unset` |

---

## 合规与风险提示

- 仅用于 **你自己的订阅、你自己的机器/网关**。  
- 第三方「共享订阅 / 卖 token」违反 ToS，且有封号风险。  
- thinrouter 是本地代理；不要把 OAuth token 提交到 git 或公开环境。
