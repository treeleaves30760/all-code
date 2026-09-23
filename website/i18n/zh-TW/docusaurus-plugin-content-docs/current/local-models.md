---
id: local-models
title: 本機模型
sidebar_label: 本機模型
sidebar_position: 4
description: 用本機的 Ollama、llama.cpp 或 vLLM 模型跑 Claude Code —— alc 會替它設定什麼，以及怎麼縮短第一輪的等待。
keywords:
  - ollama claude code
  - llama.cpp claude code
  - vllm claude code
  - local llm coding agent
  - gemma
  - qwen3-coder
  - 中文
---

# 本機模型

`alc --ollama claude`、`alc --llamacpp claude` 和 `alc --vllm claude` 會把
Claude Code 指向本機伺服器的 Anthropic Messages 端點 —— 它的根路徑，和 `/v1`
底下的 OpenAI 路由並列 —— 並依照「只提供一個模型、一次只回答一個（或少數幾個）
請求」的伺服器來設定這個 session。

```sh
alc --ollama claude
alc --llamacpp claude
alc doctor          # server, model, context, and for llama.cpp and vLLM /v1/messages
```

## alc 會設定什麼

- 每一個模型別名 —— `ANTHROPIC_DEFAULT_MODEL`、sonnet／opus／haiku 各層、
  `ANTHROPIC_SMALL_FAST_MODEL` —— 全部釘在 profile 的模型上（有設定
  `small_model` 時，haiku 那一層改指向它），這樣 Claude Code 就不會向伺服器
  要求它根本沒有的 Claude model ID。
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`，讓附帶的請求不會在真正的請求
  開始之前，先佔住伺服器的處理槽。
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS` 取自伺服器回報的視窗，讓自動壓縮跟著實際的
  視窗走：Ollama 在模型已載入時看 `/api/ps`，否則看 `/api/show`；llama.cpp 看
  `/props` 裡每個 slot 的 `n_ctx`；vLLM 看 `max_model_len`。伺服器沒開時會略過。
- 除非你自己設定過，否則補上 `API_FORCE_IDLE_TIMEOUT=0`、
  `API_TIMEOUT_MS=1800000` 與 `CLAUDE_STREAM_IDLE_TIMEOUT_MS=1800000`，讓
  Claude Code 最多等三十分鐘才等到第一個 token，而不是五、六分鐘後就放棄這個
  請求。
- llama.cpp 與 vLLM 另外補上
  `CLAUDE_CODE_MODEL_CAPABILITIES=-mid_conv_system,-mid_conv_tool_change`；原因見下文。

## 讓第一輪短一點

Claude Code 開場的第一個請求就有 25k 到 40k tokens，而筆電等級的模型每秒只讀
得了幾十個 token —— 第一個 token 出現前要先等上好幾分鐘。有幫助的做法：

- 選擇 `ollama show <model>` 列有 `tools` 的模型。
- 少開 MCP server、plugin 和 skill；每一個都會把工具 schema 加進第一個請求。
- 把 context 設在 64k 到 128k（`OLLAMA_CONTEXT_LENGTH`）：Claude Code 至少
  需要 64k，而在 24 GB 的 Mac 上開 256k 視窗，只是白白預留一堆用不到的 KV
  cache。`OLLAMA_KV_CACHE_TYPE=q8_0` 可以再把剩下的用量減半。
- `OLLAMA_KEEP_ALIVE=4h`（或 `-1`），讓 prompt cache 撐過閒置的那幾分鐘。
- session 進行中不要 `ollama pull`，也不要再跑第二個模型。

## llama.cpp 與 vLLM

```sh
llama-server -m Qwen3.8-27B-UD-Q4_K_XL.gguf --alias qwen3.8-27b --jinja -c 131072 --api-key "$LLAMA_API_KEY"

alc config upsert box --kind llamacpp --base-url http://127.0.0.1:8080/v1 --model qwen3.8-27b
printf '%s' "$LLAMA_API_KEY" | alc config key box --stdin
alc -p box claude
```

profile 保留以 `/v1` 結尾的 OpenAI 風格 URL，其他每個 agent 都用它；Claude
Code 拿到的則是根路徑。`--model` 填伺服器在 `/v1/models` 列出的名稱 ——
llama-server 的 `--alias`、vLLM 的 `--served-model-name`。替 profile 存的 key，
或 llama.cpp profile 的 `LLAMA_API_KEY`，會送給每一個 agent；Claude Code 則透過
`apiKeyHelper` 索取，不會從檔案讀。在同一個 profile 上，OpenCode 走 llama-server
的 Chat Completions 路由，Codex 走它的 Responses 路由。

`vllm` profile 的用法相同。在 alc 有這個 kind 之前、有人指向 llama-server 的
`vllm` profile 也一樣：不論它替其他 agent 指定哪一種 OpenAI 協定，Claude Code
都能在上面跑。

### 為什麼多了兩個設定

**chat template。** 這兩種伺服器都用模型自己的 Jinja chat template 來組
prompt，而不少 template —— Qwen 的就是 —— 只接受出現在最前面的 system 訊息。
Claude Code 碰到它不認得的模型，正好會送出這種訊息：把它的環境資訊當成第一則
user 訊息之後的一則 `role: "system"` 訊息。llama.cpp 回
`500 System message must be at the beginning`，而 Claude Code 不會把這當成可以
關掉的能力，於是重送同一個請求直到放棄。alc 設定
`CLAUDE_CODE_MODEL_CAPABILITIES=-mid_conv_system,-mid_conv_tool_change`，那段
內容就改以 `<system-reminder>` 放進第一則 user 訊息。

**讀 prompt 時的沉默。** llama-server 一收到請求就回 header，然後在讀完整個
prompt 之前什麼都不送：27B 模型讀 11k tokens 要半分鐘，200k 要一刻鐘。不論
`API_FORCE_IDLE_TIMEOUT` 怎麼設，Claude Code 的串流 watchdog 在任何
`ANTHROPIC_BASE_URL` 上都會在五分鐘後結束這段沉默，重試時又得把 prompt 重讀
一次。`CLAUDE_STREAM_IDLE_TIMEOUT_MS` 會拉到 watchdog 接受的上限三十分鐘。

### 檢查伺服器

`alc doctor` 會替每個啟用中的這兩種 profile 印出 **llama.cpp and vLLM** 區塊：

```text
llama.cpp and vLLM
  Claude Code needs the server's /v1/messages; the context shown is what one request gets
  ✓  box          http://127.0.0.1:8080  llama.cpp b11100-7ab4ee7ba
  ✓  qwen3.8-27b  served  131072 tokens of context  Anthropic Messages
```

它會指出：沒有東西回應、伺服器要 key 或拒絕了存下的 key、模型不在伺服器列出的
名單裡、單一請求能用的 context 不到 64k，以及沒有 `/v1/messages` —— Claude Code
需要它，其他 agent 不需要。這個路由是用空的 body 去問，伺服器在任何模型讀進一個
token 之前就會拒絕，所以這項檢查不會佔用共用伺服器的資源。
