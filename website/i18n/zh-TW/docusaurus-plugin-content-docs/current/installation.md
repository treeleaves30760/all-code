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

安裝器會把 `alc` 放進 `~/.local/bin`
（Windows 為 `%USERPROFILE%\.local\bin`），必要時會把該目錄加入你的 User PATH。
macOS 與 Linux 請依畫面提示重開終端機或 `source` 對應的設定檔；PowerShell 會同時
更新目前工作階段與 User PATH。如果系統不允許修改 PATH，安裝器會明確印出需要手動
加入的目錄。

Windows 安裝器已在 Windows PowerShell 5.1 與 PowerShell 7 上測試，包含 64 位元
Windows 上執行的 32 位元 PowerShell。

## 安裝到其他目錄

執行安裝器前設定 `ALC_INSTALL_DIR`。在 macOS 與 Linux 上，`install.sh` 不會靜默把
自訂目錄加入 PATH；需要手動設定時安裝器會告訴你。`install.ps1` 沒有這項區別，它會
像對待預設目錄一樣，把自訂目錄加入你的 User PATH，所以在 Windows 上唯一的退出方式
是 `ALC_NO_PATH_UPDATE=1`，它會直接關掉自動修改 PATH。

## 從原始碼建置

需要 Rust 1.88 以上：

```sh
cargo build --release --locked
```

Codex 橋接是 alc 自己的程式碼，放在 `src/bridge/`，直接編進執行檔裡，所以從原始碼
建置就是完整的建置 —— 不必再裝任何東西，`alc --codex <agent>` 就能用。發行包裡也
因此只有 `alc` 一個執行檔，外加放在旁邊的授權聲明：`LICENSE`、`THIRD_PARTY.md`
與 `THIRD_PARTY_LICENSES/`。

常用的開發檢查：

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## 解除安裝

把 `alc` 從安裝目錄移除，需要的話再刪掉 `alc config path`
顯示的設定目錄。刪除設定目錄同時會刪掉本機儲存的 API key，且無法復原。
