//! Google's Antigravity CLI, the `agy` binary.
//!
//! agy holds its own Google login, so alc has no key, base URL or config
//! file to hand it and touches no credential of agy's: the launch is the
//! binary, the profile's model and effort as agy's own `--model` and
//! `--effort` flags, and whatever the user passed through.
//!
//! agy's `--effort` takes `low|medium|high|xhigh|max`, the same spellings
//! alc's [`ReasoningEffort`] uses, minus `ultra`.

use std::ffi::OsString;

use anyhow::{Result, bail};

use crate::config::{Provider, ProviderKind, ReasoningEffort};
use crate::launch::{LaunchOverrides, LaunchSpec, has_option};

/// The model is this run's override, else the profile's; the effort the
/// same. A `--model` or `--effort` the user passed through outranks both,
/// and alc then sends no flag of its own rather than a second one.
pub(crate) fn build(
    spec: &mut LaunchSpec,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind != ProviderKind::Antigravity {
        bail!("the Antigravity CLI runs only on an antigravity profile");
    }

    let model = overrides.model.as_deref().unwrap_or(&provider.model).trim();
    if !model.is_empty() && !has_option(passthrough, "--model", "--model") {
        spec.args.push("--model".into());
        spec.args.push(model.into());
    }

    if !has_option(passthrough, "--effort", "--effort")
        && let Some(effort) = overrides.reasoning_effort.or(provider.reasoning_effort)
    {
        let clamped = clamp_for_agy(effort);
        if clamped != effort {
            eprintln!(
                "note: the Antigravity CLI tops out at 'max', so this session uses max rather \
                 than ultra."
            );
        }
        spec.args.push("--effort".into());
        spec.args.push(clamped.as_str().into());
    }

    spec.args.extend_from_slice(passthrough);
    Ok(())
}

/// The loosest effort agy accepts. `ultra` is a Codex tier agy's
/// `--effort` does not know, so a profile carrying it would have agy refuse
/// the launch outright; `max` is the nearest thing it does run.
pub(crate) fn clamp_for_agy(effort: ReasoningEffort) -> ReasoningEffort {
    match effort {
        ReasoningEffort::Ultra => ReasoningEffort::Max,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::Agent;

    fn antigravity() -> (LaunchSpec, Provider) {
        let mut spec = LaunchSpec::for_test();
        spec.agent = Agent::Agy;
        spec.provider_kind = ProviderKind::Antigravity;
        spec.provider_name = "antigravity".to_owned();
        (spec, Provider::for_kind(ProviderKind::Antigravity))
    }

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn agy_gets_the_profiles_model_and_effort() {
        let (mut spec, mut provider) = antigravity();
        provider.reasoning_effort = Some(ReasoningEffort::High);
        build(&mut spec, &provider, &[], &LaunchOverrides::default()).unwrap();
        assert_eq!(
            spec.args,
            args(&["--model", "claude-opus-5-5-high", "--effort", "high"])
        );
    }

    #[test]
    fn a_profile_without_an_effort_leaves_agy_on_its_own() {
        let (mut spec, provider) = antigravity();
        build(&mut spec, &provider, &[], &LaunchOverrides::default()).unwrap();
        assert_eq!(spec.args, args(&["--model", "claude-opus-5-5-high"]));
    }

    #[test]
    fn a_profile_without_a_model_sends_no_model_flag() {
        let (mut spec, mut provider) = antigravity();
        provider.model = "  ".to_owned();
        build(
            &mut spec,
            &provider,
            &args(&["-p", "hi"]),
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert_eq!(spec.args, args(&["-p", "hi"]));
    }

    #[test]
    fn launch_overrides_outrank_the_profile() {
        let (mut spec, mut provider) = antigravity();
        provider.reasoning_effort = Some(ReasoningEffort::High);
        let overrides = LaunchOverrides {
            model: Some("gemini-3.8-flash-medium".to_owned()),
            reasoning_effort: Some(ReasoningEffort::Xhigh),
            ..LaunchOverrides::default()
        };
        build(&mut spec, &provider, &args(&["-p", "hi"]), &overrides).unwrap();
        assert_eq!(
            spec.args,
            args(&[
                "--model",
                "gemini-3.8-flash-medium",
                "--effort",
                "xhigh",
                "-p",
                "hi"
            ])
        );
    }

    #[test]
    fn agy_keeps_the_users_own_model_and_effort() {
        let (mut spec, mut provider) = antigravity();
        provider.reasoning_effort = Some(ReasoningEffort::High);
        let overrides = LaunchOverrides {
            model: Some("claude-sonnet-5-5-high".to_owned()),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..LaunchOverrides::default()
        };
        let passthrough = args(&[
            "--model",
            "gemini-3.8-flash-low",
            "--effort",
            "low",
            "-p",
            "hi",
        ]);
        build(&mut spec, &provider, &passthrough, &overrides).unwrap();
        assert_eq!(spec.args, passthrough);
    }

    #[test]
    fn the_users_own_flags_count_in_their_equals_spelling_too() {
        let (mut spec, mut provider) = antigravity();
        provider.reasoning_effort = Some(ReasoningEffort::High);
        let passthrough = args(&["--model=gemini-3.8-flash-low", "--effort=low"]);
        build(
            &mut spec,
            &provider,
            &passthrough,
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert_eq!(spec.args, passthrough);
    }

    #[test]
    fn ultra_is_clamped_to_max_for_agy() {
        assert_eq!(clamp_for_agy(ReasoningEffort::Ultra), ReasoningEffort::Max);
        for effort in ReasoningEffort::ALL {
            if effort != ReasoningEffort::Ultra {
                assert_eq!(clamp_for_agy(effort), effort);
            }
        }
    }

    #[test]
    fn an_ultra_profile_launches_agy_at_max() {
        let (mut spec, mut provider) = antigravity();
        provider.reasoning_effort = Some(ReasoningEffort::Ultra);
        build(&mut spec, &provider, &[], &LaunchOverrides::default()).unwrap();
        assert_eq!(
            spec.args,
            args(&["--model", "claude-opus-5-5-high", "--effort", "max"])
        );
    }

    #[test]
    fn another_kind_is_refused() {
        let (mut spec, _) = antigravity();
        let provider = Provider::for_kind(ProviderKind::Codex);
        assert!(build(&mut spec, &provider, &[], &LaunchOverrides::default()).is_err());
    }
}
