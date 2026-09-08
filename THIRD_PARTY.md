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

The bridge reads and may refresh the Codex CLI's own `~/.codex/auth.json`. It
runs inside the `alc` process on a loopback port, serves only the agent that
launch started, and stops when that session ends. Credentials are never copied
into the alc configuration.

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
