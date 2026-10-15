use std::ffi::OsString;

use anyhow::{Context, Result, bail};

use crate::config::{AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, bridge_plan, has_effort_override,
    has_model_override, has_option, key_or_error, openai_style_base_url, toml_string,
};

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    // Codex cannot sign with agy's login itself, so on that kind it runs on
    // alc's in-process bridge over the Responses surface, pinned like every
    // other agent there; `apply_bridge` points it at the bridge once the
    // port is known.
    if provider.kind == ProviderKind::Antigravity {
        let plan = bridge_plan(provider, overrides, BridgeApi::Responses)?
            .context("the Antigravity kind always runs on alc's bridge")?;
        if !has_model_override(passthrough) {
            spec.args
                .extend([OsString::from("--model"), OsString::from(&plan.model)]);
        }
        spec.bridge = Some(plan);
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    if provider.kind == ProviderKind::Codex {
        // Codex CLI reads its login and its `config.toml` out of CODEX_HOME,
        // so a profile that pins one runs on that account rather than on
        // whichever login the shell last exported.
        if let Some(home) = provider.pinned_codex_home() {
            spec.env
                .insert(OsString::from("CODEX_HOME"), OsString::from(home));
        }
        if !has_option(passthrough, "--profile", "-p")
            && let Some(profile) = provider
                .codex_profile
                .as_deref()
                .filter(|value| !value.is_empty())
        {
            spec.args
                .extend([OsString::from("--profile"), OsString::from(profile)]);
        }
        let model = overrides.model.as_deref().unwrap_or(&provider.model);
        if !model.is_empty() && !has_model_override(passthrough) {
            spec.args
                .extend([OsString::from("--model"), OsString::from(model)]);
        }
        if !has_effort_override(passthrough)
            && let Some(effort) = overrides.reasoning_effort.or(provider.reasoning_effort)
        {
            push_codex_config(
                &mut spec.args,
                "model_reasoning_effort",
                toml_string(effort.as_str()),
            );
        }
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    if provider.kind == ProviderKind::Ollama {
        if !has_option(passthrough, "--oss", "--oss") {
            spec.args.push(OsString::from("--oss"));
        }
        if !has_option(passthrough, "--local-provider", "--local-provider") {
            spec.args
                .extend([OsString::from("--local-provider"), OsString::from("ollama")]);
        }
        if !has_model_override(passthrough) {
            spec.args.extend([
                OsString::from("--model"),
                OsString::from(overrides.model.as_deref().unwrap_or(&provider.model)),
            ]);
        }
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    if !provider.protocol.supports_responses() {
        bail!(
            "provider '{profile_name}' speaks {}, but Codex requires the OpenAI Responses API",
            provider.protocol
        );
    }
    let base_url = openai_style_base_url(provider)
        .with_context(|| format!("provider '{profile_name}' needs a base URL"))?;
    let provider_id = codex_provider_id(profile_name);

    if !has_model_override(passthrough) {
        spec.args.extend([
            OsString::from("--model"),
            OsString::from(overrides.model.as_deref().unwrap_or(&provider.model)),
        ]);
    }
    if !has_effort_override(passthrough)
        && let Some(effort) = overrides.reasoning_effort.or(provider.reasoning_effort)
    {
        push_codex_config(
            &mut spec.args,
            "model_reasoning_effort",
            toml_string(effort.as_str()),
        );
    }
    push_codex_config(&mut spec.args, "model_provider", toml_string(&provider_id));
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.name"),
        toml_string(&format!("alc: {profile_name}")),
    );
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.base_url"),
        toml_string(&base_url),
    );
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.wire_api"),
        toml_string("responses"),
    );
    push_codex_config(
        &mut spec.args,
        &format!("model_providers.{provider_id}.requires_openai_auth"),
        "false".to_owned(),
    );

    if provider.auth != AuthStyle::None {
        let key = key_or_error(
            profile_name,
            provider,
            store.credentials.key_for(profile_name, provider),
        )?;
        let env_name = "ALC_PROVIDER_API_KEY";
        spec.set_secret_env(env_name, key);
        push_codex_config(
            &mut spec.args,
            &format!("model_providers.{provider_id}.env_key"),
            toml_string(env_name),
        );
    }
    spec.args.extend_from_slice(passthrough);
    Ok(())
}

fn codex_provider_id(profile: &str) -> String {
    format!("alc_{}", profile.replace('-', "_"))
}

fn push_codex_config(args: &mut Vec<OsString>, key: &str, value: String) {
    args.push(OsString::from("--config"));
    args.push(OsString::from(format!("{key}={value}")));
}

/// Points Codex at alc's in-process bridge for a login Codex cannot sign with
/// itself (agy's). Prepended, so the flags precede any subcommand Codex was
/// given and a `-c` of the user's own still wins. Codex on its own login runs
/// natively and never gets a plan.
///
/// The window travels too: Codex has no metadata for an Antigravity slug,
/// and without one it neither compacts a long session nor shows how much
/// context is left - the session would end on Antigravity's limit instead.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    if spec.provider_kind != ProviderKind::Antigravity {
        return Ok(());
    }
    let id = "alc_antigravity";
    let mut flags = Vec::new();
    push_codex_config(&mut flags, "model_provider", toml_string(id));
    push_codex_config(
        &mut flags,
        &format!("model_providers.{id}.name"),
        toml_string("alc: Antigravity login"),
    );
    push_codex_config(
        &mut flags,
        &format!("model_providers.{id}.base_url"),
        toml_string(&format!("{base_url}/v1")),
    );
    push_codex_config(
        &mut flags,
        &format!("model_providers.{id}.wire_api"),
        toml_string("responses"),
    );
    push_codex_config(
        &mut flags,
        &format!("model_providers.{id}.requires_openai_auth"),
        "false".to_owned(),
    );
    if let Some(window) = plan.context_window {
        push_codex_config(&mut flags, "model_context_window", window.to_string());
    }
    spec.args.splice(0..0, flags);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Agent;
    use crate::launch::{BridgeApi, BridgePlan};

    #[test]
    fn codex_on_antigravity_is_pointed_at_the_bridge_ahead_of_its_subcommand() {
        let mut spec = LaunchSpec::for_test();
        spec.provider_kind = ProviderKind::Antigravity;
        spec.agent = Agent::Codex;
        spec.args = ["--model", "gemini-3.8-flash-low", "exec"]
            .map(OsString::from)
            .to_vec();
        let plan = BridgePlan {
            model: "gemini-3.8-flash-low".to_owned(),
            effort: None,
            context_window: Some(1_048_576),
            options: Vec::new(),
            api: BridgeApi::Responses,
        };
        let unwindowed = spec.clone();
        apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &args[..2],
            ["--config", "model_provider=\"alc_antigravity\""]
        );
        assert!(
            args.contains(
                &"model_providers.alc_antigravity.base_url=\"http://127.0.0.1:9/v1\"".to_owned()
            ),
            "{args:?}"
        );
        assert!(
            args.contains(&"model_providers.alc_antigravity.wire_api=\"responses\"".to_owned()),
            "{args:?}"
        );
        assert_eq!(
            &args[args.len() - 3..],
            ["--model", "gemini-3.8-flash-low", "exec"]
        );
        // Codex has no metadata for an Antigravity slug, so without the
        // window it neither compacts nor shows how much context is left.
        assert!(
            args.contains(&"model_context_window=1048576".to_owned()),
            "{args:?}"
        );

        let mut spec = unwindowed;
        let plan = BridgePlan {
            context_window: None,
            ..plan
        };
        apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();
        assert!(
            !spec
                .args
                .iter()
                .any(|arg| arg.to_string_lossy().starts_with("model_context_window")),
            "no window, no setting"
        );

        let mut native = LaunchSpec::for_test();
        apply_bridge(&mut native, "http://127.0.0.1:9", &plan).unwrap();
        assert!(
            native.args.is_empty(),
            "Codex on its own login has no bridge to point at"
        );
    }
}
