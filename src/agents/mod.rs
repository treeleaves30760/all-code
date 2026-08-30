pub mod claude;
pub mod codex;
pub mod opencode;

use std::ffi::OsString;

use anyhow::{Result, bail};

use crate::config::{Agent, Provider, Store};
use crate::launch::{LaunchOverrides, LaunchSpec};

pub fn build(
    agent: Agent,
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    match agent {
        Agent::Claude => claude::build(spec, store, profile_name, provider, passthrough, overrides),
        Agent::Codex => codex::build(spec, store, profile_name, provider, passthrough, overrides),
        Agent::Opencode => {
            opencode::build(spec, store, profile_name, provider, passthrough, overrides)
        }
        Agent::Pi | Agent::Copilot | Agent::Goose | Agent::Qwen | Agent::Kimi => {
            bail!("{agent} support is not wired up yet on this branch (arrives in a later task)")
        }
    }
}
