---
id: agents
title: Agent
sidebar_label: Agent
sidebar_position: 6
description: alc 為八個 coding agent 各自設定了什麼 —— 環境變數、旗標，或一份合併過的設定檔 —— 好讓你選的 provider 不必手動改任何東西就能用。
keywords:
  - claude code
  - codex cli
  - opencode
  - pi coding agent
  - copilot cli
  - goose
  - qwen code
  - kimi code cli
  - 中文
---

# Agent

每個 agent 對「provider 該怎麼設定」都有自己的一套想法，所以 alc 會把同一份
provider 清單，翻譯成你正要啟動的那個 agent 聽得懂的樣子。這一頁就是它逐一
設定了什麼。

每個 agent 也都能靠一次 `codex login`，透過 [Codex
橋接](./codex-to-claude.md)運作；`alc --codex <agent>` 從頭到尾都是同一個
指令，所以下面不再重複。

## Claude Code

`claude` —— [安裝](https://code.claude.com/docs/en/setup) · 支援 Anthropic
相容端點。

Claude Code 拿到的是一份設定檔（`--settings`），裡面放的是端點、模型相關變數
與選單，憑證則透過 `apiKeyHelper` 向 `alc claude-credential` 取得；見[背景
session](./background-sessions.md)。Ollama profile 拿到的還更多 ——
見[本機模型](./local-models.md)。

```sh
alc claude
alc --openrouter claude
```

## Codex CLI

`codex` —— [安裝](https://learn.chatgpt.com/docs/codex/cli) · 支援 OpenAI
Responses API。

alc 會設定 `--model`，以及有設定時的 `--config
model_reasoning_effort=<level>`。非 Codex 的 provider 還會拿到一整組
`model_providers.<id>.*` 覆寫（`base_url`、`wire_api=responses`、
`requires_openai_auth=false`），需要金鑰時再加上 `env_key` 與
`ALC_PROVIDER_API_KEY`。Ollama profile 則改成 `--oss --local-provider
ollama`。

Codex CLI 是唯一完全不經過橋接的 agent：`codex` kind 的 profile 直接用你原生
的登入執行它。

```sh
alc codex
alc --openrouter codex
```

## OpenCode

`opencode` —— [安裝](https://opencode.ai/docs) · 支援任何 API 相容的
provider。

alc 會設定一個行內的 `OPENCODE_CONFIG_CONTENT` JSON 變數，不寫任何檔案，並把
模型命名為 `<provider-id>/<model>`。Anthropic、OpenAI、OpenRouter 與 Ollama
的 profile，provider id 就是那個 kind 的名稱；其餘每一種 kind 則是
`alc-<profile>`。

同一份 JSON 裡也會放進完整的 `provider.<id>` 物件：Ollama、vLLM、custom 與
比較新的那幾個預設值一律會放；前面那四種則只有在 base URL 被指到該 kind 預設
值以外的地方時才放。`options.apiKey` 只在 profile 需要金鑰時才出現，所以預設
的 Ollama profile 不會有。

```sh
alc opencode
alc --zai opencode
```

## Pi

`pi` —— [安裝](https://github.com/earendil-works/pi)（`npm install -g
@earendil-works/pi-coding-agent`）· 支援 Anthropic、OpenAI，或 OpenAI 相容
端點。

alc 會把一筆 `alc-<profile>` 項目合併進
`$PI_CODING_AGENT_DIR/models.json`（預設為 `~/.pi/agent/models.json`），並
傳入 `--provider`、`--model`，以及有設定 effort 時的 `--thinking`。

合併是新增式的：alc 只會寫入名稱為 `alc-*` 的 key，寫入是原子性的，而一份
解析失敗的 `models.json` 會讓 alc 拒絕動作，而不是把它換掉。沒有存金鑰的
`anthropic` profile 會完全跳過合併，直接以 `--provider anthropic` 啟動，讓
Pi 使用它自己的訂閱登入。

```sh
alc pi
alc --minimax pi
```

## Copilot CLI

`copilot` —— [安裝](https://docs.github.com/en/copilot/how-tos/copilot-cli)
· 支援 OpenAI 或 Anthropic 相容端點。

alc 會設定 `COPILOT_PROVIDER_TYPE`、`COPILOT_PROVIDER_BASE_URL`、
`COPILOT_PROVIDER_API_KEY`（不需要金鑰的 provider 會略過）與
`COPILOT_MODEL`。純 BYOK：不寫任何檔案，也不需要 GitHub Copilot 登入。

```sh
alc copilot
alc --deepseek copilot
```

## Goose

`goose` —— [安裝](https://block.github.io/goose/) · 支援 OpenAI 或 Anthropic
相容端點。

alc 會設定 `GOOSE_PROVIDER`、`GOOSE_MODEL`、選用的 `GOOSE_FAST_MODEL`，以及
該 provider 需要的 BYOK 變數：`OPENROUTER_API_KEY`、`OLLAMA_HOST`、
`ANTHROPIC_API_KEY`（與 goose 自己的預設值不同時再加上 `ANTHROPIC_HOST`），
或是整組 `OPENAI_*`。

你自己沒有帶參數時，alc 會補上 goose 互動式的 `session` 子指令；帶了參數就
原樣轉發。

```sh
alc goose
alc --groq goose
```

## Qwen Code

`qwen` —— [安裝](https://github.com/QwenLM/qwen-code) · 支援 OpenAI、
Anthropic，或 Gemini 相容端點。

alc 會設定 `--auth-type <anthropic|openai|gemini>` 與 `--model`，加上對應的
環境變數：`ANTHROPIC_*`、`google` kind 的 `GEMINI_API_KEY`，或 `OPENAI_*`。

```sh
alc qwen
alc --xai qwen
```

## Kimi Code CLI

`kimi` —— [安裝](https://github.com/MoonshotAI/kimi-cli) · 支援 OpenAI 或
Anthropic 相容端點。

alc 會讀取你現有的設定（`~/.kimi/config.toml`，或 `ALC_KIMI_CONFIG`），合併進
`providers.alc-<profile>`、`models.alc-<profile>` 與 `default_model`，再把
合併後的結果寫進一份新的暫存檔（權限 0600），以 `--config-file` 傳入。你真正
的設定檔完全不會被寫入，暫存檔則在 Kimi 結束時刪除，所以金鑰只在那個行程活著
的期間落在磁碟上。自己傳 `--config-file` 就會停用以上全部行為。

```sh
alc kimi
alc --moonshot kimi
```

## 執行檔覆寫

把任何一個 agent 指向特定的執行檔，而不是從 `PATH` 解析：

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
