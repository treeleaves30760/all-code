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

第一次安裝後請重開終端機，接著執行：

```sh
alc config
```

這會開啟全螢幕 TUI，可新增或編輯 Anthropic、OpenAI API、OpenRouter、
Codex、Ollama、vLLM 與自訂 provider，並分別指定 Claude、Codex、OpenCode
的預設 provider。

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

agent 名稱後的參數會原封不動傳入：

```sh
alc --codex codex exec "review this repository"
alc --openrouter claude --print "summarize the diff"
```

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

發行包會附帶固定版本的 MIT 授權 `claude-codex` helper。`alc` 只把它綁在
隨機的 `127.0.0.1` port，Claude Code 結束時就會關閉。Helper 會讀取並可能
更新 `~/.codex/auth.json`；`alc` 不會把 Codex token 複製到自己的設定檔。

這是第三方相容層，不是 OpenAI 或 Anthropic 官方整合。使用訂閱帳號前，請
先檢閱專案的 `THIRD_PARTY.md` 與 provider 條款。

## 重要相容性

- Claude Code 需要 Anthropic Messages 相容端點。OpenRouter 與新版 Ollama
  可直接使用；純 OpenAI API 端點需要額外 gateway。
- Codex 自訂 provider 需要 OpenAI Responses API。
- OpenCode 可直接使用大部分原生或 OpenAI-compatible provider。
- vLLM 的初始 profile 預設停用，因為 model ID 與支援協定取決於你的部署；
  在 TUI 填妥後再啟用即可。

執行 `alc doctor` 可以檢查 agent、API key、Codex 登入、helper 與相容矩陣。
完整英文文件請見專案根目錄的 `README.md`。
