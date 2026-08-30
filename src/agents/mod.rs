pub mod claude;
pub mod codex;
pub mod copilot;
pub mod goose;
pub mod opencode;
pub mod pi;
pub mod qwen;

use std::ffi::OsString;

use anyhow::{Result, bail};

use crate::config::{Agent, Provider, Store};
use crate::launch::{BridgePlan, LaunchOverrides, LaunchSpec};

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
        Agent::Copilot => {
            copilot::build(spec, store, profile_name, provider, passthrough, overrides)
        }
        Agent::Opencode => {
            opencode::build(spec, store, profile_name, provider, passthrough, overrides)
        }
        Agent::Pi => pi::build(spec, store, profile_name, provider, passthrough, overrides),
        Agent::Goose => goose::build(spec, store, profile_name, provider, passthrough, overrides),
        Agent::Qwen => qwen::build(spec, store, profile_name, provider, passthrough, overrides),
        Agent::Kimi => {
            bail!("{agent} support is not wired up yet on this branch (arrives in a later task)")
        }
    }
}

/// Wires the running bridge (listening on `base_url`) into `spec` for
/// `spec.agent`. Each agent speaks a different dialect of "point me at a
/// local OpenAI-ish endpoint," so this is agent-specific.
pub fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    let agent = spec.agent;
    match agent {
        Agent::Claude => claude::apply_bridge(spec, base_url, plan),
        Agent::Codex => Ok(()),
        Agent::Copilot => copilot::apply_bridge(spec, base_url, plan),
        Agent::Opencode => opencode::apply_bridge(spec, base_url, plan),
        Agent::Pi => pi::apply_bridge(spec, base_url, plan),
        Agent::Goose => goose::apply_bridge(spec, base_url, plan),
        Agent::Qwen => qwen::apply_bridge(spec, base_url, plan),
        Agent::Kimi => {
            bail!("{agent} bridge support arrives in a later task")
        }
    }
}
