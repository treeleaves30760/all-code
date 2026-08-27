---
id: installation
title: Install alc
sidebar_label: Install
sidebar_position: 2
description: Install the alc CLI on macOS, Linux, WSL, or Windows with a one-line installer, or build it from source with Cargo.
keywords:
  - install claude code cli
  - alc install
  - windows powershell installer
---

# Install alc

## One-line installer

macOS, Linux, and WSL:

```sh
curl -fsSL https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/treeleaves30760/all-code/main/install.ps1 | iex
```

The installer puts `alc` and its Codex-to-Claude loopback helper in
`~/.local/bin` (Windows: `%USERPROFILE%\.local\bin`) and adds that directory to
your user PATH when needed. On macOS and Linux, restart the terminal or source
the profile named by the installer. PowerShell updates the current session and
your user PATH. If PATH cannot be changed, the installer prints the exact
directory to add manually.

The Windows installer is tested with both Windows PowerShell 5.1 and
PowerShell 7, including 32-bit PowerShell running on 64-bit Windows.

## Install to a different directory

Set `ALC_INSTALL_DIR` before running the installer. Custom directories are not
added to PATH silently; the installer tells you when a manual PATH change is
needed. Set `ALC_NO_PATH_UPDATE=1` to disable automatic PATH changes
explicitly.

## Build from source

Rust 1.88 or newer:

```sh
cargo build --release --locked
```

The source build produces only `alc`. To use `alc --codex claude`, put a
compatible `claude-codex` binary on PATH or set `ALC_CLAUDE_CODEX_BIN`.
Official release archives already bundle the pinned helper.

Useful development checks:

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## Uninstall

Remove `alc` and `claude-codex` from the install directory, then optionally
remove the configuration directory listed by `alc config path`. Removing the
configuration also deletes locally saved API keys and cannot be undone.
