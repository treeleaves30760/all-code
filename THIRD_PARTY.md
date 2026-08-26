# Third-party software

Official `alc` release archives bundle the following helper next to the main
binary. Building `alc` from source does not download or compile this helper.

## claude-codex 0.3.1

- Project: <https://github.com/fcakyon/claude-code-with-codex>
- Based on: <https://github.com/raine/claude-code-proxy>
- License: MIT
- Purpose: loopback-only Anthropic Messages to Codex Responses translation for
  `alc --codex claude`

The helper reads and may refresh the current user's Codex CLI credentials. It
is started only for a Codex-backed Claude Code session and is terminated when
that session exits. See the bundled license in
`THIRD_PARTY_LICENSES/claude-codex-LICENSE`.
