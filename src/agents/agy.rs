//! Google's Antigravity CLI, the `agy` binary.
//!
//! agy holds its own Google login, so alc has no key, base URL or config
//! file to hand it and touches no credential of agy's: the launch is the
//! binary, the profile's model and effort as agy's own `--model` and
//! `--effort` flags, and whatever the user passed through.
//!
//! agy lists wire ids (`claude-opus-5-5-high`) only, while a profile may hold
//! a bare family id (`claude-opus-5-5`), which the bridge reads too; a family
//! id is sent as the wire id for the effort this run resolves to.
//!
//! agy's `--effort` takes `low|medium|high|xhigh|max`, the same spellings
//! alc's [`ReasoningEffort`] uses, minus `ultra`.
//!
//! On the Codex login agy is pointed (`CLOUD_CODE_URL`) at alc's bridge,
//! whose Code Assist surface adds the Codex models to agy's own and relays
//! the rest to Google ([`crate::bridge::code_assist`]). The session starts on
//! the profile's Codex model at its effort, as the wire id that surface
//! lists (`gpt-6.1-sol-high`); agy's own login still serves agy's models.

use std::ffi::OsString;

use anyhow::{Result, bail};

use crate::config::{Provider, ProviderKind, ReasoningEffort};
use crate::launch::{BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, bridge_plan, has_option};

/// The model is this run's override, else the profile's; the effort the
/// same. A `--model` or `--effort` the user passed through outranks both,
/// and alc then sends no flag of its own rather than a second one. A bare
/// family id becomes its wire id for that effort; a wire id, or a model alc
/// does not know, is sent as it is.
pub(crate) fn build(
    spec: &mut LaunchSpec,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind == ProviderKind::Codex {
        return build_on_codex(spec, provider, passthrough, overrides);
    }
    if provider.kind != ProviderKind::Antigravity {
        bail!("the Antigravity CLI runs only on an antigravity or a codex profile");
    }

    let effort = overrides.reasoning_effort.or(provider.reasoning_effort);
    let model = overrides.model.as_deref().unwrap_or(&provider.model).trim();
    if !model.is_empty() && !has_option(passthrough, "--model", "--model") {
        let model = if crate::antigravity::family(model).is_some() {
            crate::antigravity::wire_model_for(model, effort)
        } else {
            model.to_owned()
        };
        spec.args.push("--model".into());
        spec.args.push(model.into());
    }

    if !has_option(passthrough, "--effort", "--effort")
        && let Some(effort) = effort
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

/// agy on the Codex login: started on the profile's model at its effort,
/// through the bridge. A `--model` passed through (a Codex wire id or one of
/// agy's own) outranks it; `--effort` is not sent, because the wire id
/// already carries it.
fn build_on_codex(
    spec: &mut LaunchSpec,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    let Some(mut plan) = bridge_plan(provider, overrides, BridgeApi::CodeAssist)? else {
        bail!("the Codex kind always runs agy on alc's bridge");
    };
    let known = plan.options.iter().find(|option| option.id == plan.model);
    let effort = plan
        .effort
        .or(known.map(|option| option.default_effort))
        .unwrap_or(ReasoningEffort::Medium);
    let effort = clamp_for_agy(effort);
    if plan.effort.is_some_and(|asked| asked != effort) {
        eprintln!(
            "note: the Antigravity CLI tops out at 'max', so this session uses max rather than \
             ultra."
        );
    }
    plan.effort = Some(effort);
    // A model the catalog does not list still has to be in agy's picker, or
    // agy refuses to start on it.
    if known.is_none() {
        plan.options.insert(
            0,
            crate::model_catalog::ModelInfo {
                id: plan.model.clone(),
                name: plan.model.clone(),
                description: format!("{} through your Codex login", plan.model),
                context_window: plan.context_window.unwrap_or(272_000),
                max_context_window: 0,
                default_effort: effort,
                supported_efforts: vec![effort],
                priority: None,
            },
        );
    }
    if !has_option(passthrough, "--model", "--model") {
        spec.args.push("--model".into());
        spec.args
            .push(crate::bridge::code_assist::wire_id(&plan.model, effort).into());
    }
    spec.bridge = Some(plan);
    spec.args.extend_from_slice(passthrough);
    Ok(())
}

/// Points agy at the bridge's Code Assist surface. Only a Codex plan reaches
/// here; agy on its own login is never bridged.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    if plan.api != BridgeApi::CodeAssist {
        bail!(
            "agy was handed a {:?} adapter plan; this is an alc bug - please report it",
            plan.api
        );
    }
    spec.env.insert("CLOUD_CODE_URL".into(), base_url.into());
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

    /// agy lists wire ids only, so a profile saved on a bare family id
    /// reaches it as the wire id for the profile's effort.
    #[test]
    fn a_family_id_reaches_agy_as_the_wire_id_for_its_effort() {
        let (mut spec, mut provider) = antigravity();
        provider.model = "claude-opus-5-5".to_owned();
        provider.reasoning_effort = Some(ReasoningEffort::Low);
        build(&mut spec, &provider, &[], &LaunchOverrides::default()).unwrap();
        assert_eq!(
            spec.args,
            args(&["--model", "claude-opus-5-5-low", "--effort", "low"])
        );

        // No effort is agy's own default rung, medium.
        let (mut spec, mut provider) = antigravity();
        provider.model = "gemini-3.8-flash".to_owned();
        build(&mut spec, &provider, &[], &LaunchOverrides::default()).unwrap();
        assert_eq!(spec.args, args(&["--model", "gemini-3.8-flash-medium"]));
    }

    #[test]
    fn a_family_id_given_for_this_run_is_mapped_with_this_runs_effort() {
        let (mut spec, mut provider) = antigravity();
        provider.reasoning_effort = Some(ReasoningEffort::Low);
        let overrides = LaunchOverrides {
            model: Some("gemini-3.8-flash".to_owned()),
            reasoning_effort: Some(ReasoningEffort::Max),
            ..LaunchOverrides::default()
        };
        build(&mut spec, &provider, &[], &overrides).unwrap();
        assert_eq!(
            spec.args,
            args(&["--model", "gemini-3.8-flash-high", "--effort", "max"])
        );
    }

    #[test]
    fn a_wire_id_reaches_agy_unchanged() {
        let (mut spec, mut provider) = antigravity();
        provider.model = "gemini-3.8-flash-low".to_owned();
        provider.reasoning_effort = Some(ReasoningEffort::High);
        build(&mut spec, &provider, &[], &LaunchOverrides::default()).unwrap();
        assert_eq!(
            spec.args,
            args(&["--model", "gemini-3.8-flash-low", "--effort", "high"])
        );
    }

    #[test]
    fn an_unknown_model_reaches_agy_unchanged() {
        let (mut spec, mut provider) = antigravity();
        provider.model = "gemini-9-preview".to_owned();
        provider.reasoning_effort = Some(ReasoningEffort::Low);
        build(&mut spec, &provider, &[], &LaunchOverrides::default()).unwrap();
        assert_eq!(
            spec.args,
            args(&["--model", "gemini-9-preview", "--effort", "low"])
        );
    }

    #[test]
    fn another_kind_is_refused() {
        let (mut spec, _) = antigravity();
        let provider = Provider::for_kind(ProviderKind::Openrouter);
        assert!(build(&mut spec, &provider, &[], &LaunchOverrides::default()).is_err());
    }

    fn codex() -> (LaunchSpec, Provider) {
        let mut spec = LaunchSpec::for_test();
        spec.agent = Agent::Agy;
        spec.provider_kind = ProviderKind::Codex;
        spec.provider_name = "codex".to_owned();
        let mut provider = Provider::for_kind(ProviderKind::Codex);
        provider.model = "gpt-6.1-sol".to_owned();
        (spec, provider)
    }

    fn sol() -> crate::model_catalog::ModelInfo {
        crate::model_catalog::ModelInfo {
            id: "gpt-6.1-sol".to_owned(),
            name: "GPT-6.1 Sol".to_owned(),
            description: String::new(),
            context_window: 400_000,
            max_context_window: 0,
            default_effort: ReasoningEffort::Medium,
            supported_efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
            priority: None,
        }
    }

    #[test]
    fn on_the_codex_login_agy_starts_on_the_profiles_model_through_code_assist() {
        let (mut spec, mut provider) = codex();
        provider.reasoning_effort = Some(ReasoningEffort::High);
        let overrides = LaunchOverrides {
            model_options: vec![sol()],
            ..LaunchOverrides::default()
        };
        build(&mut spec, &provider, &args(&["-p", "hi"]), &overrides).unwrap();
        assert_eq!(
            spec.args,
            args(&["--model", "gpt-6.1-sol-high", "-p", "hi"])
        );
        let plan = spec
            .bridge
            .clone()
            .expect("agy is bridged on the Codex login");
        assert_eq!(plan.api, BridgeApi::CodeAssist);
        assert_eq!(plan.effort, Some(ReasoningEffort::High));

        apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();
        assert_eq!(
            spec.env.get(std::ffi::OsStr::new("CLOUD_CODE_URL")),
            Some(&OsString::from("http://127.0.0.1:9"))
        );
    }

    #[test]
    fn on_the_codex_login_ultra_runs_as_max_and_a_passed_model_wins() {
        let (mut spec, mut provider) = codex();
        provider.reasoning_effort = Some(ReasoningEffort::Ultra);
        build(
            &mut spec,
            &provider,
            &args(&["--model", "gemini-3.8-flash-low"]),
            &LaunchOverrides::default(),
        )
        .unwrap();
        assert_eq!(spec.args, args(&["--model", "gemini-3.8-flash-low"]));
        let plan = spec.bridge.unwrap();
        assert_eq!(plan.effort, Some(ReasoningEffort::Max));
        // A model the catalog does not list is still offered to agy's picker.
        assert_eq!(plan.options[0].id, "gpt-6.1-sol");
    }
}
