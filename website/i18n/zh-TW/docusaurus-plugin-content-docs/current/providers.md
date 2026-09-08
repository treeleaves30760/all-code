---
id: providers
title: Provider 相容性
sidebar_position: 5
description: 哪些 LLM provider 能搭配八個 coding agent 中的每一個、內建的十四種 provider kind 預設值與它們各自的預設 URL／模型／金鑰環境變數，以及為什麼需要 Anthropic Messages 或 OpenAI Responses 端點。
keywords:
  - anthropic messages api
  - openai responses api
  - llm gateway
  - ollama claude code
  - provider presets
---

# Provider 相容性

這八個 coding agent 講的模型協定並不相同。`alc` 會在啟動前先驗證組合，而不是
靜靜送出一個不相容的請求。

| Agent | 支援端點 |
| --- | --- |
| Claude Code | Anthropic 相容端點 |
| Codex CLI | OpenAI Responses API |
| OpenCode | 任何 API 相容的 provider |
| Pi | Anthropic、OpenAI，或 OpenAI 相容端點 |
| Copilot CLI | OpenAI 或 Anthropic 相容端點 |
| Goose | OpenAI 或 Anthropic 相容端點 |
| Qwen Code | OpenAI、Anthropic，或 Gemini 相容端點 |
| Kimi Code CLI | OpenAI 或 Anthropic 相容端點 |

不論原生支援什麼，每個 agent 都能透過 [Codex 橋接](./codex-to-claude.md)搭配
一次 `codex login` 運作 —— `codex` 這個 provider kind 支援全部八個 agent。

## 差異從何而來

- Claude Code 的 gateway 必須提供 Anthropic Messages、Bedrock 或 Vertex API
  格式，由 `ANTHROPIC_BASE_URL` 指定使用哪一個 gateway。
- Codex CLI 自訂的 provider 使用 OpenAI Responses wire API。
- OpenRouter、Ollama，以及四個較新的預設值（DeepSeek、Moonshot、Z.ai、
  MiniMax）除了 OpenAI 格式的端點之外，也各自提供一個 Anthropic 相容端點，
  Claude Code 可以直接使用。
- OpenCode、Pi、Copilot CLI、Goose、Qwen Code、Kimi Code CLI 都能接受只有
  Chat Completions 的服務；只有 Claude Code 與 Codex CLI 需要更多。

## Provider kind 預設值

`alc config` 內建十四種 provider kind。選擇 `--kind` 會自動帶入預設端點、金鑰
環境變數與起始模型；每個值都只是 `config.toml` 裡的一般欄位，`alc config
upsert` 可以覆寫。

| Kind | 預設端點 | 金鑰環境變數 | 起始模型 |
| --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | `sonnet` |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | `gpt-5.6-terra` |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | `anthropic/claude-sonnet-4.6` |
| `codex` | —（原生 `codex login`） | — | —（見 [Codex 橋接](./codex-to-claude.md)） |
| `ollama` | `http://localhost:11434` | — | `qwen3-coder` |
| `vllm` | `http://localhost:8000/v1` | — | —（依部署而定；預設為停用狀態） |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | `deepseek-v4-pro` |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | `kimi-k3` |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | `glm-5.3` |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | `MiniMax-M3` |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | `llama-3.3-70b-versatile` |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | `grok-build-0.1` |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | `gemini-3.7-flash` |
| `custom` | —（自行提供） | —（透過 `--api-key-env` 自行命名） | —（自行提供） |

`deepseek`、`moonshot`、`zai`、`minimax` 都在主要的 OpenAI-chat 端點之外，各自
另外附帶*第二個* Anthropic 相容 base URL —— 這就是它們不用額外設定就
「Claude 可用」的原因：

| Kind | Anthropic 相容 URL |
| --- | --- |
| `deepseek` | `https://api.deepseek.com/anthropic` |
| `moonshot` | `https://api.moonshot.ai/anthropic` |
| `zai` | `https://api.z.ai/api/anthropic` |
| `minimax` | `https://api.minimax.io/anthropic` |

這些預設值只是起始值，不是永久不變的：上游 model ID 改變的速度比 alc 發版
還快，所以請把上面每一個「起始模型」都當成一個可以在 `alc config` 裡修改的
預設值，而不是 provider 目前實際提供內容的保證。

## 在本機 Ollama 模型上跑 Claude Code

`alc --ollama claude` 會把 Claude Code 指向 Ollama 伺服器的 Anthropic
Messages 端點。本機伺服器只提供已經 pull 下來的模型，而且一次只回答一個
請求，所以 alc 對這種工作階段的設定和雲端 provider 不同：

- `ANTHROPIC_DEFAULT_MODEL`、`ANTHROPIC_DEFAULT_SONNET_MODEL`、
  `ANTHROPIC_DEFAULT_OPUS_MODEL`、`ANTHROPIC_DEFAULT_HAIKU_MODEL` 與
  `ANTHROPIC_SMALL_FAST_MODEL` 全部指向 profile 的模型（有設定 `small_model`
  時，haiku 這一層改指向它），這樣 Claude Code 自己的別名、背景呼叫和
  `/model` 選單都不會向 Ollama 要求它沒有的 Claude model ID
  （`404 model not found`）。
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` 略過產生工作階段標題之類的
  附帶請求；否則它們會先佔住伺服器唯一的處理槽好幾分鐘，真正的請求才輪得到。
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS` 帶入 Ollama 回報的模型 context 長度
  （模型已載入時取 `/api/ps`，否則取 `/api/show`），讓自動壓縮依照實際的
  視窗，而不是 Claude Code 對未知 model ID 假設的 200k。伺服器沒開時會
  安靜地略過。
- 除非你自己已經設定，否則加上 `API_FORCE_IDLE_TIMEOUT=0` 與
  `API_TIMEOUT_MS=1800000`，讓 Claude Code 最多等三十分鐘才等到第一個
  token；對 Anthropic 以外的主機，它原本會在六分鐘後放棄請求並重來。

Claude Code 每個工作階段的第一個請求大約有 25k 到 40k tokens（系統提示、
工具 schema、專案內容），而筆電等級的模型每秒只能讀幾十個 token：在 M3
MacBook Air 上，`gemma4:12b` 讀完 22k tokens 的第一個請求要約六分鐘，39k
的要約十五分鐘，之後才吐出第一個 token。有了上面這兩個逾時變數，Claude Code
會一直等下去（舊版 alc 或直接執行 `claude` 時，每次嘗試六分鐘後就會被放棄；
重試會從 Ollama 的 prompt cache 接續，所以工作階段終究還是會開始）。但只有
提示夠小、模型讀得夠快，第一輪才會順暢。在筆電上這代表：

- 選擇 `ollama show <model>` 的 capabilities 列有 `tools` 的模型；不能呼叫
  工具的模型對 coding agent 毫無用處。
- 讓第一個請求維持精簡：每個 MCP server、plugin 和 skill 都會把工具 schema
  加進去，讀取時間隨長度增加 —— 對 Gemma 4 這類模型甚至比線性更快，
  因為它的全注意力層越深入提示就越慢。
- 給模型 64k 到 128k 的 context（Ollama 設定或 `OLLAMA_CONTEXT_LENGTH`）：
  Claude Code 至少需要 64k，而在 24 GB 的 Mac 上開 256k 視窗只是白白預留
  好幾 GB 的 KV cache。Flash attention 預設就已開啟；`OLLAMA_KV_CACHE_TYPE=q8_0`
  可以再把剩下的記憶體用量減半。
- 讓模型保持載入（`OLLAMA_KEEP_ALIVE=4h` 或 `-1`）：Ollama 在閒置五分鐘後
  卸載模型時，prompt cache 也跟著消失，下一輪就得重新讀完整段對話。
- 工作階段進行中，不要同時 pull 或執行其他模型。

`alc doctor` 會印出 **Ollama** 區塊：伺服器版本、模型是否已 pull、能否呼叫
工具，以及它實際拿到的 context 長度。

## alc 認得的協定

每個 provider profile 都會宣告協定，決定 alc 允許哪些組合：

| 協定 | 意義 |
| --- | --- |
| `anthropic-messages` | Anthropic Messages API |
| `openai-responses` | OpenAI Responses API |
| `openai-chat` | 只支援 Chat Completions |
| `codex-native` | Codex CLI 登入，透過內建的橋接使用 |
| `dual` | 同時提供 Anthropic Messages 與 OpenAI Responses |

執行 `alc doctor` 可以印出你自己設定的相容性矩陣 —— 每個 provider profile
對照全部八個 agent。
