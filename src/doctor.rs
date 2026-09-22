use std::env;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;

use crate::config::{Agent, AuthStyle, Provider, ProviderKind, ReasoningEffort, Store};
use crate::model_catalog::{CodexSource, ModelCatalog};
use crate::ollama;

pub(crate) const INDENT: &str = "  ";
pub(crate) const GUTTER: &str = "  ";
/// Profile names are elided past this so one outlier cannot stretch every column.
const NAME_LIMIT: usize = 24;

pub fn run(store: &Store) -> Result<bool> {
    let theme = Theme::detect();
    let mut issues = Vec::new();
    let codex_bridge_enabled = store
        .config
        .providers
        .values()
        .any(|provider| provider.enabled && provider.kind == ProviderKind::Codex);

    println!("{}", theme.paint(Tone::Head, "alc doctor"));
    environment(store, &theme, &mut issues);
    binaries(store, &theme, &mut issues);
    profiles(store, &theme, &mut issues);
    defaults(store, &theme);
    if codex_bridge_enabled {
        codex_bridge(store, &theme, &mut issues);
    }
    claude_code_default_model(store, &theme, &mut issues);
    background_sessions(store, &theme);
    local_models(store, &theme, &mut issues);
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
        println!("{INDENT}{line}");
    }
}

/// Unusable defaults are already reported by `Config::validate`, so this section
/// only points at which agent is affected instead of counting the problem twice.
fn defaults(store: &Store, theme: &Theme) {
    heading(theme, "Defaults");
    let rows = Agent::ALL.map(|agent| {
        let name = store.config.defaults.get(agent);
        match store.config.providers.get(name) {
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
            None => Row::new(
                Status::Bad,
                agent,
                format!(
                    "{name}{GUTTER}{}",
                    theme.paint(Tone::Bad, "(no such profile)")
                ),
            ),
        }
    });
    marked(theme, &rows);
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
    println!(
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
                .map_or("gpt-5.6-terra", |entry| entry.id.as_str());
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
        let note = if unroutable {
            theme.paint(Tone::Bad, "  bridge cannot route this")
        } else {
            String::new()
        };
        rows.push(Row::new(
            status,
            name.clone(),
            format!(
                "{model}{}{note}",
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
    println!(
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

pub(crate) fn summary(theme: &Theme, issues: &[Issue]) {
    print!("{}", summary_text(theme, issues));
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
    print!("{}", heading_text(theme, title));
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
        println!("{INDENT}{}{GUTTER}{value}", pad(name, label, Align::Left));
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
        println!(
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
    value.chars().count()
}

fn pad(value: &str, to: usize, align: Align) -> String {
    let current = width(value);
    if current >= to {
        return value.to_owned();
    }
    let missing = to - current;
    match align {
        Align::Left => format!("{value}{}", " ".repeat(missing)),
        Align::Center => {
            let left = missing / 2;
            format!("{}{value}{}", " ".repeat(left), " ".repeat(missing - left))
        }
    }
}

fn binary_for(agent: Agent) -> std::ffi::OsString {
    let override_name = match agent {
        Agent::Claude => "ALC_CLAUDE_BIN",
        Agent::Codex => "ALC_CODEX_BIN",
        Agent::Opencode => "ALC_OPENCODE_BIN",
        Agent::Pi => "ALC_PI_BIN",
        Agent::Copilot => "ALC_COPILOT_BIN",
        Agent::Goose => "ALC_GOOSE_BIN",
        Agent::Qwen => "ALC_QWEN_BIN",
        Agent::Kimi => "ALC_KIMI_BIN",
    };
    env::var_os(override_name).unwrap_or_else(|| agent.as_str().into())
}

fn resolve(binary: &std::ffi::OsStr) -> Option<PathBuf> {
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

    fn plain() -> Theme {
        Theme {
            color: false,
            unicode: true,
        }
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
