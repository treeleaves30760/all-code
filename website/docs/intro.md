---
id: intro
slug: /
title: all-code (alc)
sidebar_label: Overview
sidebar_position: 1
description: Run Claude Code on the Codex/ChatGPT subscription you already pay for, and seven other coding agents on any provider, from one command.
keywords:
  - claude code
  - codex cli
  - chatgpt subscription
  - opencode
  - pi coding agent
  - copilot cli
  - goose
  - qwen code
  - kimi code cli
  - llm provider
  - coding agent
hide_title: true
hide_table_of_contents: true
pagination_next: null
pagination_prev: null
---

<div className="alc-hero">

<p className="alc-hero__eyebrow">alc · one binary · macOS, Linux, Windows</p>

<h1 className="alc-hero__title">Claude Code on your <em>ChatGPT plan</em>.</h1>

<p className="alc-hero__lead">One <code>codex login</code> serves every coding agent alc launches — Claude Code and seven others — on that subscription, or on any provider you point them at.</p>

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
codex login
alc --codex claude
```

<p className="alc-hero__note">Windows PowerShell: <code>irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex</code></p>

<div className="alc-actions">

[Get started](./getting-started.md) [Download](https://github.com/treeleaves30760/all-code/releases/latest)

</div>

</div>

<div className="alc-section">

<p className="alc-section__title">What it does</p>

<div className="alc-grid">

<div className="alc-card">

[Codex bridge](./codex-to-claude.md)

`alc --codex <agent>` puts a loopback adapter in front of one session. Claude
Code gets every GPT model in its own `/model` picker; the other agents pick
one at launch.

</div>

<div className="alc-card">

[Any provider](./providers.md)

Anthropic, OpenAI, OpenRouter, Ollama, llama.cpp, vLLM, DeepSeek, Moonshot, Z.ai,
MiniMax, Groq, xAI, Google, or your own endpoint. Change it for one run with
a flag.

</div>

<div className="alc-card">

[Remote control](./remote-control.md)

`alc --share claude` mirrors the session to a web page you can drive from a
phone. Same page for every agent; your terminal keeps working.

</div>

<div className="alc-card">

[Usage](./usage.md)

`alc usage` shows what is left on every Claude and Codex login you have, what
each API-key provider has left, and which agent spent it.

</div>

</div>

</div>

<div className="alc-section">

<p className="alc-section__title">Agents</p>

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

<p className="alc-hero__note">alc launches agents that are already installed; it does not bundle them.</p>

</div>

<div className="alc-section">

<p className="alc-section__title">How it works</p>

<ul className="alc-facts">
  <li><strong>No config step</strong>The starter configuration is compiled in. <code>alc --codex claude</code> reads it in memory and writes no file of its own.</li>
  <li><strong>Checked before launch</strong>Agents and providers disagree on wire protocols. alc refuses a pair that cannot work instead of sending the request.</li>
  <li><strong>Keys stay put</strong>API keys live in <code>credentials.toml</code> (mode 0600) or in your environment. Nothing is copied between agents.</li>
</ul>

</div>
