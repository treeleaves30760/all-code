---
id: installation
title: 安裝 alc
sidebar_label: 安裝
sidebar_position: 2
description: 在 macOS、Linux、WSL 或 Windows 上用一行指令安裝 alc CLI，或用 Cargo 從原始碼建置。
keywords:
  - 安裝 alc
  - windows powershell 安裝
---

# 安裝 alc

## 一行安裝

macOS、Linux、WSL：

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

安裝器會把 `alc` 與 Codex-to-Claude 的 loopback helper 放進 `~/.local/bin`
（Windows 為 `%USERPROFILE%\.local\bin`），必要時會把該目錄加入你的 User PATH。
macOS 與 Linux 請依畫面提示重開終端機或 `source` 對應的設定檔；PowerShell 會同時
更新目前工作階段與 User PATH。如果系統不允許修改 PATH，安裝器會明確印出需要手動
加入的目錄。

Windows 安裝器已在 Windows PowerShell 5.1 與 PowerShell 7 上測試，包含 64 位元
Windows 上執行的 32 位元 PowerShell。

## 安裝到其他目錄

執行安裝器前設定 `ALC_INSTALL_DIR`。自訂目錄不會被靜默加入 PATH；需要手動設定時
安裝器會告訴你。設定 `ALC_NO_PATH_UPDATE=1` 可明確關閉自動修改 PATH。

## 從原始碼建置

需要 Rust 1.88 以上：

```sh
cargo build --release --locked
```

從原始碼建置只會產生 `alc`。要使用 `alc --codex claude`，請把相容的
`claude-codex` 執行檔放到 PATH，或設定 `ALC_CLAUDE_CODEX_BIN`。官方發行包已經
附帶固定版本的 helper。

常用的開發檢查：

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## 解除安裝

把 `alc` 與 `claude-codex` 從安裝目錄移除，需要的話再刪掉 `alc config path`
顯示的設定目錄。刪除設定目錄同時會刪掉本機儲存的 API key，且無法復原。
