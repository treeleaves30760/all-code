# all-code (`alc`) 繁體中文快速開始

`alc` 讓你先設定 LLM provider，再用同一套指令啟動 Claude Code、Codex CLI
或 OpenCode。

## 一行安裝

macOS、Linux、WSL：

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

安裝器預設會把安裝目錄加入 PATH。macOS、Linux 與 WSL 請依畫面提示重開
終端機或 `source` 對應的 shell 設定檔；PowerShell 會同時更新目前工作階段與
User PATH。如果系統不允許修改，安裝器會明確顯示需要手動加入的目錄。

接著執行：

```sh
alc config
```

這會開啟全螢幕 TUI，可新增或編輯 Anthropic、OpenAI API、OpenRouter、
Codex、Ollama、vLLM 與自訂 provider，並分別指定 Claude、Codex、OpenCode
的預設 provider。

## 更新 alc

只檢查是否有新版本，或直接更新 `alc` 與隨附的 helper：

```sh
alc update --check
alc update
```

更新時會自動選擇目前作業系統與 CPU 的發行包、核對 GitHub Release 公布的
SHA-256、確認包內版本，再一起替換 `alc` 與 `claude-codex`。Windows 會先完成
下載與驗證，等目前的 `alc.exe` 結束後立刻替換；稍候即可用 `alc --version`
確認。`alc update --force` 可重新安裝目前最新版本。

自訂 `ALC_INSTALL_DIR` 或設定 `ALC_NO_PATH_UPDATE=1` 時，如果目錄不在 PATH，
安裝器會顯示手動設定提示，不會讓使用者安裝完卻找不到 `alc`。

## 啟動方式

使用各 agent 的預設 provider：

```sh
alc claude
alc codex
alc opencode
```

只替這次執行指定 provider：

```sh
alc --codex claude
alc --openrouter codex
alc --provider company-vllm opencode
```

除了 Claude 專用的 `--model`、`--effort`、`--save`、`--no-picker` 之外，agent
名稱後的參數會原封不動傳入：

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
```

如果要把同名參數交給 Claude Code 本身，請放在 `--` 後面，例如
`alc claude -- --model sonnet`。

用 `--dry-run` 預覽啟動方式；API key 會被遮蔽：

```sh
alc --openrouter --dry-run claude
```

## 讓 Claude Code 使用 Codex 登入

先完成 Codex 官方登入流程，再啟動 Claude Code：

```sh
codex login
alc --codex claude
```

在互動式終端機裡，這個指令會先開啟兩步驟選單：

1. 選擇模型：
   - `gpt-5.6-luna`：速度快、費用低，適合簡單修改與大量例行工作。
   - `gpt-5.6-terra`：速度、能力、成本均衡，建議新手從這個開始。
   - `gpt-5.6-sol`：能力最完整，適合架構、困難除錯與大型重構。
2. 選擇推理強度：`low`、`medium`、`high`、`xhigh`、`max`。

推理強度越高，模型會花更多時間與額度思考。一般開發建議先用
`terra + medium`；很簡單的工作可用 `luna + low`，真正困難的問題再提高到
`sol + high/xhigh/max`。

按 Enter 只執行這一次；按 `S` 會儲存成預設值後執行。也可以直接用參數，
很適合腳本或 CI：

```sh
alc --codex claude --model gpt-5.6-luna --effort low
alc --codex claude --model gpt-5.6-terra --effort medium --save
alc --codex claude --no-picker
```

`--save` 會把模型與 effort 存入目前的 Codex provider；`--no-picker` 會直接使用
已儲存的值。非互動環境也會自動使用預設值，不會卡在選單。

模型清單每天最多自動向本機 Codex CLI 同步一次；離線時會使用內建清單：

```sh
alc models
alc models --refresh
alc models --json
```

發行包會附帶固定版本的 MIT 授權 `claude-codex` helper。`alc` 只把它綁在
隨機的 `127.0.0.1` port，Claude Code 結束時就會關閉。Helper 會讀取並可能
更新 `~/.codex/auth.json`；`alc` 不會把 Codex token 複製到自己的設定檔。

這是第三方相容層，不是 OpenAI 或 Anthropic 官方整合。使用訂閱帳號前，請
先檢閱專案的 `THIRD_PARTY.md` 與 provider 條款。

## 完整設定

`alc config` 的 provider 編輯畫面現在可設定 model、reasoning effort、small
model、API URL、Anthropic 相容 URL、protocol、驗證方式、API key 環境變數、
Codex profile 與啟用狀態。方向鍵左右切換選項，Enter 套用，`s` 儲存。

不想開 TUI 時也可以用指令設定：

```sh
alc config upsert codex --kind codex --model gpt-5.6-terra --effort medium
alc config upsert codex --clear-effort
alc config show
alc doctor
```

設定優先順序是「這次指令的 `--model/--effort`」→「alc provider」→「Codex
profile」→「模型目錄預設值」。因此新手可以只用選單，進階使用者仍可完整控制。

## 重要相容性

- Claude Code 需要 Anthropic Messages 相容端點。OpenRouter 與新版 Ollama
  可直接使用；純 OpenAI API 端點需要額外 gateway。
- Codex 自訂 provider 需要 OpenAI Responses API。
- OpenCode 可直接使用大部分原生或 OpenAI-compatible provider。
- vLLM 的初始 profile 預設停用，因為 model ID 與支援協定取決於你的部署；
  在 TUI 填妥後再啟用即可。

執行 `alc doctor` 可以檢查 agent、API key、Codex 登入、helper 與相容矩陣。
完整英文文件請見專案根目錄的 `README.md`。
