# all-code (`alc`)

**用你已經在付錢的 Codex／ChatGPT 訂閱跑 Claude Code** —— 另外七個 coding
agent 也一樣，用同一個登入，或是你指給它們的任何一家 provider。任何 session
都能鏡像到一個網頁，從另一台裝置操作。

[![CI](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml/badge.svg)](https://github.com/treeleaves30760/all-code/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/treeleaves30760/all-code?logo=github)](https://github.com/treeleaves30760/all-code/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)](#安裝)

📖 **[完整文件](https://treeleaves30760.github.io/all-code/zh-TW/)** ·
🇬🇧 **[English](https://treeleaves30760.github.io/all-code/)**

## 三行指令，在你的 ChatGPT 方案上跑 Claude Code

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
codex login
alc --codex claude
```

Windows PowerShell：`irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex`

**沒有設定這個步驟。** 不用 `alc config init`，也沒有檔案要編輯。初始設定
編在執行檔裡，本來就帶著一組 Codex profile；`alc --codex claude` 在記憶體
裡讀它，不會寫出自己的設定檔。它唯一留在 `~/.config/alc` 的東西是 Codex
的模型清單快取，每天最多重新抓一次。

**它要的**是 `codex login` 寫下的 `auth.json`，以及 PATH 上的 `claude` ——
alc 負責啟動 coding agent，本身不附帶它們。**它不要的**是 API key、alc 的
設定檔，或啟動時的 `codex` 執行檔；那個執行檔是用來跑 `codex login` 本身，
以及讓模型清單保持新鮮的。alc 會先檢查登入，再去找 agent，所以兩樣都缺的
人會先被告知 `codex login`：

```text
error: Codex credentials were not found at ~/.codex/auth.json; run `codex login` and retry
error: 'claude' is not installed or not on PATH; install it first, then retry `alc claude`: cannot find binary path
```

**你會得到什麼。** Claude Code 會以 `gpt-6-sol`、`medium` 強度啟動 ——
如果你以前用過 Codex CLI，就以你自己的 `~/.codex/config.toml` 裡已經寫好的
模型與強度啟動 —— 帶著 Codex 真正的 272k context window，而不是 Claude Code
對它不認得的 model ID 假設的 200k，而且 Codex 提供的每個 GPT 模型都會出現在
它自己的 `/model` 選單裡：

| 模型 | 適合的情境 | Codex 預設強度 |
| --- | --- | --- |
| `gpt-6-astra` | GPT-6，能力最強，適合複雜且吃重的工作 | `medium` |
| `gpt-6-sol` | GPT-6，日常編碼與代理式工作，建議從這個開始 | `medium` |
| `gpt-6-luna` | GPT-6，速度快、費用最低，適合快速修正與大量的例行工作 | `medium` |
| `gpt-5.6-sol` | 上一代，適合複雜的專業工作 | `low` |
| `gpt-5.6-terra` | 上一代，日常編碼的均衡選擇 | `medium` |
| `gpt-5.6-luna` | 上一代，速度快、費用低 | `medium` |

進到 session 後，用 `/model` 換模型，該畫面的左右方向鍵可調整推理強度；
`/effort` 則直接指定等級。想單次換掉起始值，或用在腳本裡：

```sh
alc --codex claude --model gpt-6-luna --effort low
```

**之後你直接跑 `claude`，還是會連到 Anthropic。** 那個選單會把你的選擇寫進
`~/.claude/settings.json`，而這台機器上每一個 Claude Code session 都會讀到
那個檔案，包含不是 alc 啟動的那些 —— 它們前面沒有轉接器。alc 會在啟動前先
讀下那一個欄位，session 結束時再放回去。細節，以及它刻意不動那個檔案的兩種
情況，都在 [Codex 橋接](#codex-橋接)。

alc 啟動時什麼都不印 —— 只有一種例外：你的 Codex 設定要的 `ultra` 強度被降
到 `max` 時會說一句 —— 所以你看到的就是 Claude Code。中間那層轉接器是第三方
相容層，不是 OpenAI 或 Anthropic 的官方整合；把訂閱憑證交給它之前，請先看過
[THIRD_PARTY.md](THIRD_PARTY.md) 與你的 provider 條款。

## 一次登入，每個 agent 都能用

同一次 `codex login` 就能驅動 alc 啟動的每個 agent。不用第二把金鑰，也不用
逐個 agent 設定。

```sh
alc --codex claude       # session 內的 /model 選單
alc --codex opencode
alc --codex pi
alc --codex copilot
alc --codex goose
alc --codex qwen
alc --codex kimi
alc codex                # Codex CLI 本身，用它自己的登入，沒有轉接器
```

| Agent | 用什麼協定接上橋接 | 怎麼換模型 |
| --- | --- | --- |
| Claude Code | Anthropic Messages | `/model` 選單，session 進行中可換 |
| OpenCode、Pi、Kimi Code CLI | OpenAI Responses | 一個模型，啟動時選定 |
| Copilot CLI、Goose、Qwen Code | OpenAI Chat Completions | 一個模型，啟動時選定 |
| Codex CLI | 原生，不經橋接 | Codex 自己的選單 |

alc 會在 loopback port 上啟動自己的 Codex 轉接器，並只讓啟動的那個 agent
行程指向它 —— 三種 wire protocol、一個登入。Claude Code 的轉接器是一個由它
的 session 共用的背景行程（見[背景 session](#背景-session)）；其他 agent 的
則住在 alc 裡面，隨 session 一起結束。Claude Code 是唯一能在 session 進行中
切換的 agent，因為它每次請求都會帶上模型與推理強度，所以 alc 從不會把任何
一項鎖在轉接器上；其他 agent 都是在啟動時選定一個模型和一個推理強度。

下方的 [Codex 橋接](#codex-橋接)有模型清單、強度分級，以及轉接器拿你的憑證
做了什麼。

## 從手機操作

任何 session、任何 agent、任何 provider —— 全都鏡像到一個網頁，任何連得到
這台機器的地方都能打開它。

```sh
alc --share claude          # 或：alc share claude
```

```text
alc session claude-7QK2M9XB4T (claude@all-code)
  open  http://127.0.0.1:8787/#k=…
  hub   127.0.0.1:8787 · loopback only (pid 48213) · this link grants input; keep it to yourself
  keys  ctrl-\ then d detaches; the session keeps running
```

**你自己的終端機完全照舊。** 共享是鏡像，不是把 session 拿走。被鏡像的是
終端機本身，所以每個 agent、每個 provider 的行為都一樣 —— 沒有任何東西需要
為個別 agent 另外支援。

**頁面給你的**是 session 清單、即時畫面、一列手機鍵盤沒有的按鍵（Esc、Tab、
Shift+Tab、Ctrl、方向鍵），以及一個把整段提示詞當成一整塊送出的輸入框，省得
在原始終端機裡跟手機鍵盤纏鬥。在寬螢幕上打開一個 session 時，返回按鈕旁有一個
按鈕能把 session 清單收起來，讓終端機用滿整個寬度 —— `--tmux` session 會真的拿到
多出來的欄數，一般 session 則是畫得更大 —— 這個選擇會記在那個瀏覽器裡。

**Session 活得比啟動它的終端機久**，因為它們由背景的 hub 擁有：

```sh
alc sessions               # 連結，然後是有哪些在跑
alc attach 7QK2            # 從任何終端機接回去
alc kill 7QK2
```

Id 可以只給任何不會有歧義的前綴，就像 git 的短雜湊那樣。`alc sessions` 把
連結放在最前面，因為 `--share` 印出的那個，在 agent 畫出自己的介面時就捲走了。

**這個連結能做什麼。** 被共享的 Claude Code、Codex 或 OpenCode session 會以
**ask** 模式啟動 —— 旗標是 alc 自己傳的，所以連結不會把一個全自主的 agent
交到別人手上。其他五個 agent 則是以各自的預設值啟動，除非你用 `--permission`
指定層級。要把 session 放寬到超過你設定的上限，必須在主機的終端機上輸入
`alc confirm <ticket>`。[遠端控制](#遠端控制)有完整的權限層級與威脅模型。

遠端控制在 macOS、Linux 與 Windows 10／11 上都能用；在 Windows 上搭配 `--tmux`
要有原生 Windows 版 tmux（安裝器會嘗試自動補裝），見[尺寸歸誰決定](#尺寸歸誰決定)。

## 背景 session

Claude Code 的 [agent view](https://code.claude.com/docs/en/agent-view) 會把
session 放到背景跑 —— `claude agents`、`claude --bg`，以及在空的提示列上按
`←` —— 由它自己的一個 supervisor 管著，活得比終端機久。alc 啟動的每一個
Claude Code session 在那裡都能用，而且跑在 alc 給它的 provider 上：

```sh
alc --codex claude agents                     # agent view；每一次派出都跑在 Codex 上
alc --codex claude --bg "fix the flaky test"  # 直接送到背景
alc --codex claude                            # 在空提示列上按 ←：一樣跑在 Codex 上
```

**怎麼做到的。** alc 用一份設定檔把 provider 交給 Claude Code，以 `--settings`
傳入，而 Claude Code 會替背景 session 留著它，每次重新啟動那個 session 時再讀
一次。檔案裡沒有金鑰：provider 需要金鑰時，Claude Code 透過它的 `apiKeyHelper`
設定向 alc 要，而 alc 就從它本來就在讀的地方讀出來。用 Claude Code 自己的登入時，
由那個登入來回答，檔案裡只放端點與模型。

**Codex 橋接現在自己獨立跑了。** 背景 session 活得比啟動它的那個 `alc` 還久，
所以轉接器也必須一樣：一個小小的 alc 行程，綁在一個它會一直留著的 loopback
port 上，只回答帶著它 token 的請求。需要它的 session 會把它叫起來，而它閒著
一小時沒事做就會停掉。

```sh
alc bridge          # 在不在跑，以及在哪裡
alc bridge stop     # 現在就停掉；下一個需要它的 session 會再把它叫起來
```

**每一個 Claude 模型都變成 Codex 模型。** 在 `alc --codex claude` 底下，沒有
任何一個請求會送到 Claude 模型。`/model` 選單只列出 Codex 模型；所有別名
（`opus`、`sonnet`、`haiku`、`fable`、`best`、`opusplan`）以及 Claude Code
自己的背景工作，都會落在 Codex 模型上；而指名完整名稱的 Claude 模型 ——
`/model claude-opus-5`、subagent 的 `model:`、fallback 鏈 —— 則由同一級的
Codex 模型來回答。fast mode 與 advisor 只存在於 Claude 模型上，所以在這些
session 裡是關閉的。`claude ultrareview` 與雲端 session 跑在 Anthropic 的
伺服器上，仍然是 Anthropic 的功能。

`alc claude attach`、`logs`、`stop`、`respawn` 與 `rm` 都是直接交給 Claude
Code，普通的 `claude attach` 也一樣：那個 session 本來就帶著它的設定檔。那些
根本不會碰到模型的指令 —— `mcp`、`doctor`、`plugin`、`update` 之類 —— 也是
直接交出去，不會啟動橋接，也不會被算成一個 session。

## 任何 provider 都行，不只 Codex

`codex login` 是最短的一條路，不是唯一的一條。八個 agent 中的任何一個都可以
指向 Anthropic、OpenAI API、OpenRouter、本機的 Ollama 或 vLLM 伺服器、
DeepSeek、Moonshot、Z.ai、MiniMax、Groq、xAI、Google，或任何自訂端點 ——
也可以只替這一次執行換掉，什麼都不用改。

```sh
alc config                 # key 和每個 agent 的預設值都在這裡
alc claude                 # 每個 agent 各用自己設定好的預設值
alc --openrouter codex
alc --deepseek pi
alc --ollama claude
alc -p local-vllm opencode
```

`--provider`（或 `-p`）接受 profile 名稱；當某個 kind 只有一個 profile 時，
也可以直接寫 kind。捷徑旗標 `--anthropic`、`--openai`、`--openrouter`、
`--codex`、`--ollama`、`--vllm`、`--deepseek`、`--moonshot`、`--zai`、
`--minimax`、`--groq`、`--xai`、`--google` 效果相同。初始設定內含 Anthropic、
OpenAI、OpenRouter、Codex、Ollama，以及一個預設停用的 vLLM 範本；key 存在
本機或從環境變數讀取，而環境變數的優先權比較高。

這八個 agent 講的模型協定並不完全相同，十四種 provider kind 對外提供的協定
也不盡相同，所以 alc 會在啟動前先驗證組合，而不是送出一個注定失敗的請求。
[Provider 與 agent](#provider-與-agent) 列出每一種 kind 的端點、金鑰環境
變數與協定。

## 指令一覽

| 指令 | 作用 |
| --- | --- |
| `alc claude`、`codex`、`opencode`、`pi`、`copilot`、`goose`、`qwen`、`kimi` | 用該 agent 設定好的 provider 啟動它 |
| `alc config` | 設定用的 TUI；另有 `init`、`show`、`path`、`upsert`、`key`、`set-default`、`remove` |
| `alc doctor` | 執行檔、憑證、相容性、預設值，以及橋接與遠端狀態 |
| `alc models` | Codex 橋接提供的 GPT 模型；`--refresh`、`--json` |
| `alc usage` | 每個 Claude／Codex 登入與 API key 的剩餘額度，以及各 provider 與 agent 的用量；`--json` |
| `alc update` | 就地更新 `alc`；`--check`、`--force` |
| `alc share <agent>` | 啟動 agent，並把 session 鏡像到網頁 |
| `alc sessions` | 先是頁面連結，然後是共享中的 session（tmux 的會標示出來） |
| `alc attach <id>` | 把這個終端機接回某個共享的 session |
| `alc rename <id> <name>` | 替頁面上某個 session 的卡片改名 |
| `alc kill <id>` | 停掉一個共享的 session |
| `alc hub` | 對擁有 session 的那個行程下 `status`、`start`、`stop --drain` |
| `alc bridge` | Claude Code 的 session 用來連上 Codex 登入的那個背景橋接；`status`、`stop` |
| `alc remote` | `status`、`url`、`on`/`off`、`auto-share`、`allow-host`、`token --rotate` |
| `alc confirm <ticket>` | 核准某個共享 session 提出的權限變更 |

各指令有哪些旗標，`alc <command> --help` 會告訴你。

## 執行 agent

**參數轉送。** 除了 Claude 專用的 `--model`、`--effort`、`--save` 之外，
agent 名稱後的參數會原封不動傳入：

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
alc --ollama opencode run "fix the failing test"
```

如果要把同名參數交給 Claude 本身，請放在 `--` 後面：
`alc claude -- --model sonnet`。你自己傳的 `--settings` 會被合併進 alc 那
一份，衝突時以你的為準，因為 Claude Code 只讀一份。

alc 自己的旗標 —— `--share`、`--no-share`、`--bind-lan`、`--name`、
`--permission`、`--tmux`、`-t` —— 必須放在 agent 名稱**之前**。放在後面
的話，它們會被當成 prompt 文字交給 agent，所以 alc 會就此停下來，並直接
告訴你 —— 除非你在 agent 名稱後面緊接著寫上 `--`，那就表示你指的是 agent
自己的旗標：

```sh
alc --codex claude -- -p "fix the flaky test"
alc --codex claude -- --bg --name nightly "run the slow suite"
```

`--` 要緊接在 agent 名稱後面，排在它所有旗標之前；寫得比那更後面，它就會
變成一個參數本身，被傳到 agent 手上。

**預覽。** `alc --codex --dry-run claude` 會印出解析後的 agent 與
provider、機密已遮蔽的指令、啟動有用到內建轉接器時的那一行，以及它會
寫入的每一個檔案 —— 而且會說明哪些啟動會被拒絕，不只是列出哪些會成功。

## 診斷

```sh
alc doctor
```

`alc doctor` 會回報環境與憑證路徑、全部八個 agent 的執行檔、每個 provider
profile 對照全部八個 agent 的結果、解析後的各 agent 預設值、殘留在
`~/.claude/settings.json` 裡被釘住的 GPT 模型、Codex 橋接的模型、推理強度
與 `codex login` 狀態、已啟用的 Ollama profile 對照執行中伺服器的檢查，
以及遠端控制的現況 —— 最後是一份問題摘要，每一項都附上修法。只要找到
其中一個問題，它就會以非零狀態結束。

具名錯誤與各自的修法，請見
[疑難排解指南](https://treeleaves30760.github.io/all-code/troubleshooting)。

## 還剩多少，又花到哪裡去

```sh
alc usage
```

```text
Accounts
     PROFILE     ACCOUNT                  PLAN  REMAINING
  ✓  anthropic   ~/.claude                max   5h 97% left, resets in 2h 53m — week 79% left, resets in 6d 4h — Fable week 62% left, resets in 6d 4h
  ✓  codex       you@example.com          pro   week 66% left, resets in 5d 9h — no credits
  ·  ollama      —                        —     no quota API
  ·  openrouter  —                        —     no API key; run `alc config key openrouter`

Usage by provider and agent
  PROVIDER  AGENT     LAUNCHES  TURNS  INPUT  OUTPUT  LAST
  codex     claude    1         1      20.8K  35      7m ago
  ollama    opencode  1         —      —      —       12m ago
  source: ~/.config/alc/usage.jsonl — tokens are counted only where alc carries the traffic; a direct launch counts as a launch alone

✓ ready
```

alc 會向每個登入所屬的服務商查詢剩餘額度：Codex 登入問 chatgpt.com、Claude
Code 的登入問 api.anthropic.com，OpenRouter、DeepSeek、Moonshot、MiniMax 與
Z.ai 的 key 則走各自公開的餘額端點。過程中不會寫回任何東西，也不會更新
token —— 一個查詢狀態的指令不該讓執行中的 session 手上的憑證失效。

**同一種登入有兩個帳號，就開兩個 profile。**`codex_home` 與
`claude_config_dir` 會固定某個 profile 的憑證目錄，而且每次透過該 profile
啟動都會用同一個帳號 —— 所以你讀到的那一列，就是實際花掉額度的帳號：

```sh
CODEX_HOME=~/.codex-work codex login
alc config upsert codex-work --kind codex --codex-home ~/.codex-work
alc --provider codex-work claude
```

第二張表來自設定目錄裡的 `usage.jsonl`：每次啟動一行，Codex 橋接經手的每個
回合再一行。alc 沒有經手流量的 provider 顯示 `—` 而不是 0，因為那些 token
是未知，不是沒有。遠端控制頁面標題列的用量按鈕後面是同樣這兩個區塊。

完整說明請見 [用量](https://treeleaves30760.github.io/all-code/usage)。

## Provider 與 agent

以下兩張對照表，`alc doctor` 會依你自己的設定解析出對應的版本。

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

### Agent

| Agent | 執行檔 | 支援端點 | alc 注入內容 | Codex 橋接 |
| --- | --- | --- | --- | --- |
| [Claude Code](https://code.claude.com/docs/en/setup) | `claude` | Anthropic 相容端點 | `--settings` 檔案（端點、模型別名、選單）+ `apiKeyHelper` | 可（`/model` 選單） |
| [Codex CLI](https://learn.chatgpt.com/docs/codex/cli) | `codex` | OpenAI Responses API | 旗標 + `--config` 覆寫 | 可（原生登入） |
| [OpenCode](https://opencode.ai/docs) | `opencode` | 任何 API 相容的 provider | 行內 `OPENCODE_CONFIG_CONTENT` 環境變數 | 可 |
| [Pi](https://github.com/earendil-works/pi) | `pi` | Anthropic、OpenAI，或 OpenAI 相容端點 | 合併進 `models.json` + 旗標 | 可 |
| [Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli) | `copilot` | OpenAI 或 Anthropic 相容端點 | `COPILOT_PROVIDER_*` 環境變數 | 可 |
| [Goose](https://block.github.io/goose/) | `goose` | OpenAI 或 Anthropic 相容端點 | `GOOSE_*` + provider 金鑰環境變數 | 可 |
| [Qwen Code](https://github.com/QwenLM/qwen-code) | `qwen` | OpenAI、Anthropic，或 Gemini 相容端點 | `--auth-type` 旗標 + 環境變數 | 可 |
| [Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli) | `kimi` | OpenAI 或 Anthropic 相容端點 | 暫時的 `--config-file`（合併後的 TOML，執行後即刪除） | 可 |

`alc` 只啟動已經安裝好的 agent —— 你打算用的那幾個，請從上面的連結裝起來
（Pi 是 `npm install -g @earendil-works/pi-coding-agent`）。不論原生支援
什麼，每個 agent 都只要一次 `codex login` 就能接上 Codex 橋接；而
`ALC_CLAUDE_BIN` 與它的同類環境變數可以覆寫執行檔路徑。

## Codex 橋接

alc 追蹤六個 Codex 模型 —— 就是上面表格裡的那六個 —— 並從你的 ChatGPT 帳號
同步它們的細節。橋接本身不保留任何允許清單：收到什麼 slug 就往上游送，由
chatgpt.com 決定，所以一個 alc 沒被教過的模型，仍然可以在 Codex 推出的當天用
`--model` 指名叫到。1.5.0 的 `gpt-6-astra` 就是這樣能用起來的：當時 alc 所依賴
的那個橋接裡寫死的清單，落後了一個版本。

**推理強度。** 每個模型都接受 `low`、`medium`、`high`、`xhigh`、`max`。強度越
高，模型思考的空間越大，但也會花更多時間與額度。除了兩個 Luna 之外，每個模型
都另外提供高於 `max` 的 `ultra` 一級。這一級可以用原生的 `alc codex` 使用，但
**無法**透過橋接：內建 helper 自己的強度範圍到 `max` 為止，所以 alc 會在啟動時
把它降到 `max` 並明說，而不是讓請求在 session 進行到一半被拒絕。

上游的最新細節可參考 OpenAI 的
[模型選擇指南](https://developers.openai.com/api/docs/guides/latest-model)、
[Sol 說明](https://developers.openai.com/api/docs/models/gpt-6-sol)與
[Luna 說明](https://developers.openai.com/api/docs/models/gpt-6-luna)。

**挑選預設值。**

```sh
alc --codex claude --model gpt-6-sol --effort medium --save
```

`--save` 會把兩者都存進選定的 alc provider。沒有這些參數時，session 的起始值
依序取自 alc provider、選定的 Codex profile、模型自己文件上的預設值。放在 `--`
之後的 `--model` 或 `--effort` 會原樣交給 Claude Code，並蓋過 alc 原本要注入的
值；你自己傳的 `--settings` 則會被合併進 alc 那一份，衝突時以你的為準，因為
Claude Code 只讀一份。用 `/model` 選的模型只影響那一次 session；下次啟動又會從
alc 的預設值開始，所以 `alc config` 仍然是唯一的真實來源。

**選單。** alc 會透過 Claude Code 的
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
設定傳入模型清單，這個設定自 Claude Code 2.1.243 起提供。選單只會顯示這些 GPT
模型與 Default 一列，因為 Claude 自家的模型無法經由轉接器服務；舊版的 client
會忽略這個設定，仍可拿到啟動時的預設模型作為可選項目。Claude Code 的內建別名
也一併留在 Codex 上：Default 一列跟著 alc 的預設值，`haiku` 與背景工作使用清單
裡最便宜的模型，`sonnet` 跟著這次 session 的起始模型，`opus` 使用最強的那一個。

**你的 Claude Code 預設值。** 有一件事要知道，因為那個選單是 Claude Code 的、
不是 alc 的：session 最後落在哪個模型，Claude Code 也會把它寫進
`~/.claude/settings.json`，當成之後每個新 session 的預設值。那個檔案會被這台
機器上每一個 Claude Code session 讀到，包含不是 alc 啟動的那些，而那些前面沒有
轉接器 —— 之後直接執行 `claude` 就會向 Anthropic 要一個 GPT 模型，然後被告知
它不存在。

alc 會在 session 結束時把那一個欄位放回去。它在啟動前先讀下原本的值，結束後再
寫回，所以你自己的預設值撐得過一趟轉接器。有兩種情況它刻意不動：你在 session
中途自己切到某個真正的 Claude 模型 —— 那是你的選擇，不該由 alc 推翻；以及
session 是被直接砍掉的 —— 那時候沒有任何東西跑得起來去還原。後者 `alc doctor`
仍然會指出那個檔案與那一行，而下一次 `alc --codex claude` 就會把它清掉：一個
*本來就*只有轉接器服務得了的值，會被直接移除，而不是再寫回去。

### 其他每個 agent

OpenCode、Pi、Kimi Code CLI 會直接使用轉接器的 OpenAI Responses 介面；Copilot
CLI、Goose、Qwen Code 則使用它的 OpenAI Chat Completions 介面。每一個都用各自
的機制接上（`OPENCODE_CONFIG_CONTENT` 裡的 `alc-codex`、一筆 `alc-codex` 的
`models.json` 項目、一份 `alc-codex` 暫存設定，或是每個 agent 原本用在 `openai`
這個 kind 上、同一套 BYOK 環境變數與 `--auth-type`），只是改指向 loopback
轉接器，而不是 session 內的選單。

模型清單直接向你的 ChatGPT 帳號同步——也就是轉接器每一輪請求送去的同一方——
所以只要那個帳號跑得動的模型，即使已安裝的 Codex CLI 沒聽過，也照樣會出現。
抓不到時才退回 `codex debug models`，而執行檔內建的那份清單是兩者都不能低於
的底線：同步回來的清單只能新增模型，不能拿掉。alc 每天同步一次，Codex 一升級
就會立刻再同步一次：

```sh
alc models
alc models --refresh
alc models --json
```

同步到的 Codex context window 也會透過 Claude Code 官方文件上的
[`CLAUDE_CODE_MAX_CONTEXT_TOKENS`](https://code.claude.com/docs/en/env-vars)
gateway 設定傳入，讓它不認得的 GPT ID 依照 Codex 的正確上限壓縮對話，而不是用
Claude 的通用預設值。

### 橋接怎麼運作

橋接是 alc 自己的程式碼（`src/bridge/`）。給 Claude Code 用的時候，它是一個
獨立的背景行程，綁在一個它會一直留著的 loopback port 上（`alc bridge`）；其他
每一個 agent 則是跑在 `alc` 行程內、綁在隨機的 port 上，並在該 session 結束時
關閉。它會讀取並可能更新 `~/.codex/auth.json`；憑證不會被複製到 `alc` 的設定
裡。

## 本機模型

`alc --ollama claude` 會把 Claude Code 指向 Ollama 伺服器的 Anthropic Messages
端點。本機伺服器只提供已經 pull 下來的模型，而且一次只回答一個請求，所以 alc
對這種 session 的設定和雲端 provider 不同：每一個模型別名
（`ANTHROPIC_DEFAULT_MODEL` 以及 sonnet／opus／haiku 三層）都釘在 profile 的
模型上，這樣 Claude Code 永遠不會向 Ollama 要它沒有的 model ID；關掉非必要的
附帶流量；從 `/api/ps` 或 `/api/show` 讀出真正的 context window；第一個 token
的逾時拉長到三十分鐘。

最後這一項比聽起來重要。Claude Code 每個 session 的第一個請求大約有 25k 到 40k
tokens —— 系統提示、工具 schema、專案內容 —— 而筆電等級的模型每秒只讀得了幾十
個 token：在 M3 MacBook Air 上，`gemma4:12b` 讀完 22k tokens 的請求要約六分鐘
才吐出第一個 token，39k 的要十五分鐘。沒有拉長逾時，Claude Code 每次嘗試六分鐘
後就放棄，然後從頭再來一次。

在筆電上要跑得舒服，靠的是：`ollama show <model>` 的 capabilities 列有 `tools`
的模型、64k 到 128k 的 context window、維持精簡的第一個請求（每個 MCP server、
plugin 和 skill 都會再往裡面加工具 schema），以及讓模型保持載入，好讓 prompt
cache 在每一輪之間撐下來。`alc doctor` 會印出 **Ollama** 區塊：伺服器版本、模型
是否已 pull、能否呼叫工具，以及它實際拿到的 context 長度。

完整的調校筆記 —— KV cache 型別、keep-alive、為什麼在 Gemma 4 上提示長度的代價
比線性還高 —— 都在
[provider 指南](https://treeleaves30760.github.io/all-code/local-models)
裡。

## 遠端控制

[從手機操作](#從手機操作)是短版，這裡是其餘的部分。

```sh
alc sessions                 # 連結，然後是有哪些在跑（tmux session 會標示出來）
alc attach 7QK2              # 任何不會有歧義的 id 前綴
alc rename 7QK2 review
alc kill 7QK2
alc hub status               # 或直接 `alc hub`
alc hub stop --drain
alc remote url               # 連結捲走之後再拿一次
alc remote status            # 開/關、綁定方式、上限、檔案位置
alc remote auto-share on     # 每個 session 都共享，不必加 --share
alc remote off               # 完全禁止共享
alc remote token --rotate    # 讓已發出的連結全部失效
```

Session 由背景的 hub 擁有，所以它們活得比啟動它們的終端機久，也因此全部出現在
同一個頁面上；`ctrl-\` 然後 `d` 卸離。預設共享在 `alc config` 的
**Sharing & remote** 畫面裡也能開。

### 尺寸歸誰決定

不加 `--tmux` 的共享 session 是一個終端機、兩個觀看者，而一個終端機只有一種尺寸。
那個尺寸屬於你啟動時所在的終端機 —— 它就在那裡，正用那個尺寸畫著畫面 —— 所以頁面
不會去動它。頁面改成照 agent 真正的格線去畫，在放得下的前提下盡量放大、置中，比例
對不上的地方留黑。你把終端機拉大縮小，頁面幾秒內就跟上。

`--tmux` 是給「頁面才是你真正要用的那一邊」的情況：

```sh
alc --share --tmux --codex claude    # 或 -t
```

你的終端機和 hub 各自以獨立的 tmux client 附著上去，所以誰都不必遷就誰。取捨是它
整個反過來跑 —— 尺寸由頁面決定，比它更窄或更矮的終端機會看到畫面的左上角（用
`ctrl-b :refresh-client -L/-R/-U/-D` 平移）。沒有瀏覽器接上時，尺寸就停在啟動時
的樣子。鍵盤也歸 tmux 管，所以卸離要按 `ctrl-b` 再按 `d`，換來的是 tmux 自己的
捲動紀錄，以及一個 ssh 斷線也還在的 session。

alc 為每個 session 開自己的 tmux server，而且啟動時完全不讀設定檔，所以你原本的
tmux 完全不受影響，alc 的 session 對每個人的行為都一樣，而 `~/.tmux.conf` 也永遠
不會變成進入某個 session 環境的途徑。在 tmux 裡面跑 alc 也沒問題。需要 tmux 3.2
以上（Windows 的裝法見下面）；`alc doctor` 會告訴你裝的是哪一版。`--tmux` 只對被
共享的 session 有意義 —— 沒有共享就傳這個旗標，它會直接說。有一點要說清楚：你本機
的終端機現在是直接的 tmux client，不再是鏡像，所以它看到的是 agent 的原始輸出。
瀏覽器那邊仍然會把 alc 注入的 API key 遮蔽掉，你自己的終端機不會。

**在 Windows 上**，alc 安裝器會自動檢查並嘗試補裝原生 Windows 版 tmux。
若跳過自動補裝或未能完成，以下是手動備援：

```powershell
winget install --id arndawg.tmux-windows --exact
```

裝完請開一個新的終端機，讓 PATH 讀得到它；`alc doctor` 的 **tmux** 那一列會說有沒有
找到這個原生版本。psmux 也會裝一個 `tmux.exe`，但 alc 驅動不了它 —— 它跑不動 alc
建立 session 用的那串指令 —— 所以 alc 會越過 PATH 上的 psmux 去找原生版本，兩個
同時裝著也沒關係；MSYS2、Cygwin、WSL 版的 tmux 在 Windows 上也不會用到。

Windows 版 tmux 用 ANSI 字碼頁傳遞命令列、環境變數與工作目錄，所以在 Windows 上，
tmux 窗格裡跑的是一個小小的啟動器（就是 alc 自己），由它經 loopback 向 hub 取回
agent 確切的啟動內容。結果是：非 ASCII 的資料夾名稱、參數與環境變數值在 `--tmux`
下都能用，provider 的 API key 也從不進入 tmux 自己的環境。如果 alc 本身裝在路徑
不是純 ASCII 的資料夾，alc 會改用 Windows 的 8.3 短路徑；在關掉短檔名的磁碟上，
`--tmux` 會拒絕執行，並請你把 alc 裝到 ASCII 路徑底下。

在 Windows 上停掉一個 `--tmux` session（`alc kill` 或 `alc hub stop --drain`）會
結束它的 tmux server，agent 也跟著結束。Windows 沒有 hangup 訊號，所以這種情況下
session 卡片只會顯示 session 已結束，沒有結束狀態（macOS／Linux 上仍然會顯示那個
訊號）。

### 從手機連上

三種方式都支援：

```sh
# 自己的 Wi-Fi —— 什麼都不用裝
alc claude --share --bind-lan          # 印出 http://192.168.1.42:8787/#k=…

# Tailscale —— alc 只綁 loopback，走 HTTPS，中間沒有第三方
alc remote allow-host box.tail1a2b.ts.net
tailscale serve 8787

# Cloudflare Tunnel —— 行動網路也能連，不需要 VPN
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
```

alc 只回應你允許過的名字。Loopback 永遠在清單上，`--bind-lan` 會加上這台機器自己的
位址；隧道的主機名用 `alc remote allow-host` 加，可以精確指定，也可以用
`*.example.com` 涵蓋每次都改名的隧道。新加的允許主機要等執行中的 hub 重啟
（`alc hub stop --drain`）才會生效。LAN 連結是純 HTTP，token 會以明文經過你的區域
網路 —— 在家裡沒問題，在咖啡廳請改用隧道。

### 權限

alc 有五個層級，越後面越鬆：`plan`（只讀、只規劃，什麼都不寫）、`ask`（任何會改變
世界的動作都先問）、`auto-edit`（改檔案不問，執行指令還是要問）、`auto`（在 agent
自己那個沙箱的範圍內自行動作），以及 `full`（完全不設閘門）。`--permission <rung>`
決定 session 從哪一級開始。

沒有指定 `--permission` 的共享 session，在那三個 alc 拿真正的 `--help` 驗證過旗標的
agent 上 —— Claude Code、Codex 和 OpenCode —— 會從 **ask** 開始，所以在手機上打開的
連結，看到的不會是一個自行其是的 agent。另外五個則完全照它們自己的預設啟動：把一個
沒驗證過的旗標名稱猜進 argv，換來的不是更緊的 session，而是一個根本起不來的
session。你還是可以用 `--permission` 指名層級，alc 就會照文件把旗標傳過去 —— Pi
除外，它根本沒有權限模型可設。session 跑著的時候頁面可以改層級，但有一個上限 ——
預設是 `auto-edit`，在 `alc config` 的 **Sharing & remote** 畫面裡設定。往緊的方向
調永遠不需要放行。

超過上限時，頁面會給你一張 ticket，你到主機那台機器的終端機上輸入它：

```sh
alc confirm 7QK2M9XB4T
```

它會印出 `granted: <rung>`，接下來一分鐘內頁面可以套用這一次變更。沒被兌現的
ticket 五分鐘後過期，而 `alc confirm` 在沒有控制終端機的情況下會拒絕執行 —— 這正是
重點：agent 沒辦法自己兌現自己的 ticket。

八個 agent 對「權限模式」是什麼並沒有共識，所以 alc 為每一個都記下：旗標有沒有拿
真正的 `--help` 驗證過、模式到底能不能在 session 中途改（Kimi 只能重啟換；Pi 沒有
權限模型，也沒有沙箱），以及目前的模式是啟動時設定的、從畫面上讀回來的，還是只是
假設的。頁面會把 agent 自己的說法擺在 alc 的層級旁邊
（`auto-edit · Accept edits`），因為只用一個共同標籤會誤導人。

一個被共享的 `alc --codex claude`，跑的是和沒被共享的那一個同一個背景橋接。

### 共享實際上授予了什麼

一個能對 coding agent 輸入的網頁，等於你機器上的遠端程式碼執行，所以值得把模型
講清楚：

- 連結的 fragment（`#k=…`）**就是**憑證。拿到的人就能對 session 輸入。它不會送到
  伺服器、proxy 或存取紀錄 —— 但它在你的剪貼簿裡，把它當密碼看待。
- alc 比對 `Host` 到連接埠、要求 WebSocket 升級帶 `Origin`、以固定時間比對 token。
  這是用來阻擋別的來源網頁透過你的瀏覽器操控你的 agent。
- 瀏覽器走的 HTTP 平面，和真正建立行程的那條通道，是兩個不同的 socket，憑證也不
  同 —— 在 Unix 上，控制通道是放在 `0700` 目錄裡的 `0600` unix socket，所以瀏覽器
  的 token 碰不到 session 的建立。
- 共享的 session 就是螢幕分享。alc 會遮蔽**它自己**放進環境變數的 API key，但 agent
  印出的其他任何東西，觀看者都看得到。
- `alc remote token --rotate` 會讓已發出的連結全部失效。
- alc 不讀取工作目錄裡的任何設定，所以被 commit 進 repo 的檔案永遠無法開啟共享。

`--share` 需要兩端都是真正的終端機，輸入或輸出被重導向時會拒絕，所以像
`alc claude -p … > out.txt` 這種腳本用法行為完全不變。

## 完整設定

| 平台 | 設定目錄 |
| --- | --- |
| Windows | `%APPDATA%\alc` |
| macOS/Linux | `${XDG_CONFIG_HOME:-$HOME/.config}/alc` |

- `config.toml`：provider 中繼資料、模型、預設值、URL 與環境變數名稱。
- `credentials.toml`：本機儲存的 API key。在 Unix 上 alc 會以 `0600` 權限
  寫入；在 Windows 則位於目前使用者的 AppData 底下。
- `remote.toml`：共享設定 —— 開或關、是否預設共享、綁定位址、port 與權限
  上限。`alc config show` 會把 sharing 與 share-by-default 兩個值以註解
  的形式印在輸出的最後。
- `usage.jsonl`：每次啟動一行，Codex 橋接經手的每個回合再一行。`alc usage`
  會彙整它；刪掉就從頭重新計算。

可用 `ALC_CONFIG_DIR` 覆寫目錄位置。常用的腳本化指令：

```sh
alc config init
alc config show
alc config path
alc config upsert codex --kind codex --model gpt-6-sol --effort medium
alc config upsert work --kind openrouter --model anthropic/claude-sonnet-4.6
printf '%s' "$OPENROUTER_API_KEY" | alc config key work --stdin
alc config set-default claude work
alc config remove work
```

TUI 每個畫面底部都會顯示可用按鍵。`Tab`／`Shift+Tab`，或直接按 `1`／`2`／
`3`，可以在標題列列出的三個畫面之間切換 —— Providers、Agent defaults 與
Sharing & remote。在 Codex profile 上，把游標移到 Model 欄位並按 `←`/`→`，
會開啟引導式的 GPT 模型與推理強度選擇畫面，寫入 `alc --codex claude` 的啟
動預設值。

憑證的優先順序、`alc config upsert` 的完整旗標清單，以及 Codex 到 Claude
的設定優先順序，都寫在
[設定指南](https://treeleaves30760.github.io/all-code/configuration)裡。

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

安裝器會把 `alc` 放進 `~/.local/bin`（Windows 為
`%USERPROFILE%\.local\bin`），必要時會把該目錄加入你的 User PATH。macOS／
Linux 請重開終端機，或 `source` 安裝器提示的設定檔；PowerShell 會同時更新
目前的 session 與 User PATH。如果系統不允許修改 PATH，安裝器會明確印出需
要手動加入的目錄。要安裝到其他目錄，請設定 `ALC_INSTALL_DIR`：在 macOS 與
Linux 上，自訂目錄永遠不會自動幫你加進 PATH；在 Windows 上則會像預設目錄
一樣加進你的 User PATH。Windows 安裝器支援 Windows PowerShell 5.1 與
PowerShell 7，包含 64 位元 Windows 上執行的 32 位元 PowerShell。

### 選用的 tmux 補裝

下載 alc、通過 SHA-256 驗證並完成安裝之後，安裝器會用 `tmux -V` 檢查是否為
**3.2 以上**。已有相容版本就不更動；否則會透過現有的系統套件管理器嘗試
安裝或升級 tmux：

- **Windows：**使用 WinGet 的 `arndawg.tmux-windows` 套件，限使用者範圍，
  不強制 CPU 架構。自動流程會接受套件與來源同意，並關閉互動提示。會跳過
  psmux 與非原生移植版；PATH 上第一個原生版若過舊或無法解析，仍會擋住後方新版。
- **macOS：**使用 Homebrew（`brew install tmux`；已安裝則用
  `brew upgrade tmux`）。Homebrew 不會透過 sudo 執行。
- **Linux / WSL：**使用第一個找到的 `apt-get`、`dnf`、`yum`、`pacman`、
  `zypper` 或 `apk`。非 root 使用者會先用 sudo 快取憑證；只有 controlling
  terminal 可用且 stdout 或 stderr 是終端機時，才會在該終端機要求密碼。
  實際套件操作採非互動 sudo，不會讀取管線裡的安裝腳本。pacman 不會單獨
  更新套件索引，避免 partial upgrade。

**只有 `--tmux` 需要 tmux；一般 alc 與普通 `--share` 不需要。**安裝器不會
自動安裝 Homebrew／WinGet、不從原始碼編譯、不移除 psmux，也不修改 tmux 設定。
缺少套件管理器、權限不足、套件／架構不支援、安裝失敗，或新版仍被舊 PATH
項目遮蔽時，都只會警告並提供手動指令，不會讓 alc 安裝失敗。安裝器會重新
檢查版本，不會把套件管理器結束當成可用的保證。

PowerShell 會把新增的 User／Machine PATH 項目附加到目前 session，保留只存在
於 session 的路徑。如果仍找不到 tmux，請重開終端機，檢查 `tmux -V` 與
`alc doctor`。手動備援指令（依平台選一個）：

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

`ALC_NO_TMUX_INSTALL=1` 跳過自動 tmux 補裝；`ALC_NO_PATH_UPDATE=1` 則獨立控制
**alc 安裝器本身**的 PATH 修改，包含 session PATH 刷新。WinGet 本身仍可能
修改永久 PATH。若要同時避免補裝依賴的副作用與安裝器修改 PATH，請**兩個都設**：

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

### 更新

```sh
alc update --check
alc update
```

`alc update` 會挑選符合目前作業系統與 CPU 的發行包、用 Release 公布的
SHA-256 核對壓縮檔、確認包內版本，再替換 `alc`。Linux 與 macOS 會立即完成
替換。Windows 會先把驗證過的檔案放好，等執行中的 `alc.exe` 一結束就接著替
換；稍候再用 `alc --version` 確認。`alc update --force` 可以重新安裝目前的
最新版本。

執行中的 session 會沿用啟動時的那個執行檔，所以更新後請重開所有由 alc 啟
動的 agent，等它的 session 都結束之後再 `alc hub stop`。

## 從原始碼建置

需要 Rust 1.88 以上：

```sh
cargo build --release --locked
```

Codex 橋接是 alc 自己的程式碼（`src/bridge/`），直接編進執行檔裡，所以從
原始碼建置就是完整的建置 —— 什麼都不用再裝，`alc --codex <agent>` 就能
用。發行包也因為同樣的理由只放一個執行檔 `alc`，旁邊附上授權聲明
（`LICENSE`、`THIRD_PARTY.md`、`THIRD_PARTY_LICENSES/`）。

常用的開發檢查：

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## 解除安裝

把 `alc` 從安裝目錄移除，需要的話再刪掉 `alc config path` 顯示的設定目
錄。刪除設定目錄同時會刪掉本機儲存的 API key，且無法復原。

## 授權

`alc` 採用 MIT 授權。隨附的第三方授權聲明請見
[THIRD_PARTY.md](THIRD_PARTY.md)。
