//! alc's background bridge: the Codex adapter as a process of its own.
//!
//! Claude Code runs background sessions - agent view, `claude --bg`, `←` on an
//! empty prompt - under a supervisor that outlives the terminal and the `alc`
//! that started them. An adapter living inside that `alc` died with it, on a
//! port the next launch could not know. So for Claude Code the adapter is one
//! detached `alc bridge serve` per configuration directory: loopback only, a
//! port chosen once and kept, a token on every model request, started on
//! demand by a launch or by the `apiKeyHelper` inside any session, and gone
//! after an hour with nothing to do. The other seven agents have no background
//! mode and keep their in-process adapter.

#[allow(
    dead_code,
    reason = "the background bridge and the Claude launch are wired to these next"
)]
pub(crate) mod files;
