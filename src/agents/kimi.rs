use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config::{AuthStyle, Provider, ProviderKind, Store};
use crate::launch::{
    BridgeApi, BridgePlan, FileSetup, LaunchOverrides, LaunchSpec, anthropic_shaped, has_option,
    home_dir, key_or_error, prepend_args, resolve_codex_effort, resolve_codex_model,
};

/// Kimi Code CLI (the `kimi` binary) is driven through a TOML config file
/// (`--config-file <path>`) rather than through environment variables or
/// dedicated CLI flags: `<home>/.kimi/config.toml` (or `ALC_KIMI_CONFIG`
/// when set) holds `[providers.*]` / `[models.*]` tables plus a
/// `default_model` key. alc never overwrites that file — it only reads it
/// (when present) to preserve every unrelated key, merges in an
/// `alc-<profile>` provider/model pair pointing at the resolved endpoint,
/// and writes the merged result to a *fresh* temp file that is passed via
/// `--config-file` and removed once the run ends (`FileSetup::WriteTemp`
/// with `secret: true, cleanup: true`), so the real API key exists on disk
/// only for the lifetime of the child process and never touches argv or
/// dry-run output. A user-supplied `--config-file`/`--config` disables all
/// of this: alc forwards the passthrough verbatim instead.
pub(crate) fn build(
    spec: &mut LaunchSpec,
    store: &Store,
    profile_name: &str,
    provider: &Provider,
    passthrough: &[OsString],
    overrides: &LaunchOverrides,
) -> Result<()> {
    if provider.kind == ProviderKind::Codex {
        let model = overrides
            .model
            .clone()
            .map_or_else(|| resolve_codex_model(provider), Ok)?;
        let effort = overrides
            .reasoning_effort
            .or(provider.reasoning_effort)
            .or(resolve_codex_effort(provider)?);
        spec.bridge = Some(BridgePlan {
            model,
            effort,
            context_window: overrides.context_window,
            options: overrides.model_options.clone(),
            api: BridgeApi::Responses,
        });
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    if has_config_flag(passthrough) {
        spec.args.extend_from_slice(passthrough);
        return Ok(());
    }

    let (provider_type, base_url) = provider_entry(profile_name, provider)?;
    let api_key = if provider.auth == AuthStyle::None {
        "alc".to_owned()
    } else {
        key_or_error(
            profile_name,
            provider,
            store.credentials.key_for(profile_name, provider),
        )?
    };
    let model = overrides.model.as_deref().unwrap_or(&provider.model);

    let path = queue_config_file(
        spec,
        profile_name,
        provider_type,
        &base_url,
        &api_key,
        model,
    )?;
    spec.args.push(OsString::from("--config-file"));
    spec.args.push(path.into_os_string());
    spec.args.extend_from_slice(passthrough);
    Ok(())
}

/// Wires the bundled Codex bridge (listening on `base_url`) into Kimi Code's
/// config file as an `alc-codex` `openai_responses` provider. `build`
/// already copied the user's passthrough into `spec.args` verbatim on the
/// codex branch (the bridge port is not known until now), so the injected
/// `--config-file` flag is prepended ahead of it instead of pushed, and the
/// check for a user-supplied `--config-file`/`--config` reads `spec.args`
/// itself rather than a separate passthrough slice.
pub(crate) fn apply_bridge(spec: &mut LaunchSpec, base_url: &str, plan: &BridgePlan) -> Result<()> {
    if has_config_flag(&spec.args) {
        return Ok(());
    }
    let path = queue_config_file(
        spec,
        "codex",
        "openai_responses",
        &format!("{base_url}/v1"),
        "alc",
        &plan.model,
    )?;
    let path = path.to_string_lossy().into_owned();
    prepend_args(spec, &["--config-file", &path]);
    Ok(())
}

/// Merges `provider_type`/`base_url`/`api_key`/`model` under `alc-<profile>`
/// into the user's existing Kimi config (read fresh every call; alc never
/// caches or mutates it), then queues the merged document as a
/// `FileSetup::WriteTemp` on `spec` and returns the temp path so the caller
/// can inject `--config-file <path>`. The temp file is written only at
/// `launch::execute` time (never on `--dry-run`), is marked secret (0600 on
/// unix) since the merged contents carry the real API key, and is cleaned up
/// once the child process exits.
fn queue_config_file(
    spec: &mut LaunchSpec,
    profile: &str,
    provider_type: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<PathBuf> {
    let config_path = user_config_path();
    let existing = read_existing_config(config_path.as_deref())?;
    let contents = merged_config(
        existing.as_deref(),
        profile,
        provider_type,
        base_url,
        api_key,
        model,
    )
    .with_context(|| match &config_path {
        Some(path) => format!("failed to parse Kimi config at {}", path.display()),
        None => "failed to build the Kimi config".to_owned(),
    })?;

    let temp_path = temp_config_path();
    spec.file_setup.push(FileSetup::WriteTemp {
        path: temp_path.clone(),
        contents,
        secret: true,
        cleanup: true,
    });
    Ok(temp_path)
}

/// The user's Kimi config path: `ALC_KIMI_CONFIG` when set to a non-empty
/// value, else `<home>/.kimi/config.toml`. Pure path arithmetic; never
/// touches disk itself.
fn user_config_path() -> Option<PathBuf> {
    if let Some(value) = env::var_os("ALC_KIMI_CONFIG").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    home_dir().map(|home| home.join(".kimi/config.toml"))
}

/// Reads `path`'s contents when it names an existing file; `None` when there
/// is no configured path or nothing exists there yet (a fresh Kimi install,
/// or a first alc run). A path that exists but cannot be read is an error
/// naming the path, not a silent fallback to an empty config.
fn read_existing_config(path: Option<&Path>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if !path.is_file() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .with_context(|| format!("failed to read Kimi config at {}", path.display()))
}

/// A fresh, unique-per-run path under the OS temp directory; never collides
/// across concurrent `alc` invocations.
fn temp_config_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    env::temp_dir().join(format!("alc-kimi-{}-{nanos}.toml", std::process::id()))
}

/// Whether the user already passed their own `--config-file` or `--config`
/// flag, in which case alc must inject nothing at all (no env, no temp file,
/// no flags) and simply forward `args` verbatim.
fn has_config_flag(args: &[OsString]) -> bool {
    has_option(args, "--config-file", "--config-file") || has_option(args, "--config", "--config")
}

/// The `type` id and base URL for a non-bridge, non-codex provider's config
/// entry: an Anthropic-shaped provider speaks Kimi's `anthropic` type over
/// its Anthropic base URL; `Openai` kind speaks `openai_responses`; every
/// other chat-capable provider speaks `openai_legacy`, both over the
/// provider's plain base URL (verbatim, no Ollama `/v1` special-casing,
/// mirroring the Copilot builder rather than Pi's).
fn provider_entry(profile_name: &str, provider: &Provider) -> Result<(&'static str, String)> {
    if anthropic_shaped(provider) {
        let base_url = provider
            .effective_anthropic_base_url()
            .with_context(|| format!("provider '{profile_name}' needs an Anthropic base URL"))?
            .to_owned();
        Ok(("anthropic", base_url))
    } else if provider.kind == ProviderKind::Openai {
        let base_url = provider
            .effective_base_url()
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?
            .to_owned();
        Ok(("openai_responses", base_url))
    } else {
        let base_url = provider
            .effective_base_url()
            .with_context(|| format!("provider '{profile_name}' needs a base URL"))?
            .to_owned();
        Ok(("openai_legacy", base_url))
    }
}

/// Parses `existing` (when present) as a TOML table, merges in
/// `providers.alc-<profile>`, `models.alc-<profile>`, and `default_model`,
/// preserving every other key untouched, and serializes the result. Starts
/// from an empty table when `existing` is `None` (no user config yet).
/// Bails rather than proceeding when `existing` fails to parse — alc must
/// never paper over a broken user file with an empty one.
fn merged_config(
    existing: Option<&str>,
    profile: &str,
    provider_type: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> Result<String> {
    let mut document = match existing {
        Some(text) => toml::from_str::<toml::Value>(text)
            .context("failed to parse the existing Kimi config as TOML")?,
        None => toml::Value::Table(toml::Table::new()),
    };
    let root = document
        .as_table_mut()
        .context("Kimi config root is not a TOML table")?;

    let key = format!("alc-{profile}");

    let mut provider_table = toml::Table::new();
    provider_table.insert(
        "type".to_owned(),
        toml::Value::String(provider_type.to_owned()),
    );
    provider_table.insert(
        "base_url".to_owned(),
        toml::Value::String(base_url.to_owned()),
    );
    provider_table.insert(
        "api_key".to_owned(),
        toml::Value::String(api_key.to_owned()),
    );

    let providers = root
        .entry("providers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("Kimi config's `providers` key is not a table")?;
    providers.insert(key.clone(), toml::Value::Table(provider_table));

    let mut model_table = toml::Table::new();
    model_table.insert("provider".to_owned(), toml::Value::String(key.clone()));
    model_table.insert("model".to_owned(), toml::Value::String(model.to_owned()));

    let models = root
        .entry("models")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("Kimi config's `models` key is not a table")?;
    models.insert(key.clone(), toml::Value::Table(model_table));

    root.insert("default_model".to_owned(), toml::Value::String(key));

    toml::to_string_pretty(&document).context("failed to serialize the merged Kimi config")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{Agent, Config, Credentials, ReasoningEffort};
    use crate::launch::{self, BridgeApi};
    use crate::model_catalog::ModelCatalog;

    /// Guards `ALC_KIMI_CONFIG` mutation. Exactly one test in this module
    /// (`alc_kimi_config_override_is_honored`) mutates that variable; every
    /// other test only reaches `user_config_path()` indirectly (through
    /// `launch::build`/`apply_bridge`) and only asserts on `contains(..)`
    /// substrings of the merged output, never on its full text or on the
    /// resolved path itself — so a read racing an unsynchronized mutation
    /// cannot fail a sibling assertion here today, the same reasoning
    /// `src/agents/pi.rs` documents for its own `ENV_LOCK`. This lock does
    /// not make those reads synchronized with the mutation — `std::env`
    /// offers no such thing, and readers here don't take the lock. Its real
    /// job is to serialize the mutating test against itself and against any
    /// *future* env-mutating test added to this module, since
    /// `set_var`/`remove_var` are `unsafe` precisely because unsynchronized
    /// concurrent mutation across threads is unsound.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn store(config: Config, credentials: Credentials) -> Store {
        Store {
            dir: PathBuf::from("test"),
            config,
            credentials,
        }
    }

    /// Panics unless `spec` queued exactly one `WriteTemp` entry; returns its
    /// `(path, contents, secret, cleanup)` for the caller to assert against.
    fn only_write_temp(spec: &LaunchSpec) -> (PathBuf, String, bool, bool) {
        match spec.file_setup.as_slice() {
            [
                FileSetup::WriteTemp {
                    path,
                    contents,
                    secret,
                    cleanup,
                },
            ] => (path.clone(), contents.clone(), *secret, *cleanup),
            other => panic!("expected exactly one WriteTemp entry, got {other:?}"),
        }
    }

    // (1) merged_config preserves a foreign `[loop_control]` table and an
    // existing `[providers.mine]` entry while adding `alc-work` +
    // `default_model`.
    #[test]
    fn merged_config_preserves_foreign_keys_and_adds_the_alc_provider() {
        let existing = r#"
[loop_control]
max_iterations = 3

[providers.mine]
type = "openai_responses"
base_url = "https://example.test/v1"
api_key = "existing-key"
"#;

        let merged = merged_config(
            Some(existing),
            "work",
            "openai_responses",
            "https://api.example.com/v1",
            "secret",
            "gpt-5.6-terra",
        )
        .unwrap();

        let document: toml::Value =
            toml::from_str(&merged).expect("merged_config must emit valid, parseable TOML");
        assert_eq!(
            document["loop_control"]["max_iterations"].as_integer(),
            Some(3),
            "a foreign table must survive the merge"
        );
        assert_eq!(
            document["providers"]["mine"]["api_key"].as_str(),
            Some("existing-key"),
            "an existing unrelated provider must survive the merge"
        );
        assert_eq!(
            document["providers"]["alc-work"]["type"].as_str(),
            Some("openai_responses")
        );
        assert_eq!(
            document["providers"]["alc-work"]["base_url"].as_str(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(
            document["providers"]["alc-work"]["api_key"].as_str(),
            Some("secret")
        );
        assert_eq!(
            document["models"]["alc-work"]["provider"].as_str(),
            Some("alc-work")
        );
        assert_eq!(
            document["models"]["alc-work"]["model"].as_str(),
            Some("gpt-5.6-terra")
        );
        assert_eq!(document["default_model"].as_str(), Some("alc-work"));
    }

    // (2) invalid existing TOML must bail rather than silently discard the
    // user's file content.
    #[test]
    fn merged_config_bails_on_invalid_toml() {
        let error = merged_config(
            Some("not [ valid toml"),
            "work",
            "openai_responses",
            "https://api.example.com/v1",
            "secret",
            "gpt-5.6-terra",
        )
        .unwrap_err();

        assert!(
            error.to_string().to_lowercase().contains("toml")
                || error.to_string().to_lowercase().contains("pars"),
            "unexpected error: {error}"
        );
    }

    // (3) openai kind: WriteTemp is pushed (secret+cleanup true), args start
    // with `--config-file <path>`, and the contents carry the openai_responses
    // type plus the alc-openai default_model.
    #[test]
    fn openai_kind_writes_a_temporary_config_and_injects_the_config_file_flag() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Kimi,
            Some("openai"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        let (path, contents, secret, cleanup) = only_write_temp(&spec);
        assert!(
            secret,
            "the merged config holds a real key and must be written secretly"
        );
        assert!(cleanup, "the temp config must be removed after the run");
        assert!(contents.contains(r#"type = "openai_responses""#));
        assert!(contents.contains(r#"default_model = "alc-openai""#));
        assert!(
            contents.contains("secret"),
            "the real key belongs in the file"
        );

        assert_eq!(
            spec.args,
            vec![OsString::from("--config-file"), path.into_os_string()]
        );
    }

    // (4) anthropic kind: the "anthropic" type plus the Anthropic base URL.
    #[test]
    fn anthropic_kind_uses_the_anthropic_type_and_base_url() {
        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("anthropic".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Kimi,
            Some("anthropic"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        let (_, contents, ..) = only_write_temp(&spec);
        assert!(contents.contains(r#"type = "anthropic""#));
        assert!(contents.contains("https://api.anthropic.com"));
        assert!(contents.contains(r#"default_model = "alc-anthropic""#));
    }

    // (4b) deepseek kind: a dual chat+anthropic surface, but chat-first, so
    // anthropic_shaped() must be false and provider_entry's third branch
    // ("openai_legacy") must be reached — not "openai_responses" (deepseek's
    // kind isn't Openai) and not "anthropic" (chat wins over the dual
    // surface), mirroring the qwen/goose deepseek tests. This is the only
    // test exercising the openai_legacy branch of provider_entry.
    #[test]
    fn deepseek_kind_uses_the_openai_legacy_type_and_chat_base_url() {
        let mut config = Config::default();
        config
            .providers
            .insert("ds".into(), Provider::for_kind(ProviderKind::Deepseek));
        let mut credentials = Credentials::default();
        credentials.api_keys.insert("ds".into(), "secret".into());
        let spec = launch::build(
            &store(config, credentials),
            Agent::Kimi,
            Some("ds"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        let (_, contents, ..) = only_write_temp(&spec);
        assert!(contents.contains(r#"type = "openai_legacy""#));
        assert!(contents.contains("https://api.deepseek.com/v1"));
        assert!(
            !contents.contains(r#"type = "anthropic""#),
            "deepseek is dual-surface; the chat route must win over the Anthropic-shaped branch"
        );
        assert!(contents.contains(r#"default_model = "alc-ds""#));
    }

    // (5) a user-supplied --config-file disables alc's own injection
    // entirely: no WriteTemp, no extra args, passthrough forwarded verbatim,
    // and no stored key is required even though "openai" normally needs one.
    #[test]
    fn explicit_config_file_passthrough_disables_injection() {
        let passthrough = [OsString::from("--config-file"), OsString::from("mine.toml")];
        let spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Kimi,
            Some("openai"),
            &passthrough,
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert!(
            spec.file_setup.is_empty(),
            "a user-supplied --config-file must disable alc's own merge/write"
        );
        assert_eq!(
            spec.args,
            vec![OsString::from("--config-file"), OsString::from("mine.toml"),]
        );
    }

    // (5b) the same holds for the shorter --config spelling.
    #[test]
    fn explicit_config_passthrough_also_disables_injection() {
        let passthrough = [OsString::from("--config"), OsString::from("mine.toml")];
        let spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Kimi,
            Some("openai"),
            &passthrough,
            &LaunchOverrides::default(),
        )
        .unwrap();

        assert!(spec.file_setup.is_empty());
        assert_eq!(
            spec.args,
            vec![OsString::from("--config"), OsString::from("mine.toml")]
        );
    }

    // (6) codex bridge: BridgePlan uses the Responses API, and apply_bridge
    // pushes a WriteTemp for `alc-codex` before prepending --config-file
    // ahead of the passthrough `build` already copied verbatim.
    #[test]
    fn codex_bridge_uses_responses_api_and_apply_bridge_writes_alc_codex() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            reasoning_effort: Some(ReasoningEffort::Medium),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let passthrough = [OsString::from("some-positional")];
        let mut spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Kimi,
            Some("codex"),
            &passthrough,
            &overrides,
        )
        .unwrap();
        let plan = spec.bridge.clone().expect("codex bridge plan");
        assert_eq!(plan.api, BridgeApi::Responses);
        assert_eq!(plan.model, "gpt-5.6-terra");
        assert_eq!(spec.args, vec![OsString::from("some-positional")]);

        super::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();

        let (path, contents, secret, cleanup) = only_write_temp(&spec);
        assert!(secret);
        assert!(cleanup);
        assert!(contents.contains(r#"type = "openai_responses""#));
        assert!(contents.contains("http://127.0.0.1:9/v1"));
        assert!(contents.contains(r#"default_model = "alc-codex""#));

        assert_eq!(
            spec.args,
            vec![
                OsString::from("--config-file"),
                path.into_os_string(),
                OsString::from("some-positional"),
            ]
        );
    }

    // (6b) apply_bridge must also honor an explicit --config-file that was
    // already copied into spec.args verbatim by build()'s codex branch.
    #[test]
    fn codex_bridge_honors_an_explicit_config_file_passthrough() {
        let overrides = LaunchOverrides {
            model: Some("gpt-5.6-terra".into()),
            model_options: ModelCatalog::built_in().models,
            ..LaunchOverrides::default()
        };
        let passthrough = [OsString::from("--config-file"), OsString::from("mine.toml")];
        let mut spec = launch::build(
            &store(Config::default(), Credentials::default()),
            Agent::Kimi,
            Some("codex"),
            &passthrough,
            &overrides,
        )
        .unwrap();
        let plan = spec.bridge.clone().expect("codex bridge plan");

        super::apply_bridge(&mut spec, "http://127.0.0.1:9", &plan).unwrap();

        assert!(spec.file_setup.is_empty());
        assert_eq!(
            spec.args,
            vec![OsString::from("--config-file"), OsString::from("mine.toml"),]
        );
    }

    // (7) ALC_KIMI_CONFIG overrides the default `<home>/.kimi/config.toml`
    // path; the merge must read that overridden path's existing content.
    // This is the only test in the module that mutates ALC_KIMI_CONFIG; see
    // ENV_LOCK's doc comment for why holding it for the whole body is enough.
    #[test]
    fn alc_kimi_config_override_is_honored() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = env::var_os("ALC_KIMI_CONFIG");

        // SAFETY: serialized by ENV_LOCK; this is the only test in the
        // module that mutates ALC_KIMI_CONFIG.
        unsafe { env::remove_var("ALC_KIMI_CONFIG") };
        assert_eq!(
            user_config_path(),
            home_dir().map(|home| home.join(".kimi/config.toml")),
            "unset falls back to home/.kimi/config.toml"
        );

        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("config.toml");
        std::fs::write(&config_path, "[loop_control]\nmax_iterations = 7\n").unwrap();
        // SAFETY: see above.
        unsafe { env::set_var("ALC_KIMI_CONFIG", &config_path) };
        assert_eq!(user_config_path(), Some(config_path.clone()));

        let mut credentials = Credentials::default();
        credentials
            .api_keys
            .insert("openai".into(), "secret".into());
        let spec = launch::build(
            &store(Config::default(), credentials),
            Agent::Kimi,
            Some("openai"),
            &[],
            &LaunchOverrides::default(),
        )
        .unwrap();

        let (write_path, contents, ..) = only_write_temp(&spec);
        assert!(
            contents.contains("max_iterations = 7"),
            "the merge must have read the overridden path's existing content"
        );
        assert!(
            !write_path.exists(),
            "building a spec must never perform file I/O; writes happen only in execute()"
        );

        // SAFETY: see above. Restores whatever ALC_KIMI_CONFIG held before
        // this test ran instead of assuming it was unset.
        unsafe {
            match &previous {
                Some(value) => env::set_var("ALC_KIMI_CONFIG", value),
                None => env::remove_var("ALC_KIMI_CONFIG"),
            }
        }
    }
}
