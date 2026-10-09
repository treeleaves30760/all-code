# Third-party software

## The Codex bridge

`alc` translates between what a coding agent speaks and what Codex serves. That
translation is alc's own code (`src/bridge/`) as of 1.5.0 — there is no bridge
dependency, no second binary, and no list of models maintained by anyone else.

It was written against a captured corpus of real traffic, and its design was
informed by reading
[`claude-codex`](https://github.com/fcakyon/claude-code-with-codex) (MIT),
which alc depended on through 1.4.1. That license is kept in
`THIRD_PARTY_LICENSES/claude-codex-LICENSE` in acknowledgement of the work it
made possible.

The bridge reads and may refresh the Codex CLI's own `~/.codex/auth.json`.
Session-owned adapters run inside `alc` and stop with the session. Claude Code
instead uses a loopback-only background host that survives launcher exit,
restarts through its credential helper, and stops after an hour idle or with
`alc bridge stop`. Model requests require local authentication; native Codex
credentials are never copied into the alc configuration.

# Bundled chart font

`alc usage --chart` renders PNGs with the unmodified Noto Sans variable font,
Copyright 2022 The Noto Project Authors, under SIL Open Font License 1.1.
It is embedded in the binary for offline, cross-platform text rendering without
system font libraries. The pinned upstream URL and SHA-256 digest are in
`assets/fonts/README.md`; the notice and license are retained in
`THIRD_PARTY_LICENSES/NotoSans-OFL.txt`, included in every release archive.

# Bundled pricing data

`alc usage` includes a curated, offline subset of LiteLLM's model pricing map
from [BerriAI/litellm](https://github.com/BerriAI/litellm), pinned to commit
`33d908e0ae2c0a257eeb5d546df08527d348a670` (2026-10-08). Only selected,
provider-verified reference rates are included; the full map is not shipped or
fetched at runtime. The exact source URL, SHA-256 digest and verification notes
are recorded in `src/usage/prices/source.toml`.

The data is MIT-licensed, copyright (c) 2023 Berri AI. Its notice is retained in
`src/usage/prices/LICENSE` and `THIRD_PARTY_LICENSES/LiteLLM-LICENSE`; the latter
is included in every release archive. These rates support API-equivalent cost
estimates, not invoices or subscription charges.

# Vendored browser assets

The remote-control page (`alc --share`) is served out of the `alc` binary
itself, so that it works with no network access and under a
`default-src 'self'` content-security policy. These files are committed under
`web/vendor/`, with their SHA-256 digests recorded in `web/vendor/VENDOR.lock`,
and compressed into the binary at build time.

## @xterm/xterm 6.0.0

- Project: <https://github.com/xtermjs/xterm.js>
- License: MIT — see `THIRD_PARTY_LICENSES/xterm.js-LICENSE`
- Purpose: the terminal emulator the page renders a mirrored session with

## @xterm/addon-fit 0.11.0

- Project: <https://github.com/xtermjs/xterm.js>
- License: MIT — see `THIRD_PARTY_LICENSES/xterm.js-LICENSE`
- Purpose: sizes the terminal to the viewport, which is what makes the page
  usable on a phone

# Rust dependencies under Apache-2.0

Most of alc's dependencies are dual MIT/Apache-2.0. These are Apache-2.0
only, and are noted here because that license carries attribution obligations
the MIT license does not. Regenerate the list with:

```sh
cargo tree --prefix none --format '{p} :: {l}' | grep ':: Apache-2.0$' | sort -u
```

- `avt` — <https://github.com/asciinema/avt> — the terminal emulator that
  tracks what a mirrored session's screen currently looks like, so a viewer
  joining late is sent a screen rather than a backlog.
- `rpassword`, `rtoolbox` — <https://github.com/conradkleinespel/rpassword> —
  the hidden prompt `alc config key` reads an API key with.
- `normalize-line-endings`, `zopfli` — build-time helpers for the compressed
  page assets.
- `prost`, `prost-derive`, `sync_wrapper` — reached through the Codex bridge's
  HTTP stack.
