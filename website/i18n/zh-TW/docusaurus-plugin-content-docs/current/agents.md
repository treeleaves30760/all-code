---
id: agents
title: Agent
sidebar_label: Agent
sidebar_position: 6
description: alc 為九個 coding agent 各自設定了什麼 —— 環境變數、旗標，或一份合併過的設定檔 —— 好讓你選的 provider 不必手動改任何東西就能用。
keywords:
  - claude code
  - codex cli
  - opencode
  - pi coding agent
  - copilot cli
  - goose
  - qwen code
  - kimi code cli
  - antigravity cli
  - 中文
---

# Agent

每個 agent 對「provider 該怎麼設定」都有自己的一套想法，所以 alc 會把同一份
provider 清單，翻譯成你正要啟動的那個 agent 聽得懂的樣子。這一頁就是它逐一
設定了什麼。

每個 agent 也都能靠一次 `codex login`，透過 [Codex
橋接](./codex-to-claude.md)運作；`alc --codex <agent>` 從頭到尾都是同一個
指令，所以下面不再重複。[Antigravity CLI](#antigravity-cli-agy) 在那裡的運作
方式不太一樣：它保留自己的 Google 登入，再在自己的模型旁多出 Codex 的模型（見
[在 Codex 登入上](#在-codex-登入上)）。那個 Google 登入反過來也能給其他每個
agent 使用（`alc --agy <agent>`）。

## Claude Code

`claude` —— [安裝](https://code.claude.com/docs/en/setup) · 支援 Anthropic
相容端點；其他 API 透過 alc 的橋接（見[任何 provider 接任何
agent](./providers.md#任何-provider-接任何-agent)）。

Claude Code 拿到的是一份設定檔（`--settings`），裡面放端點、模型相關變數與
選單，憑證透過 `apiKeyHelper` 向雜湊釘住的 alc 執行檔取得，帶有
`--runtime <id> claude-credential`。舊的未指定 scope 的 helper 仍走 legacy。
用 Claude Code 自己的登入時，由那個登入來回答，檔案裡只放端點與模型。見[背景
session](./background-sessions.md)。Ollama profile 拿到的還更多 ——
見[本機模型](./local-models.md)。

```sh
alc claude
alc --openrouter claude
```

## Codex CLI

`codex` —— [安裝](https://learn.chatgpt.com/docs/codex/cli) · 支援 OpenAI
Responses API；只有 Chat Completions 或只有 Anthropic 的 provider 透過 alc 的橋接，
設定為 `model_providers.alc_bridge`。

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

同一份 JSON 裡也會放進完整的 `provider.<id>` 物件：Ollama、vLLM、llama.cpp、custom 與
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

## Antigravity CLI (agy)

`agy` —— Google 的 Antigravity CLI · 跑在它自己的 Google 登入上，那個登入也能
給其他每個 agent 使用；它也能透過 alc 的橋接跑在 Codex 登入或任何 API provider 上。

agy 自己登入 Google，所以 alc 沒有端點、金鑰或設定檔要交給它，也完全不碰 agy
的任何憑證。它跑在初始設定內附的 `antigravity` profile 上，alc 只依那個 profile
設定 `--model`，以及 profile 有指定時的 `--effort`。agy 的強度是 `low`、
`medium`、`high`、`xhigh` 與 `max`；設成 `ultra` 的 profile 會以 `max` 啟動，並
印出一行說明。你在 `alc agy` 後面自己寫的 `--model` 或 `--effort` 會原封不動
轉送，alc 這時就不再送自己的那一個。agy 只列出 wire id，所以 profile 上的家族
id（例如 `claude-opus-5-5`，其他 agent 的橋接也讀得懂）會依 profile 的強度換成
對應的 wire id 再交給 agy。

```sh
alc agy
alc agy --model gemini-3.8-flash-low
alc models --agy
```

起始模型是 `claude-opus-5-5-high`，沒有設定強度，所以 agy 用它自己的預設值。
`alc models --agy` 會列出 `agy models` 回報的模型，保留一天（`--refresh` 會重新
問一次），問不到 agy 時就用內建的清單，並把沒有任何模型家族提供的 id 標上
` (not bridgeable)`：agy 也許能跑，其他 agent 不行。`alc config` 裡 antigravity
profile 的 Model 那一列也是在同一份清單裡切換。舊版 alc 寫出的 `config.toml` 沒有
`antigravity` profile，這時 `alc agy` 會給出修法：
`alc config upsert antigravity --kind antigravity`。

同一個登入也能透過 alc 的橋接給其他每個 agent 使用：`alc --agy claude`、
`alc --agy opencode` 等等。alc 借用了什麼、agy 仍管什麼，見
[Provider](./providers.md#antigravity)。

### 在 Codex 登入上

`alc --codex agy` 會啟動 alc 的橋接，並把 agy 的 `CLOUD_CODE_URL` 指向它。橋接
會把 Codex 的模型加進 agy 自己的模型清單，每個推理強度一筆
（`gpt-6.1-sol-high`）：agy 的 `/model` 選單會顯示成 `GPT-6.1-Sol · Codex`，
搭配 agy 自己從 `low` 到 `max` 的強度滑桿，Codex 的 `ultra` 則以 `max` 執行。
Codex 模型上的每一輪都送到你的 `codex login`。agy 問的其他所有東西 —— 它自己的
Gemini、Claude 與 GPT-OSS 模型、登入檢查、額度，以及標題、checkpoint 這類輔助
請求 —— 都原封不動轉送給 Google，用的是 agy 自己送出的 token；alc 不讀取、也
不保留任何 agy 憑證。所以 agy 仍然需要它自己的 Google 登入，而 `/model` 可以在
session 中途於 Codex 與 Google 的模型之間切換。

```sh
alc --codex agy                                   # 用 codex profile 的模型與強度開始
alc --codex agy --model gemini-3.8-flash-low   # 你自己的 --model 優先
```

session 一開始用 codex profile 的模型與強度，以 `--model gpt-6.1-sol-<effort>`
傳入。Codex 模型上的每一輪會以 agent `agy`、codex profile 記錄下來，所以
[`alc usage` 與 `alc tps`](./usage.md) 都會算進去。其他有 API 的 provider 也以
同樣方式透過橋接的轉譯接上 agy（見[任何 provider 接任何 agent](./providers.md#任何-provider-接任何-agent)）：
`alc --openrouter agy` 會把 profile 的模型加進 agy 的選單，標上 profile 名稱。

## 直接 API 量測

`alc --metrics <agent>` 會在支援的、由 alc 管理的端點設定位置啟用觀測，保留原生
SDK、協定、模型、參數與**上游驗證**。持續 Claude 觀測在本機這一段使用密封
helper 憑證，不是原始服務商金鑰。不加旗標的一般直接啟動維持不變。Codex
協定轉譯橋接的流量本來就會自動觀測。原生 Codex CLI 登入流量不是橋接流量。

| Agent | 支援的直接觀測設定位置 |
| --- | --- |
| Claude Code | alc 的 API-key 設定／helper 與 Anthropic 端點；經驗證的持續 host 與 AEAD 密封、綁定 route／instance 的替代憑證支援背景重啟。原生登入、無金鑰端點，或被覆寫的 helper／端點／驗證設定會被拒絕。 |
| Codex CLI | alc 產生的 HTTP Responses provider 設定，須明確傳入 `--config model_providers.alc_<profile-normalized>.supports_websockets=false`（profile 連字號改成底線）；沒帶或為 true 就拒絕。WebSocket 不被觀測、不被強制停用；一般沒加 metrics 的啟動維持不變。原生登入與原生 Ollama 整合會被拒絕。 |
| OpenCode | 選取的 alc 管理 provider 的行內端點／設定，不是自訂 provider／config 覆寫。 |
| Copilot CLI | alc 的 Anthropic 或 OpenAI 相容 BYOK 環境。 |
| Qwen Code | alc 的 Anthropic/OpenAI 分支，不含 Google/Gemini 整合。 |
| Goose | alc 的 Anthropic/OpenAI 分支，不含原生 OpenRouter/Ollama 整合。OpenAI 分拆端點含 query／fragment 時，量測會被拒絕；一般啟動維持不變。 |
| Kimi Code CLI | alc 產生的暫存 provider 設定裡的端點，不是使用者的 config／model／provider 覆寫。 |
| Pi | 不支援直接啟動觀測：端點存在持續共用的 `models.json`，alc 不會讓它指向短命的觀測器。 |
| Antigravity CLI | 不支援：agy 跑在它自己的登入上。跑在那個登入上的其他 agent，以及 `alc --codex agy` 下 agy 在 Codex 模型上的請求，會由 alc 的橋接自動觀測。 |

原生 OAuth／登入，以及超出這些設定位置的明確端點／helper／config／provider
覆寫，不會被偷偷攔截；明確要求 `--metrics` 卻不安全或不支援時，會說明原因並
失敗。`--dry-run` 只描述計畫，不啟動 listener，也不寫入任何東西。Claude helper
用 runtime 裡只有擁有者可讀的 `bridge.observer-key` 驗證 host，只註冊摘要，
並密封本機憑證；host 只有在派送到固定端點時才還原上游金鑰／header。
Host 重啟後舊替代憑證收到 401，請重新執行 helper。`forward-observer-v2` 驗證
實際 host 能力，不是版本字串。新啟動用自己的世代 host，舊 host 繼續服務舊
session，不需要停止再重啟那些 session。

本機請求仍用 loopback HTTP，不是 TLS。需信任本機行程：Claude 替代憑證保護
原始金鑰，不保護遭劫持 port 上的明文請求內容。其他 agent 的短命 route 不保證
密封憑證。量測基準、成本與涵蓋限制，請見[用量](./usage.md)。

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
| Antigravity CLI | `ALC_AGY_BIN` |
