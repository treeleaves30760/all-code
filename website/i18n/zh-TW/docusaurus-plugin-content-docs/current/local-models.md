---
id: local-models
title: 本機模型
sidebar_label: 本機模型
sidebar_position: 4
description: 用本機的 Ollama 模型跑 Claude Code —— alc 會替它設定什麼，以及在筆電上怎麼縮短第一輪的等待。
keywords:
  - ollama claude code
  - local llm coding agent
  - gemma
  - qwen3-coder
  - 中文
---

# 本機模型

`alc --ollama claude` 會把 Claude Code 指向 Ollama 伺服器的 Anthropic
Messages 端點，並依照「一次只回答一個請求」的伺服器來設定這個 session。

```sh
alc --ollama claude
alc doctor          # the Ollama section: server, model pulled, tool calling, context
```

## alc 會設定什麼

- 每一個模型別名 —— `ANTHROPIC_DEFAULT_MODEL`、sonnet／opus／haiku 各層、
  `ANTHROPIC_SMALL_FAST_MODEL` —— 全部釘在 profile 的模型上（有設定
  `small_model` 時，haiku 那一層改指向它），這樣 Claude Code 就不會向 Ollama
  要求它根本沒有的 Claude model ID。
- `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`，讓附帶的請求不會在真正的請求
  開始之前，先佔住伺服器唯一的處理槽。
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS` 取自 Ollama 回報的視窗（模型已載入時看
  `/api/ps`，否則看 `/api/show`），讓自動壓縮跟著實際的視窗走。伺服器沒開
  時會略過。
- 除非你自己設定過，否則補上 `API_FORCE_IDLE_TIMEOUT=0` 與
  `API_TIMEOUT_MS=1800000`，讓 Claude Code 最多等三十分鐘才等到第一個 token，
  而不是六分鐘後就放棄這個請求。

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
