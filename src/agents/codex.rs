use std::ffi::OsString;

use anyhow::{Context, Result, bail};

use crate::config::{AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    LaunchOverrides, LaunchSpec, has_effort_override, has_model_override, has_option, key_or_error,
    toml_string,
};

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind == ProviderKind::Codex {
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
    let base_url = provider
        .effective_base_url()
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
        toml_string(base_url),
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
        spec.env
            .insert(OsString::from(env_name), OsString::from(key));
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
