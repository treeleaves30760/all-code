//! Which Codex model answers when Claude Code names one of Claude's own.
//!
//! `alc --codex claude` promises that no request reaches a Claude model. The
//! launch keeps that promise for every alias Claude Code resolves itself
//! (`ANTHROPIC_DEFAULT_*_MODEL`), but a full Claude id can still arrive:
//! `/model claude-opus-5`, a subagent whose frontmatter names one, a
//! `fallbackModel` chain in the user's settings, or a Claude model released
//! after this alc. chatgpt.com serves none of them, so the bridge answers each
//! with the Codex model of the same tier instead of relaying a refusal.
//!
//! Only ids that are recognisably Claude's are touched. Anything else - a GPT
//! id, or a model alc has never heard of - passes through untouched, which is
//! what keeps a new Codex model usable on the day it ships.

use serde::{Deserialize, Serialize};

/// The three Codex models Claude's tiers land on, taken from the catalog the
/// launch built its aliases from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelTiers {
    /// Where `opus`, `fable` and `best` land: the catalog's most capable model.
    pub strongest: String,
    /// Where `sonnet` and any other Claude id land: the session's starting model.
    pub default: String,
    /// Where `haiku` lands, and Claude Code's background work with it.
    pub cheapest: String,
}

/// One of Claude's three tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Strongest,
    Default,
    Cheapest,
}

/// `model` lowercased, trimmed, and without a `[1m]` suffix. The suffix asks
/// for a context window, not another model, so the tier is the one the name in
/// front of it picks: `opus[1m]` is Opus. Matching the whole string left every
/// bracketed alias unrecognised and relayed it to chatgpt.com, which serves
/// none of Claude's.
fn model_id(model: &str) -> String {
    let asked = model.trim().to_ascii_lowercase();
    match asked.split_once('[') {
        Some((name, _)) => name.to_owned(),
        None => asked,
    }
}

/// The tier a Claude Code alias picks, or `None` when `id` is not one.
fn alias_tier(id: &str) -> Option<Tier> {
    match id {
        "opus" | "fable" | "best" | "opusplan" => Some(Tier::Strongest),
        "haiku" => Some(Tier::Cheapest),
        "sonnet" | "default" => Some(Tier::Default),
        _ => None,
    }
}

/// Whether `model` is one of the aliases Claude Code resolves itself
/// (`sonnet`, `opus`, `opusplan[1m]`, ...) rather than a model id.
pub(crate) fn is_claude_code_alias(model: &str) -> bool {
    alias_tier(&model_id(model)).is_some()
}

/// What every Claude model's full id starts with (`claude-opus-5`,
/// `claude-3-5-haiku-latest`); a provider's own spelling such as
/// `anthropic/claude-sonnet-5` does not.
const CLAUDE_ID_PREFIX: &str = "claude-";

/// Whether `model` names a Claude model in full (`claude-sonnet-5`,
/// `Claude-Haiku-5[1m]`) - the ids Anthropic, and a gateway in front of it,
/// serves by name.
pub(crate) fn is_claude_model_id(model: &str) -> bool {
    model_id(model).starts_with(CLAUDE_ID_PREFIX)
}

impl ModelTiers {
    fn tier(&self, tier: Tier) -> &str {
        match tier {
            Tier::Strongest => &self.strongest,
            Tier::Default => &self.default,
            Tier::Cheapest => &self.cheapest,
        }
    }

    /// The Codex model to send upstream for `model`, or `None` to send it as
    /// it came.
    pub(crate) fn serve_as(&self, model: &str) -> Option<&str> {
        let id = model_id(model);
        if let Some(tier) = alias_tier(&id) {
            return Some(self.tier(tier));
        }
        let family = id.strip_prefix(CLAUDE_ID_PREFIX)?;
        if ["fable", "mythos", "opus"]
            .iter()
            .any(|name| family.contains(name))
        {
            Some(&self.strongest)
        } else if family.contains("haiku") {
            Some(&self.cheapest)
        } else {
            Some(&self.default)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers() -> ModelTiers {
        ModelTiers {
            strongest: "gpt-6-astra".to_owned(),
            default: "gpt-5.6-terra".to_owned(),
            cheapest: "gpt-5.6-luna".to_owned(),
        }
    }

    #[test]
    fn every_claude_family_lands_on_its_codex_tier() {
        let tiers = tiers();
        for (asked, served) in [
            ("claude-opus-5", "gpt-6-astra"),
            ("claude-opus-4-8", "gpt-6-astra"),
            ("claude-3-opus-20240229", "gpt-6-astra"),
            ("claude-fable-5-1", "gpt-6-astra"),
            ("claude-mythos-1", "gpt-6-astra"),
            ("Claude-Opus-5", "gpt-6-astra"),
            ("claude-sonnet-5", "gpt-5.6-terra"),
            ("claude-sonnet-4-5-20250929", "gpt-5.6-terra"),
            ("claude-something-released-tomorrow", "gpt-5.6-terra"),
            ("claude-haiku-4-5-20251001", "gpt-5.6-luna"),
            ("claude-3-5-haiku-latest", "gpt-5.6-luna"),
            ("opus", "gpt-6-astra"),
            ("fable", "gpt-6-astra"),
            ("best", "gpt-6-astra"),
            ("opusplan", "gpt-6-astra"),
            ("sonnet", "gpt-5.6-terra"),
            ("default", "gpt-5.6-terra"),
            ("haiku", "gpt-5.6-luna"),
            // A `[1m]` suffix asks for a context window, not another model,
            // so each of these is the tier its name alone picks.
            ("opus[1m]", "gpt-6-astra"),
            ("opusplan[1m]", "gpt-6-astra"),
            ("sonnet[1m]", "gpt-5.6-terra"),
            ("haiku[1m]", "gpt-5.6-luna"),
            ("claude-sonnet-4-5[1m]", "gpt-5.6-terra"),
        ] {
            assert_eq!(tiers.serve_as(asked), Some(served), "{asked}");
        }
    }

    #[test]
    fn claude_codes_aliases_are_told_from_model_ids() {
        for alias in [
            "sonnet",
            "Opus",
            "haiku",
            "fable",
            "best",
            "opusplan",
            "default",
            "opus[1m]",
            "sonnet[1m]",
        ] {
            assert!(is_claude_code_alias(alias), "{alias}");
        }
        for model in [
            "claude-sonnet-5",
            "anthropic/claude-sonnet-5",
            "gpt-6-astra",
            "qwen3-coder",
            "sonnet-proxy",
        ] {
            assert!(!is_claude_code_alias(model), "{model}");
        }
    }

    #[test]
    fn claude_ids_in_full_are_told_from_other_models() {
        for model in [
            "claude-opus-5",
            "claude-sonnet-5-5",
            "Claude-Haiku-5[1m]",
            " claude-3-5-haiku-latest ",
        ] {
            assert!(is_claude_model_id(model), "{model}");
        }
        for model in [
            "anthropic/claude-sonnet-5",
            "sonnet",
            "claude",
            "gpt-6-astra",
            "qwen3-coder",
        ] {
            assert!(!is_claude_model_id(model), "{model}");
        }
    }

    /// The other half of the promise: a model that is not Claude's reaches
    /// chatgpt.com exactly as it was asked for.
    #[test]
    fn anything_that_is_not_claudes_passes_through() {
        let tiers = tiers();
        for model in [
            "gpt-6-astra",
            "gpt-7-nova",
            "o5-mini",
            "qwen3-coder",
            "my-claude-proxy",
            // Cutting the context-window suffix off must not turn a model
            // that is not Claude's into one alc rewrites.
            "gpt-6-astra[1m]",
            "qwen3-coder[1m]",
        ] {
            assert_eq!(tiers.serve_as(model), None, "{model}");
        }
    }
}
