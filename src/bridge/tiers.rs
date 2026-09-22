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

impl ModelTiers {
    /// The Codex model to send upstream for `model`, or `None` to send it as
    /// it came.
    pub(crate) fn serve_as(&self, model: &str) -> Option<&str> {
        let id = model.trim().to_ascii_lowercase();
        match id.as_str() {
            "opus" | "fable" | "best" | "opusplan" => return Some(&self.strongest),
            "haiku" => return Some(&self.cheapest),
            "sonnet" | "default" => return Some(&self.default),
            _ => {}
        }
        let family = id.strip_prefix("claude-")?;
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
        ] {
            assert_eq!(tiers.serve_as(asked), Some(served), "{asked}");
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
        ] {
            assert_eq!(tiers.serve_as(model), None, "{model}");
        }
    }
}
