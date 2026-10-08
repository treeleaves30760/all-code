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
裡讀它，不會寫出 `config.toml`。它會留下模型清單快取、純中繼資料用量紀錄，
以及讓背景 session 持續運作的 runtime 與設定檔。

**它要的**是 `codex login` 寫下的 `auth.json`，以及 PATH 上的 `claude` ——
alc 負責啟動 coding agent，本身不附帶它們。**它不要的**是 API key、alc 的
設定檔，或啟動時的 `codex` 執行檔；那個執行檔是用來跑 `codex login` 本身，
以及讓模型清單保持新鮮的。alc 會先檢查登入，再去找 agent，所以兩樣都缺的
人會先被告知 `codex login`：

```text
error: Codex credentials were not found at ~/.codex/auth.json; run `codex login` and retry
error: 'claude' is not installed or not on PATH; install it first, then retry `alc claude`: cannot find binary path
```

**你會得到什麼。** Claude Code 會以 `gpt-6.1-sol`、`low` 強度啟動 ——
如果你以前用過 Codex CLI，就以你自己的 `~/.codex/config.toml` 裡已經寫好的
模型與強度啟動 —— 帶著 Codex 真正的 272k context window，而不是 Claude Code
對它不認得的 model ID 假設的 200k，而且 Codex 提供的每個 GPT 模型都會出現在
它自己的 `/model` 選單裡：

| 模型 | 適合的情境 | Codex 預設強度 |
| --- | --- | --- |
| `gpt-6.1-sol` | GPT-6.1 Sol，最新的主力模型，適合編碼與日常工作，建議從這個開始 | `low` |
| `gpt-6-astra` | GPT-6，適合複雜且吃重的工作 | `medium` |
| `gpt-6-sol` | GPT-6，日常編碼與代理式工作 | `medium` |
| `gpt-5.6-sol` | 上一代，適合複雜的專業工作 | `low` |
| `gpt-5.6-terra` | 上一代，日常編碼的均衡選擇 | `medium` |
| `gpt-5.6-luna` | 上一代，速度快、費用低 | `medium` |
| `gpt-6-luna` | GPT-6，速度快、費用最低，適合快速修正與大量的例行工作 | `medium` |

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

Id 可以只給任何不會有歧義的前綴，就像 git 的短雜湊那樣。`alc sessions` 會列出
legacy 與各保留世代的 owner，以及各自的頁面連結；`attach`、`kill`、`rename`
會找到 session 真正的 owner。前綴有歧義時直接拒絕，不會送到錯的 hub。

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
所以轉接器也必須一樣：每個 runtime 世代各有一個小小的 alc 行程，綁在它會留著
的 loopback port 上，只回答帶著它 token 的請求。需要它的 session 會把它叫起來，
閒著一小時沒事做就會停掉。更新後舊 host 繼續服務既有 session，新啟動則用新世代。

```sh
alc bridge                         # 在不在跑，以及在哪裡
alc --runtime legacy bridge stop   # 明確停掉 legacy owner
```

新設定會釘住 helper 執行檔並傳入 `--runtime` 身分。舊的、未指定 scope 的
`claude-credential` 仍走 legacy。多個 owner 執行中時，`bridge stop` 必須指定
`--runtime <id|legacy>`；停止可能中斷它正在處理的請求，不是更新的必要步驟。

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

**API-key 量測也能跨背景重啟。** 支援的 `alc --openrouter --metrics claude`
啟動，使用同一個持續 host 上的原生協定轉送 route。設定裡放 loopback 端點與 alc
helper，不放金鑰。helper 用 runtime 裡獨立、只有擁有者可讀的 `bridge.observer-key`
驗證 host、只註冊摘要，再回傳綁定固定 route／本次 host instance 的 AEAD 密封
替代憑證 —— 不透過本機 HTTP 傳服務商金鑰。host 只有在派送時才還原上游驗證。
host 重啟後舊替代憑證收到 HTTP 401；請重新執行 helper。只在 shell 裡的 key
要用 `alc config key <profile>` 存起來，之後重啟才能使用。一般直接啟動仍是直接
連線，Claude 原生登入不在觀測範圍內；修改或停用 route 的 provider 端點後須
重新啟動。Codex 協定轉譯請求會自動觀測。[用量與效能](#ttfttps-與估算成本)
說明實際量測的是什麼。

## 任何 provider 都行，不只 Codex

`codex login` 是最短的一條路，不是唯一的一條。八個 agent 中的任何一個都可以
指向 Anthropic、OpenAI API、OpenRouter、本機的 Ollama、llama.cpp 或 vLLM 伺服器、
DeepSeek、Moonshot、Z.ai、MiniMax、Groq、xAI、Google，或任何自訂端點 ——
也可以只替這一次執行換掉，什麼都不用改。

```sh
alc config                 # key 和每個 agent 的預設值都在這裡
alc claude                 # 每個 agent 各用自己設定好的預設值
alc --openrouter codex
alc --deepseek pi
alc --ollama claude
alc --llamacpp claude
alc -p local-vllm opencode
```

`--provider`（或 `-p`）接受 profile 名稱；當某個 kind 只有一個 profile 時，
也可以直接寫 kind。捷徑旗標 `--anthropic`、`--openai`、`--openrouter`、
`--codex`、`--ollama`、`--vllm`、`--llamacpp`、`--deepseek`、`--moonshot`、
`--zai`、`--minimax`、`--groq`、`--xai`、`--google` 效果相同。初始設定內含 Anthropic、
OpenAI、OpenRouter、Codex、Ollama，以及一個預設停用的 vLLM 範本；key 存在
本機或從環境變數讀取，而環境變數的優先權比較高。

這八個 agent 講的模型協定並不完全相同，十五種 provider kind 對外提供的協定
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
| `alc usage [weekly\|monthly\|yearly]` | 帳號／額度、相容帳本與每日 token／成本統計；`--timezone`、`--chart[=PATH]`、`--daily`、`--monthly`、`--offline`、`--pricing-file`、`--json` |
| `alc tps` | 用戶端觀測的 TTFT 與估算／E2E 每秒 token 數；預設最新 20 筆有 timing 的請求，`--include-unmeasured`、`--limit`、`--json` |
| `alc update` | 驗證並啟用不可變的 alc 世代；`--check`、`--force`、`--download-only`、`--from ... --offline`、`--rollback` |
| `alc share <agent>` | 啟動 agent，並把 session 鏡像到網頁 |
| `alc sessions` | 跨世代與 legacy 的共享 session，以及各 owner 的頁面連結 |
| `alc attach <id>` | 把這個終端機接回某個共享的 session |
| `alc rename <id> <name>` | 替頁面上某個 session 的卡片改名 |
| `alc kill <id>` | 停掉一個共享的 session |
| `alc hub` | `status`、`start`、`stop --drain`；多個 owner 時停止須指定 `--runtime <id\|legacy>` |
| `alc bridge` | Claude 的持續 Codex／量測 host；`status`、`stop`；多個 owner 時停止須指定 `--runtime <id\|legacy>` |
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

alc 自己的旗標 —— `--metrics`、`--runtime`、`--share`、`--no-share`、`--bind-lan`、`--name`、
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
Dry-run 不啟動 listener，也不寫入任何東西，包含加了 `--metrics` 的情況。

## 診斷

```sh
alc doctor
```

`alc doctor` 會回報環境與憑證路徑、全部八個 agent 的執行檔、每個 provider
profile 對照全部八個 agent 的結果、解析後的各 agent 預設值、殘留在
`~/.claude/settings.json` 裡被釘住的 GPT 模型、Codex 橋接的模型、推理強度
與 `codex login` 狀態、已啟用的 Ollama、llama.cpp 或 vLLM profile 對照其執行中伺服器的檢查，
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
  PROVIDER  AGENT     LAUNCHES  TURNS   INPUT  CACHED  CACHE %  OUTPUT  LAST
  codex     claude           1      1  20,800  15,400      74%      35  7m ago
  ollama    opencode         1      —       —       —        —       —  12m ago
  source: ~/.config/alc/usage.jsonl — tokens are counted only where alc carries the traffic; an unobserved direct launch counts as a launch alone (opt in with --metrics)

✓ ready
```

這份節錄是保留的 Accounts 與相容帳本畫面；一般 CLI 報告現在會在其後新增
token／成本統計。

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

相容表讀取 `usage.jsonl`：啟動紀錄，加上 Codex 協定轉譯橋接與選擇啟用的直接 API
觀測產生的純中繼資料請求紀錄。`CACHED` 是 `INPUT` 中從提示快取讀取的部分；
`CACHE %` 是這部分 token 的整數比例。未觀測的流量顯示 `—`，不是零。遠端控制
頁面保留 Accounts 與這份帳本；新增的原生歷史／成本與 TPS 報告只在 CLI 提供。

全域 `--provider`／provider 捷徑**只篩選 Accounts**。帳本不受篩選；要篩選有
紀錄的統計，請用 `--filter-profile`。`alc usage --json` 保留既有的
`schema_version: 1`、`accounts` 與 `ledger` 欄位，另加 `statistics`。

### TTFT、TPS 與估算成本

```sh
alc tps                         # 最新 20 筆符合條件且有 timing 的請求
alc tps --include-unmeasured --limit 50 --json
alc usage weekly --offline --timezone Asia/Taipei --chart
alc usage monthly --offline --chart="$HOME/ai-usage-month.png" --json
alc usage yearly --offline
alc usage --offline --daily --since 2026-10-01 --until 2026-10-08
alc usage --offline --monthly --source claude,codex --json
alc usage --offline --source alc --filter-profile work --filter-agent claude
```

**曆法視窗，不是往回滾動的期間。** 位置參數 `weekly`、`monthly`、`yearly` 選取
本週（星期一開始）、本月或本年，保留每日明細；不能與 `--since`、`--until`、
`--daily`、`--monthly` 共用。不指定視窗或日期界線仍查全部歷史；既有的
`--daily`／`--monthly` 旗標只把選取的歷史分組。

兩種查詢都接受 `--source all|alc|claude|codex`（也可用逗號分隔清單）、含起點的
`--since`、不含終點的 `--until`，以及精確的 `--filter-profile`、`--filter-agent`
與 `--filter-model`。`--timezone UTC|local|<IANA>` 預設 UTC，用於只有日期的
`YYYY-MM-DD` 界線、曆法視窗與日期分桶；RFC3339 界線仍是其時區偏移指定的確切
時刻。Usage 預設所有來源；TPS 預設 `alc`，`--limit` 接受 1–10000。
`alc usage --offline` 只讀本機設定、歷史與價格：不讀憑證／鑰匙圈、不查額度、
不更新登入、不連網。`alc tps` 也只讀本機、不讀憑證，不探測執行中的 daemon。

統計主表提供每日總計，使用完整整數、千分位與靠右對齊的數字；模型／來源明細
另列。`--chart[=PATH]` 才會產生離線 PNG，三區分別為日期 token 長條、日期 USD
長條，以及未快取輸入／快取／輸出的 token 圓餅。預設寫到 home 目錄的
`ai-usage.png`。繪圖用 Rust 與內嵌字型，不依賴 Python 或系統字型。圖與報告
使用相同的選取範圍與時區；較長期間的圖會標示較粗的分桶，CLI 仍保留每日明細。
未知或缺日不是零，部分費用是已知小計；來源重疊會停用不安全的合併總計與圓餅。
`--json --chart` 的 stdout 只放 JSON，產物路徑寫到 stderr。

可重複指定的 `--claude-dir /absolute/config-root` 讀底下的 `projects/`；
`--codex-dir /absolute/codex-home` 讀 `sessions/` 與 `archived_sessions/`。
沒明確指定根目錄時，alc 包含 profile 釘住的目錄，加上對應的 shell 目錄或
`~/.claude`／`~/.codex`。這些 JSONL 來源唯讀、只保留中繼資料，每行最多 4 MiB：
不把 prompt、生成輸出、工具內容或金鑰複製進 alc 帳本或匯入快取。過去的原生用量
不會歸到今天的 alc profile。只有完全相同、協定命名空間相符的 ID 才能證實重複；
無法驗證的重疊保留各來源小計，**不提供可相加的總計**。先整併與去重，再篩選
日期；之後分別檢查選取範圍與每日的重疊安全性。Codex 的累計差值與 checkpoint
不等於請求次數；checkpoint 不會累加為 token 用量。跨日期的差值歸到較晚的
checkpoint 日期，不代表還原了每天實際發生的流量。涵蓋診斷會揭露
跳過或有歧義的紀錄，不把它們當成零。新統計把舊 v1/v2 turn 每個為零的輸入／
輸出計數分別保留為未知；正值與相容帳本總數維持不變。

**新直接請求要選擇啟用。** Codex 協定轉譯橋接會自動觀測。支援的直接 API 啟動
使用 `alc --metrics <agent>`；不加時，行為維持不變：

```sh
alc --openrouter --metrics claude
alc --metrics --provider openai codex --config model_providers.alc_openai.supports_websockets=false
alc --openrouter --metrics --dry-run claude
```

觀測使用支援的、由 alc 管理的端點設定位置，保留 SDK、協定、模型與**上游驗證**；
持續 Claude 量測在本機使用上述密封 helper 憑證。Codex CLI 只觀測 HTTP，需你
**明確**傳入
`--config model_providers.alc_<profile-normalized>.supports_websockets=false`
（profile 的連字號改成底線）；沒帶或為 true 就拒絕。alc 不會強制停用 WebSocket，
也不宣稱能觀測它；一般沒加 metrics 的 WebSocket 行為不受影響。OpenCode／Copilot、
Qwen／Goose 支援的 Anthropic/OpenAI 分支，以及 Kimi 產生的暫存設定有支援的設定
位置。直接 Pi、原生 OAuth／登入、Qwen Google/Gemini、Goose 原生 OpenRouter/Ollama
或含 query／fragment 的 OpenAI 分拆端點，以及超出支援位置的明確端點／helper／
config／provider 覆寫都不被觀測；明確要求不安全／不支援的 `--metrics` 會被拒絕，
不會偷偷改道。一般沒加 metrics 的啟動不變。Dry-run 不啟動 listener，也不寫入
任何東西。

持續 Claude 的握手／控制 challenge 驗證 host，不公開 `bridge.observer-key`。
新啟動的量測請求要求 `request-metrics-v3`，持續轉送要求 `forward-observer-v2`，
使用自己世代的 host，不必停掉舊 host。舊 host 繼續服務舊 session；版本字串相同
不代表能力相符。持續 Claude 觀測檔案不寫入明文服務商金鑰。**本機資料平面仍是
loopback HTTP，不是 TLS**：需信任本機行程。Claude 替代憑證避免原始服務商金鑰外洩，不能避免
被劫持的本機 port 攔截明文請求內容；其他 agent 的短命 route 不保證密封憑證。

**時間數字代表什麼。** TTFT 是請求開始到第一份非空的生成文字／thinking／工具
參數的時間，不是 headers、role、usage 或 ping；隱藏推理的橋接會等可見內容。串流 TPS 只在
輸出／推理基準已知時估算 `(N - 1) / (terminal - first matching content)`。
`BASIS` 顯示 `gross`、`non-reasoning` 或 `unknown`。
E2E TPS 是總輸出除以請求開始到終止的時間，包含排隊／網路／推理 —— **不是
伺服器解碼速度**。非串流 TTFT 與舊帳本／原生歷史時間是 `N/A`。摘要提供有效
樣本數與按時間加權的 TPS，不是把同時執行的請求速度相加。

TPS 在**排序與套用 limit 之前**，先選出帶有 timing 物件的實際 `Request` 紀錄。
已觀測的失敗、取消與沒有 usage 的請求仍列出；每種量測各自需要有效證據。
涵蓋計數說明被排除的 legacy、未量測與非請求紀錄；`--include-unmeasured`
恢復歷史檢視，不可用的量測仍保留未知。舊報告全部 `N/A`，可能只是重用舊橋接
產生的 v1/v2 turn，不是效能為零。alc 無法還原過去的 TTFT/TPS；在新世代開新
session 才能取得後續量測，不必停止舊 session。

**美元代表什麼。** USD API-token／API-equivalent 快照估算，不是發票或訂閱
帳單；不含稅、折扣、工具與非 token 費用。缺少計數或精確費率時，保留未知／部分
成本、原因與可為 `null` 的總額。每筆先依自己的 tier、context 與快取 TTL 計價，
再相加。JSON 保留總輸入 `input_tokens`，新增 `uncached_input_tokens` 與
`cost_components`，分開未快取輸入、快取讀取、快取寫入與輸出。金額用精確的
pico-dollar 運算與十進位 USD 字串，不用浮點總數。推理是輸出子集，不重複計費；
快取讀寫與 TTL 保留各自語意。價格是日期為 2026-10-08 的離線精選 LiteLLM 子集，固定在 commit
`33d908e0ae2c0a257eeb5d546df08527d348a670` 並附 SHA-256／MIT 來源，不是即時價格。
精確的本機／自訂費率放在獨立的設定目錄 `pricing.toml`，或用
`alc usage --pricing-file PATH`；找不到價格不等於免費，免費費率要明寫 `"0"`
字串。歷史用量依具名快照重新定價。見[價格 sidecar
格式](https://treeleaves30760.github.io/all-code/zh-TW/configuration#價格-sidecar)。

完整量測、涵蓋、成本與 JSON 參考，請見
[用量](https://treeleaves30760.github.io/all-code/zh-TW/usage)。

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
| `vllm` | `http://localhost:8000/v1` | — | responses, chat (+ anthropic) | 可 |
| `llamacpp` | `http://localhost:8080/v1` | `LLAMA_API_KEY` | anthropic, responses, chat | 可 |
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
這正是這四個 kind 不需要額外設定就「Claude 可用」的原因。`ollama`、`vllm`、
`llamacpp` 則是本機伺服器：它們都在根路徑上提供 Anthropic Messages，和 `/v1`
底下的 OpenAI 路由並列，所以不論 profile 替其他 agent 指定哪一種協定，Claude
Code 都能跑（見[本機模型](#本機模型)）。預設值只是
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

alc 追蹤七個 Codex 模型 —— 就是上面表格裡的那七個 —— 並從你的 ChatGPT 帳號
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
[GPT-6.1 Sol 說明](https://developers.openai.com/api/docs/models/gpt-6.1-sol)與
[Luna 說明](https://developers.openai.com/api/docs/models/gpt-6-luna)。

**挑選預設值。** 只有在沒有選定模型與強度時，才會以 GPT-6.1 Sol、`low` 作為
起點；你已儲存的選擇維持原樣。

```sh
alc --codex claude --model gpt-6.1-sol --effort low --save
```

`--save` 會把兩者都存進選定的 alc provider。沒有這些參數時，session 的起始值
依序取自 alc provider、選定的 Codex profile、模型自己文件上的預設值。放在 `--`
之後的 `--model` 或 `--effort` 會原樣交給 Claude Code，並蓋過 alc 原本要注入的
值；你自己傳的 `--settings` 則會被合併進 alc 那一份，衝突時以你的為準，因為
Claude Code 只讀一份。用 `/model` 選的模型只影響那一次 session；下次啟動又會從
alc 的預設值開始，所以 `alc config` 仍然是唯一的真實來源。

**Auto 模式與快取。** 不用額外設定或旗標。Auto 模式仍然可用，但分類器請求會
使用 Codex 額度。alc 也會自動讓同一個 session 保持穩定的快取路由；`alc usage`
會顯示 Codex 實際重用了多少輸入。快取重用是盡力而為，不保證節省額度或費用。
詳情請見 [透過 Claude Code 使用 Codex](https://treeleaves30760.github.io/all-code/zh-TW/codex-to-claude)。
你自己的 `--settings` 仍會優先於 alc 的 Claude Code 預設值。

**選單。** alc 會透過 Claude Code 的
[`modelPicker`](https://code.claude.com/docs/en/settings-reference#modelpicker)
設定傳入模型清單，這個設定自 Claude Code 2.1.243 起提供。選單只會顯示這些 GPT
模型與 Default 一列，因為 Claude 自家的模型無法經由轉接器服務；舊版的 client
會忽略這個設定，仍可拿到啟動時的預設模型作為可選項目。Claude Code 的內建別名
也一併留在 Codex 上：Default 一列跟著 alc 的預設值，`haiku` 與背景工作使用
GPT-6 Luna，`sonnet` 跟著這次 session 的起始模型，`opus` 與 `fable` 則使用清單
第一個模型 GPT-6.1 Sol。

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
裡。新版 alc-managed runtime 會用正規 auth 路徑的跨行程鎖協調更新，鎖定後再讀
一次。舊 host 與外部 Codex CLI 不使用這把鎖；共用登入的 token 輪替仍可能影響它們。

## 本機模型

`alc --ollama claude`、`alc --llamacpp claude` 和 `alc --vllm claude` 會把
Claude Code 指向本機伺服器的 Anthropic Messages 端點 —— 也就是它的根路徑，和
`/v1` 底下的 OpenAI 路由並列。本機伺服器只提供它載入的模型，一次只回答一個
（或少數幾個）請求，所以 alc 對這種 session 的設定和雲端 provider 不同：每一個
模型別名（`ANTHROPIC_DEFAULT_MODEL` 以及 sonnet／opus／haiku 三層）都釘在
profile 的模型上，這樣 Claude Code 永遠不會向伺服器要它沒有的 model ID；關掉
非必要的附帶流量；從伺服器讀出真正的 context window；第一個 token 的逾時拉長到
三十分鐘。

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

### llama.cpp 與 vLLM

```sh
llama-server -m Qwen3.8-27B-UD-Q4_K_XL.gguf --alias qwen3.8-27b --jinja -c 131072 --api-key "$LLAMA_API_KEY"

alc config upsert box --kind llamacpp --base-url http://127.0.0.1:8080/v1 --model qwen3.8-27b
printf '%s' "$LLAMA_API_KEY" | alc config key box --stdin
alc -p box claude
```

profile 保留以 `/v1` 結尾的 OpenAI 風格 URL，其他每個 agent 都用它，Claude Code
拿到的則是根路徑。替 profile 存的 key —— 或是 llama-server 自己讀的
`LLAMA_API_KEY` —— 會送給每一個 agent，Claude Code 則透過它的 `apiKeyHelper`
取得，絕不寫進檔案。`vllm` profile 搭配 vLLM 的 `--api-key` 也是一樣；在這個
kind 出現之前、有人指向 llama-server 的 `vllm` profile 同樣能用。

有兩個設定是這類伺服器專屬的。兩者都用模型自己的 Jinja chat template 來組
prompt，而其中不少 —— Qwen 的就是 —— 只接受出現在最前面的 system 訊息；
Claude Code 碰到它不認得的模型時，卻會在對話中段送出一則 system 訊息。alc 會設
`CLAUDE_CODE_MODEL_CAPABILITIES`，讓那段內容改放在第一則 user 訊息裡。另外，
llama-server 一收到請求就回 header，接著在讀完整個 prompt 之前什麼都不送 ——
長的 prompt 要好幾分鐘 —— 而 Claude Code 的串流 watchdog 五分鐘後就會中斷；
所以每個本機伺服器的 `CLAUDE_STREAM_IDLE_TIMEOUT_MS` 都會拉到上限的三十分鐘。

context 來自 llama.cpp 的 `/props` —— 每個 slot 的 `n_ctx`，也就是伺服器同時跑
好幾個 slot 時單一請求能用的量 —— 或是 vLLM 的 `max_model_len`。`alc doctor`
會印出 **llama.cpp and vLLM** 區塊：伺服器與它的 build、是否接受 key、有沒有列出
這個模型、那個 context，以及 `/v1/messages` 是否存在。

## 遠端控制

[從手機操作](#從手機操作)是短版，這裡是其餘的部分。

```sh
alc sessions                 # 跨 owner 的 session 與各自的頁面連結
alc attach 7QK2              # 在所有 owner 間都沒有歧義的 id 前綴
alc rename 7QK2 review
alc kill 7QK2
alc hub status               # 或直接 `alc hub`
alc --runtime legacy hub stop --drain # 明確停掉該 owner 與其 session
alc remote url               # 選定 runtime 的連結
alc remote status            # 開/關、綁定方式、上限、檔案位置
alc remote auto-share on     # 每個 session 都共享，不必加 --share
alc remote off               # 完全禁止共享
alc remote token --rotate    # 讓選定 runtime 的連結失效
```

每個 runtime 世代各有一個背景 hub 擁有 session，所以它們活得比啟動它們的
終端機久；`ctrl-\` 然後 `d` 卸離。各 owner 的網頁只列自己的 session，
`alc sessions` 則跨 owner 彙整，包含 legacy。多個 owner 執行中時，`hub stop`
或 `bridge stop` 必須明確指定全域 `--runtime <id|legacy>`。`hub stop --drain`
會結束該 owner 的 session；兩種停止都不是零中斷，也不是更新的必要步驟。
`remote.toml` 的共享政策仍由各世代共用，也能在 `alc config` 的
**Sharing & remote** 畫面設定。

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
alc --share --bind-lan claude          # 印出 owner 的 LAN 頁面連結

# Tailscale —— alc 只綁 loopback，走 HTTPS，中間沒有第三方
alc remote allow-host box.tail1a2b.ts.net
tailscale serve 8787

# Cloudflare Tunnel —— 行動網路也能連，不需要 VPN
alc remote allow-host '*.trycloudflare.com'
cloudflared tunnel --url http://127.0.0.1:8787
```

alc 只回應你允許過的名字。Loopback 永遠在清單上，`--bind-lan` 會加上這台機器自己的
位址；隧道的主機名用 `alc remote allow-host` 加，可以精確指定，也可以用
`*.example.com` 涵蓋每次都改名的隧道。既有 hub 保留啟動時的 allowlist；等它的
session 結束後，再明確停止／重啟該 owner 才會讀到新名字。Drain 會結束那些
session，不是更新步驟。多個 owner 時，隧道請用目標 owner 連結中的 port，不一定
是 8787。LAN 連結是純 HTTP，token 會以明文經過你的區域網路 —— 在家裡沒問題，
在咖啡廳請改用隧道。

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
- `alc remote token --rotate` 會讓選定 runtime 的連結失效。
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
- `usage.jsonl`：啟動紀錄與 Codex 協定轉譯橋接、選擇啟用的 `--metrics` 觀測
  產生的純中繼資料請求。`alc usage`／`alc tps` 會讀取它；刪除只會重設 alc
  紀錄，不重設 Claude／Codex 原生歷史。
- Legacy 的 `run/`、新世代的 `run/g/<shortid>/`：host socket、port、token
  與固定 route。provider 設定、憑證與 `usage.jsonl` 仍共用真實設定目錄；
  `remote.toml` 仍是共用政策，不是各世代獨立副本。
- Runtime 裡的 `bridge.observer-key`：獨立、只有擁有者可讀的本機觀測 secret
  （Unix 上為 `0600`），不是 API key。驗證 host／控制 challenge，並以 AEAD
  把持續 Claude 量測憑證密封綁定到固定 route／本次 host instance；握手不公開它，
  也不送往上游。它不提供 TLS 或本機請求內容的機密性。
- `pricing.toml`：選用的精確 token 費率覆寫，獨立於 `config.toml`：
  `version = 1`、`currency = "USD"`、`units = "USD-per-million-tokens"`，
  加上必填 `provider`／`model` 的 `[[models]]`。選用條件是 `profile`、
  `endpoint`、`tier`、`context_min_tokens`、`context_max_tokens` 與精確的
  `aliases`。費率是十進位字串 `input`、`output`、`cache_read`，以及
  `cache_write` 或 `cache_write_5m`／`cache_write_1h`，單位為每百萬 token 的
  USD。選到的覆寫取代內建範圍；省略費率仍是未知。見[完整
  格式](https://treeleaves30760.github.io/all-code/zh-TW/configuration#價格-sidecar)。

可用 `ALC_CONFIG_DIR` 覆寫目錄位置。常用的腳本化指令：

```sh
alc config init
alc config show
alc config path
alc config upsert codex --kind codex --model gpt-6.1-sol --effort low
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

驗證過的 payload 透過 alc 內部安裝交易發布。`alc` 仍是完整執行檔的穩定入口，
不是另外一個 launcher 執行檔；旁邊的 `.alc/active.json` 選定
`.alc/generations/<digest>/alc`（Windows 為 `alc.exe`）。完成首次遷移後，
啟用只切換 manifest，不替換 Windows 鎖住的入口。請保留旁邊的 `.alc` 目錄。

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
alc update --download-only "$HOME/alc-bundle"
alc update --from "$HOME/alc-bundle" --offline
alc update --rollback previous --offline
```

`alc update` 會挑選符合目前作業系統與 CPU 的發行包、核對公開 SHA-256 與包內
執行檔版本、發布不可變世代，再以原子方式啟用。穩定入口讀取旁邊的
`.alc/active.json`；新呼叫進入 `.alc/generations/<digest>/alc[.exe]`。已在某個
世代的行程留在原世代，helper／daemon 執行檔則以雜湊釘住。既有 agent、host、
route 與 session 不會被重啟或 drain。新 session 使用自己世代的 hub／橋接，
不需要一律停止再重啟。舊世代保留，不自動回收。

- `--check` 在線上檢查但不套用；`--force` 即使目前版本相同也重新安裝選定發行包。
- `--download-only BUNDLE_DIR` 保存已驗證的發行 bundle，不套用；最新版本已安裝
  時也一樣會下載。目標目錄須為新建或空目錄；各 bundle 分開保存。
- `--from BUNDLE_DIR --offline` 從本機套用，**不查 GitHub、不連網**。啟用前以
  有界驗證核對 bundle 中繼資料、壓縮檔檢查碼、平台與包內執行檔版本；請一起保留
  完整 bundle。
- `--rollback previous` 或 `--rollback <digest>` 啟用保留的世代；不回復設定、
  憑證或共用用量帳本。

Runtime 狀態與這些真實共用檔案分開：新 host 用 `<config>/run/g/<shortid>`，
legacy host 保留 `<config>/run`。`alc sessions` 能找到兩者，各 owner 各有頁面
連結。要明確停掉某個 owner，請用全域 `--runtime <id|legacy>`；停止可能中斷
長請求，hub drain 會結束它的 session。

**範圍與遷移限制。** `alc --codex update` 仍然更新 **alc**，不是 Codex CLI。
Self-update 不會更新 Claude Code、其他 agent、tmux 或 PATH。共用憑證仍會輪替，
舊 host 與外部 Codex 不使用新版的更新鎖；外部套件更新或驗證憑證輪替，不能保證
對執行中的工作毫無影響。

舊 2.0.0 updater 無法被它下載的 payload 追溯修復，尤其是 Windows 的退出後
finalizer。首次遷移到穩定入口請用 2.0.1 以上的安裝器。Unix 可以一次原子替換舊入口；
Windows 若鎖住入口，遷移會明確失敗，保留驗證過的 payload 供重試，不啟用、不殺
行程，也不排程新的 finalizer。等舊行程自然結束後重試，或用另一個
`ALC_INSTALL_DIR` 並排安裝，明確呼叫那個路徑。待處理的遷移不等於更新完成。

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

等使用保留世代的 session 與 helper 都結束後，再移除 `alc` 及旁邊的 `.alc`
安裝目錄。需要的話再刪掉 `alc config path` 顯示的設定目錄。刪除設定目錄同時
會刪掉本機儲存的 API key，且無法復原。

## 授權

`alc` 採用 MIT 授權。隨附的第三方授權聲明請見
[THIRD_PARTY.md](THIRD_PARTY.md)。
