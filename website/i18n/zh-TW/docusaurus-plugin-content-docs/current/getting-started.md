---
id: getting-started
title: 快速上手
sidebar_label: 快速上手
sidebar_position: 2
description: 安裝 alc、用你的 Codex 登入跑 Claude Code、把任何 agent 指向別的 provider、傳遞參數、啟動前先預覽，以及更新。
keywords:
  - alc install
  - alc update
  - launch claude code
  - switch llm provider
  - 中文
---

# 快速上手

安裝、登入 Codex 一次、啟動。這一頁其餘的內容都是選用的。

## 安裝

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

安裝器會把 `alc` 放進 `~/.local/bin`（Windows 是
`%USERPROFILE%\.local\bin`），並在做得到的時候把那個目錄加進你的 User
PATH；做不到時，就把你該自己加上的那一行印出來。`ALC_INSTALL_DIR`
可以改成別的目錄；`ALC_NO_PATH_UPDATE=1` 則完全不動 PATH。

從原始碼建置需要 Rust 1.88 以上：`cargo build --release --locked`。Codex
橋接就編在執行檔裡，所以不必再裝別的東西。

## 第一次執行

```sh
codex login
alc --codex claude
```

沒有「先設定」這一步。起始設定已編譯進執行檔，`alc --codex claude`
直接在記憶體裡讀它。它只需要 `codex login` 寫下的 `auth.json`，以及 PATH
上的 `claude`；少了哪一個，它都會直接說是哪一個：

```text
error: Codex credentials were not found at ~/.codex/auth.json; run `codex login` and retry
error: 'claude' is not installed or not on PATH; install it first, then retry `alc claude`: cannot find binary path
```

Claude Code 會以 `gpt-5.6-terra` 搭配 `medium` 推理強度啟動 —— 或是你自己
`~/.codex/config.toml` 裡指定的那一組 —— 而 `/model` 選單裡會列出每一個 GPT
模型。模型、推理強度分級，以及另外七個 agent，都在 [Codex
橋接](./codex-to-claude.md)。

## 其他 provider

```sh
alc config                 # keys and per-agent defaults
alc claude                 # each agent on its configured default
alc --openrouter codex
alc --deepseek pi
alc --ollama claude
alc -p local-vllm opencode
```

`--provider`（`-p`）接受 profile 名稱；當某個 kind 只有一個 profile
時，也可以直接寫 kind。`--anthropic`、`--openai`、`--openrouter`、`--codex`、
`--ollama`、`--vllm`、`--deepseek`、`--moonshot`、`--zai`、`--minimax`、
`--groq`、`--xai`、`--google` 則是捷徑。金鑰可以存在本機，也可以從環境變數
讀取；環境變數優先。各個 kind 講的是什麼協定見 [Provider
相容性](./providers.md)，檔案放在哪裡見[設定](./configuration.md)。

## 參數傳遞

除了 Claude 專用、屬於 alc 的 `--model`、`--effort`、`--save` 之外，agent
名稱後面的參數都會原封不動傳給 agent：

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc claude -- --model sonnet      # `--` hands even those names to Claude
```

alc 自己的旗標 —— `--share`、`--no-share`、`--bind-lan`、`--name`、
`--permission`、`--tmux`、`-t` —— 要寫在 agent 名稱前面。寫在後面，alc
會直接停下來，而不是把它們交給 agent：

```text
error: `--share` is alc's own flag but it came after the agent's arguments, where it would be passed to claude instead; put it before the agent name, or use `alc share claude -- <args>`
```

## 預覽與檢查

```sh
alc --codex --dry-run claude   # the resolved command, secrets redacted; says when a launch would be refused
alc doctor                     # binaries, credentials, compatibility, defaults, bridge, remote state
```

`alc doctor` 只要發現問題就會以非零狀態碼結束，並逐項說明該怎麼修。錯誤訊息
本身收在[疑難排解](./troubleshooting.md)。

## 更新

```sh
alc update --check
alc update
```

`alc update` 會挑出符合這台機器作業系統與 CPU 的發行包，核對它的 SHA-256
檢查碼，再替換掉 `alc`。Linux 與 macOS 會立刻換好；Windows 則等目前執行中的
`alc.exe` 結束後才完成替換。`--force` 會重裝目前這個版本。已經在跑的 session
會繼續使用啟動當下的執行檔，請重開它們；hub 也一樣，等它底下的 session
都結束後再執行 `alc hub stop`。

## 解除安裝

把 `alc` 從安裝目錄刪掉。設定目錄的位置由 `alc config path` 告訴你；刪掉它
的同時，也會刪掉本機儲存的 API key。
