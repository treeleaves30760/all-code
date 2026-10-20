use std::env;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use unicode_width::UnicodeWidthStr;

use crate::config::{
    ADD_ANTIGRAVITY, Agent, AuthStyle, Config, ContextChoice, Provider, ProviderKind,
    ReasoningEffort, Store,
};
use crate::dashboard::{out, outln};
use crate::model_catalog::{CodexSource, ModelCatalog};
use crate::{ollama, openai_server};

pub(crate) const INDENT: &str = "  ";
pub(crate) const GUTTER: &str = "  ";
/// Profile names are elided past this so one outlier cannot stretch every column.
const NAME_LIMIT: usize = 24;

/// The report in the interactive viewer, each section as its check ends.
pub fn run_interactive(store: &Store) -> Result<u8> {
    struct Doctor(std::sync::Arc<Store>);
    impl crate::dashboard::report::Controls for Doctor {
        fn loaders(&self) -> Vec<(String, crate::dashboard::report::Loader)> {
            let store = std::sync::Arc::clone(&self.0);
            vec![(
                "Checks".to_owned(),
                std::sync::Arc::new(move || Ok(if run(&store)? { 0 } else { 1 })),
            )]
        }
    }
    crate::dashboard::report::Viewer::new("alc doctor", Doctor(std::sync::Arc::new(store.clone())))
        .run()
}

pub fn run(store: &Store) -> Result<bool> {
    let theme = Theme::detect();
    let mut issues = Vec::new();
    let codex_bridge_enabled = store
        .config
        .providers
        .values()
        .any(|provider| provider.enabled && provider.kind == ProviderKind::Codex);

    outln!("{}", theme.paint(Tone::Head, "alc doctor"));
    environment(store, &theme, &mut issues);
    binaries(store, &theme, &mut issues);
    profiles(store, &theme, &mut issues);
    defaults(store, &theme);
    if codex_bridge_enabled {
        codex_bridge(store, &theme, &mut issues);
    }
    antigravity(store, &theme);
    claude_code_default_model(store, &theme, &mut issues);
    background_sessions(store, &theme);
    local_models(store, &theme, &mut issues);
    openai_servers(store, &theme, &mut issues);
    remote(store, &theme, &mut issues);
    summary(&theme, &issues);

    Ok(issues.is_empty())
}

fn environment(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    heading(theme, "Environment");
    let validation = match store.config.validate() {
        Ok(()) => format!("{} ok", theme.mark(Status::Good)),
        Err(error) => {
            let error = error.to_string();
            let line = format!(
                "{} {}",
                theme.mark(Status::Bad),
                theme.paint(Tone::Bad, &error)
            );
            issues.push(Issue::new("config", error, None));
            line
        }
    };
    pairs(&[
        ("Config", store.config_path().display().to_string()),
        (
            "Credentials",
            store.credentials_path().display().to_string(),
        ),
        ("Validation", validation),
    ]);
}

fn binaries(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    heading(theme, "Agent binaries");
    let mut rows = Vec::new();

    for agent in Agent::ALL {
        let binary = binary_for(agent);
        match resolve(&binary) {
            Some(path) => {
                // Codex alone carries its version here. alc does not route
                // through it, but its release is what used to decide the
                // model list, and not printing it anywhere is the single
                // reason a picker silently missing GPT-6 took a day to
                // diagnose. `doctor` can afford the spawn; a launch cannot.
                let version = (agent == Agent::Codex)
                    .then(|| codex_version(&path))
                    .flatten()
                    .map(|version| theme.paint(Tone::Dim, &format!("{GUTTER}({version})")))
                    .unwrap_or_default();
                rows.push(Row::new(
                    Status::Good,
                    agent,
                    format!("{}{version}", path.display()),
                ));
            }
            None => {
                let name = binary.to_string_lossy().into_owned();
                // An agent nobody has pointed a default at yet is just not
                // installed; only an explicit default's missing binary is
                // something the user actually needs to fix.
                if store.config.defaults.is_explicit(agent) {
                    rows.push(Row::new(
                        Status::Bad,
                        agent,
                        theme.paint(Tone::Bad, &format!("not found ({name})")),
                    ));
                    issues.push(Issue::new(
                        agent.to_string(),
                        format!("`{name}` is not on PATH"),
                        Some(format!("install {agent}, then reopen your shell")),
                    ));
                } else {
                    rows.push(Row::new(
                        Status::Warn,
                        agent,
                        theme.paint(Tone::Warn, &format!("not found ({name})")),
                    ));
                }
            }
        }
    }

    // The bridge is linked into this binary, so there is nothing to find
    // and nothing that can be a different version from alc.
    rows.push(Row::new(
        Status::Good,
        "adapter",
        format!(
            "built in{}",
            theme.paint(
                Tone::Dim,
                &format!("{GUTTER}({})", crate::launch::bridge_label())
            )
        ),
    ));

    marked(theme, &rows);
}

fn profiles(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    heading(theme, "Provider profiles");
    let mut headers = vec!["PROFILE".to_owned(), "KIND".to_owned(), "KEY".to_owned()];
    headers.extend(Agent::ALL.map(|agent| agent.as_str().to_uppercase()));
    let mut table = Table::new(headers);

    for (name, provider) in &store.config.providers {
        let (key, tone) = key_status(store, name, provider);
        if tone == Tone::Bad {
            issues.push(Issue::new(
                name.clone(),
                "API key missing".to_owned(),
                Some(format!("alc config key {name}")),
            ));
        }

        let mut row = vec![
            Cell::left(
                truncate(name, NAME_LIMIT),
                if provider.enabled {
                    Tone::Plain
                } else {
                    Tone::Dim
                },
            ),
            Cell::left(provider.kind.to_string(), Tone::Dim),
            Cell::left(key, tone),
        ];
        row.extend(Agent::ALL.map(|agent| {
            let status = if provider.supports(agent) {
                Status::Good
            } else {
                Status::Off
            };
            Cell::center(theme.glyph(status), status.tone())
        }));
        table.push(row);
    }

    for line in table.render(theme) {
        outln!("{INDENT}{line}");
    }
}

/// Unusable defaults are already reported by `Config::validate`, so this section
/// only points at which agent is affected instead of counting the problem twice.
fn defaults(store: &Store, theme: &Theme) {
    heading(theme, "Defaults");
    let rows = Agent::ALL.map(|agent| default_row(&store.config, theme, agent));
    marked(theme, &rows);
}

fn default_row(config: &Config, theme: &Theme, agent: Agent) -> Row {
    let name = config.defaults.get(agent);
    match config.providers.get(name) {
        Some(provider) if provider.supports(agent) => {
            Row::new(Status::Good, agent, name.to_owned())
        }
        Some(_) => Row::new(
            Status::Bad,
            agent,
            format!(
                "{name}{GUTTER}{}",
                theme.paint(Tone::Bad, "(cannot run this agent)")
            ),
        ),
        // Every config.toml written before the antigravity kind lacks the
        // profile agy falls back to. That is an upgrade, not a mistake, so it
        // is a warning carrying the command that adds it - unless a profile
        // of the kind under another name already serves the launch, which is
        // what `resolve` does next.
        None if agent == Agent::Agy && !config.defaults.is_explicit(agent) => {
            match config.resolve(agent, None) {
                Ok((resolved, _)) => Row::new(Status::Good, agent, resolved.to_owned()),
                Err(_) => Row::new(
                    Status::Warn,
                    agent,
                    format!(
                        "{name}{GUTTER}{}",
                        theme.paint(Tone::Warn, &format!("(no such profile; {ADD_ANTIGRAVITY})"))
                    ),
                ),
            }
        }
        None => Row::new(
            Status::Bad,
            agent,
            format!(
                "{name}{GUTTER}{}",
                theme.paint(Tone::Bad, "(no such profile)")
            ),
        ),
    }
}

/// Reports a Codex-only model left pinned as Claude Code's own default.
///
/// alc teaches Claude Code's `/model` picker the models the bridge serves, so
/// they can be switched mid-session. Claude Code saves the model it settles
/// on to its user-level `settings.json` as "your default for new sessions",
/// and that file is read by every Claude Code session on the machine -
/// including the ones alc did not start, which have no bridge in front of
/// them. Those ask api.anthropic.com for a GPT model and are told, correctly,
/// that it does not exist.
///
/// Since 1.8.0 alc puts that one key back when a bridged session exits
/// (`agents::claude::DefaultModelGuard`), so anything found here is
/// *leftover*: a session killed outright, so its guard never ran; a pin
/// written by an alc older than 1.8.0; or a value the user set by hand. The
/// remediation says so, and names the launch that clears it - a pre-launch
/// value that is itself bridge-only is removed rather than re-pinned.
///
/// Reported whether or not a Codex profile is still configured, and that
/// matters more now than it did: the pin is a machine-global side effect
/// that outlives the profile that produced it, and somebody who removed the
/// profile will never run the launch that self-heals.
fn claude_code_default_model(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    // Resolved through the profile Claude Code would launch on, so a profile
    // that pins its own config directory is checked there rather than in
    // whatever `~/.claude` the shell points at. An unresolvable default falls
    // back to a profile that pins nothing, which is the environment-only
    // answer this check used before profiles could pin anything.
    let fallback = Provider::for_kind(ProviderKind::Anthropic);
    let provider = store
        .config
        .resolve(Agent::Claude, None)
        .map(|(_, provider)| provider)
        .unwrap_or(&fallback);
    let Some(settings) = crate::agents::claude::user_settings_path(provider) else {
        return;
    };
    let Some(model) = crate::agents::claude::pinned_model(&settings) else {
        return;
    };
    let offered: Vec<String> = ModelCatalog::load(&store.dir)
        .models
        .into_iter()
        .map(|info| info.id)
        .collect();
    if !crate::agents::claude::bridge_only_model(&offered, &model) {
        return;
    }
    heading(theme, "Claude Code settings");
    marked(
        theme,
        &[Row::new(
            Status::Warn,
            "default model",
            format!(
                "{model}{}",
                theme.paint(
                    Tone::Warn,
                    "  plain `claude` cannot reach this; only `alc --codex claude` can"
                )
            ),
        )],
    );
    issues.push(Issue::new(
        "claude settings",
        format!(
            "{} pins model '{model}', which only the Codex bridge serves",
            settings.display()
        ),
        Some(format!(
            "run `alc --codex claude` once and it clears the line on exit, or remove the \"model\" \
             line from {} yourself; alc passes the model itself",
            settings.display()
        )),
    ));
}

/// The bridge Claude Code sessions reach the Codex login through, and whether
/// agent view is switched off. Informational: a bridge that is not running is
/// the normal state between sessions, since the next one starts it.
fn background_sessions(store: &Store, theme: &Theme) {
    heading(theme, "Background sessions");
    let mut rows = crate::bridge_host::status_rows(&store.dir);
    if let Some(reason) = agent_view_switched_off() {
        rows.push((
            "agent view",
            format!("off ({reason}); Claude Code's own setting, not alc's"),
        ));
    }
    pairs(&rows);
}

/// Why Claude Code's agent view is off, when it is.
fn agent_view_switched_off() -> Option<String> {
    if env::var_os("CLAUDE_CODE_DISABLE_AGENT_VIEW").is_some_and(|value| !value.is_empty()) {
        return Some("CLAUDE_CODE_DISABLE_AGENT_VIEW".to_owned());
    }
    let settings = crate::agents::claude::resolve_claude_config_dir(
        None,
        env::var_os("CLAUDE_CONFIG_DIR"),
        crate::launch::home_dir(),
    )?
    .join("settings.json");
    let text = std::fs::read_to_string(&settings).ok()?;
    let document: serde_json::Value = serde_json::from_str(&text).ok()?;
    (document.get("disableAgentView") == Some(&serde_json::Value::Bool(true)))
        .then(|| format!("disableAgentView in {}", settings.display()))
}

fn codex_bridge(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    heading(theme, "Codex bridge");
    outln!(
        "{INDENT}{}",
        theme.paint(Tone::Dim, "serves every agent through one `codex login`")
    );
    let catalog = ModelCatalog::load(&store.dir);
    let mut rows = Vec::new();

    for (name, provider) in &store.config.providers {
        if !provider.enabled || provider.kind != ProviderKind::Codex {
            continue;
        }
        let (mut status, model) = match crate::launch::resolve_codex_model(provider) {
            Ok(model) => (Status::Good, model),
            Err(_) => (Status::Warn, "<unresolved>".to_owned()),
        };
        // Resolving says nothing about whether the bridge can serve it. A
        // green tick against a model every launch refuses sends the reader
        // looking for the problem somewhere it is not.
        //
        // Only asked when a model was actually resolved: `<unresolved>` is a
        // placeholder, not a slug, and reporting that the bridge cannot route
        // it would be true of every string that is not a model.
        let unroutable = status != Status::Warn
            && crate::launch::bridge_codex_models()
                .is_some_and(|routable| !routable.contains(&model));
        if unroutable {
            status = Status::Bad;
            // Names a model out of the catalog rather than a slug written
            // down here, so the advice cannot go stale the way the catalog
            // itself once did.
            let suggestion = catalog
                .models
                .first()
                .map_or("gpt-6.1-sol", |entry| entry.id.as_str());
            issues.push(Issue::new(
                name.clone(),
                format!("the bridge cannot route '{model}'"),
                Some(format!("alc config upsert {name} --model {suggestion}")),
            ));
        }
        let effort = crate::launch::resolve_codex_effort(provider)
            .ok()
            .flatten()
            .or_else(|| catalog.find(&model).map(|entry| entry.default_effort))
            .unwrap_or(ReasoningEffort::Medium);
        // The standard window is Codex's own default and goes unsaid; the
        // maximum spends plan limits faster past it, so it is named.
        let context = if provider.context == ContextChoice::Max {
            theme.paint(Tone::Dim, " / max context")
        } else {
            String::new()
        };
        let note = if unroutable {
            theme.paint(Tone::Bad, "  bridge cannot route this")
        } else {
            String::new()
        };
        rows.push(Row::new(
            status,
            name.clone(),
            format!(
                "{model}{}{context}{note}",
                theme.paint(Tone::Dim, &format!(" / {effort}"))
            ),
        ));
    }

    match codex_login_status() {
        Some(true) => rows.push(Row::new(Status::Good, "login", "signed in".to_owned())),
        Some(false) => {
            rows.push(Row::new(
                Status::Bad,
                "login",
                theme.paint(Tone::Bad, "signed out or expired"),
            ));
            issues.push(Issue::new(
                "codex login",
                "signed out or expired".to_owned(),
                Some("codex login".to_owned()),
            ));
        }
        None => rows.push(Row::new(
            Status::Warn,
            "login",
            theme.paint(Tone::Warn, "cannot check (Codex binary missing)"),
        )),
    }
    rows.push(Row::blank(
        "catalog",
        theme.paint(Tone::Dim, &catalog.source),
    ));
    // The models alc had to supply itself. A catalog that is complete only
    // because alc insisted looks identical to one the source agreed with,
    // and telling the two apart is what this whole section could not do when
    // GPT-6 went missing.
    for id in &catalog.unreported {
        rows.push(Row::blank(
            "",
            theme.paint(
                Tone::Dim,
                &format!("{id} restored from the catalog alc ships"),
            ),
        ));
    }
    // The stamp the catalog was written against versus the Codex that is
    // installed now. A disagreement means the list on disk predates the Codex
    // release this machine actually has, which is exactly the window a user
    // upgrading Codex used to fall into.
    //
    // A catalog that has never been synced is excluded rather than warned
    // about, because "written before the installed Codex release" would be a
    // plain falsehood: the bundled fallback was written before every Codex
    // release, it ships inside the binary, and `refreshed_at == 0` is how it
    // says so. Warning there put a yellow row and a remediation on a fresh
    // install whose only fault was not having launched anything yet - and the
    // first launch syncs it without being asked.
    let installed_stamp = CodexSource::detect().cache_stamp();
    let ever_synced = catalog.refreshed_at != 0;
    if ever_synced && installed_stamp.is_some() && installed_stamp != catalog.codex_cache_stamp {
        rows.push(Row::new(
            Status::Warn,
            "catalog",
            theme.paint(Tone::Warn, "written before the installed Codex release"),
        ));
        issues.push(Issue::new(
            "codex catalog",
            "the model catalog predates the installed Codex".to_owned(),
            Some("alc models --refresh".to_owned()),
        ));
    }
    if let Some(reason) = &catalog.fallback_reason {
        rows.push(Row::new(
            Status::Warn,
            "catalog",
            theme.paint(Tone::Warn, reason),
        ));
        issues.push(Issue::new(
            "codex catalog",
            reason.clone(),
            Some("alc models --refresh".to_owned()),
        ));
    }

    marked(theme, &rows);
}

/// The Antigravity CLI and agy's login: is agy installed, is its token ready
/// (offline), does the account have a project (one read-only call, only for
/// a ready token), and which profile `alc agy` launches. Never the token
/// itself.
///
/// Raises no issue of its own. A missing agy that is an explicit default is
/// already one in the binaries section, and an explicit default naming a
/// profile that is gone is already one from `Config::validate`; counting
/// either again here would double the summary.
fn antigravity(store: &Store, theme: &Theme) {
    use crate::bridge::agy_auth::{self, AgyAuthError, LoginState};
    heading(theme, "Antigravity");
    outln!(
        "{INDENT}{}",
        theme.paint(
            Tone::Dim,
            "agy owns its Google login and renews it; alc borrows its access token for the other agents"
        )
    );
    let binary = binary_for(Agent::Agy);
    let found = resolve(&binary);
    let (login, project) = match agy_auth::AgyLocator::resolve() {
        Ok(locator) => {
            // One read of the store: the state and, for a fresh token, the
            // token the project lookup is signed with.
            let loaded = agy_auth::load(&locator.store());
            let login = agy_auth::state_of(&loaded, agy_auth::now_secs());
            // One read-only lookup, and only for a fresh token: doctor never renews.
            let project = match (&login, &loaded) {
                (LoginState::Ready { .. }, Ok(token)) => Some(
                    crate::usage::quota::antigravity_project(token)
                        .map(|project| project.is_some())
                        .map_err(|outcome| outcome.error.unwrap_or_else(|| "no answer".to_owned())),
                ),
                _ => None,
            };
            (login, project)
        }
        Err(_) => (LoginState::Missing(AgyAuthError::LoginRequired), None),
    };
    let facts = AgyFacts {
        binary,
        found,
        login,
        project,
        // Off the disk only: asking agy is a network round trip, and doctor
        // stays fast and writes nothing.
        models: crate::antigravity::source_on_disk(&store.dir),
    };
    marked(theme, &antigravity_rows(&store.config, theme, &facts));
}

/// What the Antigravity section reports on, gathered by the caller so the
/// rows can be built without touching PATH, the credential store or the
/// network.
struct AgyFacts {
    binary: std::ffi::OsString,
    found: Option<PathBuf>,
    login: crate::bridge::agy_auth::LoginState,
    /// `Some` only for a fresh token: whether a Cloud Code project exists, or
    /// why that could not be asked.
    project: Option<Result<bool, String>>,
    models: String,
}

fn antigravity_rows(config: &Config, theme: &Theme, facts: &AgyFacts) -> Vec<Row> {
    use crate::bridge::agy_auth::{AgyAuthError, LoginState};
    let mut rows = Vec::new();
    let explicit = config.defaults.is_explicit(Agent::Agy);

    match &facts.found {
        Some(path) => rows.push(Row::new(Status::Good, "agy", path.display().to_string())),
        None => {
            // The same rule, and words, as the binaries section.
            let status = if explicit { Status::Bad } else { Status::Warn };
            let name = facts.binary.to_string_lossy();
            rows.push(Row::new(
                status,
                "agy",
                theme.paint(status.tone(), &format!("not found ({name})")),
            ));
        }
    }

    // The token's state, read offline; never the token. An expired token is
    // routine (agy's last about an hour) and healthy as long as agy is on
    // PATH to renew it at the next launch; without agy it is stranded.
    rows.push(match facts.login {
        LoginState::Ready { .. } => Row::new(
            Status::Good,
            "login",
            "ready (agy's Google login)".to_owned(),
        ),
        LoginState::Expired if facts.found.is_some() => Row::new(
            Status::Good,
            "login",
            "expired; agy renews it on the next launch".to_owned(),
        ),
        LoginState::Expired => Row::new(
            Status::Warn,
            "login",
            theme.paint(Tone::Warn, AgyAuthError::AgyNotInstalled.message()),
        ),
        LoginState::Missing(AgyAuthError::LoginRequired) => Row::new(
            Status::Warn,
            "login",
            theme.paint(Tone::Warn, "not logged in; run `agy` and sign in"),
        ),
        LoginState::Missing(error) => Row::new(
            Status::Warn,
            "login",
            theme.paint(Tone::Warn, error.message()),
        ),
    });
    match &facts.project {
        Some(Ok(true)) => rows.push(Row::new(
            Status::Good,
            "project",
            "onboarded (a Cloud Code project exists)".to_owned(),
        )),
        Some(Ok(false)) => rows.push(Row::new(
            Status::Warn,
            "project",
            theme.paint(
                Tone::Warn,
                "not onboarded; open agy once and finish its setup",
            ),
        )),
        Some(Err(reason)) => rows.push(Row::new(
            Status::Warn,
            "project",
            theme.paint(Tone::Warn, reason),
        )),
        None => {}
    }

    rows.push(match config.resolve(Agent::Agy, None) {
        Ok((name, provider)) => {
            let model = match provider.model.trim() {
                "" => "agy's default model",
                model => model,
            };
            // Shown as the launch sends it: agy has no `ultra`.
            let effort = provider
                .reasoning_effort
                .map(crate::agents::agy::clamp_for_agy)
                .map_or_else(|| "agy's default effort".to_owned(), |e| e.to_string());
            Row::new(
                Status::Good,
                "profile",
                format!(
                    "{name}{GUTTER}{model}{}",
                    theme.paint(Tone::Dim, &format!(" / {effort}"))
                ),
            )
        }
        Err(error) => {
            let status = if explicit { Status::Bad } else { Status::Warn };
            let none_of_the_kind = !config
                .providers
                .values()
                .any(|provider| provider.kind == ProviderKind::Antigravity);
            // The upsert hint only where the name is free: `upsert` on a
            // taken name rewrites that profile for the new kind.
            let taken = config.providers.get("antigravity");
            let detail = match taken {
                None if none_of_the_kind && !explicit => {
                    format!("no antigravity profile; {ADD_ANTIGRAVITY}")
                }
                Some(other) if !explicit && other.kind != ProviderKind::Antigravity => format!(
                    "the profile named 'antigravity' is {}, not the antigravity kind; add one under another name with `alc config upsert <name> --kind antigravity` and make it agy's default with `alc config set-default agy <name>`",
                    other.kind
                ),
                _ => error.to_string(),
            };
            Row::new(status, "profile", theme.paint(status.tone(), &detail))
        }
    });

    rows.push(Row::blank("models", theme.paint(Tone::Dim, &facts.models)));
    rows.push(Row::new(
        Status::Good,
        "bridge",
        if cfg!(windows) {
            "every agent can run on this login through alc's bridge".to_owned()
        } else {
            "every agent can run on this login through alc's bridge (verified on Windows; reading agy's login here is best effort)".to_owned()
        },
    ));
    rows
}

/// The installed Codex's own version string, for the one row that reports it.
fn codex_version(binary: &Path) -> Option<String> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // `codex-cli 0.154.0`: the trailing token, so a rename of the product
    // prefix does not cost the version.
    let version = text.split_whitespace().next_back()?.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// Claude Code opens every session with a prompt of roughly 25k-40k tokens
/// (system prompt, tool schemas, project context); a smaller window means
/// Ollama silently truncates it before the model ever sees the request.
const MIN_LOCAL_CONTEXT_TOKENS: u64 = 65_536;

/// Every enabled Ollama profile, checked against the running server: is it
/// up, is the model pulled, can the model call tools, and how much context
/// does it really get. All three answers come from local metadata calls that
/// give up after a moment, so a stopped server only costs a warning.
fn local_models(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    let profiles: Vec<_> = store
        .config
        .providers
        .iter()
        .filter(|(_, provider)| provider.enabled && provider.kind == ProviderKind::Ollama)
        .collect();
    if profiles.is_empty() {
        return;
    }

    heading(theme, "Ollama");
    outln!(
        "{INDENT}{}",
        theme.paint(
            Tone::Dim,
            "coding agents need a model that calls tools and a context window that fits their first prompt"
        )
    );
    let mut rows = Vec::new();
    for (name, provider) in profiles {
        let Some(root) = ollama::api_root(provider) else {
            continue;
        };
        match ollama::server_version(&root) {
            Some(version) => rows.push(Row::new(
                Status::Good,
                name.clone(),
                format!(
                    "{root}{}",
                    theme.paint(Tone::Dim, &format!("  Ollama {version}"))
                ),
            )),
            None => {
                rows.push(Row::new(
                    Status::Bad,
                    name.clone(),
                    theme.paint(Tone::Bad, &format!("no server answering at {root}")),
                ));
                issues.push(Issue::new(
                    name.clone(),
                    format!("Ollama is not answering at {root}"),
                    Some("start Ollama, or fix the profile's base_url".to_owned()),
                ));
                continue;
            }
        }

        let model = provider.model.as_str();
        let Some(facts) = ollama::model_facts(&root, model) else {
            rows.push(Row::new(
                Status::Bad,
                model,
                theme.paint(Tone::Bad, "not pulled"),
            ));
            issues.push(Issue::new(
                name.clone(),
                format!("model '{model}' is not pulled"),
                Some(format!("ollama pull {model}")),
            ));
            continue;
        };
        let context = match facts.context_length {
            Some(tokens) => format!("{tokens} tokens of context"),
            None => "unknown context length".to_owned(),
        };
        if !facts.supports_tools() {
            rows.push(Row::new(
                Status::Bad,
                model,
                format!(
                    "{}{}",
                    theme.paint(Tone::Bad, "cannot call tools"),
                    theme.paint(Tone::Dim, &format!("  {context}"))
                ),
            ));
            issues.push(Issue::new(
                name.clone(),
                format!("model '{model}' cannot call tools, which every coding agent relies on"),
                Some("pick a model whose `ollama show` lists the tools capability".to_owned()),
            ));
        } else if facts
            .context_length
            .is_some_and(|tokens| tokens < MIN_LOCAL_CONTEXT_TOKENS)
        {
            rows.push(Row::new(
                Status::Warn,
                model,
                format!(
                    "tools{}",
                    theme.paint(
                        Tone::Warn,
                        &format!(
                            "  {context}, below the {MIN_LOCAL_CONTEXT_TOKENS} Claude Code needs"
                        )
                    )
                ),
            ));
            issues.push(Issue::new(
                name.clone(),
                format!("model '{model}' gets only {context}; Claude Code's first request alone can be 40k"),
                Some(
                    "raise the context length in Ollama's settings (or OLLAMA_CONTEXT_LENGTH)"
                        .to_owned(),
                ),
            ));
        } else {
            rows.push(Row::new(
                Status::Good,
                model,
                format!("tools{}", theme.paint(Tone::Dim, &format!("  {context}"))),
            ));
        }
    }
    marked(theme, &rows);
}

/// Every enabled llama.cpp and vLLM profile, checked against its server: is
/// it up and does it take the key, does it list the model, how much context
/// does one request get, and is there an Anthropic Messages route for Claude
/// Code. The route is asked with an empty body, which the server refuses
/// before any model reads a token, so this costs a shared server nothing.
fn openai_servers(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    let profiles: Vec<_> = store
        .config
        .providers
        .iter()
        .filter(|(_, provider)| {
            provider.enabled && matches!(provider.kind, ProviderKind::Llamacpp | ProviderKind::Vllm)
        })
        .collect();
    if profiles.is_empty() {
        return;
    }

    heading(theme, "llama.cpp and vLLM");
    outln!(
        "{INDENT}{}",
        theme.paint(
            Tone::Dim,
            "Claude Code needs the server's /v1/messages; the context shown is what one request gets"
        )
    );
    let mut rows = Vec::new();
    for (name, provider) in profiles {
        let Some(root) = openai_server::root(provider) else {
            continue;
        };
        let key = store.credentials.key_for(name, provider);
        let model = provider.model.as_str();
        let facts = match openai_server::inspect(provider, key.as_deref(), model) {
            openai_server::Reach::Answered(facts) => facts,
            openai_server::Reach::Refused(status @ (401 | 403)) => {
                let (problem, fix) = if key.is_some() {
                    (
                        format!("{root} refused this profile's key ({status})"),
                        format!("save the key the server was started with: alc config key {name}"),
                    )
                } else {
                    (
                        format!("{root} wants an API key ({status})"),
                        format!("alc config key {name}"),
                    )
                };
                rows.push(Row::new(
                    Status::Bad,
                    name.clone(),
                    theme.paint(Tone::Bad, &problem),
                ));
                issues.push(Issue::new(name.clone(), problem, Some(fix)));
                continue;
            }
            openai_server::Reach::Refused(status) => {
                let problem = format!("{root}/v1/models answered {status}");
                rows.push(Row::new(
                    Status::Bad,
                    name.clone(),
                    theme.paint(Tone::Bad, &problem),
                ));
                issues.push(Issue::new(
                    name.clone(),
                    problem,
                    Some("check the profile's base_url; it should end in /v1".to_owned()),
                ));
                continue;
            }
            openai_server::Reach::Unreachable => {
                rows.push(Row::new(
                    Status::Bad,
                    name.clone(),
                    theme.paint(Tone::Bad, &format!("no server answering at {root}")),
                ));
                issues.push(Issue::new(
                    name.clone(),
                    format!("nothing is answering at {root}"),
                    Some(
                        "start the server (or its SSH tunnel), or fix the profile's base_url"
                            .to_owned(),
                    ),
                ));
                continue;
            }
        };
        let engine = facts
            .engine
            .as_deref()
            .map(|engine| theme.paint(Tone::Dim, &format!("  {engine}")))
            .unwrap_or_default();
        rows.push(Row::new(
            Status::Good,
            name.clone(),
            format!("{root}{engine}"),
        ));

        let mut status = Status::Good;
        let mut notes = Vec::new();
        if facts.serves(model) {
            notes.push("served".to_owned());
        } else {
            let listed = facts.models.join(", ");
            // One model loaded in llama-server answers under any name, so the
            // mismatch costs nothing but a misleading name; anywhere else the
            // request is a 404.
            let lenient = provider.kind == ProviderKind::Llamacpp && facts.models.len() == 1;
            status = if lenient { Status::Warn } else { Status::Bad };
            notes.push(theme.paint(
                if lenient { Tone::Warn } else { Tone::Bad },
                &format!("not listed; the server serves {listed}"),
            ));
            issues.push(Issue::new(
                name.clone(),
                format!("model '{model}' is not one the server lists ({listed})"),
                facts
                    .models
                    .first()
                    .map(|first| format!("alc config upsert {name} --model {first}")),
            ));
        }
        match facts.context_length {
            Some(tokens) if tokens < MIN_LOCAL_CONTEXT_TOKENS => {
                if status == Status::Good {
                    status = Status::Warn;
                }
                notes.push(theme.paint(
                    Tone::Warn,
                    &format!("{tokens} tokens of context, below the {MIN_LOCAL_CONTEXT_TOKENS} Claude Code needs"),
                ));
                issues.push(Issue::new(
                    name.clone(),
                    format!("one request gets only {tokens} tokens; Claude Code's first request alone can be 40k"),
                    Some(match provider.kind {
                        ProviderKind::Vllm => "start vLLM with a larger --max-model-len".to_owned(),
                        _ => "start llama-server with a larger --ctx-size (it is split across --parallel slots)".to_owned(),
                    }),
                ));
            }
            Some(tokens) => notes.push(format!("{tokens} tokens of context")),
            None => notes.push(theme.paint(Tone::Dim, "unknown context length")),
        }
        match openai_server::serves_messages(provider, key.as_deref()) {
            Some(true) => notes.push("Anthropic Messages".to_owned()),
            Some(false) => {
                status = Status::Bad;
                notes.push(theme.paint(Tone::Bad, "no /v1/messages"));
                issues.push(Issue::new(
                    name.clone(),
                    format!("{root} has no Anthropic Messages route, which Claude Code needs; the other agents are unaffected"),
                    Some(match provider.kind {
                        ProviderKind::Vllm => "update vLLM".to_owned(),
                        _ => "update llama.cpp".to_owned(),
                    }),
                ));
            }
            None => {}
        }
        rows.push(Row::new(status, model, notes.join("  ")));
    }
    marked(theme, &rows);
}

pub(crate) fn summary(theme: &Theme, issues: &[Issue]) {
    out!("{}", summary_text(theme, issues));
}

/// The same block as a string, for the reports that build one rather than
/// streaming to a terminal. `alc usage` renders through this so the two
/// commands cannot drift apart.
pub(crate) fn summary_text(theme: &Theme, issues: &[Issue]) -> String {
    let mut out = String::from("\n");
    if issues.is_empty() {
        out.push_str(&format!(
            "{} {}\n",
            theme.mark(Status::Good),
            theme.paint(Tone::Good, "ready")
        ));
        return out;
    }

    let count = issues.len();
    let noun = if count == 1 { "issue" } else { "issues" };
    out.push_str(&format!(
        "{} {}\n",
        theme.mark(Status::Bad),
        theme.paint(
            Tone::Bad,
            &format!("needs attention {} {count} {noun}", theme.dash())
        )
    ));

    let subject = issues
        .iter()
        .map(|issue| width(&issue.subject))
        .max()
        .unwrap_or_default();
    // Only the issues that carry a fix line up their arrows; a long fix-less problem
    // would otherwise push that column off to the right.
    let problem = issues
        .iter()
        .filter(|issue| issue.fix.is_some())
        .map(|issue| width(&issue.problem))
        .max()
        .unwrap_or_default();
    for issue in issues {
        let fix = match &issue.fix {
            Some(fix) => format!(
                "{GUTTER}{}{GUTTER}{}",
                theme.arrow(),
                theme.paint(Tone::Dim, fix)
            ),
            None => String::new(),
        };
        let tail = pad(
            &issue.problem,
            if fix.is_empty() { 0 } else { problem },
            Align::Left,
        );
        out.push_str(&format!(
            "{INDENT}{} {}{GUTTER}{tail}{fix}\n",
            theme.bullet(),
            pad(&issue.subject, subject, Align::Left)
        ));
    }
    out
}

/// Remote control's posture.
///
/// Almost all of this is informational: a user who never turns sharing on
/// should not start failing `alc doctor` because the feature exists. Only a
/// credential another account can read is a real problem.
fn remote(store: &Store, theme: &Theme, issues: &mut Vec<Issue>) {
    let report = crate::remote::report(store);
    heading(theme, "Remote control");
    pairs(&report.rows);
    for (subject, problem, fix) in report.issues {
        issues.push(Issue::new(subject, problem, Some(fix)));
    }
}

pub(crate) fn heading(theme: &Theme, title: &str) {
    out!("{}", heading_text(theme, title));
}

/// See [`summary_text`].
pub(crate) fn heading_text(theme: &Theme, title: &str) -> String {
    format!("\n{}\n", theme.paint(Tone::Head, title))
}

/// Prints `label  value` rows with the labels padded to a common width.
fn pairs(rows: &[(&str, String)]) {
    let label = rows
        .iter()
        .map(|(label, _)| width(label))
        .max()
        .unwrap_or_default();
    for (name, value) in rows {
        outln!("{INDENT}{}{GUTTER}{value}", pad(name, label, Align::Left));
    }
}

/// Prints `glyph  name  detail` rows with the names padded to a common width.
pub(crate) fn marked(theme: &Theme, rows: &[Row]) {
    let name = rows
        .iter()
        .map(|row| width(&row.name))
        .max()
        .unwrap_or_default();
    for row in rows {
        let mark = match row.status {
            Some(status) => theme.mark(status),
            None => " ".to_owned(),
        };
        outln!(
            "{INDENT}{mark}{GUTTER}{}{GUTTER}{}",
            pad(&row.name, name, Align::Left),
            row.detail
        );
    }
}

fn key_status(store: &Store, name: &str, provider: &Provider) -> (String, Tone) {
    if provider
        .api_key_env
        .as_deref()
        .and_then(|variable| env::var(variable).ok())
        .is_some_and(|value| !value.is_empty())
    {
        ("env".to_owned(), Tone::Good)
    } else if store.credentials.api_keys.contains_key(name) {
        ("saved".to_owned(), Tone::Good)
    } else if matches!(provider.auth, AuthStyle::Native | AuthStyle::None) {
        ("n/a".to_owned(), Tone::Dim)
    } else if provider.kind == ProviderKind::Anthropic {
        ("native/key".to_owned(), Tone::Plain)
    } else {
        ("missing".to_owned(), Tone::Bad)
    }
}

/// A `glyph  name  detail` line; `status` is `None` for rows that only carry information.
pub(crate) struct Row {
    status: Option<Status>,
    name: String,
    detail: String,
}

impl Row {
    pub(crate) fn new(status: Status, name: impl ToString, detail: String) -> Self {
        Self {
            status: Some(status),
            name: name.to_string(),
            detail,
        }
    }

    pub(crate) fn blank(name: impl ToString, detail: String) -> Self {
        Self {
            status: None,
            name: name.to_string(),
            detail,
        }
    }
}

/// One reason `alc doctor` reports failure, with the command that clears it.
pub(crate) struct Issue {
    subject: String,
    problem: String,
    fix: Option<String>,
}

impl Issue {
    pub(crate) fn new(subject: impl Into<String>, problem: String, fix: Option<String>) -> Self {
        Self {
            subject: subject.into(),
            problem,
            fix,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Good,
    Warn,
    Bad,
    Off,
}

impl Status {
    pub(crate) fn tone(self) -> Tone {
        match self {
            Self::Good => Tone::Good,
            Self::Warn => Tone::Warn,
            Self::Bad => Tone::Bad,
            Self::Off => Tone::Dim,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Plain,
    Head,
    Dim,
    Good,
    Warn,
    Bad,
}

impl Tone {
    fn code(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Head => Some("1"),
            Self::Dim => Some("90"),
            Self::Good => Some("32"),
            Self::Warn => Some("33"),
            Self::Bad => Some("31"),
        }
    }
}

/// What the stream the report is written to can render.
pub(crate) struct Theme {
    color: bool,
    unicode: bool,
}

impl Theme {
    pub(crate) fn detect() -> Self {
        Self {
            color: color_supported(),
            unicode: unicode_supported(),
        }
    }

    /// A theme with both capabilities pinned, so a test asserts on one
    /// rendering rather than on whatever the machine running it supports.
    #[cfg(test)]
    pub(crate) fn for_test(color: bool, unicode: bool) -> Self {
        Self { color, unicode }
    }

    pub(crate) fn paint(&self, tone: Tone, text: &str) -> String {
        match tone.code() {
            Some(code) if self.color => format!("\x1b[{code}m{text}\x1b[0m"),
            _ => text.to_owned(),
        }
    }

    fn glyph(&self, status: Status) -> &'static str {
        match (status, self.unicode) {
            (Status::Good, true) => "✓",
            (Status::Good, false) => "+",
            (Status::Warn, _) => "!",
            (Status::Bad, true) => "✗",
            (Status::Bad, false) => "x",
            (Status::Off, true) => "·",
            (Status::Off, false) => "-",
        }
    }

    pub(crate) fn mark(&self, status: Status) -> String {
        self.paint(status.tone(), self.glyph(status))
    }

    fn arrow(&self) -> &'static str {
        if self.unicode { "→" } else { "->" }
    }

    fn bullet(&self) -> &'static str {
        if self.unicode { "•" } else { "*" }
    }

    pub(crate) fn unicode(&self) -> bool {
        self.unicode
    }

    pub(crate) fn dash(&self) -> &'static str {
        if self.unicode { "—" } else { "-" }
    }
}

fn color_supported() -> bool {
    if env::var_os("NO_COLOR").is_some() || !io::stdout().is_terminal() {
        return false;
    }
    if env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    #[cfg(windows)]
    {
        crossterm::ansi_support::supports_ansi()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

fn unicode_supported() -> bool {
    if env::var_os("ALC_ASCII").is_some() {
        return false;
    }
    // Rust routes console writes on Windows through the wide API, so the code page
    // does not decide this; elsewhere the locale does.
    if cfg!(windows) {
        return true;
    }
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|value| !value.is_empty()))
        .is_none_or(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("utf-8") || value.contains("utf8")
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

pub(crate) struct Cell {
    text: String,
    tone: Tone,
    align: Align,
}

impl Cell {
    pub(crate) fn left(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            align: Align::Left,
        }
    }

    pub(crate) fn right(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            align: Align::Right,
        }
    }

    fn center(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            align: Align::Center,
        }
    }
}

/// A whitespace-aligned table whose columns are sized from the widest value in each,
/// so the layout cannot drift as profile names and provider kinds change.
pub(crate) struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    pub(crate) fn new<S: Into<String>>(headers: Vec<S>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, row: Vec<Cell>) {
        self.rows.push(row);
    }

    fn widths(&self) -> Vec<usize> {
        self.headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                self.rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| width(&cell.text))
                    .chain(std::iter::once(width(header)))
                    .max()
                    .unwrap_or_default()
            })
            .collect()
    }

    pub(crate) fn render(&self, theme: &Theme) -> Vec<String> {
        let widths = self.widths();
        let last = self.headers.len().saturating_sub(1);
        let mut lines = Vec::with_capacity(self.rows.len() + 1);

        let header: Vec<String> = self
            .headers
            .iter()
            .enumerate()
            .map(|(column, header)| {
                trim_last(pad(header, widths[column], Align::Left), column, last)
            })
            .collect();
        lines.push(theme.paint(Tone::Dim, &header.join(GUTTER)));

        for row in &self.rows {
            let cells: Vec<String> = row
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    let text = trim_last(pad(&cell.text, widths[column], cell.align), column, last);
                    theme.paint(cell.tone, &text)
                })
                .collect();
            lines.push(cells.join(GUTTER));
        }
        lines
    }
}

/// Drops the padding of the final column so rows carry no trailing whitespace.
fn trim_last(text: String, column: usize, last: usize) -> String {
    if column == last {
        text.trim_end().to_owned()
    } else {
        text
    }
}

pub(crate) fn width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

fn pad(value: &str, to: usize, align: Align) -> String {
    let current = width(value);
    if current >= to {
        return value.to_owned();
    }
    let missing = to - current;
    match align {
        Align::Left => format!("{value}{}", " ".repeat(missing)),
        Align::Right => format!("{}{value}", " ".repeat(missing)),
        Align::Center => {
            let left = missing / 2;
            format!("{}{value}{}", " ".repeat(left), " ".repeat(missing - left))
        }
    }
}

pub(crate) fn binary_for(agent: Agent) -> std::ffi::OsString {
    let override_name = match agent {
        Agent::Claude => "ALC_CLAUDE_BIN",
        Agent::Codex => "ALC_CODEX_BIN",
        Agent::Opencode => "ALC_OPENCODE_BIN",
        Agent::Pi => "ALC_PI_BIN",
        Agent::Copilot => "ALC_COPILOT_BIN",
        Agent::Goose => "ALC_GOOSE_BIN",
        Agent::Qwen => "ALC_QWEN_BIN",
        Agent::Kimi => "ALC_KIMI_BIN",
        Agent::Agy => "ALC_AGY_BIN",
    };
    env::var_os(override_name).unwrap_or_else(|| agent.as_str().into())
}

pub(crate) fn resolve(binary: &std::ffi::OsStr) -> Option<PathBuf> {
    let path = PathBuf::from(binary);
    if path.components().count() > 1 || path.is_absolute() {
        return path.is_file().then_some(path);
    }
    which::which(binary).ok()
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
    use crate::bridge::agy_auth::{AgyAuthError, LoginState};

    fn plain() -> Theme {
        Theme {
            color: false,
            unicode: true,
        }
    }

    fn upgraded_config() -> Config {
        let mut config = Config::default();
        config.providers.remove("antigravity");
        config
    }

    fn facts(found: Option<PathBuf>, login: LoginState) -> AgyFacts {
        AgyFacts {
            binary: "agy".into(),
            found,
            login,
            project: None,
            models: "bundled".to_owned(),
        }
    }

    fn row<'a>(rows: &'a [Row], name: &str) -> &'a Row {
        rows.iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no {name} row"))
    }

    #[test]
    fn the_antigravity_login_row_reports_the_token_state_and_never_the_token() {
        let config = Config::default();
        let login = |found: Option<PathBuf>, state: LoginState| {
            antigravity_rows(&config, &plain(), &facts(found, state))
                .into_iter()
                .find(|row| row.name == "login")
                .unwrap()
        };
        let ready = login(None, LoginState::Ready { expires_at: 0 });
        assert!(ready.status == Some(Status::Good));
        assert_eq!(ready.detail, "ready (agy's Google login)");
        // Expired with agy at hand is routine (agy's tokens last an hour):
        // healthy, and it says agy renews it.
        let renewable = login(Some(PathBuf::from("/opt/agy")), LoginState::Expired);
        assert!(renewable.status == Some(Status::Good));
        assert!(
            renewable.detail.contains("agy renews it"),
            "{}",
            renewable.detail
        );
        let stranded = login(None, LoginState::Expired);
        assert!(stranded.status == Some(Status::Warn));
        assert!(stranded.detail.contains("sign in"), "{}", stranded.detail);
        let missing = login(None, LoginState::Missing(AgyAuthError::LoginRequired));
        assert_eq!(missing.detail, "not logged in; run `agy` and sign in");
    }

    #[test]
    fn doctor_names_ambiguous_agy_logins() {
        let rows = antigravity_rows(
            &Config::default(),
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::AmbiguousSources)),
        );
        let login = row(&rows, "login");
        assert!(login.status == Some(Status::Warn));
        assert!(
            login.detail.contains("ambiguous sources"),
            "{}",
            login.detail
        );
    }

    #[test]
    fn the_project_and_bridge_rows_say_what_they_found_and_never_an_id() {
        let mut found = facts(None, LoginState::Ready { expires_at: 0 });
        found.project = Some(Ok(true));
        let rows = antigravity_rows(&Config::default(), &plain(), &found);
        assert_eq!(
            row(&rows, "project").detail,
            "onboarded (a Cloud Code project exists)"
        );
        assert!(row(&rows, "bridge").detail.contains("through alc's bridge"));
        found.project = Some(Ok(false));
        let rows = antigravity_rows(&Config::default(), &plain(), &found);
        assert!(row(&rows, "project").detail.contains("open agy once"));
        // Not signed in: nothing was asked, so there is no project row.
        let rows = antigravity_rows(
            &Config::default(),
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        assert!(rows.iter().all(|row| row.name != "project"));
    }

    /// None of the section's rows is a problem of its own: the binaries
    /// section already counts an explicit default's missing agy.
    #[test]
    fn no_antigravity_login_state_is_a_problem() {
        for state in [
            LoginState::Expired,
            LoginState::Missing(AgyAuthError::LoginRequired),
            LoginState::Missing(AgyAuthError::AmbiguousSources),
            LoginState::Missing(AgyAuthError::MalformedCredential),
        ] {
            let rows = antigravity_rows(&Config::default(), &plain(), &facts(None, state));
            assert!(row(&rows, "login").status != Some(Status::Bad), "{state:?}");
        }
    }

    #[test]
    fn a_missing_agy_is_a_problem_only_when_agy_is_an_explicit_default() {
        let rows = antigravity_rows(
            &Config::default(),
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        let agy = row(&rows, "agy");
        assert!(agy.status == Some(Status::Warn));
        assert_eq!(agy.detail, "not found (agy)");

        let mut config = Config::default();
        config.defaults.set(Agent::Agy, "antigravity");
        let rows = antigravity_rows(
            &config,
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        assert!(row(&rows, "agy").status == Some(Status::Bad));

        let found = PathBuf::from("/opt/agy/bin/agy");
        let rows = antigravity_rows(
            &config,
            &plain(),
            &facts(
                Some(found.clone()),
                LoginState::Missing(AgyAuthError::LoginRequired),
            ),
        );
        let agy = row(&rows, "agy");
        assert!(agy.status == Some(Status::Good));
        assert_eq!(agy.detail, found.display().to_string());
    }

    #[test]
    fn the_antigravity_profile_row_shows_its_model_and_effort() {
        let mut config = Config::default();
        let rows = antigravity_rows(
            &config,
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        let profile = row(&rows, "profile");
        assert!(profile.status == Some(Status::Good));
        assert!(profile.detail.contains("antigravity"), "{}", profile.detail);
        assert!(
            profile.detail.contains("claude-opus-5-5-high"),
            "{}",
            profile.detail
        );
        assert!(
            profile.detail.contains("agy's default effort"),
            "{}",
            profile.detail
        );

        config
            .providers
            .get_mut("antigravity")
            .unwrap()
            .reasoning_effort = Some(ReasoningEffort::Xhigh);
        let rows = antigravity_rows(
            &config,
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        assert!(row(&rows, "profile").detail.contains("/ xhigh"));
        assert_eq!(row(&rows, "models").detail, "bundled");
    }

    #[test]
    fn a_config_from_before_the_antigravity_kind_is_warned_with_the_command_that_adds_it() {
        let rows = antigravity_rows(
            &upgraded_config(),
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        let profile = row(&rows, "profile");
        assert!(profile.status == Some(Status::Warn));
        assert!(
            profile
                .detail
                .contains("run `alc config upsert antigravity --kind antigravity`"),
            "{}",
            profile.detail
        );

        let default = default_row(&upgraded_config(), &plain(), Agent::Agy);
        assert!(default.status == Some(Status::Warn));
        assert!(
            default
                .detail
                .contains("run `alc config upsert antigravity --kind antigravity`"),
            "{}",
            default.detail
        );
    }

    #[test]
    fn a_profile_named_antigravity_of_another_kind_is_not_offered_the_upsert_that_replaces_it() {
        let mut config = upgraded_config();
        config.providers.insert(
            "antigravity".to_owned(),
            Provider::for_kind(ProviderKind::Openrouter),
        );
        let rows = antigravity_rows(
            &config,
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        let profile = row(&rows, "profile");
        assert!(profile.status == Some(Status::Warn));
        assert!(
            !profile.detail.contains("upsert antigravity"),
            "{}",
            profile.detail
        );
        assert!(
            profile.detail.contains(
                "the profile named 'antigravity' is openrouter, not the antigravity kind"
            ),
            "{}",
            profile.detail
        );
    }

    #[test]
    fn an_explicit_default_that_does_not_exist_stays_a_problem() {
        let mut config = upgraded_config();
        config.defaults.set(Agent::Agy, "antigravity");
        let default = default_row(&config, &plain(), Agent::Agy);
        assert!(default.status == Some(Status::Bad));
        assert!(default.detail.contains("(no such profile)"));
        let rows = antigravity_rows(
            &config,
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        assert!(row(&rows, "profile").status == Some(Status::Bad));

        let mut config = Config::default();
        config.defaults.set(Agent::Pi, "nowhere");
        assert!(default_row(&config, &plain(), Agent::Pi).status == Some(Status::Bad));
    }

    #[test]
    fn an_antigravity_profile_under_another_name_serves_the_implicit_default() {
        let mut config = upgraded_config();
        config.providers.insert(
            "work-agy".to_owned(),
            Provider::for_kind(ProviderKind::Antigravity),
        );
        let default = default_row(&config, &plain(), Agent::Agy);
        assert!(default.status == Some(Status::Good));
        assert!(default.detail.contains("work-agy"), "{}", default.detail);
        let rows = antigravity_rows(
            &config,
            &plain(),
            &facts(None, LoginState::Missing(AgyAuthError::LoginRequired)),
        );
        assert!(row(&rows, "profile").detail.contains("work-agy"));
    }

    #[test]
    fn truncation_keeps_short_values() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghijk", 5), "abcd…");
    }

    #[test]
    fn padding_fills_to_the_requested_width() {
        assert_eq!(pad("ab", 5, Align::Left), "ab   ");
        assert_eq!(pad("ab", 5, Align::Center), " ab  ");
        assert_eq!(pad("✓", 5, Align::Center), "  ✓  ");
    }

    #[test]
    fn right_aligned_counts_use_unicode_display_columns_and_survive_colour() {
        assert_eq!(width("模型"), 4);
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(pad("模型", 7, Align::Right), "   模型");
        let build = || {
            let mut table = Table::new(vec!["NAME", "COUNT", "USD"]);
            table.push(vec![
                Cell::left("模型", Tone::Plain),
                Cell::right("1,234,567", Tone::Good),
                Cell::right("0.000000000001", Tone::Plain),
            ]);
            table.push(vec![
                Cell::left("e\u{301}", Tone::Plain),
                Cell::right("7", Tone::Good),
                Cell::right("1", Tone::Plain),
            ]);
            table
        };
        let plain_lines = build().render(&plain());
        let starts = [
            plain_lines[1].rfind("1,234,567").unwrap(),
            plain_lines[2].rfind('7').unwrap(),
        ];
        assert_eq!(
            width(&plain_lines[1][..starts[0]]) + 9,
            width(&plain_lines[2][..starts[1]]) + 1
        );
        let colored = build().render(&Theme {
            color: true,
            unicode: true,
        });
        assert_eq!(
            colored
                .iter()
                .map(|line| strip_ansi(line))
                .collect::<Vec<_>>(),
            plain_lines
        );
        assert!(plain_lines.iter().all(|line| line == line.trim_end()));
    }

    #[test]
    fn padding_never_shrinks_a_wide_value() {
        assert_eq!(pad("abcdef", 3, Align::Left), "abcdef");
    }

    #[test]
    fn columns_take_the_width_of_their_widest_value() {
        let mut table = Table::new(vec!["PROFILE", "KIND"]);
        table.push(vec![
            Cell::left("openrouter", Tone::Plain),
            Cell::left("x", Tone::Plain),
        ]);
        table.push(vec![
            Cell::left("a", Tone::Plain),
            Cell::left("anthropic", Tone::Plain),
        ]);
        assert_eq!(table.widths(), vec![10, 9]);
    }

    #[test]
    fn every_rendered_row_lines_up() {
        let mut table = Table::new(vec!["PROFILE", "KIND", "CLAUDE"]);
        table.push(vec![
            Cell::left("openrouter", Tone::Plain),
            Cell::left("openrouter", Tone::Plain),
            Cell::center("✓", Tone::Good),
        ]);
        table.push(vec![
            Cell::left("vllm", Tone::Plain),
            Cell::left("vllm", Tone::Plain),
            Cell::center("·", Tone::Dim),
        ]);

        let lines = table.render(&plain());
        let column = lines[0].find("KIND").expect("header column");
        for line in &lines[1..] {
            assert_eq!(
                line.char_indices()
                    .nth(column)
                    .map(|(_, character)| character),
                line[column..].chars().next(),
                "row does not start the KIND column at {column}: {line}"
            );
        }
        assert!(lines.iter().all(|line| line == line.trim_end()));
    }

    #[test]
    fn rendered_rows_carry_no_escape_codes_without_colour() {
        let mut table = Table::new(vec!["KEY"]);
        table.push(vec![Cell::left("missing", Tone::Bad)]);
        assert_eq!(table.render(&plain())[1], "missing");
    }

    /// Colour codes are invisible but not zero-width to `str::len`, so padding has to
    /// happen before painting or every coloured column drifts.
    #[test]
    fn colour_does_not_disturb_the_layout() {
        let build = || {
            let mut table = Table::new(vec!["PROFILE", "KEY", "CLAUDE"]);
            table.push(vec![
                Cell::left("openrouter", Tone::Plain),
                Cell::left("missing", Tone::Bad),
                Cell::center("✓", Tone::Good),
            ]);
            table.push(vec![
                Cell::left("vllm", Tone::Plain),
                Cell::left("n/a", Tone::Dim),
                Cell::center("·", Tone::Dim),
            ]);
            table
        };

        let coloured = build().render(&Theme {
            color: true,
            unicode: true,
        });
        assert!(
            coloured.iter().any(|line| line.contains('\x1b')),
            "expected colour codes to be emitted"
        );
        let stripped: Vec<String> = coloured.iter().map(|line| strip_ansi(line)).collect();
        assert_eq!(stripped, build().render(&plain()));
    }

    fn strip_ansi(line: &str) -> String {
        let mut out = String::new();
        let mut characters = line.chars();
        while let Some(character) = characters.next() {
            if character == '\x1b' {
                for escaped in characters.by_ref() {
                    if escaped == 'm' {
                        break;
                    }
                }
            } else {
                out.push(character);
            }
        }
        out
    }

    #[test]
    fn ascii_themes_avoid_multi_byte_glyphs() {
        let ascii = Theme {
            color: false,
            unicode: false,
        };
        for status in [Status::Good, Status::Warn, Status::Bad, Status::Off] {
            assert!(ascii.glyph(status).is_ascii());
        }
        assert!(ascii.arrow().is_ascii() && ascii.bullet().is_ascii() && ascii.dash().is_ascii());
    }

    #[test]
    fn every_glyph_is_one_column_wide() {
        let theme = plain();
        for status in [Status::Good, Status::Warn, Status::Bad, Status::Off] {
            assert_eq!(width(theme.glyph(status)), 1);
        }
    }
}
