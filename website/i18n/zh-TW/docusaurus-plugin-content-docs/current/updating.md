---
id: updating
title: 更新 alc
sidebar_label: 更新
sidebar_position: 7
description: 在 macOS、Linux 與 Windows 上檢查並安裝新版 alc，過程會核對 SHA-256 檢查碼。
keywords:
  - alc update
  - 自我更新
---

# 更新 alc

檢查是否有新版本，或直接更新 `alc` 與隨附的 helper：

```sh
alc update --check
alc update
```

`alc update` 會挑選符合目前作業系統與 CPU 的發行包、核對 GitHub Release 公布的
SHA-256、確認包內版本，再一起替換兩個執行檔。

- **Linux 與 macOS** 會立即完成替換。
- **Windows** 會先完成下載與驗證，等目前的 `alc.exe` 結束後立刻替換；稍候再用
  `alc --version` 確認。

`alc update --force` 可以重新安裝目前的最新版本。

:::note

已經在執行中的工作階段會繼續使用啟動當下的執行檔。更新後請重開由 alc 啟動的
agent。

:::
