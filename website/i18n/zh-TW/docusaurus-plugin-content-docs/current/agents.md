---
id: agents
title: 支援的 agent
sidebar_label: 支援的 agent
sidebar_position: 6
description: alc 啟動的八個 coding agent，每一個各自會被注入什麼 —— 逐一列出實際的環境變數、旗標與設定檔。
keywords:
  - claude code
  - codex cli
  - opencode
  - pi coding agent
  - github copilot cli
  - goose
  - qwen code
  - kimi code cli
---

# 支援的 agent

alc 會啟動八個 coding agent。每一個對「如何設定 provider」都有自己的一套做法
—— 環境變數、CLI 旗標，或是一份設定檔 —— 所以這一頁會逐一列出，alc 實際上會
設定什麼，好讓你選的 provider 能正常運作。每個 agent 也都能透過單一一次
`codex login`，經由 [Codex 橋接](./codex-to-claude.md)運作；橋接本身的運作
方式請見該頁。

## Claude Code

- 執行檔：`claude` —— [安裝說明](https://code.claude.com/docs/en/setup)
- 支援端點：Anthropic 相容端點
- alc 會注入：`ANTHROPIC_BASE_URL`、`ANTHROPIC_MODEL`、`ANTHROPIC_API_KEY`
  （像 OpenRouter 這種 bearer 型 provider 則改為 `ANTHROPIC_AUTH_TOKEN`），
  以及在 profile 有設定 small model 時的 `ANTHROPIC_SMALL_FAST_MODEL`
- 透過橋接時，Claude Code 是唯一能在工作階段中切換的 agent：每個 GPT 模型
  都會出現在它自己的 `/model` 選單裡，`/model`／`/effort` 可以直接變更正在
  執行的工作階段 —— 完整說明請見 [Codex 橋接](./codex-to-claude.md)。

```sh
alc claude
alc --openrouter claude
alc --codex claude --model gpt-5.6-terra --effort medium
```

## Codex CLI

- 執行檔：`codex` —— [安裝說明](https://learn.chatgpt.com/docs/codex/cli)
- 支援端點：OpenAI Responses API
- alc 會注入：`--model`，以及在有設定時的 `--config
  model_reasoning_effort=<level>`。非 Codex 的 provider 還會得到完整的
  `model_providers.<id>.*` 覆寫（`base_url`、`wire_api=responses`、
  `requires_openai_auth=false`），加上 —— 只在該 profile 真的需要 key 時
  才會有的 —— `env_key` 與帶著它的 `ALC_PROVIDER_API_KEY` 環境變數；Ollama
  profile 則改為得到 `--oss --local-provider ollama --model <model>`。
- Codex CLI 是唯一完全不會經過橋接的 agent：`codex` kind 的 profile 會直接用
  你原生的 `codex login` 執行 `codex`。

```sh
alc codex
alc --openrouter codex
```

## OpenCode

- 執行檔：`opencode` —— [安裝說明](https://opencode.ai/docs)
- 支援端點：任何 API 相容的 provider
- alc 會注入一個行內的 `OPENCODE_CONFIG_CONTENT` JSON 環境變數（不寫任何
  檔案），把模型命名為 `<provider-id>/<model>`。對 Anthropic、OpenAI、
  OpenRouter、Ollama 這幾個 profile 來說，provider id 就是 kind 本身的名稱
  （`anthropic`、`openai`、`openrouter`、`ollama`）；其他所有 kind ——
  vLLM、Custom，以及七個新的 provider kind 預設值 —— 則一律改用
  `alc-<profile>`。
- 同一份 JSON 裡也會寫入完整的 `provider.<id>` 物件（npm 套件、`name`、
  `options.baseURL`、`models`）：Ollama、vLLM、Custom 與七個新預設值一律
  會寫；Anthropic／OpenAI／OpenRouter 的 profile 則只有在 base URL 被改成
  不是該 kind 自己的預設值時才會寫。`options.apiKey:
  "{env:ALC_PROVIDER_API_KEY}"` 只在該 profile 真的需要 key 時才會加上 ——
  預設（不需要 key）的 Ollama profile 就不會有 `apiKey` 欄位。
- 透過橋接時，同一套機制會定義一個指向 loopback 轉接器的 `alc-codex`
  provider。

```sh
alc opencode
alc --zai opencode
alc --codex opencode
```

## Pi

- 執行檔：`pi` —— [安裝說明](https://github.com/earendil-works/pi)
  （`npm install -g @earendil-works/pi-coding-agent`）
- 支援端點：Anthropic、OpenAI，或 OpenAI 相容端點
- alc 會注入：合併進 `$PI_CODING_AGENT_DIR/models.json`（預設為
  `~/.pi/agent/models.json`）的一筆 `alc-<profile>` 項目，加上
  `--provider`、`--model`，以及在有設定 effort 時的 `--thinking` 旗標。
- **合併是新增式的。** alc 只會寫入名稱為 `alc-*` 的 key，所以你自己加入
  的 provider 完全不會被動到。寫入是原子性的；如果你原本的 `models.json`
  解析失敗，alc 會直接拒絕動作 —— 而不是用一份全新的檔案把它蓋掉。
- **Anthropic 訂閱的特例：** 沒有存 API key 的 `anthropic` kind profile
  會完全跳過 `models.json` 的寫入，改成直接以 `--provider anthropic
  --model <model>` 啟動，讓 Pi 改用它自己的 `/login`（訂閱）憑證，而不是
  一筆沒有東西會用到的 `models.json` 項目。

```sh
alc pi
alc --minimax pi
alc --codex pi
```

## Copilot CLI

- 執行檔：`copilot` —— [安裝說明](https://docs.github.com/en/copilot/how-tos/copilot-cli)
- 支援端點：OpenAI 或 Anthropic 相容端點
- alc 會注入：`COPILOT_PROVIDER_TYPE`（`anthropic` 或 `openai`）、
  `COPILOT_PROVIDER_BASE_URL`、`COPILOT_PROVIDER_API_KEY`（像 Ollama 這種
  不需要 key 的 provider 會略過），以及 `COPILOT_MODEL`。
- 這是純粹的 BYOK 模式 —— 不會寫任何設定檔，也不需要 GitHub Copilot 登入。

```sh
alc copilot
alc --deepseek copilot
alc --codex copilot
```

## Goose

- 執行檔：`goose` —— [安裝說明](https://block.github.io/goose/)
- 支援端點：OpenAI 或 Anthropic 相容端點
- alc 會注入 `GOOSE_PROVIDER`，加上對應的 BYOK 變數：OpenRouter 用
  `OPENROUTER_API_KEY`、Ollama 用 `OLLAMA_HOST`、Anthropic 型 provider 用
  `ANTHROPIC_API_KEY`（只有在跟 goose 自己的預設值
  `https://api.anthropic.com` 不同時才會加上 `ANTHROPIC_HOST`），其餘則
  用 `OPENAI_API_KEY`／`OPENAI_HOST`／`OPENAI_BASE_PATH` —— 再加上
  `GOOSE_MODEL`，以及有設定時的 `GOOSE_FAST_MODEL`。
- **預設是 `session`：** 如果你自己沒有加參數，alc 會補上 goose 互動式的
  `session` 子指令；只要你自己帶了參數（`alc goose run ...`），就會照
  原樣轉發，不會再補 `session`。

```sh
alc goose
alc --groq goose
alc --codex goose
```

## Qwen Code

- 執行檔：`qwen` —— [安裝說明](https://github.com/QwenLM/qwen-code)
- 支援端點：OpenAI、Anthropic，或 Gemini 相容端點
- alc 會注入 `--auth-type <anthropic|openai|gemini>` 與 `--model`，加上
  對應的環境變數：Anthropic 型 provider 用
  `ANTHROPIC_BASE_URL`／`ANTHROPIC_API_KEY`，`google` kind 用
  `GEMINI_API_KEY`，其餘則用 `OPENAI_BASE_URL`／`OPENAI_API_KEY`。

```sh
alc qwen
alc --xai qwen
alc --codex qwen
```

## Kimi Code CLI

- 執行檔：`kimi` —— [安裝說明](https://github.com/MoonshotAI/kimi-cli)
- 支援端點：OpenAI 或 Anthropic 相容端點
- alc 除了 `--config-file` 之外，不會注入任何環境變數或旗標。它會讀取你
  現有的設定（`~/.kimi/config.toml`，或是有設定時 `ALC_KIMI_CONFIG`
  指定的路徑，如果存在的話），合併進 `providers.alc-<profile>`（型別依
  provider kind 而定，可能是 `anthropic`、`openai_responses` 或
  `openai_legacy`）、`models.alc-<profile>`，以及
  `default_model = "alc-<profile>"`，再把**合併後**的結果寫進一份全新的
  暫存檔（Unix 上權限為 `0600`）。
- **你原本的設定檔不會被寫入。** 這份暫存檔會以 `--config-file <path>`
  傳入，等 Kimi 結束後就會刪除，所以真正的 API key 只會在那一個行程存活
  期間留在磁碟上。
- 只要你自己帶了 `--config-file`（或 `--config`），就會停用上述所有行為
  —— alc 會直接照原樣轉發你的參數。

```sh
alc kimi
alc --moonshot kimi
alc --codex kimi
```

## 執行檔覆寫

每個 agent 的執行檔都可以指定成特定路徑，而不是從 `PATH` 解析：

| Agent | 覆寫用環境變數 |
| --- | --- |
| Claude Code | `ALC_CLAUDE_BIN` |
| Codex CLI | `ALC_CODEX_BIN` |
| OpenCode | `ALC_OPENCODE_BIN` |
| Pi | `ALC_PI_BIN` |
| Copilot CLI | `ALC_COPILOT_BIN` |
| Goose | `ALC_GOOSE_BIN` |
| Qwen Code | `ALC_QWEN_BIN` |
| Kimi Code CLI | `ALC_KIMI_BIN` |

每個 provider kind 各自支援什麼協定，請見
[Provider 相容性](./providers.md)；`alc --codex <agent>` 底層如何運作，請見
[Codex 橋接](./codex-to-claude.md)。
