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

The installer puts `alc` in
`~/.local/bin` (Windows: `%USERPROFILE%\.local\bin`) and adds that directory to
your user PATH when needed. On macOS and Linux, restart the terminal or source
the profile named by the installer. PowerShell updates the current session and
your user PATH. If PATH cannot be changed, the installer prints the exact
directory to add manually.

The Windows installer is tested with both Windows PowerShell 5.1 and
PowerShell 7, including 32-bit PowerShell running on 64-bit Windows.

## Install to a different directory

Set `ALC_INSTALL_DIR` before running the installer. On macOS and Linux,
`install.sh` never adds a custom directory to PATH silently; it tells you when
a manual PATH change is needed. `install.ps1` draws no such distinction — it
adds a custom directory to your User PATH exactly like the default one, so
`ALC_NO_PATH_UPDATE=1`, which disables automatic PATH changes outright, is the
only way to opt out on Windows.

## Build from source

Rust 1.88 or newer:

```sh
cargo build --release --locked
```

The Codex bridge is alc's own code in `src/bridge/`, compiled into the binary,
so a source build is a complete one — `alc --codex <agent>` works with nothing
else installed. Release archives ship the one `alc` binary for the same reason,
plus the license notices beside it: `LICENSE`, `THIRD_PARTY.md` and
`THIRD_PARTY_LICENSES/`.

Useful development checks:

```sh
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## Uninstall

Remove `alc` from the install directory, then optionally
remove the configuration directory listed by `alc config path`. Removing the
configuration also deletes locally saved API keys and cannot be undone.
