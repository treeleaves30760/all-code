use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::Result;

use crate::config::{Agent, AuthStyle, ProviderKind, Store};

pub fn run(store: &Store) -> Result<bool> {
    println!("alc doctor\n");
    println!("Config       {}", store.config_path().display());
    println!("Credentials  {}", store.credentials_path().display());

    let mut healthy = true;
    match store.config.validate() {
        Ok(()) => println!("Config       OK"),
        Err(error) => {
            println!("Config       ERROR  {error}");
            healthy = false;
        }
    }

    println!("\nAgent binaries");
    for agent in Agent::ALL {
        let binary = binary_for(agent);
        match resolve(&binary) {
            Some(path) => println!("  {:<10} OK      {}", agent, path.display()),
            None => {
                println!("  {:<10} MISSING {}", agent, binary.to_string_lossy());
                healthy = false;
            }
        }
    }

    let helper = helper_path();
    let codex_for_claude = store
        .config
        .providers
        .values()
        .any(|provider| provider.enabled && provider.kind == ProviderKind::Codex);
    match helper {
        Some(path) => println!(
            "  {:<10} OK      {} (Codex -> Claude adapter {})",
            "adapter",
            path.display(),
            crate::launch::CLAUDE_CODEX_HELPER_VERSION
        ),
        None if codex_for_claude => {
            println!(
                "  {:<10} MISSING claude-codex (required by Codex -> Claude profiles)",
                "adapter"
            );
            healthy = false;
        }
        None => println!("  {:<10} optional helper not installed", "adapter"),
    }

    println!("\nProvider profiles");
    println!("  NAME             KIND         KEY           CLAUDE CODEX OPENCODE");
    for (name, provider) in &store.config.providers {
        let key_status = if provider
            .api_key_env
            .as_deref()
            .and_then(|variable| env::var(variable).ok())
            .is_some_and(|value| !value.is_empty())
        {
            "env"
        } else if store.credentials.api_keys.contains_key(name) {
            "saved"
        } else if matches!(provider.auth, AuthStyle::Native | AuthStyle::None) {
            "n/a"
        } else if provider.kind == ProviderKind::Anthropic {
            "native/key"
        } else {
            healthy = false;
            "MISSING"
        };
        println!(
            "  {:<16} {:<12} {:<13} {:<6} {:<5} {:<8}",
            truncate(name, 16),
            provider.kind,
            key_status,
            yes_no(provider.supports(Agent::Claude)),
            yes_no(provider.supports(Agent::Codex)),
            yes_no(provider.supports(Agent::Opencode)),
        );
    }

    println!("\nDefaults");
    for agent in Agent::ALL {
        println!("  {:<10} {}", agent, store.config.defaults.get(agent));
    }

    if codex_for_claude {
        match codex_login_status() {
            Some(true) => println!("\nCodex login  OK"),
            Some(false) => {
                println!("\nCodex login  MISSING or expired; run `codex login`");
                healthy = false;
            }
            None => println!("\nCodex login  unable to check (Codex binary missing)"),
        }
    }

    println!(
        "\nResult       {}",
        if healthy { "ready" } else { "needs attention" }
    );
    Ok(healthy)
}

fn binary_for(agent: Agent) -> std::ffi::OsString {
    let override_name = match agent {
        Agent::Claude => "ALC_CLAUDE_BIN",
        Agent::Codex => "ALC_CODEX_BIN",
        Agent::Opencode => "ALC_OPENCODE_BIN",
    };
    env::var_os(override_name).unwrap_or_else(|| agent.as_str().into())
}

fn resolve(binary: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(binary);
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file().then_some(path);
    }
    which::which(binary).ok()
}

fn helper_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ALC_CLAUDE_CODEX_BIN") {
        return resolve(&path);
    }
    let file_name = if cfg!(windows) {
        "claude-codex.exe"
    } else {
        "claude-codex"
    };
    if let Ok(current) = env::current_exe()
        && let Some(parent) = current.parent()
    {
        let sibling = parent.join(file_name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    which::which("claude-codex").ok()
}

fn codex_login_status() -> Option<bool> {
    let binary = resolve(&binary_for(Agent::Codex))?;
    Command::new(binary)
        .args(["login", "status"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .map(|status| status.success())
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "-" }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.to_owned()
    } else {
        let mut result: String = value.chars().take(width.saturating_sub(1)).collect();
        result.push('…');
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_short_values() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghijk", 5), "abcd…");
    }
}
