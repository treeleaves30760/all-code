# Third-party software

`alc` links the following in at build time. There is no second binary to
download, install, or keep in step with `alc`.

## claude-codex 0.3.1

- Project: <https://github.com/fcakyon/claude-code-with-codex>
- Based on: <https://github.com/raine/claude-code-proxy>
- License: MIT
- Purpose: loopback-only Anthropic Messages / OpenAI Responses / Chat
  Completions translation for the alc Codex bridge (`alc --codex <agent>`)

Pinned in `Cargo.toml` by tag and in `Cargo.lock` by commit, and compiled into
`alc` as a library rather than shipped beside it. It is served on a loopback
port owned by the `alc` process, started only for Codex-backed sessions — of
any supported agent — and stopped when that session ends. It reads and may
refresh the current user's Codex CLI credentials. See the bundled license in
`THIRD_PARTY_LICENSES/claude-codex-LICENSE`.

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
