# all-code (`alc`)

**一個 CLI，就能管好八個 coding agent。** 先設定好你的 LLM
provider，之後就能用任何一家 provider 啟動
[Claude Code](https://code.claude.com/docs/en/setup)、
[Codex CLI](https://learn.chatgpt.com/docs/codex/cli)、
[OpenCode](https://opencode.ai/docs)、
[Pi](https://github.com/earendil-works/pi)、
[Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli)、
[Goose](https://block.github.io/goose/)、
[Qwen Code](https://github.com/QwenLM/qwen-code)，或
[Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli) ——
包括讓其中任何一個都能使用你的 Codex／ChatGPT 訂閱。

[![CI](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml/badge.svg)](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/treeleaves30760/all-code?logo=github)](https://github.com/treeleaves30760/all-code/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)](#安裝)

📖 **[完整文件](https://treeleaves30760.github.io/all-code/zh-TW/)** ·
🇬🇧 **[English](https://treeleaves30760.github.io/all-code/)**

```text
alc config
alc update
alc claude
alc codex
alc opencode
alc pi
alc copilot
alc goose
alc qwen
alc kimi
alc --codex claude
alc --codex opencode
alc --codex claude --model gpt-5.6-terra --effort medium
alc --deepseek claude
alc --openrouter codex
alc --provider work opencode
```

## alc 能做什麼

- **每個 agent 各自選 provider。** 讓八個 agent 中的任何一個指向
  Anthropic、OpenAI API、OpenRouter、Ollama、vLLM、DeepSeek、Moonshot、
  Z.ai、MiniMax、Groq、xAI、Google，或任何自訂端點，也可以只為這次執行
  換掉，不用手改設定檔。
- **讓每個 agent 都能用 GPT 模型執行。** `alc --codex <agent>` 會把你的
  Codex／ChatGPT 登入橋接給你啟動的那個 agent。Claude Code 會把所有 GPT
  模型列進自己的 `/model` 選單，讓你在工作階段中隨時切換模型與推理強度；
  其他每個 agent 則是在啟動時就選定一個模型，整個工作階段都使用它。
- **啟動前先驗證相容性。** alc 會先確認 agent 與 provider 的模型協定相容，
  而不是送出一個注定失敗的請求。
- **憑證分開存放。** API key 放在獨立檔案或改用環境變數，不會在 agent
  之間被複製。

## 安裝

macOS、Linux、WSL：

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

安裝器會把 `alc` 與它的 Codex 橋接 helper 放進 `~/.local/bin`（Windows 為
`%USERPROFILE%\.local\bin`），必要時會把該目錄加入你的 User PATH。macOS／
Linux 請重開終端機，或 `source` 安裝器提示的設定檔；PowerShell 會同時更新
目前工作階段與 User PATH。如果系統不允許修改 PATH，安裝器會明確印出需要
手動加入的目錄。

Windows 安裝器已在 Windows PowerShell 5.1 與 PowerShell 7 上測試，包含 64
位元 Windows 上執行的 32 位元 PowerShell。

要安裝到其他目錄，請設定 `ALC_INSTALL_DIR`。自訂目錄不會被靜默加入
PATH；需要手動設定時安裝器會告訴你。設定 `ALC_NO_PATH_UPDATE=1` 可以明確
關閉自動修改 PATH。

## 更新

檢查是否有新版本，或直接更新 `alc` 與隨附的 helper：

```sh
alc update --check
alc update
```

`alc update` 會挑選符合目前作業系統與 CPU 的發行包、核對 GitHub Release
公布的 SHA-256、確認包內版本，再一起替換兩個執行檔。Linux 與 macOS 會
立即完成替換。Windows 會先完成下載與驗證，等目前的 `alc.exe` 結束後立刻
替換；稍候再用 `alc --version` 確認。`alc update --force` 可以重新安裝
目前的最新版本。

`alc` 只負責啟動已經安裝好的 coding agent，請自行安裝你要用的：

- [Claude Code](https://code.claude.com/docs/en/setup)
- [Codex CLI](https://learn.chatgpt.com/docs/codex/cli)
- [OpenCode](https://opencode.ai/docs)
- [Pi](https://github.com/earendil-works/pi)（`npm install -g @earendil-works/pi-coding-agent`）
- [Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli)
- [Goose](https://block.github.io/goose/)
- [Qwen Code](https://github.com/QwenLM/qwen-code)
- [Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli)

## 快速開始

開啟全螢幕設定介面：

```sh
alc config
```

初始設定內含 Anthropic、OpenAI、OpenRouter、Codex、Ollama，以及一個預設
停用的 vLLM 範本。你可以在 TUI 裡填入 API key，或讓 profile 指向某個環境
變數。環境變數的優先權高於本機儲存的 key。

接著用各 agent 設定好的預設值啟動：

```sh
alc claude
alc codex
alc opencode
alc pi
alc copilot
alc goose
alc qwen
alc kimi
```

只替這次工作階段換掉 provider：

```sh
alc --codex claude
alc --openrouter codex
alc --deepseek pi
alc --codex opencode
alc -p local-vllm opencode
```

`--provider`（或 `-p`）接受 profile 名稱；當某個 kind 只有一個 profile
時，也可以直接寫 kind。捷徑旗標 `--anthropic`、`--openai`、
`--openrouter`、`--codex`、`--ollama`、`--vllm`、`--deepseek`、
`--moonshot`、`--zai`、`--minimax`、`--groq`、`--xai`、`--google` 效果
相同。

`alc --codex claude` 會直接啟動 Claude Code，並把所有 GPT 模型列進 Claude
Code 自己的 `/model` 選單，讓你在工作階段中切換模型與推理強度。
`alc --codex <agent>` 則會把其他每個 agent 橋接到同一個模型，整個工作
階段都使用它。啟動預設值可以在 `alc config` 裡設定。

除了 Claude 專用的 `--model`、`--effort`、`--save` 之外，agent 名稱後的
參數會原封不動傳入：

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
alc goose run --name my-session
```

如果要把同名參數交給 Claude 本身，請放在 `--` 後面，例如
`alc claude -- --model sonnet`。

用 `--dry-run` 預覽實際會執行的轉接器指令，不會真的啟動；API key 會被
遮蔽：

```sh
alc --openrouter --dry-run claude
```

執行診斷：

```sh
alc doctor
```

`alc doctor` 會列出全部八個 agent 的執行檔狀態、憑證狀態、每個 provider
profile 各自的 agent 相容性欄位、解析後的預設值，以及（當設定了 Codex
provider 時）Codex 橋接的登入狀態。

## Provider 與 agent

這八個 agent 講的模型協定並不完全相同，十四種 provider kind 對外提供的
協定也不盡相同。`alc` 會在啟動前先驗證組合，而不是靜靜送出一個注定失敗的
請求。

### Provider 種類

| Kind | 預設端點 | 金鑰環境變數 | 支援協定 | Claude 可用？ |
| --- | --- | --- | --- | --- |
| `anthropic` | `https://api.anthropic.com` | `ANTHROPIC_API_KEY` | anthropic | 可 |
| `openai` | `https://api.openai.com/v1` | `OPENAI_API_KEY` | responses, chat | 否 |
| `openrouter` | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` | anthropic, responses, chat | 可 |
| `codex` | —（原生 `codex login`） | — | native | 可（橋接） |
| `ollama` | `http://localhost:11434` | — | anthropic, responses, chat | 可 |
| `vllm` | `http://localhost:8000/v1` | — | responses, chat | 否 |
| `deepseek` | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` | chat (+ anthropic) | 可 |
| `moonshot` | `https://api.moonshot.ai/v1` | `MOONSHOT_API_KEY` | chat (+ anthropic) | 可 |
| `zai` | `https://api.z.ai/api/paas/v4` | `ZAI_API_KEY` | chat (+ anthropic) | 可 |
| `minimax` | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` | chat (+ anthropic) | 可 |
| `groq` | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` | chat | 否 |
| `xai` | `https://api.x.ai/v1` | `XAI_API_KEY` | chat | 否 |
| `google` | `https://generativelanguage.googleapis.com/v1beta/openai` | `GEMINI_API_KEY` | chat | 否 |
| `custom` | 使用者自訂 | 使用者自訂（`--api-key-env`） | 可設定 | 否，除非另行設定 |

`deepseek`、`moonshot`、`zai`、`minimax` 都在主要的 OpenAI-chat 端點之外，
各自另外提供一個 Anthropic 相容的 base URL（見 `alc config show`）——
這正是這四個 kind 不需要額外設定就「Claude 可用」的原因。預設值只是
起始值：執行 `alc config show` 可以看到 profile 目前實際使用的 model
ID，等上游改名或棄用某個模型時，再用 `alc config upsert` 修改。

### Agent 需求

| Agent | 執行檔 | 支援端點 | alc 注入內容 | Codex 橋接 |
| --- | --- | --- | --- | --- |
| Claude Code | `claude` | Anthropic 相容端點 | env（`ANTHROPIC_BASE_URL`／`ANTHROPIC_MODEL`／`ANTHROPIC_API_KEY`） | 可（`/model` 選單） |
| Codex CLI | `codex` | OpenAI Responses API | 旗標 + `--config` 覆寫 | 可（原生登入） |
| OpenCode | `opencode` | 任何 API 相容的 provider | 行內 `OPENCODE_CONFIG_CONTENT` 環境變數 | 可 |
| Pi | `pi` | Anthropic、OpenAI，或 OpenAI 相容端點 | 合併進 `models.json` + 旗標 | 可 |
| Copilot CLI | `copilot` | OpenAI 或 Anthropic 相容端點 | `COPILOT_PROVIDER_*` 環境變數 | 可 |
| Goose | `goose` | OpenAI 或 Anthropic 相容端點 | `GOOSE_*` + provider 金鑰環境變數 | 可 |
| Qwen Code | `qwen` | OpenAI、Anthropic，或 Gemini 相容端點 | `--auth-type` 旗標 + 環境變數 | 可 |
| Kimi Code CLI | `kimi` | OpenAI 或 Anthropic 相容端點 | 暫時的 `--config-file`（合併後的 TOML，執行後即刪除） | 可 |

不論原生支援什麼，每個 agent 都只要一次 `codex login` 就能使用 Codex
橋接 —— 詳見下方的 [Codex 橋接](#codex-橋接)。執行 `alc doctor` 可取得
完整的相容矩陣（每個 provider profile 對照全部八個 agent），依你目前的
設定解析。

## Codex 橋接

一次 `codex login` 就能讓 alc 啟動的每個 agent 使用：

```sh
codex login
alc --codex claude
alc --codex opencode
alc --codex pi
alc --codex copilot
alc --codex goose
alc --codex qwen
alc --codex kimi
```

`alc` 會在 loopback port 上啟動內建的 `claude-codex` 轉接器，並只讓啟動
的那個 agent 行程指向它。轉接器對 Claude Code 說 Anthropic Messages，對
OpenCode／Pi／Kimi Code CLI 說 OpenAI Responses，對 Copilot CLI／Goose／
Qwen Code 說 OpenAI Chat Completions —— 三種不同的 wire protocol，背後
都是同一個登入。Claude Code 是唯一能在工作階段中切換的 agent（它每次
請求都會帶上模型與 effort，所以 alc 從不會把任何一項鎖在轉接器上）；其他
每個 agent 都是在啟動時就選定一個模型，以及該次工作階段固定的一個推理
強度。

### Claude Code

Claude Code 會直接以你儲存的預設值啟動，並把所有模型都放進它自己的
`/model` 選單：

| 模型 | 適合的情境 | Codex 預設強度 |
| --- | --- | --- |
| `gpt-5.6-sol` | 能力最完整，適合架構、困難除錯與大型重構 | `low` |
| `gpt-5.6-terra` | 速度、能力、成本均衡，建議新手從這個開始 | `medium` |
| `gpt-5.6-luna` | 速度快、費用低，適合簡單修改與大量例行工作 | `medium` |

清單依能力由強到弱排列，與 Codex 對這個系列公布的分級一致。

上游細節可參考 OpenAI 的
[模型選擇指南](https://developers.openai.com/api/docs/guides/latest-model)、
[Luna 說明](https://developers.openai.com/api/docs/models/gpt-5.6-luna)與
[Sol 說明](https://developers.openai.com/api/docs/models/gpt-5.6-sol)。

進到工作階段後，用 `/model` 換模型，該畫面的左右方向鍵可調整推理強度；
也可以用 `/effort` 直接指定等級。每個模型都接受 `low`、`medium`、
`high`、`xhigh`、`max`。強度越高，模型思考的空間越大，但也會花更多
時間與額度。

alc 會透過 Claude Code 的
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
設定傳入模型清單，這個設定自 Claude Code 2.1.243 起提供。選單只會顯示
這些 GPT 模型與 Default 一列，因為 Claude 自家的模型無法經由 Codex
轉接器服務；舊版會忽略這個設定，仍可拿到啟動時的預設模型作為可選
項目。因為 Claude Code 每次請求都會帶上模型與 effort，alc 不會把任何
一項鎖在轉接器上。

想單次換掉起始值，或用在腳本與 CI：

```sh
alc --codex claude --model gpt-5.6-luna --effort low
alc --codex claude --model gpt-5.6-terra --effort medium --save
```

`--save` 會把模型與 effort 存入選定的 alc provider。沒有這些參數時，
起始值依序取自 alc provider、選定的 Codex profile、模型內建預設值。
放在 `--` 之後的 `--model`、`--effort`、`--settings` 會原樣交給 Claude
Code，並蓋過 alc 的注入值。

Claude Code 的內建別名也一併留在 Codex 上：選單的 Default 一列跟著 alc
的預設值，`haiku` 與背景工作使用最便宜的目錄模型，`sonnet` 跟著本次的
起始模型，`opus` 使用最強的模型。

用 `/model` 選的模型只影響那一次 Claude Code 工作階段。下次執行
`alc --codex claude` 會再從 alc provider 的預設值開始，所以 `alc config`
仍然是唯一的真實來源。

### 其他每個 agent

OpenCode、Pi、Kimi Code CLI 會直接使用轉接器的 OpenAI Responses 介面；
Copilot CLI、Goose、Qwen Code 則使用它的 OpenAI Chat Completions 介面。
每一個都用各自的機制接上（`OPENCODE_CONFIG_CONTENT` 裡的 `alc-codex`、
一筆 `alc-codex` 的 `models.json` 項目、一份 `alc-codex` 暫存設定，或是
每個 agent 原本用在 `openai` 這個 kind 上、同一套 BYOK 環境變數／
`--auth-type`），只是改指向 loopback 轉接器，而不是工作階段內的選單。

模型清單每天最多自動向本機 Codex CLI 同步一次；離線時會使用內建清單：

```sh
alc models
alc models --refresh
alc models --json
```

同步到的 Codex context window 也會透過 Claude Code 官方的
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars)
設定傳入，讓 Claude Code 不認得的 GPT ID 依照 Codex 的實際上限壓縮
對話，而不是用它的通用預設值。

發行包會附帶
[`claude-codex` 0.3.1](https://github.com/fcakyon/claude-code-with-codex)，
一個 MIT 授權的 helper。`alc` 會把它綁在隨機的 `127.0.0.1` port，只讓
啟動的那個 agent 行程指向它，並在該行程結束時關閉。Helper 會讀取並
可能更新 `~/.codex/auth.json`；憑證不會被複製到 `alc` 的設定裡。

這個轉接器是第三方相容層，不是 OpenAI 或 Anthropic 的官方整合。使用
訂閱帳號前，請先檢閱 [THIRD_PARTY.md](THIRD_PARTY.md) 與你的 provider
條款。

## 完整設定

檔案位置：

| 平台 | 設定目錄 |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

檔案：

- `config.toml`：provider 中繼資料、模型、預設值、URL 與環境變數名稱。
- `credentials.toml`：本機儲存的 API key。在 Unix 上 alc 會以 `0600`
  權限寫入；在 Windows 則位於目前使用者的 AppData 底下。

可用 `ALC_CONFIG_DIR` 覆寫目錄位置。常用的腳本化指令：

```sh
alc config init
alc config show
alc config path
alc config upsert codex --kind codex --model gpt-5.6-terra --effort medium
alc config upsert work --kind openrouter --model anthropic/claude-sonnet-4.6
printf '%s' "$OPENROUTER_API_KEY" | alc config key work --stdin
alc config set-default claude work
alc config remove work
```

每個畫面底部都會顯示可用按鍵，主要操作如下：

- `a`、`e`/Enter、`d`：新增、編輯、刪除 provider。
- `Tab`：在 provider 清單與 agent 預設值之間切換。
- 方向鍵：移動欄位與切換選項，包含推理強度。
- 在 Codex profile 上，把游標移到 Model 欄位並按 `←`/`→`，會開啟引導式的
  GPT 模型與推理強度選擇畫面，寫入 `alc --codex claude` 的啟動預設值。
- `s`：儲存；`q`：儲存並離開；`Ctrl+C`：不儲存離開。

## 從原始碼建置

需要 Rust 1.88 以上：

```sh
cargo build --release --locked
```

從原始碼建置只會產生 `alc`。要使用 `alc --codex <agent>`，請把相容的
`claude-codex` 執行檔放到 PATH，或設定 `ALC_CLAUDE_CODEX_BIN`。官方的
`alc` 發行包已經附帶固定版本的 helper。

常用的開發檢查：

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## 解除安裝

把 `alc` 與 `claude-codex` 從安裝目錄移除，需要的話再刪掉 `alc config
path` 顯示的設定目錄。刪除設定目錄同時會刪掉本機儲存的 API key，且
無法復原。

## 授權

`alc` 採用 MIT 授權。隨附的第三方授權聲明請見
[THIRD_PARTY.md](THIRD_PARTY.md)。
