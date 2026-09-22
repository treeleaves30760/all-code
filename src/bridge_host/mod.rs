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

use std::time::Duration;

use anyhow::{Context, Result};

#[allow(
    dead_code,
    reason = "the background bridge and the Claude launch are wired to these next"
)]
pub(crate) mod files;
#[allow(
    dead_code,
    reason = "`alc bridge serve` starts this process, and is added next"
)]
mod serve;

#[allow(
    unused_imports,
    reason = "`alc bridge serve` calls this, and is added next"
)]
pub(crate) use serve::run as serve;

const HELLO_TIMEOUT: Duration = Duration::from_secs(2);

/// What a running bridge says about itself.
#[derive(Debug, Clone, serde::Deserialize)]
#[allow(
    dead_code,
    reason = "`alc bridge status` reports these, and is added next"
)]
pub(crate) struct Hello {
    pub instance: String,
    pub alc: String,
    pub pid: u32,
    pub port: u16,
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(HELLO_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

/// Asks whatever listens on `port` whether it is this configuration's bridge.
/// Only a listener that knows the token can answer, so some other program on
/// the port is never mistaken for it.
pub(crate) fn hello(port: u16, token: &str) -> Result<Hello> {
    let mut response = agent()
        .get(&format!("http://127.0.0.1:{port}/alc/hello"))
        .header("authorization", &format!("Bearer {token}"))
        .call()
        .context("no alc bridge answered")?;
    let text = response
        .body_mut()
        .read_to_string()
        .context("the bridge's answer could not be read")?;
    serde_json::from_str(&text).context("the bridge's answer did not parse")
}
