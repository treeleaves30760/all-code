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

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

### macOS

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

### Linux / WSL

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

安裝器會把 `alc` 放進 `~/.local/bin`（Windows 是
`%USERPROFILE%\.local\bin`），並在做得到的時候把那個目錄加進你的 User
PATH；做不到時，就印出你該自己加入的目錄。macOS／Linux 請重開終端機，
或 `source` 提示的設定檔；PowerShell 會同時更新 User PATH 與目前的 session。
`ALC_INSTALL_DIR` 可以指定別的目錄：macOS／Linux 不會自動將自訂目錄加入
PATH，Windows 則會。支援 Windows PowerShell 5.1 與 PowerShell 7，包含
64 位元 Windows 上執行的 32 位元 PowerShell。

### 選用的 tmux 補裝

下載 alc、通過 SHA-256 驗證並完成安裝後，安裝器會用 `tmux -V` 檢查是否為
**3.2 以上**。已有相容版本就不更動；否則會透過現有系統套件管理器嘗試
安裝或升級 tmux：

- **Windows：**WinGet 的 `arndawg.tmux-windows` 套件，限使用者範圍，不強制
  CPU 架構。自動流程會接受套件／來源同意並關閉互動提示。會跳過 psmux
  與非原生移植版；PATH 上第一個原生版若過舊或無法解析，仍會擋住後方新版。
- **macOS：**`brew install tmux`，已安裝則用 `brew upgrade tmux`；不會透過
  sudo 執行 Homebrew。
- **Linux / WSL：**使用第一個找到的 `apt-get`、`dnf`、`yum`、`pacman`、
  `zypper` 或 `apk`。非 root 使用者會先用 sudo 快取憑證；只有 controlling
  terminal 可用且 stdout 或 stderr 是終端機時，才會在該終端機要求密碼。
  套件操作使用非互動 sudo，不會讀取管線裡的腳本。pacman 不單獨更新索引，
  避免 partial upgrade。

**只有 `--tmux` 需要 tmux，一般 alc 與普通 `--share` 都不需要。**缺少管理器、
權限不足、套件／架構不支援、安裝失敗，或舊 PATH 項目遮住新版時，都只會
警告並提供手動指令，不會讓 alc 安裝失敗。不會自動安裝 Homebrew／WinGet、
從原始碼編譯、移除 psmux 或修改 tmux 設定。套件操作之後會再次檢查版本，
不會直接假設安裝成功。

PowerShell 會附加新增的 User／Machine PATH 項目，保留 session 專有的路徑。
若仍找不到 tmux，請重開終端機並檢查 `tmux -V` 與 `alc doctor`。
手動備援指令（依平台選一個）：

```powershell
winget install --id arndawg.tmux-windows --exact
```

```sh
brew install tmux                                      # macOS（已安裝則用 upgrade）
sudo apt-get update && sudo apt-get install -y tmux     # Debian / Ubuntu / WSL
sudo dnf install -y tmux                               # Fedora / RHEL（或 yum）
sudo pacman -S --needed tmux                           # Arch；請保持整個系統更新
sudo zypper install tmux                               # openSUSE
sudo apk add --upgrade tmux                            # Alpine
```

### 停用自動處理

`ALC_NO_TMUX_INSTALL=1` 跳過自動 tmux 補裝。`ALC_NO_PATH_UPDATE=1` 則獨立
停用 **alc 安裝器本身**的 PATH 修改與 session PATH 刷新；套件管理器變更的
PATH 請重開終端機讀取。WinGet 本身仍可能修改永久 PATH。若要同時避免
補裝依賴的副作用與安裝器修改 PATH，請**兩個都設**：

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | ALC_NO_TMUX_INSTALL=1 ALC_NO_PATH_UPDATE=1 sh
```

```powershell
$env:ALC_NO_TMUX_INSTALL = '1'
$env:ALC_NO_PATH_UPDATE = '1'
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
# 選用：清除覆寫，讓此 session 之後的安裝恢復預設行為。
Remove-Item Env:ALC_NO_TMUX_INSTALL, Env:ALC_NO_PATH_UPDATE
```

從原始碼建置需要 Rust 1.88 以上：`cargo build --release --locked`。Codex
橋接就編在執行檔裡，不需要額外安裝橋接元件。原始碼建置若要搭配 `--tmux`，
請另行安裝 tmux。

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

Claude Code 會以 `gpt-6-sol` 搭配 `medium` 推理強度啟動 —— 或是你自己
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
會直接停下來，而不是把它們交給 agent —— 除非你在 agent 名稱後面緊接著寫上
`--`，那就表示你指的是 agent 自己的旗標：

```text
error: `--share` is alc's own flag but it came after the agent's arguments, where it would be passed to claude instead; put it before the agent name, or put the agent's own flags after `--`, as in `alc claude -- <args>`
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
