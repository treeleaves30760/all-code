---
id: usage
title: 用量
sidebar_label: 用量
sidebar_position: 6
description: 查詢 Claude 與 Codex 額度、查看用戶端觀測的 TTFT 與每秒 token 數，並用內建與 LiteLLM 價格表估算本機歷史紀錄的 token 成本。
keywords:
  - alc usage
  - alc tps
  - TTFT
  - tokens per second
  - estimated token cost
  - claude code usage
  - codex quota
  - chatgpt plan limit
  - openrouter credits
  - multiple accounts
  - 中文
---

# 用量

每個登入還剩多少、哪個 agent 用掉了 token，以及新請求的效能如何。

```sh
alc usage                   # 帳號額度、相容帳本、每日 token／成本統計
alc usage --offline         # 只讀本機統計；不讀憑證、不連網
alc usage weekly --offline --timezone Asia/Taipei --chart
alc usage yearly --wrapped  # 一張涵蓋所有 agent 與 provider 的分享圖
alc tps                     # 最新 20 筆符合條件且有 timing 的請求
```

一般用量報告依序顯示 **Accounts**、**Usage by provider and agent**，接著是
**Token usage** 與 **By model**：

```text
Accounts
     PROFILE     ACCOUNT          PLAN  LEFT        REMAINING
  ✓  anthropic   ~/.claude        max   ▰▰▰▰▰▰▰▰▱▱  5h 97% left, resets in 2h 53m — week 79% left, resets in 6d 4h
  ✓  codex       you@example.com  pro   ▰▰▰▰▰▰▰▱▱▱  week 66% left, resets in 5d 9h — no credits
  ·  ollama      —                —     —           no quota API
  ·  openrouter  —                —     —           no API key; run `alc config key openrouter`

Usage by provider and agent
  ╭──────────┬──────────┬──────────┬───────┬────────┬────────┬─────────┬────────┬─────────╮
  │ Provider │ Agent    │ Launches │ Turns │  Input │ Cached │ Cache % │ Output │ Last    │
  ├──────────┼──────────┼──────────┼───────┼────────┼────────┼─────────┼────────┼─────────┤
  │ codex    │ claude   │        1 │     1 │ 20,800 │ 15,400 │     74% │     35 │ 7m ago  │
  │ ollama   │ opencode │        1 │     — │      — │      — │       — │      — │ 12m ago │
  ╰──────────┴──────────┴──────────┴───────┴────────┴────────┴─────────┴────────┴─────────╯
  ~/.config/alc/usage.jsonl — tokens count only where alc carries the traffic (opt in with --metrics)

✓ ready

Token usage
  since 2026-10-05 · UTC · 3,412 records · prices alc-curated-2026-10-08-litellm-33d908e0
  ╭────────────┬───────────────┬──────────────────────┬───────────┬─────────┬─────────────┬────────────┬──────────────┬─────────╮
  │ Date       │ Agents        │ Models               │     Input │  Output │ Cache write │ Cache read │ Total tokens │    Cost │
  ├────────────┼───────────────┼──────────────────────┼───────────┼─────────┼─────────────┼────────────┼──────────────┼─────────┤
  │ 2026-10-05 │ claude, codex │ gpt-5.2-codex,       │ 1,204,330 │  88,410 │     310,201 │ 21,733,090 │   23,336,031 │  $19.42 │
  │            │               │ sonnet-4-6           │           │         │             │            │              │         │
  │ 2026-10-06 │ claude        │ sonnet-4-6           │   402,118 │  51,902 │     120,554 │  9,108,422 │   9,682,996+ │  $6.71+ │
  ├────────────┼───────────────┼──────────────────────┼───────────┼─────────┼─────────────┼────────────┼──────────────┼─────────┤
  │ Total      │               │                      │ 1,606,448 │ 140,312 │     430,755 │ 30,841,512 │  33,019,027+ │ $26.13+ │
  ╰────────────┴───────────────┴──────────────────────┴───────────┴─────────┴─────────────┴────────────┴──────────────┴─────────╯
```

每個啟用的 provider profile 在 Accounts 裡各有一列。`REMAINING` 是倒數的：
`63% left` 指的是你還剩多少，不是你花掉了多少。當登入過期或被拒絕，或是連不上
服務商、服務商回了錯誤時，結束碼是 1；其餘情況都是 0，所以單純把方案用完並不會
讓腳本失敗。查詢參數無效、價格檔無法讀取等指令錯誤仍會失敗；成本未知不等於
額度查詢失敗。

`alc --provider codex-work usage` 或 `alc --codex usage` **只篩選 Accounts**，
不篩選相容帳本或統計。要篩選有紀錄的統計，請用 `--filter-profile codex-work`。
`alc usage --json` 保留既有的 `schema_version: 1`、`accounts` 與 `ledger` 欄位，
另加一個 `statistics` 物件；見 [JSON 報告](#json-報告)。

## Codex 登入

alc 讀 `codex login` 寫下的 `auth.json`，然後向 chatgpt.com 問這份登入還剩多少。
它不寫回任何東西：OpenAI 的 refresh token 是一次性的，一個只是查狀態的指令若把它
換掉，正在執行的 session 手上那份就會失效。過期的 token 會直接叫你執行
`codex login`，而下一次真正的 session 本來就會替你更新它。

## Claude 登入

alc 讀 Claude Code 自己的登入 —— 先找 macOS 鑰匙圈，再找它設定目錄裡的
`.credentials.json` —— 然後向 api.anthropic.com 問五小時與每週這兩個額度視窗，
方案裡有的話再多問一個各別模型的視窗。只讀，永遠不更新。macOS 可能會問你一次，
要不要允許讀取那個鑰匙圈項目。

改用 API key 的 Anthropic profile 則顯示 `no quota API`：那把金鑰是按 token 計費的，
沒有「剩餘多少」這回事可以回報。

## 同一種的多個登入

兩個 ChatGPT 登入就是兩個 profile。讓每個 profile 指向自己的目錄，之後每一次從那個
profile 啟動都用那個帳號 —— 於是你讀到的那一列，就是你實際花掉的那個帳號：

```sh
CODEX_HOME=~/.codex-work codex login
alc config upsert codex-work --kind codex --codex-home ~/.codex-work
alc --provider codex-work claude
```

Claude Code 也一樣，用它存放登入的那個目錄：

```sh
CLAUDE_CONFIG_DIR=~/.claude-work claude          # 登入一次
alc config upsert anthropic-work --kind anthropic --claude-config-dir ~/.claude-work
```

兩個路徑都必須是絕對路徑。profile 自己的目錄優先於你 shell 裡的 `CODEX_HOME` 或
`CLAUDE_CONFIG_DIR`，所以留在 shell 設定檔裡的一個變數，沒辦法偷偷改掉一個具名
profile 花的是哪個帳號。

`--provider` 與各種捷徑參數會先比對 profile 名稱，再比對種類。所以同時有 `codex`
與 `codex-work` 兩個 profile 時，`alc --codex claude` 會解析到名字就叫 `codex`
的那一個，而不會反問你指的是哪一個 —— 另一個請照上面的寫法指名。只有在沒有任何
profile 叫這個種類的名字、而又有好幾個是同一種時，捷徑才會拒絕替你選。

## 有餘額 API 的 provider

| Kind | 那一列會顯示什麼 |
| --- | --- |
| `openrouter` | 額度上限、已用與剩餘 |
| `deepseek` | 餘額，以這個帳號計費的幣別顯示 |
| `moonshot` | 可用餘額 |
| `minimax` | 每個視窗剩下的請求數 |
| `zai` | token 視窗與點數餘額 |

其餘的 —— OpenAI、Groq、xAI、Google、Ollama、vLLM、llama.cpp、自訂端點 —— 一律回報
`no quota API`，因為沒有一家為 API key 提供這種查詢。這些查詢一定是送到廠商自己的
端點，所以指向 proxy 的 profile 會被回報成沒有額度 API，而不是把金鑰送去一台並非
發出這把金鑰的主機。

## 各 provider 與 agent 的用量

這張相容用的表只讀[設定目錄](./configuration.md)裡的 `usage.jsonl`。啟動時追加
啟動紀錄；[Codex 協定轉譯橋接](./codex-to-claude.md)或選擇啟用的直接 API 觀測器
承載請求時，追加中繼資料紀錄。一般直接啟動仍只計為一次啟動。原生歷史是獨立的
統計來源，不會匯入這份帳本。

`INPUT` 是上游回報的總輸入。`CACHED` 是其中從提示快取讀取的部分。`CACHE %`
是四捨五入後的 `CACHED / INPUT`；它是 token 比例，不是請求命中率。`0` 代表實際
測得零；`—` 代表數值未知或無法計算，包含舊版 hub 產生的快取資料。

JSON 的 `ledger` 各列保留原始且可為 `null` 的 `cached_tokens` 總數。百分比只用於
畫面顯示，不是 JSON 欄位。[透過 Claude Code 使用 Codex](./codex-to-claude.md#auto-模式與提示快取)
說明提示快取可能未命中的情況。

alc 沒有承載的流量，token 欄位顯示 `—`，不是零。例如一般 `alc claude` 走
Anthropic 時是直接連線；加上 `--metrics` 才會在支援的情況下啟用觀測。
刪掉 `usage.jsonl` 只會重設 alc 的紀錄，不會重設獨立的 Claude 或 Codex 歷史。

## TTFT 與每秒 token 數

```sh
alc tps
alc tps --limit 50 --filter-profile work --filter-agent claude
alc tps --since 2026-10-01 --until 2026-10-08 --timezone Asia/Taipei --json
alc tps --include-unmeasured --source all --json
```

`alc tps` 只讀本機紀錄，不讀憑證、不查額度、不連網，也不探測執行中的 daemon。
預設為 `--source alc --limit 20`。查詢篩選後，先選出**帶有 timing 物件的實際
`Request` 紀錄，再按最新排序並套用 limit**，較新的歷史 turn 不會遮住較舊的
量測請求。`--limit` 接受 1–10000。

一次啟動不是效能樣本。只有具備量測能力的轉譯橋接或支援的
`alc --metrics <agent>` 啟動承載請求，才開始量測。涵蓋計數列出被排除的
legacy、未量測與非請求紀錄。`--include-unmeasured` 恢復歷史檢視，包含原生
差值與 checkpoint；不可用的量測仍是 `N/A`，不會變成零。

舊報告全部 `N/A`，可能包含重用舊橋接產生的 v1/v2 turn，當時根本沒記錄請求
時間。沒有歷史 TTFT/TPS 可以還原。新啟動用自己世代的 host，量測請求要求
`request-metrics-v3`；舊 host 繼續服務舊 session。升級後開新 session 才能取得
後續量測，不必停止舊工作。只有版本字串不能證明有量測能力。

這些是**用戶端觀測值**，不是模型伺服器的效能基準：

| 欄位 | 意義 |
| --- | --- |
| `TTFT ms` | 串流請求從開始，到 alc 看見第一份非空的生成文字、公開的 thinking，或工具參數內容。headers、只有 role 或空的 delta、usage、ping、推理摘要與簽章都不會啟動首內容時計。隱藏推理的橋接會等可見內容。 |
| `TPS est.` | 串流估算：`(N - 1) / (terminal - first matching content)`，時間以秒計。`N` 是已知量測 token 範圍內的輸出；若只呈現非推理輸出，就扣除有回報的推理 token。輸出／推理基準未知、沒有終止時間、`N <= 1` 或區間不為正時，顯示 `N/A`。不會把 SSE chunk 當 token 計數。 |
| `BASIS` | 串流分子的範圍：`gross`（回報的輸出）、`non-reasoning`（輸出扣除明確回報的推理），或 `unknown`（無法計算串流 TPS）。不改變使用總輸出的 E2E TPS。 |
| `TPS E2E` | 上游回報的總輸出 token，除以請求開始到終止的秒數。包含排隊、網路、提示處理與推理時間；**不是伺服器解碼速度**。 |

非串流請求在輸出與終止時間已知時可以有 E2E TPS，但 TTFT 與串流 TPS 為 `N/A`。
已觀測的失敗、取消、逾時、截斷串流與沒有 usage 的請求，仍列在預設報告裡。
它們保留各自結果；缺少計數或終止時間，不會被編造成零或成功的量測。

摘要分別列出 TTFT、串流 TPS 與 E2E TPS 的有效樣本數。TTFT 有平均值、p50 與 p95。
加權 TPS 是有效樣本 token 分子的總和，除以對應時間的總和；不是各請求速度的
算術平均，也不是同時執行的請求速度相加。

### 選擇啟用直接 API 觀測

```sh
alc --metrics --provider openai codex --config model_providers.alc_openai.supports_websockets=false
alc --openrouter --metrics claude
alc --openrouter --metrics --dry-run claude
```

`--metrics` 要放在 **agent 名稱之前**。它在 alc 管理的端點設定位置加入受保護的
loopback HTTP 轉送 route，保留 agent 的 SDK、協定、模型、參數與**上游驗證**。
持續 Claude 觀測會把本機 helper 憑證替換為下文說明的密封替代憑證。這不是協定
轉譯。不加時，一般直接啟動的行為不變；Codex 協定轉譯橋接本來就會觀測，不必
另外啟用。

| 直接啟動 | 觀測邊界 |
| --- | --- |
| 使用 alc 管理的 API-key helper 的 Claude Code | Anthropic 相容 HTTP 端點。持續 host／helper 使用綁定 route／instance 的密封本機憑證，保留上游 API-key 驗證；[背景 session](./background-sessions.md#背景-session-的-api-key-量測)可以重新啟動它。 |
| 使用 alc 產生的 API 端點的 Codex CLI | 只觀測 HTTP Responses，且必須明確用 `--config model_providers.alc_<profile-normalized>.supports_websockets=false` 選 HTTP-only。沒帶或為 true 就拒絕。WebSocket 流量不被觀測；alc 不會強制停用它。原生 Codex 登入與原生 Ollama 整合不在觀測範圍內。 |
| OpenCode / Copilot CLI | 支援的、由 alc 管理的 Anthropic 或 OpenAI 相容端點設定。 |
| Qwen Code / Goose | 支援的、由 alc 管理的 Anthropic 或 OpenAI 分支。Qwen 的 Google/Gemini 分支與 Goose 的原生 OpenRouter/Ollama 整合不在觀測範圍內。Goose 的 OpenAI 分拆端點若含 query 或 fragment，量測會被拒絕；一般啟動維持不變。 |
| Kimi Code CLI | alc 產生的暫存設定裡的 provider 端點，不是使用者自帶的設定。 |
| Pi | 不支援直接觀測：alc 不會把短命的 listener 寫進持續共用的 `models.json`。 |

Codex 產生的 provider ID 是 `alc_` 加上 alc profile 名稱，連字號改成底線：
`openai-work` profile 要用 `model_providers.alc_openai_work.supports_websockets=false`。
這必須是此次量測啟動明確傳入的 agent 參數；alc 不會讀 Codex 設定檔來推論。
一般沒加 metrics 的 WebSocket 行為不受影響。

原生 OAuth／登入驗證，以及超出支援設定位置的明確端點、helper、config 或
provider 覆寫，都保持原樣。明確要求 `--metrics` 卻無法安全觀測時，alc 會說明
原因並拒絕，不會偷偷切換 provider、驗證或傳輸方式。`--dry-run` 只檢查並描述
計畫，不啟動 listener，也不寫入任何東西。這張表描述的是支援的設定位置，不代表
涵蓋所有 SDK、傳輸方式或服務商自訂的回應擴充。持續 Claude 量測也會拒絕所選
`api_key_env` 為 `ANTHROPIC_API_KEY` 或 `ANTHROPIC_AUTH_TOKEN`、且已匯出非空值的
profile，因為用戶端會繞過密封 helper：請用 `alc config key <profile>` 存金鑰、
取消匯出該變數，再重新啟動；一般沒加 metrics 的驗證行為不變。

**持續 Claude API-key 觀測**的 helper 先用 runtime 裡獨立、只有擁有者可讀的
`bridge.observer-key` 與新的 challenge 驗證 host，再回傳 AEAD 密封替代憑證，
不是服務商金鑰。它綁定固定 route 與本次 host instance；host 只有在派送到固定
上游端點時才還原原始金鑰／header。註冊只保留金鑰摘要，觀測檔案不寫入明文
服務商金鑰。host 重啟後，舊替代憑證收到 HTTP 401；重新執行 helper 即可產生新
憑證。握手／控制 challenge 不會公開 secret。`forward-observer-v2` 檢查要求
實際能力，不是版本字串相同就好。新啟動使用自己世代的 host，不必停舊 host、
中斷它的 session。Runtime 身分與 helper 執行檔都釘住，見[背景 session](./background-sessions.md)。

這不是 TLS，也不保證完整的本機資料平面機密性。請求仍透過 loopback HTTP，仍需
信任本機行程：Claude 替代憑證避免原始服務商金鑰外洩，但遭劫持的本機 port 可以
攔截明文請求內容。不要假設其他 agent 的短命 route 也會密封憑證。

## 本機歷史與 token／成本統計

```sh
alc usage weekly --offline --timezone Asia/Taipei
alc usage monthly --offline --chart
alc usage yearly --offline --json
alc usage --offline --daily --since 2026-10-01 --until 2026-10-08
alc usage --offline --monthly --source claude,codex --json
alc usage --offline --source alc --filter-profile work --filter-model example-model
alc usage --offline --claude-dir "$HOME/.claude-work" --codex-dir "$HOME/.codex-work"
```

`--offline` 只讀本機設定、歷史與價格資料。不讀 API key、登入檔或鑰匙圈，不更新
憑證、不查額度、不下載價格，也不做任何其他網路存取。文字輸出顯示
`Accounts: not fetched (--offline)`；JSON 的 `accounts` 是空陣列，仍保留相容的
`ledger`。

### 曆法視窗與每日總計

位置參數 `weekly`、`monthly`、`yearly` 表示**本週、本月或本年**，不是過去
7／30／365 天。星期一為週起點。各視窗從第一天午夜（包含）到下一個視窗的
第一天午夜（不含），保留每日明細。不能與 `--since`、`--until`、`--daily` 或
`--monthly` 共用。

不指定位置視窗或日期界線，仍讀取全部歷史。既有 `--daily`、`--monthly` 仍是
互斥的分組選項，將明確的日期／來源／模型篩選後的歷史分組；`--monthly`
**不表示**本月。

終端畫面以依終端寬度調整的框線表格呈現：寬度不夠時先讓模型清單換行，再改用
`1.23M` 形式的數字，最後才省略次要欄位；導向檔案或管線時保留完整整數。總和中有
一部分未知時（沒有 token 數的檢查點、無法讀取的計數器、沒有價格的模型），欄位會
顯示已知的部分並標上 `+`，表示至少這麼多，而不是把整天變成 `N/A`。`~` 表示
alc 帳本與原生歷史在那天都看到同一個 agent，可能把同一個請求算兩次；用
`--source` 擇一。`--details` 會附上嚴格的逐來源表，保留 provider 與 granularity
的區別、精確總和或 `N/A`、費用組成、假設與涵蓋缺口。

`--timezone UTC|local|<IANA>` 預設 UTC。只有日期的界線、曆法視窗與日／月桶都用
相同時區。例如 `--since 2026-10-01 --until 2026-10-08 --timezone Asia/Taipei`
包含台北的 10 月 1–7 日。RFC3339 界線是時區偏移指定的確切時刻，不會被選取的
時區重新解讀。

### Wrapped 分享圖

```sh
alc usage --wrapped                 # 全部歷史，寫到 ~/alc-wrapped.png
alc usage yearly --wrapped="$HOME/2026.png"
alc usage --source claude,codex --since 2026-01-01 --wrapped
```

`--wrapped[=PATH]` 會把選取範圍內所有 agent 與 provider 的用量畫成一張可分享的
PNG：總 token、第一天與最忙的一天、每週節奏、GitHub 風格的活動熱度圖、各 agent
占比、常用模型與 provider、請求數、session 數、活躍天數、最長連續天數、快取命中率、
尖峰時段與估計成本。它取代文字報告，並在 iTerm2、WezTerm、kitty 與 Ghostty 中
直接顯示（tmux 內不顯示）。原生歷史只記錄模型而不記錄路由，所以其 provider 以模型
的開發商標示。數字是上述的已知總和；頁尾會註明何時是下限。

### 離線 PNG 匯出

```sh
alc usage weekly --offline --chart
alc usage monthly --offline --timezone local --chart="$HOME/ai-usage-month.png"
alc usage --offline --source alc --json --chart="$HOME/ai-usage.png" > usage.json
```

`--chart[=PATH]` 是選擇啟用；一般報告不寫圖片。不帶路徑時，寫到實際 home 目錄
的 `ai-usage.png`。明確路徑須用 `=`，上層目錄必須存在，寫入錯誤會明確失敗。
PNG 由 Rust 離線繪製，附帶已授權的內嵌字型，不依賴 Python、fontconfig 或系統
字型設定。不修改帳本、原生歷史或憑證。搭配 `--json` 時，stdout 只放 JSON，
產物路徑寫到 stderr。

三個面板使用報告相同的選取範圍、時區與來源：

- **日期 token 長條：**未快取輸入、快取、輸出互不重疊。總輸入已包含快取，
  不能再與快取堆疊一次。
- **日期 USD 長條：**token 費率估算有獨立刻度，不用 token／USD 雙軸。部分
  費用標示為已知小計；找不到價格不會畫成免費用量。
- **Token 組成圓餅：**未快取輸入、快取（讀取加寫入）、輸出。讀寫計數與費用
  在表格及 JSON 仍分開。

來源重疊時，不安全的合併桶／總計與圓餅保留不可用；安全的每日／來源明細仍保留。
缺日或缺少計數保留為缺口，不是零。空資料、全零或無法量測的組成，顯示無資料訊息，不畫
沒有意義的扇形。較長期間會標示按週、月或年彙整的圖桶，維持可讀性；CLI／JSON
每日明細仍精確保留。只要選取來源可能重疊，較粗的合併長條就保守停用；
各日安全不代表跨日期的來源也能證實互不重疊。

### 共用查詢選項

這些選項用於 `alc usage` 的統計與 `alc tps`，不影響 Accounts 或相容帳本：

| 選項 | 意義 |
| --- | --- |
| `--source all\|alc\|claude\|codex` | 接受逗號分隔的來源，例如 `--source alc,claude`。Usage 預設 `all`；TPS 預設 `alc`。 |
| `--since DATE` | 含起點。`YYYY-MM-DD` 表示 `--timezone` 的午夜；RFC3339 保留時區偏移指定的時刻。 |
| `--until DATE` | 不含終點，格式相同。要包含選取時區的 10 月 7 日整天，用 `--until 2026-10-08`。 |
| `--timezone ZONE` | `UTC`（預設）、`local` 或 `Asia/Taipei` 等 IANA 名稱；日期界線、視窗與分桶共用。 |
| `--filter-profile PROFILE` | 精確比對有紀錄的 alc profile。沒有 profile 的原生紀錄不會符合。 |
| `--filter-agent AGENT` | 有紀錄的 coding agent：`claude`、`codex`、`opencode`、`pi`、`copilot`、`goose`、`qwen` 或 `kimi`。 |
| `--filter-model MODEL` | 精確比對回報的 model ID，不模糊搜尋別名。 |
| `--claude-dir PATH` | 可重複指定的絕對 Claude **設定根目錄**；讀取底下的 `projects/`。 |
| `--codex-dir PATH` | 可重複指定的絕對 Codex **home**；讀取底下的 `sessions/` 與 `archived_sessions/`。 |

明確指定根目錄清單，就會取代該原生來源的自動根目錄。否則 alc 會包含 provider
profile 釘住的目錄，再加上已設定的 `CLAUDE_CONFIG_DIR` 或 `CODEX_HOME`，沒有時
用對應的 `~/.claude` / `~/.codex`。根目錄會去重。它們只決定去哪裡讀，不會把
過去的原生用量歸到現在的 profile 或帳號。

### 涵蓋範圍與去重

原生讀取器是唯讀的，只保留用量與識別中繼資料。不會把 prompt、生成輸出、工具
內容或金鑰複製進 alc 帳本或匯入快取。JSONL 檢查每行最多 4 MiB；過大、格式錯誤、
不支援或有歧義的紀錄，會透過來源涵蓋診斷回報，不會被當成零用量。

Claude assistant 快照依穩定的 message／request 識別整併，不用逐區塊的 transcript
UUID。有穩定 response ID 的 Codex 紀錄可以識別請求。較舊的累計 token 計數只轉成
有根據的差值；第一個非零基準、計數重設、人為的 context-window checkpoint，
或無法分配的模型變更，仍保留為 checkpoint。**累計差值與 checkpoint 不等於
請求次數**；checkpoint 不會累加為 token 用量，兩者也不會編造 TTFT/TPS。
要查看這些時間為 `N/A` 的列，請用 `alc tps --source codex --include-unmeasured`。
跨多日的累計差值，會依選取時區歸到**較晚 checkpoint 的日期**，無法還原原本
每天實際發生的流量。

跨來源只在同一個 agent 的 request／message／response ID 完全相同、且協定命名
空間相符時，才認定重複；證實相符後優先採 alc 紀錄。相近的時間、token 總數或
session 名稱不算證據。無法驗證的重疊會保留、標記 `possible_overlap`，並顯示
各來源小計，**不提供可相加的總計**。原生累計解析與精確 ID 去重先於篩選；
之後重新檢查選取範圍的重疊，也獨立檢查各每日總計。來源警告或跳過紀錄，
同樣表示整體涵蓋不完整。對舊 v1/v2 alc turn 紀錄，新統計會把
每個為零的輸入／輸出計數分別保留為未知，因為原格式沒有欄位是否存在的證據；
正值仍會保留。相容帳本原有的總數不變。

### 一個估算美元代表什麼

成本是依具名價格快照計算的 **USD token 費率估算**，不是發票、訂閱帳單、額度扣款，
也不能證明實際花費。原生與訂閱流量有精確參考價格時，使用 API-equivalent
參考估算。原生中繼資料不會被重新標成今天的 alc profile；參考價格也不會改掉
仍然未知的 provider 歸屬。

輸入、快取讀取、快取寫入與輸出是分開的成本項目。既有 JSON `input_tokens`
仍是**總輸入**；新增 `uncached_input_tokens` 是繪圖使用、互不重疊的剩餘輸入。
OpenAI 格式的 input 已包含快取子集；Anthropic 格式的 input 是未快取的剩餘
輸入，因此總輸入要加上讀取與寫入。推理是輸出的子集，不是額外計費的 token。
快取寫入的 TTL 分桶取代總寫入計數，不再加一次。5 分鐘與 1 小時費率不同時，
不會猜未知的 TTL 分配。每筆先依自己的 tier、context 與 TTL 計價，再以溢位檢查
相加；不把總 token 乘上任意單一費率。金額運算用精確整數 pico-dollar
（10^-12 USD），不用浮點數。

缺少計數或適用的精確費率、有紀錄的服務層級沒有費率，或分級費率缺少逐請求
context 資料時，會產生未知／部分成本並列出原因。未記錄服務層級時假設 standard；
OpenAI 的 `default` 對應 standard。預設畫面以 `+` 標示已知小計（完全沒有價格時顯示
`—`）；`--details` 顯示 `N/A` 或已知小計加 `?`；JSON 的 `total_usd` 保留 `null`。
已知的零計數不等於缺少計數，找不到模型價格**不代表免費**。本機／自訂端點需要
精確覆寫，除非精確的官方端點能提供支援的參考；免費參考費率必須明寫 `"0"` 字串。

價格先取離線、精選的 LiteLLM 子集。內建快照日期為 **2026-10-08**，固定在 LiteLLM
commit `33d908e0ae2c0a257eeb5d546df08527d348a670`，附上游 SHA-256 與 MIT 授權來源。
子集沒有的模型改用 **LiteLLM 公開的價格表**定價，也就是 `npx ccusage` 讀的同一個
檔案。alc 每天最多下載一次到設定目錄的 `litellm-prices.json`，而且只在有紀錄缺價格
時才下載。它只採用 Anthropic 與 OpenAI 的官方列（含服務層級與長上下文區間），且絕不
取代精選或覆寫的費率。`--offline` 只用快取、不下載。此時報告的 `pricing_snapshot`
結尾會是 `+litellm-live-<日期>:sha256:<雜湊>`，這些列的 `price_sources` 種類為
`litellm`；這些費率未經官方驗證。請求的快取計數器未知時，長上下文區間依已知的輸入
下限選擇，因此成本仍標示為下限。歷史用量依這些價格重新定價；不含稅、折扣、訂閱、未記錄的工具或非 token 費用。
精確費率加在設定目錄的 **`pricing.toml`**，或用
**`alc usage --pricing-file PATH`** 指定檔案。[價格 sidecar 參考](./configuration.md#價格-sidecar)
列出格式；它不是主要 `config.toml` 裡的一張 table。

## JSON 報告

- `alc usage --json`：既有頂層的 `schema_version: 1`、`generated_at`、
  `resolved_by`、`accounts` 與 `ledger`，另加 `statistics`（schema version
  也是 1）。既有總輸入 `input_tokens` 語意與十進位 USD 字串不變。新增
  `timezone`、解析後的 `range`、選取的 `window`、`daily_rollups`、
  `uncached_input_tokens`，以及總計、每日與模型／來源層級的 `cost_components`。
  分項為 `uncached_input`、`cache_read`、`cache_write`、`output`，各有可為
  `null` 的 `known_subtotal_usd` 與 `total_usd`。每個層級的嚴格總計旁都有
  `known_tokens`（`uncached_input`、`cache_read`、`cache_write`、`output`、
  `incomplete`）：即終端畫面顯示的已知總和，`incomplete` 為真時是下限。每日總計
  也列出其 `agents` 與 `models`。統計保留 source/profile/provider/
  agent/model 列、`granularity`、可為 `null` 的 token 總數、`records`、可為
  `null` 的 `requests`、`known_requests`、`priced_records`、`unpriced_records`、
  `deduplicated_records`、`known_subtotal_usd`、可為 `null` 的 `total_usd`、
  `possible_overlap`、`pricing_snapshot` 與來源診斷。各列包含 `cost_status`
  （`complete`、`partial` 或 `unknown`）、`partial_records`、
  `reference_providers`、`reference_models`、`price_sources`、`provenance`、
  假設與原因。USD 金額是十進位**字串**，不是浮點 JSON 數值。來源可能重疊時，
  不安全的合併總計／小計與各分項都保留 `null`，安全的各來源列仍保留。
- `alc tps --json`：`schema_version: 1`、`measurement`、`timezone`、`range`、
  `rows`、`summary`、`coverage` 與 `sources`。Coverage 包含 `matching_records`、
  `measured_requests`、`excluded_legacy_records`、`excluded_unmeasured_records`、
  `excluded_nonrequest_records`、`eligible_records`、`returned_records`、
  `limited_records` 與 `include_unmeasured`。排除分類互不重疊，先於排序／limit
  計數。各列包含中繼資料紀錄、`provenance`、以微秒為單位的原始 `timing`
  偏移、token 計數、結果，以及 `metrics`（`ttft_ms`、`stream_tps`、`e2e_tps`、
  `stream_output_basis`）。摘要包含 `records`、`known_requests`、可為 `null` 的
  `requests`、有效樣本數、`ttft_mean_ms`、`ttft_p50_ms`、`ttft_p95_ms`、
  `weighted_stream_tps` 與 `weighted_e2e_tps`；不可用的量測是 `null`。使用
  `--include-unmeasured` 時，選取列若有累計差值或 checkpoint，而非可驗證的 API
  請求，`requests` 就是 `null`。

## 在遠端控制頁面上

[遠端控制頁面](./remote-control.md)標題列的用量按鈕後面，仍是 Accounts 與相容
帳本：每個額度視窗一條量表，接著是那份帳本。面板開著的時候，每分鐘更新一次。
原生歷史統計、成本估算與逐請求 TPS 報告只在 CLI 提供；hub 不會為這個頁面掃描
原生歷史。

只能看、不能輸入的連結看得到數字，但看不到 email、帳號 id 與憑證路徑。頁面是由 hub
提供的，而 hub 不讀任何 shell 變數，也永遠不會去開鑰匙圈 —— 所以在 macOS 上，那裡的
Claude 那一列會請你回到終端機執行 `alc usage`。
