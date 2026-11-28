use std::ffi::OsString;

use anyhow::{Context, Result, bail};

use crate::config::{AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, has_effort_override, has_model_override,
    has_option, key_or_error, openai_style_base_url, toml_string,
};

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    // agy's login is agy's alone; `Config::resolve` refuses it first, and
    // this keeps a path that skips it from launching anyway.
    if provider.kind == ProviderKind::Antigravity {
        bail!(
            "{}",
            crate::config::agy_login_is_agy_only(profile_name, crate::config::Agent::Codex)
        );
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

    // A provider without the Responses API is reached through alc's
    // in-process bridge, which translates; `apply_bridge` points Codex at it.
    if let Some(plan) = crate::launch::api_bridge_plan(
        spec,
        store,
        profile_name,
        provider,
        overrides,
        BridgeApi::Responses,
    )? {
        if !has_model_override(passthrough) {
            spec.args
                .extend([OsString::from("--model"), OsString::from(&plan.model)]);
        }
        spec.bridge = Some(plan);
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

/// Points Codex at alc's in-process bridge for a provider API Codex cannot
/// speak itself. Prepended, so the flags precede any subcommand Codex was
/// given and a `-c` of the user's own still wins. Codex on its own login runs
/// natively and never gets a plan.
///
/// The window travels too: Codex has no metadata for a slug it does not
/// ship, and without one it neither compacts a long session nor shows how
/// much context is left - the session would end on the provider's limit.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    let (id, name) = if plan.upstream.is_some() {
        (
            "alc_bridge",
            format!("alc: {} via alc's bridge", spec.provider_name),
        )
    } else {
        return Ok(());
    };
    let mut flags = Vec::new();
    push_codex_config(&mut flags, "model_provider", toml_string(id));
    push_codex_config(
        &mut flags,
        &format!("model_providers.{id}.name"),
        toml_string(&name),
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
    // The wire id already carries the effort; this only keeps Codex from
    // showing the effort of its own config beside it.
    if let Some(effort) = plan.effort {
        push_codex_config(
            &mut flags,
            "model_reasoning_effort",
            toml_string(effort.as_str()),
        );
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
    fn codex_on_a_bridged_api_is_pointed_at_the_bridge_ahead_of_its_subcommand() {
        let mut spec = LaunchSpec::for_test();
        spec.provider_kind = ProviderKind::Deepseek;
        spec.provider_name = "deepseek".to_owned();
        spec.agent = Agent::Codex;
        spec.args = ["--model", "deepseek-chat", "exec"]
            .map(OsString::from)
            .to_vec();
        let plan = BridgePlan {
            model: "deepseek-chat".to_owned(),
            effort: None,
            context_window: Some(1_048_576),
            options: Vec::new(),
            api: BridgeApi::Responses,
            upstream: Some(crate::config::ApiEndpoint {
                kind: ProviderKind::Deepseek,
                wire: crate::config::ApiWire::Chat,
                url: "https://api.deepseek.com/v1/chat/completions".to_owned(),
                auth: crate::config::AuthStyle::Bearer,
                key: None,
            }),
        };
        let unwindowed = spec.clone();
        apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&args[..2], ["--config", "model_provider=\"alc_bridge\""]);
        assert!(
            args.contains(
                &"model_providers.alc_bridge.base_url=\"http://127.0.0.1:9/v1\"".to_owned()
            ),
            "{args:?}"
        );
        assert!(
            args.contains(&"model_providers.alc_bridge.wire_api=\"responses\"".to_owned()),
            "{args:?}"
        );
        assert_eq!(
            &args[args.len() - 3..],
            ["--model", "deepseek-chat", "exec"]
        );
        // Codex has no metadata for a slug it does not ship, so without the
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
        let plan = BridgePlan {
            upstream: None,
            ..plan
        };
        apply_bridge(&mut native, "http://127.0.0.1:9", &plan).unwrap();
        assert!(
            native.args.is_empty(),
            "Codex on its own login has no bridge to point at"
        );
    }
}
