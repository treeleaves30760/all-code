---
id: intro
slug: /
title: all-code (alc)
sidebar_label: 總覽
sidebar_position: 1
description: 用你已經在付費的 Codex／ChatGPT 訂閱跑 Claude Code，再加上另外七個 coding agent，任何 provider 都行，一行指令就好。
keywords:
  - claude code
  - codex cli
  - chatgpt 訂閱
  - opencode
  - pi coding agent
  - copilot cli
  - goose
  - qwen code
  - kimi code cli
  - llm provider
  - coding agent
  - 中文
hide_title: true
hide_table_of_contents: true
pagination_next: null
pagination_prev: null
---

<div className="alc-hero">

<p className="alc-hero__eyebrow">alc · 單一執行檔 · macOS、Linux、Windows</p>

<h1 className="alc-hero__title">用你的 <em>ChatGPT 方案</em>跑 Claude Code。</h1>

<p className="alc-hero__lead">一次 <code>codex login</code>，alc 啟動的每個 coding agent —— Claude Code 和另外七個 —— 都能用那份訂閱，或是用你指定的任何 provider。</p>

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
codex login
alc --codex claude
```

<p className="alc-hero__note">Windows PowerShell：<code>irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex</code></p>

<div className="alc-actions">

[快速上手](./getting-started.md) [下載](https://github.com/treeleaves30760/all-code/releases/latest)

</div>

</div>

<div className="alc-section">

<p className="alc-section__title">它能做什麼</p>

<div className="alc-grid">

<div className="alc-card">

[Codex 橋接](./codex-to-claude.md)

`alc --codex <agent>` 在單一 session 前面放一個 loopback 轉接器。Claude Code
的 `/model` 選單裡會列出每個 GPT 模型；其他 agent 在啟動時選定一個。

</div>

<div className="alc-card">

[任何 provider](./providers.md)

Anthropic、OpenAI、OpenRouter、Ollama、vLLM、DeepSeek、Moonshot、Z.ai、
MiniMax、Groq、xAI、Google，或你自己的端點。加一個旗標就能只為這次執行換掉。

</div>

<div className="alc-card">

[遠端控制](./remote-control.md)

`alc --share claude` 把 session 鏡射到網頁，用手機就能操作。每個 agent
都是同一個頁面；你的終端機照常運作。

</div>

<div className="alc-card">

[用量](./usage.md)

`alc usage` 顯示你每個 Claude 與 Codex 登入還剩多少額度、每個 API key
provider 還剩多少，以及是哪個 agent 用掉的。

</div>

</div>

</div>

<div className="alc-section">

<p className="alc-section__title">Agent</p>

<div className="alc-chips">

[Claude Code](./agents.md#claude-code)
[Codex CLI](./agents.md#codex-cli)
[OpenCode](./agents.md#opencode)
[Pi](./agents.md#pi)
[Copilot CLI](./agents.md#copilot-cli)
[Goose](./agents.md#goose)
[Qwen Code](./agents.md#qwen-code)
[Kimi Code CLI](./agents.md#kimi-code-cli)

</div>

<p className="alc-hero__note">alc 只啟動已經安裝好的 agent，不會替你打包它們。</p>

</div>

<div className="alc-section">

<p className="alc-section__title">運作方式</p>

<ul className="alc-facts">
  <li><strong>不用先設定</strong>起始設定已編譯進執行檔。<code>alc --codex claude</code> 直接在記憶體裡讀它，不寫任何自己的檔案。</li>
  <li><strong>啟動前先檢查</strong>agent 與 provider 講的線路協定不一樣。alc 會拒絕注定失敗的組合，而不是把請求送出去。</li>
  <li><strong>金鑰留在原地</strong>API key 放在 <code>credentials.toml</code>（權限 0600）或環境變數裡，不會在 agent 之間複製。</li>
</ul>

</div>
