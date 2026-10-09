---
id: troubleshooting
title: 疑難排解
sidebar_label: 疑難排解
sidebar_position: 8
description: alc 會印出哪些錯誤，以及各自要做什麼才會消失 —— 找不到 agent、provider 不相容、Codex 登入、Ollama 逾時，還有留在 Claude Code 設定裡的 GPT 模型。
keywords:
  - alc doctor
  - claude code error
  - codex login
  - ollama timeout
  - model not found
  - 中文
---

# 疑難排解

先跑 `alc doctor`。它會回報每個 agent 的執行檔、憑證狀態、每個 provider profile
以及它各自的 agent 相容性欄位、解析後的預設值，還有 Codex 的登入狀態。

## `'claude' is not installed or not on PATH`

alc 只啟動已經裝好的 agent。請先安裝那個 agent，或用[覆寫變數](./agents.md#執行檔覆寫)把
alc 指向某個執行檔。

## `provider '…' cannot be used with claude; Claude Code needs Anthropic Messages`

那個 profile 講的協定 Claude Code 用不了。請換一個 Anthropic 相容的端點或本機伺服器
（`--kind ollama`、`llamacpp` 或 `vllm`），或改用
[`alc --codex claude`](./codex-to-claude.md)。對照表在 [Provider 相容性](./providers.md)。

## `provider '…' is turned off`

這個 profile 存在，但被停用了 —— 內建的 `vllm` 範本出廠就是這樣。用
`alc config upsert <profile> --enable` 把它打開；如果它還沒有模型，再加上
`--model <id>`。

## `provider '…' has no API key`

用 `alc config key <profile>` 存一把，或設定那個 profile 的 `api_key_env` 指名的
環境變數。

## `Codex credentials were not found`

執行 `codex login`，然後再試一次。[`alc usage`](./usage.md) 會告訴你哪些登入還有效。

## Auto 模式說伺服器端檢查無法使用

升級 alc 並開啟新的 session。不需要額外旗標；已在執行的背景 session 會保留啟動時
的設定。如果訊息仍然出現，請檢查你自己的 `--settings` 是否覆寫了 alc 的預設值。
[Auto 模式與提示快取](./codex-to-claude.md#auto-模式與提示快取)說明這個備援方式：
Auto 模式仍然可用，但分類器請求會使用 Codex 額度。

## `CACHED` 是零或 `—`

`0` 是上游實際回報的數值；`—` 表示 alc 無法判定。快取重用會自動設定，但仍是盡力
而為；可重用的提示前綴改變或到期時，未命中是正常的。詳情請見
[用量](./usage.md#各-provider-與-agent-的用量)與
[Auto 模式與提示快取](./codex-to-claude.md#auto-模式與提示快取)。

## Ollama profile 出現 `API Error: Request timed out`（或 `500`）

模型沒能在 Claude Code 放棄之前，讀完那個 25k 到 40k tokens 的第一個請求。alc 會為
Ollama、llama.cpp 與 vLLM profile 設定 `API_FORCE_IDLE_TIMEOUT=0`、
`API_TIMEOUT_MS=1800000` 與 `CLAUDE_STREAM_IDLE_TIMEOUT_MS=1800000`，讓它
願意等下去；而重試本來就會從 Ollama 的 prompt cache 接續，所以不管走哪一條路，
session 通常在第二次嘗試就會開始。

想讓第一輪不只是撐得過去，而是真的快，請看[本機模型](./local-models.md)。先看
`alc doctor` 的 **Ollama** 區塊：模型必須已經 pull 下來、能呼叫工具，而且 context
至少 64k。

## llama.cpp 或 vLLM 回傳 `500 System message must be at the beginning`

模型的 chat template 拒絕了一則不在最前面的 system 訊息，而 Claude Code 碰到它不
認得的模型就會送出這種訊息。alc 會替 `llamacpp` 與 `vllm` profile 關掉這個行為；
會看到這個錯誤，表示 Claude Code 是用別的方式跑在這種伺服器上，或是跑在 `custom`
profile 上。請改用 `llamacpp` 或 `vllm` profile，或自己設定
`CLAUDE_CODE_MODEL_CAPABILITIES=-mid_conv_system,-mid_conv_tool_change`。
細節見[本機模型](./local-models.md#為什麼多了兩個設定)。

## Ollama 回傳 `404 model 'claude-…' not found`

Claude Code 向伺服器要了一個它自己的 model ID，通常是透過它拿來做背景工作的
`haiku` 別名。alc 會把 Ollama profile 的每個別名都釘在 profile 的模型上；請把那個
profile 的 `small_model` 設成一個你真的 pull 下來的模型。

## 模型清單看起來過期了

模型清單每天向你的 ChatGPT 帳號同步一次，Codex 一升級也會立刻再同步一次。要現在
同步：

```sh
alc models --refresh
```

`alc models` 會說出這份清單是從哪裡來的。出現 `fallback:` 那一行，代表帳號這一
關問不到，改由已安裝的 Codex CLI 回答——舊版 Codex 看得到的模型比你的帳號實際跑
得動的少，所以那一行要先看。標示為來自 `the catalog alc ships` 的模型，是回答的
那一方沒有列出、由 alc 補回去的：alc 內建的清單是底線，所以某一方回答得不完整，
只會讓清單不夠新，不會讓你少一個模型。

## `the model may not exist or you may not have access to it`

有兩種成因。

**留在 Claude Code 設定裡的 GPT 模型。** 一個 session 最後落在哪個模型，就會被寫進
`~/.claude/settings.json`，成為你之後新 session 的預設值，而之後直接跑的 `claude`
前面並沒有轉接器。alc 會在轉接過的 session 結束時把那個欄位放回去，所以你現在還找
得到的，是被直接砍掉的 session 留下的殘留，或是手動設進去的值。

```sh
alc doctor            # names the file and the line when it finds one
alc --codex claude    # clears it on exit; alc passes the model itself
```

**還在跑舊版本的 hub。** hub 的設計本來就是比終端機活得久，所以它也會比一次升級活
得久，而 alc 寧可拒絕，也不會把需要轉接的 session 交給版本不同的 hub。`alc doctor`
會指出落後的那一個：

```sh
alc hub stop
```

## Your apiKeyHelper script is failing

`alc claude-credential` —— 也就是 alc 寫出來的那份設定檔裡指名的那個 helper ——
沒有印出憑證就結束時，Claude Code 就會這樣說。有四件事會擋住它：

- **沒有登入 Codex。** 執行 `codex login`，然後再試一次。
- **一把只有某個 shell 知道的 key。** key 放在環境變數裡的 profile，只有從那個
  shell 啟動的 session 讀得到，別的都讀不到，所以之後才開始跑的背景 session
  問了也拿不到東西。請用 `alc config key <profile>` 把它存起來。
- **橋接起不來。** `alc bridge serve` 會在這個終端機裡直接跑它，並印出它為什麼
  起不來。
- **一條不見了的 route。** 橋接手上那個 Codex profile 的紀錄被刪掉了；跑一次
  `alc --codex claude` 就會重新寫回去。

`alc doctor` 一次就會回報登入狀態、存下來的金鑰與橋接。這個 helper 永遠不會退
回去用你的 Claude 登入：一個由 alc 設定好的 session，要嘛連上你指定的那個
provider，要嘛直接說它做不到。

## 橋接換了位置之後，背景 session 連不上

世代橋接記住的 port 是固定設定／origin 的一部分。Host 關著時若被無關的程式
佔走，alc 會失敗，不輪替 token 或改寫 origin。請解決衝突的 listener，不要只
為了更新停掉另一個執行中的 alc 世代。第一次分配可以用 ephemeral port，
`alc sessions` 會顯示不同 owner 與各自的連結。

Legacy 保留舊的 port 遷移行為；legacy 橋接若改用新 port，只需讓受影響的
legacy session 讀到新位置：

```sh
claude respawn <id>
```

alc 更新本身不搬動舊 session，也不要求重啟它們。

## 我派出的 session 是 Anthropic 在回答，不是 Codex

agent view 屬於你當初從哪一個 Claude Code 打開它。從一個普通的 `claude agents`
派出工作，拿到的就是用你 Anthropic 登入的普通 Claude Code session，不管 alc
在另一個終端機裡做著什麼。請改成透過 alc 打開 agent view：

```sh
alc --codex claude agents
```

那些 session 帶著 alc 寫出來的那份設定檔，而且 Claude Code 每一次重新啟動它們
時都還帶著。其餘的都在[背景 session](./background-sessions.md)。

## 每一次 Codex 或本機伺服器執行都出現的兩則提示

每一次在 Codex profile，或在 Ollama、llama.cpp、vLLM profile 上以 print 模式執行，
都會往 stderr 寫兩行：一行是
`CLAUDE_CODE_DISABLE_1M_CONTEXT is set, but the 200K limit isn't enforced for <model>`，
另一行是 `[claude-code:unrecognized_model]`。兩行都是 Claude Code 在告訴你，它
不認得 alc 交給它的那個 model id —— 而這正是重點：那是一個 Codex 或本機模型。它們是
診斷訊息，不是錯誤；session 是好的。

有兩種記載過的解法，alc 兩種都不用。一筆 `modelOverrides` 設定會讓 Claude Code
在算 context 預算時把那個 Codex id 當成 Claude 模型，反而毀掉 alc 以
`CLAUDE_CODE_MAX_CONTEXT_TOKENS` 傳給它的、真正的 Codex window。
`CLAUDE_CODE_AUTO_COMPACT_WINDOW` 則會把 session 釘在 200K 來讓第一行閉嘴，
然後狀態列上的那個百分比就不再有任何意義。

## `/model` 說 `ANTHROPIC_MODEL` 蓋過我的選擇

在一個 alc 的 session 裡挑模型，會回你兩行，第二行是一個警告：

```text
Set model to GPT-6-Astra and saved as your default for new sessions
ANTHROPIC_MODEL is set to GPT-5.6-Sol — new sessions use that while it is set
```

兩行都是真的。你挑的那個只對你正在用的這個 session 有效，而 alc 會替它啟動的
每一個 session 把模型釘住，所以之後再跑一次 `alc --codex claude`，起點又會是
alc 的模型，而不是你挑的那一個。要改的是 alc 從哪一個模型起跑：

```sh
alc --codex claude --model gpt-6-astra
alc --codex claude --model gpt-6-astra --save
```

## 用腳本盯著一個背景 session

`claude agents --json --all` 同時帶有 `state` 與 `status`，而會告訴你工作做完了
沒有的是 `state`：`working` 會變成 `done`。`status` 只會從 `busy` 變成 `idle`。

一段你用 `←` 送到背景的對話，最後會停在 `state: "blocked"` —— 卡片上寫的是
「Needs input」—— 而不是 `done`，因為它是一個還活著、正在等你下一句話的
session。

## `alc update` 找不到釋出的壓縮檔

1.4.0 之前裝好的版本，會找已不再發布的第二個執行檔。請用 2.0.1 以上的安裝器遷移到
穩定入口；Windows 若鎖住舊執行檔，請延後或並排安裝，不要停止執行中的工作。
見[首次遷移](./getting-started.md#從舊-alc-首次遷移)。

## `alc tps` 沒有量測列，或舊列全部 `N/A`

舊 v1/v2 turn、啟動、原生歷史與累計 checkpoint 沒有實際觀測的請求時間。
它們不是速度為零的請求，也無法事後還原 TTFT/TPS。CLI 更新後，重用的舊橋接
仍可能繼續寫這類 turn。

預設報告在排序／limit 前先選有 timing 的 `Request`，涵蓋計數說明排除的歷史。
要查看它們：

```sh
alc tps --include-unmeasured --source all --json
```

後續量測請在新世代開新 session，host 必須宣告 `request-metrics-v3`。既有舊
host／session 可以繼續跑。即使是有量測的失敗／取消或沒有 usage 的請求，個別
數字仍可能無法計算；見[時間定義](./usage.md#ttft-與每秒-token-數)。

## Windows 遷移是待處理，不是完成

舊 2.0.0 自我更新使用退出後 finalizer；新 payload 不能追溯改變舊 updater。
請用 2.0.1 以上的安裝器建立穩定入口。舊入口被鎖住時會明確失敗，保留已驗證 payload
供重試，不排程新 finalizer、不殺行程，也不切換 active。等舊行程自然結束，
或改用另一個 `ALC_INSTALL_DIR` 並排安裝，明確呼叫該路徑。完成首次建立後，
更新只啟用 manifest，不替換入口。

## 本機更新 bundle 驗證失敗

請完整保留 `alc update --download-only BUNDLE_DIR` 產生的目錄；目標須為新建或
空目錄，各 bundle 分開保存。
`alc update --from BUNDLE_DIR --offline` 不查 GitHub、不連網，啟用前以有界驗證
核對中繼資料、壓縮檔檢查碼、平台與包內執行檔版本。不繞過不符的驗證，也不執行
bundle 裡的腳本；請為此平台取得新的已驗證 bundle。驗證失敗不改 active 世代。
回滾需要保留的世代，不回滾設定／憑證，也不重啟 host。

## 停止 host 或 session 前綴有歧義

`alc sessions` 列出 legacy 與各世代的 owner 及其頁面 URL。請用完整 session ID，
或全域 `--runtime <id|legacy>` 指定目標 owner。多個 owner 執行中時，`hub stop`
與 `bridge stop` 要明確指定目標。停止橋接可能中斷長請求；`hub stop --drain`
會結束它的 session。不要把它們當作更新步驟。

## 在 `alc config` 裡找不到共享設定

它在第三個畫面 —— `Tab`、`Shift+Tab` 或直接按數字鍵，就能在 `1 Providers`、
`2 Agent defaults` 與 `3 Sharing & remote` 之間切換。預設共享、綁定位址與權限上限
都在那裡。

在 TUI 之外，`alc remote auto-share on` 設定的是同一個東西，`alc remote status`
會回報它。某一列顯示 `on (inactive)`，代表共享本身是關的：請執行 `alc remote on`。

## Windows 上的 `--tmux` 找不到 tmux，或找到的版本不對

在 Windows 上，`--tmux` 需要 tmux 的原生 Windows 移植版：

```powershell
winget install arndawg.tmux-windows
```

裝完之後開一個新的終端機，讓它讀到更新後的 PATH。`alc doctor` 的 **tmux** 那一列會告訴你
有沒有找到原生版。

psmux 也會裝一個 `tmux.exe`，但 alc 驅動不了它 —— 它跑不了 alc 用來建立 session 的
那串指令。兩個可以同時裝著：alc 會越過 PATH 上的 psmux，去找原生版。MSYS2、Cygwin 或
WSL 版的 tmux，在 Windows 上一樣不會被拿來用。不想裝的話，拿掉 `--tmux` 就好，
[遠端控制](./remote-control.md)不靠它也能用。

## `alc is installed at …, and tmux for Windows cannot start a program whose path is not plain ASCII`

tmux 的 Windows 移植版用 ANSI 字碼頁傳遞命令列，所以路徑不是純 ASCII 的程式，它根本
啟動不了。alc 裝在這種資料夾底下時，會改用 Windows 的 8.3 短路徑；只有那顆磁碟
關掉了短檔名，`--tmux` 才會拒絕。請把 alc 裝到一個純 ASCII 的路徑底下，或拿掉 `--tmux`。

這裡說的是 alc 自己的安裝路徑：專案資料夾、參數與環境變數含有中文之類的非 ASCII
字元，在 `--tmux` 底下都沒問題。

## 輸出裡的機密資料

`alc --dry-run` 會遮蔽 API key 與 auth token，`alc config show` 只會顯示某個 profile
有沒有金鑰，而 `alc usage` 不管哪一種憑證都不會印出來。
