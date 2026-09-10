use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::config::{AuthStyle, Provider, ProviderKind, ReasoningEffort, Store};
use crate::launch::{
    BridgeApi, BridgePlan, LaunchOverrides, LaunchSpec, has_model_override, has_option,
    key_or_error, missing_key, resolve_codex_effort, resolve_codex_model,
};
use crate::model_catalog::ModelInfo;

pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    clear_cloud_provider_env(spec);

    if provider.kind == ProviderKind::Codex {
        if !overrides.model_options.is_empty()
            && !has_option(passthrough, "--settings", "--settings")
        {
            spec.args.extend([
                OsString::from("--settings"),
                OsString::from(claude_model_picker_settings(&overrides.model_options)?),
            ]);
        }
        let model = overrides
            .model
            .clone()
            .unwrap_or(resolve_codex_model(provider)?);
        let effort = overrides
            .reasoning_effort
            .or(provider.reasoning_effort)
            .or(resolve_codex_effort(provider)?)
            .unwrap_or(ReasoningEffort::Medium);
        spec.bridge = Some(BridgePlan {
            model: model.clone(),
            // Claude Code sends the effort with every request, so pinning it
            // on the bridge would freeze the in-session effort slider.
            effort: None,
            context_window: overrides.context_window,
            options: overrides.model_options.clone(),
            api: BridgeApi::Messages,
        });
        if !has_model_override(passthrough) {
            spec.args
                .extend([OsString::from("--model"), OsString::from(model)]);
        }
        if !has_option(passthrough, "--effort", "--effort") {
            spec.args
                .extend([OsString::from("--effort"), OsString::from(effort.as_str())]);
        }
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    spec.args.extend_from_slice(passthrough);
    if !provider.speaks_anthropic() {
        bail!(
            "provider '{profile_name}' speaks {}, but Claude Code needs Anthropic Messages; use an Anthropic-compatible endpoint, OpenRouter, Ollama, or `alc --codex claude`",
            provider.protocol
        );
    }

    let base_url = claude_base_url(provider)
        .with_context(|| format!("provider '{profile_name}' needs an Anthropic base URL"))?;
    spec.env.insert(
        OsString::from("ANTHROPIC_BASE_URL"),
        OsString::from(base_url),
    );
    let model = overrides.model.as_deref().unwrap_or(&provider.model);
    let small_model = provider
        .small_model
        .as_deref()
        .filter(|value| !value.is_empty());
    spec.env
        .insert(OsString::from("ANTHROPIC_MODEL"), OsString::from(model));
    if let Some(small_model) = small_model {
        spec.env.insert(
            OsString::from("ANTHROPIC_SMALL_FAST_MODEL"),
            OsString::from(small_model),
        );
    }
    if let Some(context_window) = overrides.context_window {
        spec.env.insert(
            OsString::from("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            OsString::from(context_window.to_string()),
        );
    }
    if provider.kind == ProviderKind::Ollama {
        apply_local_server_env(spec, model, small_model);
    }

    let key = store.credentials.key_for(profile_name, provider);
    match provider.auth {
        AuthStyle::ApiKey => {
            if let Some(key) = key {
                spec.set_secret_env("ANTHROPIC_API_KEY", key);
                spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
            } else if provider.kind != ProviderKind::Anthropic {
                missing_key(profile_name, provider)?;
            } else {
                // No configured key means the user selected Claude's native
                // login. Do not let an unrelated ambient token override it.
                spec.env_remove.push(OsString::from("ANTHROPIC_API_KEY"));
                spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
            }
        }
        AuthStyle::Bearer => {
            let key = key_or_error(profile_name, provider, key)?;
            spec.set_secret_env("ANTHROPIC_AUTH_TOKEN", key);
            // Claude Code and OpenRouter both require this to be explicitly empty.
            spec.env
                .insert(OsString::from("ANTHROPIC_API_KEY"), OsString::new());
        }
        AuthStyle::Native => {
            spec.env_remove.push(OsString::from("ANTHROPIC_API_KEY"));
            spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
        }
        AuthStyle::None => {
            let token = if provider.kind == ProviderKind::Ollama {
                "ollama"
            } else {
                "alc"
            };
            spec.env.insert(
                OsString::from("ANTHROPIC_AUTH_TOKEN"),
                OsString::from(token),
            );
            spec.env
                .insert(OsString::from("ANTHROPIC_API_KEY"), OsString::new());
        }
    }
    Ok(())
}

/// Everything Claude Code has to be told about a local Ollama server that it
/// would otherwise assume from Anthropic's API.
fn apply_local_server_env(spec: &mut LaunchSpec, model: &str, small_model: Option<&str>) {
    let small = small_model.unwrap_or(model);
    for (name, value) in [
        // Ollama serves only the models that were pulled, so every alias
        // Claude Code resolves on its own (`haiku` for background work, the
        // picker's Default row, `/model sonnet`) has to land on this one
        // instead of a Claude model id the server answers with 404.
        ("ANTHROPIC_DEFAULT_MODEL", model),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", model),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", model),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", small),
        ("ANTHROPIC_SMALL_FAST_MODEL", small),
        // A local server answers one request at a time, so the session-title
        // and similar side requests would queue ahead of the real one for
        // minutes.
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ] {
        spec.env.insert(OsString::from(name), OsString::from(value));
    }
    let is_set = |name: &str| env::var_os(name).is_some_and(|value| !value.is_empty());
    for (name, value) in local_server_timeout_env(is_set) {
        spec.env.insert(OsString::from(name), OsString::from(value));
    }
}

/// Talking to any host other than Anthropic's, Claude Code leaves its
/// runtime's idle timeout on and abandons a request that has not produced a
/// byte after about six minutes; its SDK then caps the wait for response
/// headers at ten. A laptop model can need longer than either just to read
/// the ~30k-token prompt Claude Code opens every session with, and Ollama
/// sends nothing at all until the first generated token.
pub(crate) const LOCAL_SERVER_TIMEOUT_ENV: [(&str, &str); 2] = [
    ("API_FORCE_IDLE_TIMEOUT", "0"),
    ("API_TIMEOUT_MS", "1800000"),
];

/// The timeout variables to inject for a local server, minus any the user
/// already set (`is_set`), whose own value must win.
pub(crate) fn local_server_timeout_env(
    is_set: impl Fn(&str) -> bool,
) -> Vec<(&'static str, &'static str)> {
    LOCAL_SERVER_TIMEOUT_ENV
        .into_iter()
        .filter(|(name, _)| !is_set(name))
        .collect()
}

/// Claude Code lists these rows in `/model`, so the user picks the GPT model
/// inside the session instead of before launch.
fn claude_model_picker_settings(models: &[ModelInfo]) -> Result<String> {
    let options: Vec<Value> = models
        .iter()
        .map(|model| {
            json!({
                "model": model.id,
                "label": model.name,
                "description": model.description,
            })
        })
        .collect();
    serde_json::to_string(&json!({
        "modelPicker": {
            "options": options,
            // Claude's own lineup cannot be served through the Codex adapter.
            "replaceBuiltInOptions": true,
        }
    }))
    .context("failed to encode the Claude Code model picker")
}

pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    // Claude Code resolves its built-in aliases even when the picker lists GPT
    // models, so every alias has to land on a model the adapter can serve.
    let strongest = plan
        .options
        .first()
        .map_or(plan.model.as_str(), |model| model.id.as_str());
    let cheapest = plan
        .options
        .last()
        .map_or(plan.model.as_str(), |model| model.id.as_str());
    for (name, value) in [
        ("ANTHROPIC_MODEL", plan.model.as_str()),
        // Keeps the picker's Default row on a model the adapter can serve.
        ("ANTHROPIC_DEFAULT_MODEL", plan.model.as_str()),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", plan.model.as_str()),
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", strongest),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", cheapest),
        ("ANTHROPIC_SMALL_FAST_MODEL", cheapest),
    ] {
        spec.env.insert(OsString::from(name), OsString::from(value));
    }
    spec.env.insert(
        OsString::from("ANTHROPIC_BASE_URL"),
        OsString::from(base_url),
    );
    // Clients older than the `modelPicker` setting still get one selectable
    // GPT entry from the documented custom-model variables.
    spec.env.insert(
        OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION"),
        OsString::from(plan.model.clone()),
    );
    spec.env.insert(
        OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_NAME"),
        OsString::from(format!("{} via Codex", plan.model)),
    );
    spec.env.insert(
        OsString::from("ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION"),
        OsString::from("Selected by all-code using your Codex login"),
    );
    if let Some(context_window) = plan.context_window {
        spec.env.insert(
            OsString::from("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            OsString::from(context_window.to_string()),
        );
    }
    spec.env.insert(
        OsString::from("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
        OsString::from("1"),
    );
    spec.env_remove.push(OsString::from("ANTHROPIC_API_KEY"));
    spec.env_remove.push(OsString::from("ANTHROPIC_AUTH_TOKEN"));
    Ok(())
}

/// Claude Code's own user-level settings file.
///
/// alc reads it in two places - `pinned_model` below, and the launch guard
/// that puts that one key back - and writes exactly one key of it, only when
/// alc's own adapter is what put a value there. Everything else in the file
/// is Claude Code's.
///
/// `CLAUDE_CONFIG_DIR` moves it, and Claude Code requires that to be
/// absolute; anything else is ignored rather than guessed at, because a
/// relative path here would have alc reading some file inside the current
/// repository and calling it the user's settings.
///
/// Resolved in the shell the user typed into, never in the hub: both inputs
/// come out of the environment, and a daemon started from some other shell
/// days ago would answer for that shell's `CLAUDE_CONFIG_DIR`. The resolved
/// path travels on `LaunchSpec::claude_settings_file` for the same reason
/// `codex_auth_file` does.
pub(crate) fn user_settings_path() -> Option<PathBuf> {
    resolve_user_settings_path(env::var_os("CLAUDE_CONFIG_DIR"), crate::launch::home_dir())
}

/// The resolution itself, taking its inputs rather than reading them, so a
/// test can pin one arrangement without a process-wide environment variable
/// that every other test in the binary shares.
fn resolve_user_settings_path(
    config_dir: Option<OsString>,
    user_home: Option<PathBuf>,
) -> Option<PathBuf> {
    let dir = match config_dir.filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = PathBuf::from(value);
            path.is_absolute().then_some(path)?
        }
        None => user_home?.join(".claude"),
    };
    Some(dir.join("settings.json"))
}

/// The model Claude Code will start every session on when nothing overrides
/// it, read from `settings.json`.
///
/// Worth reading because of where that value comes from: Claude Code writes
/// the model it settles on into that file as "your default for new
/// sessions", so a GPT model reached through `alc --codex claude` becomes the
/// default for plain `claude` too - and plain `claude` has no bridge, so the
/// next session outside alc asks api.anthropic.com for a model it has never
/// heard of and is told so.
///
/// alc still cannot stop Claude Code writing its own settings. What it does
/// instead is read this before a bridged launch and put the value back when
/// the agent exits (`DefaultModelGuard`), which leaves `alc doctor` reading
/// it for the cases a guard cannot cover: a session killed outright, and a
/// pin written by an alc older than 1.8.0.
///
/// A file that is missing, unreadable, or not JSON reads as "no opinion".
/// For the diagnostic that is because a parse error in somebody else's
/// config is not alc's to report; for the guard it is the stronger promise -
/// a file alc cannot parse is a file alc will not rewrite.
pub(crate) fn pinned_model(settings: &Path) -> Option<String> {
    let text = fs::read_to_string(settings).ok()?;
    let document: Value = serde_json::from_str(&text).ok()?;
    document
        .get("model")?
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
}

/// Whether `model` is one only alc's Codex adapter can serve.
///
/// `offered` is the ids alc itself put in Claude Code's picker, which is the
/// precise answer. The `gpt-` prefix answers for the models a Codex newer
/// than that list has and it does not: a stale catalog against a Codex
/// release is how `gpt-6-astra` became unreachable in the first place, and
/// api.anthropic.com serves none of those ids either way.
///
/// Takes the ids rather than a `ModelCatalog` because the two callers have
/// different ones to hand: `alc doctor` has the cache on disk, and the
/// launch guard has the list this session was actually given - with no
/// `Store` in reach, and running inside a hub whose own config directory is
/// not the user's.
pub(crate) fn bridge_only_model(offered: &[String], model: &str) -> bool {
    offered.iter().any(|offered| offered == model) || model.starts_with("gpt-")
}

/// What a bridged session leaves alc to do about Claude Code's own default
/// model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefaultModelFix {
    /// Nothing. Either the key is absent, or it names a model alc's adapter
    /// had no part in - a real Claude model the user picked mid-session is
    /// theirs, and putting the old value back over it would be alc
    /// overruling a choice it was never asked about.
    Leave,
    /// Put this value back: it is what the file said before the session.
    Restore(String),
    /// Drop the key. Reached when the value alc found before the session was
    /// itself adapter-only, which means an earlier session's pin outlived
    /// its guard. Re-pinning it would carry a broken default forward
    /// forever, so it goes, and Claude Code chooses for itself again.
    Remove,
}

/// The decision, over the three things it depends on, so the cases can be
/// asserted rather than assumed: what the file said before the session, what
/// it says now, and which models this session offered.
///
/// The `current: None` arm is the one that needs explaining. A key that has
/// gone missing during the session is nearly always another guard's
/// `Remove`, which is what two overlapping bridged sessions do to each
/// other, and the last one out is then the only thing left that still
/// remembers what the user actually had. So it puts it back, and that is
/// what makes two overlapping sessions converge on the user's own default
/// whichever order they exit in, rather than on no default at all. Claude
/// Code's picker writes values rather than removing them, so the alternative
/// reading, that the user cleared it deliberately mid-session, is the far
/// rarer one.
pub(crate) fn decide_default_model(
    previous: Option<&str>,
    current: Option<&str>,
    offered: &[String],
) -> DefaultModelFix {
    let restorable = previous.filter(|previous| !bridge_only_model(offered, previous));
    let Some(current) = current else {
        return match restorable {
            Some(previous) => DefaultModelFix::Restore(previous.to_owned()),
            None => DefaultModelFix::Leave,
        };
    };
    if !bridge_only_model(offered, current) {
        return DefaultModelFix::Leave;
    }
    match restorable {
        Some(previous) => DefaultModelFix::Restore(previous.to_owned()),
        None => DefaultModelFix::Remove,
    }
}

/// Restores Claude Code's own default model when a bridged session ends.
///
/// This is the one file belonging to another tool that alc writes, and the
/// exception is narrow enough to state in full: alc is what caused the value
/// (it taught Claude Code's picker models only its adapter can serve), it
/// touches the single `model` key and nothing else, and it writes nothing at
/// all unless the value in the file is one of those adapter-only ids. A user
/// who switched to a real Claude model mid-session keeps it.
///
/// Held by `launch::SessionGuards`, so it fires wherever a session ends -
/// `launch::execute` reaping the child, or the hub's pump thread reaching the
/// pty's EOF, tmux sessions included. It is a `Drop`, so `kill -9` skips it;
/// `alc doctor` is the report for what that leaves behind, and the
/// `Remove` arm above is what clears it on the next bridged launch.
///
/// Every failure is swallowed. It runs while a session is being torn down,
/// there is no one left to tell, and a settings file alc could not rewrite
/// is the state `alc doctor` already knows how to describe.
pub(crate) struct DefaultModelGuard {
    settings: PathBuf,
    previous: Option<String>,
    offered: Vec<String>,
}

impl DefaultModelGuard {
    /// Takes the snapshot. Call before the agent starts.
    pub(crate) fn arm(settings: PathBuf, offered: Vec<String>) -> Self {
        let previous = pinned_model(&settings);
        Self {
            settings,
            previous,
            offered,
        }
    }
}

impl Drop for DefaultModelGuard {
    fn drop(&mut self) {
        let fix = decide_default_model(
            self.previous.as_deref(),
            pinned_model(&self.settings).as_deref(),
            &self.offered,
        );
        let value = match &fix {
            DefaultModelFix::Leave => return,
            DefaultModelFix::Restore(model) => Some(model.as_str()),
            DefaultModelFix::Remove => None,
        };
        let _ = write_default_model(&self.settings, value);
    }
}

/// Sets or removes the top-level `model` key of an existing JSON file,
/// leaving every other key as it was.
///
/// The file must already exist and already parse: alc is putting a value
/// back, not deciding that Claude Code should have a settings file. Three
/// things are carried across the write so that it really is one key and
/// nothing else that changes:
///
/// * **The real path.** `atomic_write` renames over its destination, which
///   would replace a symlink with a regular file - and a settings.json
///   symlinked into a dotfiles repository is a common arrangement, where
///   that would both break the link and leave the actual file still pinned.
///   So the link is followed first and the target is what gets written.
/// * **The unix mode.** `atomic_write` creates its temp file at 0644, which
///   would relax a settings file the user restricted; the same care
///   `launch::upsert_json_key` takes over an agent's own config.
/// * **An unguessable temp name**, which is what `secret` buys here rather
///   than any secrecy: the alternative is a fixed `.settings.json.tmp`
///   opened with `truncate`, and two guards firing together - two bridged
///   sessions ended by one `alc hub stop --drain` - would interleave their
///   bytes into it and rename the result over the user's settings. With a
///   random `create_new` name each write lands whole and the last rename
///   wins.
///
/// Rewriting does reformat: serde_json's map is a `BTreeMap` here, so the
/// keys come back sorted and indented its way. That is the cost of the only
/// write alc makes, and it is paid only by a file whose `model` key alc's
/// own adapter is responsible for.
fn write_default_model(settings: &Path, model: Option<&str>) -> Result<()> {
    let settings = &fs::canonicalize(settings)
        .with_context(|| format!("failed to resolve {}", settings.display()))?;
    let text = fs::read_to_string(settings)
        .with_context(|| format!("failed to read {}", settings.display()))?;
    let mut document: Value = serde_json::from_str(&text)
        .with_context(|| format!("failed to parse {} as JSON", settings.display()))?;
    let object = document
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", settings.display()))?;
    match model {
        Some(model) => object.insert("model".to_owned(), Value::String(model.to_owned())),
        None => object.remove("model"),
    };

    #[cfg(unix)]
    let previous_mode = crate::launch::unix_mode(settings)?;

    let encoded = serde_json::to_vec_pretty(&document).context("failed to encode JSON")?;
    crate::config::atomic_write(settings, &encoded, true)?;

    #[cfg(unix)]
    if let Some(mode) = previous_mode {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(settings, fs::Permissions::from_mode(mode))
            .with_context(|| format!("failed to restore permissions on {}", settings.display()))?;
    }
    Ok(())
}

fn clear_cloud_provider_env(spec: &mut LaunchSpec) {
    for name in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        spec.env_remove.push(OsString::from(name));
    }
}

fn claude_base_url(provider: &Provider) -> Option<String> {
    let base = provider
        .effective_anthropic_base_url()?
        .trim_end_matches('/');
    if provider.kind == ProviderKind::Openrouter && base.ends_with("/api/v1") {
        Some(base.trim_end_matches("/v1").to_owned())
    } else {
        Some(base.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_server_timeouts_are_injected_when_unset() {
        assert_eq!(
            local_server_timeout_env(|_| false),
            vec![
                ("API_FORCE_IDLE_TIMEOUT", "0"),
                ("API_TIMEOUT_MS", "1800000")
            ]
        );
    }

    #[test]
    fn the_settings_file_follows_claude_codes_own_rules() {
        // Built rather than spelled: `/work/claude` is an absolute path on
        // unix and a relative one on Windows, which would make this test
        // assert the opposite of itself on one of the two platforms.
        let home = env::temp_dir().join("ada");
        let elsewhere = env::temp_dir().join("work-claude");
        assert!(home.is_absolute() && elsewhere.is_absolute());

        assert_eq!(
            resolve_user_settings_path(None, Some(home.clone())),
            Some(home.join(".claude").join("settings.json"))
        );
        assert_eq!(
            resolve_user_settings_path(
                Some(elsewhere.clone().into_os_string()),
                Some(home.clone())
            ),
            Some(elsewhere.join("settings.json"))
        );
        // Claude Code requires an absolute CLAUDE_CONFIG_DIR and ignores
        // anything else; guessing would have alc reading a file inside
        // whatever repository it happens to be standing in.
        assert_eq!(
            resolve_user_settings_path(Some(OsString::from("relative/claude")), Some(home.clone())),
            None
        );
        assert_eq!(resolve_user_settings_path(None, None), None);
    }

    #[test]
    fn a_pinned_model_is_read_and_anything_unreadable_is_no_opinion() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");

        assert_eq!(pinned_model(&settings), None, "a file that is not there");

        fs::write(
            &settings,
            r#"{"model": "gpt-5.6-sol", "tui": "fullscreen"}"#,
        )
        .unwrap();
        assert_eq!(pinned_model(&settings), Some("gpt-5.6-sol".to_owned()));

        fs::write(&settings, r#"{"tui": "fullscreen"}"#).unwrap();
        assert_eq!(pinned_model(&settings), None, "no model key");

        fs::write(&settings, r#"{"model": "   "}"#).unwrap();
        assert_eq!(pinned_model(&settings), None, "blank is not a model");

        // Someone else's malformed config is not alc's to report.
        fs::write(&settings, "{not json").unwrap();
        assert_eq!(pinned_model(&settings), None);
    }

    #[test]
    fn a_user_set_timeout_variable_is_left_alone() {
        assert_eq!(
            local_server_timeout_env(|name| name == "API_TIMEOUT_MS"),
            vec![("API_FORCE_IDLE_TIMEOUT", "0")]
        );
        assert!(local_server_timeout_env(|_| true).is_empty());
    }

    fn offered() -> Vec<String> {
        vec!["gpt-6-astra".to_owned(), "kimi-k2-thinking".to_owned()]
    }

    #[test]
    fn a_model_is_bridge_only_by_the_offered_list_or_the_gpt_prefix() {
        assert!(bridge_only_model(&offered(), "gpt-6-astra"), "offered");
        // Not in the list and not `gpt-` prefixed, but alc offered it, which
        // is the case the list exists for: a Codex-served id that does not
        // look like one.
        assert!(bridge_only_model(&offered(), "kimi-k2-thinking"));
        // A Codex newer than the catalog alc has. The prefix answers for it.
        assert!(bridge_only_model(&[], "gpt-7-nova"), "prefix, empty list");
        assert!(!bridge_only_model(&offered(), "opus[1m]"));
        assert!(!bridge_only_model(&offered(), "claude-sonnet-4-5"));
    }

    #[test]
    fn the_default_model_decision_covers_every_arrangement() {
        use DefaultModelFix::*;

        // Nothing pinned before or after: alc has no business here.
        assert_eq!(decide_default_model(None, None, &offered()), Leave);
        // The key went missing during the session. Nearly always another
        // guard's `Remove`, so the value this session remembers is the last
        // record of what the user had - see the note on the function.
        assert_eq!(
            decide_default_model(Some("opus[1m]"), None, &offered()),
            Restore("opus[1m]".to_owned())
        );
        assert_eq!(
            decide_default_model(Some("gpt-6-astra"), None, &offered()),
            Leave,
            "nothing worth putting back, and the key is already gone"
        );
        assert_eq!(
            decide_default_model(Some("opus[1m]"), Some("opus[1m]"), &offered()),
            Leave,
            "untouched"
        );
        // Switched to a real Claude model mid-session. That is a choice alc
        // was never asked about, and putting the old value back over it
        // would be alc overruling it.
        assert_eq!(
            decide_default_model(Some("opus[1m]"), Some("sonnet"), &offered()),
            Leave
        );

        // The case the guard exists for.
        assert_eq!(
            decide_default_model(Some("opus[1m]"), Some("gpt-6-astra"), &offered()),
            Restore("opus[1m]".to_owned())
        );

        // Nothing was pinned before, so there is nothing to put back.
        assert_eq!(
            decide_default_model(None, Some("gpt-6-astra"), &offered()),
            Remove
        );
        // An earlier session's pin outlived its guard. Re-pinning it would
        // carry a broken default forward forever, so this is the self-heal.
        assert_eq!(
            decide_default_model(Some("gpt-5.6-sol"), Some("gpt-6-astra"), &offered()),
            Remove
        );
        assert_eq!(
            decide_default_model(Some("gpt-6-astra"), Some("gpt-6-astra"), &offered()),
            Remove,
            "a pin this session never touched is still cleared"
        );
    }

    /// Two bridged sessions overlapping, in both exit orders.
    ///
    /// Session B snapshots the pin session A is still using, so a guard that
    /// only ever restored its own snapshot would have B remove the key and A
    /// then find nothing to put back - leaving the user with no default at
    /// all, which is not what they had either. Both orders must land on
    /// `opus[1m]`.
    #[test]
    fn two_overlapping_sessions_converge_on_what_the_user_actually_had() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");

        for b_first in [true, false] {
            fs::write(&settings, r#"{"model": "opus[1m]"}"#).unwrap();
            let a = DefaultModelGuard::arm(settings.clone(), offered());
            // What Claude Code writes once the session settles on a bridged
            // model, and what B therefore snapshots.
            fs::write(&settings, r#"{"model": "gpt-6-astra"}"#).unwrap();
            let b = DefaultModelGuard::arm(settings.clone(), offered());

            if b_first {
                drop(b);
                drop(a);
            } else {
                drop(a);
                drop(b);
            }
            assert_eq!(
                pinned_model(&settings),
                Some("opus[1m]".to_owned()),
                "b_first={b_first}"
            );
        }
    }

    /// A settings.json symlinked into a dotfiles repository is a common
    /// arrangement, and `atomic_write` renames over its destination - so
    /// without following the link first, the restore would replace the link
    /// with a regular file and leave the real file still pinned.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_settings_file_is_written_through_rather_than_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-settings.json");
        let link = dir.path().join("settings.json");
        fs::write(&real, r#"{"model": "gpt-6-astra", "tui": "fullscreen"}"#).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_default_model(&link, Some("opus[1m]")).unwrap();

        assert!(
            fs::symlink_metadata(&link).unwrap().is_symlink(),
            "still a link"
        );
        assert_eq!(
            pinned_model(&real),
            Some("opus[1m]".to_owned()),
            "the real file"
        );
        let document: Value = serde_json::from_str(&fs::read_to_string(&real).unwrap()).unwrap();
        assert_eq!(document["tui"], "fullscreen");
    }

    #[test]
    fn restoring_the_default_model_leaves_every_other_key_alone() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        fs::write(
            &settings,
            r#"{"$schema": "https://example.test/s.json", "model": "gpt-6-astra",
                "permissions": {"defaultMode": "auto"}, "cleanupPeriodDays": 99999}"#,
        )
        .unwrap();

        write_default_model(&settings, Some("opus[1m]")).unwrap();
        let document: Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(document["model"], "opus[1m]");
        // The keys alc has no opinion about. This is the assertion that
        // catches a rewrite that dropped one.
        assert_eq!(document["$schema"], "https://example.test/s.json");
        assert_eq!(document["permissions"]["defaultMode"], "auto");
        assert_eq!(document["cleanupPeriodDays"], 99999);

        write_default_model(&settings, None).unwrap();
        let document: Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(document.get("model").is_none(), "the key is gone");
        assert_eq!(document["permissions"]["defaultMode"], "auto");
    }

    #[test]
    fn a_guard_writes_nothing_when_there_is_nothing_of_alcs_to_undo() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        let original = "{\n  \"model\": \"opus[1m]\",\n  \"tui\": \"fullscreen\"\n}\n";
        fs::write(&settings, original).unwrap();

        drop(DefaultModelGuard::arm(settings.clone(), offered()));

        // Byte-for-byte: a session that left no adapter-only model behind
        // must not even reformat the file.
        assert_eq!(fs::read_to_string(&settings).unwrap(), original);
    }

    #[test]
    fn a_guard_puts_back_what_it_found_and_clears_a_pin_it_inherited() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");

        fs::write(&settings, r#"{"model": "opus[1m]"}"#).unwrap();
        let guard = DefaultModelGuard::arm(settings.clone(), offered());
        // What Claude Code does when the session settles on a bridged model.
        fs::write(&settings, r#"{"model": "gpt-6-astra"}"#).unwrap();
        drop(guard);
        assert_eq!(pinned_model(&settings), Some("opus[1m]".to_owned()));

        // The same run again, starting from the pin a killed session left.
        let guard = DefaultModelGuard::arm(settings.clone(), offered());
        fs::write(&settings, r#"{"model": "gpt-6-astra"}"#).unwrap();
        drop(guard);
        assert_eq!(pinned_model(&settings), Some("opus[1m]".to_owned()));

        fs::write(&settings, r#"{"model": "gpt-5.6-sol"}"#).unwrap();
        drop(DefaultModelGuard::arm(settings.clone(), offered()));
        assert_eq!(pinned_model(&settings), None, "an inherited pin is cleared");
    }

    #[test]
    fn a_settings_file_alc_cannot_parse_or_find_is_not_written() {
        let dir = tempfile::tempdir().unwrap();

        // Missing: alc is putting a value back, not deciding that Claude
        // Code should have a settings file.
        let absent = dir.path().join("gone").join("settings.json");
        drop(DefaultModelGuard::arm(absent.clone(), offered()));
        assert!(!absent.exists(), "no file and no directory were created");

        // Unparseable: a file alc cannot read is a file alc will not write.
        let broken = dir.path().join("settings.json");
        fs::write(&broken, "{not json").unwrap();
        drop(DefaultModelGuard::arm(broken.clone(), offered()));
        assert_eq!(fs::read_to_string(&broken).unwrap(), "{not json");
    }

    #[cfg(unix)]
    #[test]
    fn restoring_the_default_model_keeps_a_restricted_files_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        fs::write(&settings, r#"{"model": "gpt-6-astra"}"#).unwrap();
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();

        write_default_model(&settings, Some("opus[1m]")).unwrap();

        let mode = fs::metadata(&settings).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "atomic_write's 0644 must not have leaked");
    }
}
