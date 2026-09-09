use std::ffi::OsString;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::config::{
    Agent, AuthStyle, Protocol, Provider, ProviderKind, ReasoningEffort, Store,
    validate_profile_name,
};
use crate::model_catalog::{ModelCatalog, ModelInfo};
use crate::remote::RemoteCommand;
use crate::{doctor, launch, ollama, remote, tui, update};

#[derive(Debug, Parser)]
#[command(
    name = "alc",
    version,
    about = "Configure once, launch any coding agent with the provider you want",
    long_about = None,
    arg_required_else_help = true
)]
struct Cli {
    /// Select a provider profile by name (or by kind when it is unique).
    #[arg(short = 'p', long, global = true, value_name = "PROFILE")]
    provider: Option<String>,

    /// Shortcut for --provider codex.
    #[arg(long, global = true)]
    codex: bool,

    /// Shortcut for --provider anthropic.
    #[arg(long, global = true)]
    anthropic: bool,

    /// Shortcut for --provider openai.
    #[arg(long, global = true)]
    openai: bool,

    /// Shortcut for --provider openrouter.
    #[arg(long, global = true)]
    openrouter: bool,

    /// Shortcut for --provider ollama.
    #[arg(long, global = true)]
    ollama: bool,

    /// Shortcut for --provider vllm.
    #[arg(long, global = true)]
    vllm: bool,

    /// Shortcut for --provider deepseek.
    #[arg(long, global = true)]
    deepseek: bool,

    /// Shortcut for --provider moonshot.
    #[arg(long, global = true)]
    moonshot: bool,

    /// Shortcut for --provider zai.
    #[arg(long, global = true)]
    zai: bool,

    /// Shortcut for --provider minimax.
    #[arg(long, global = true)]
    minimax: bool,

    /// Shortcut for --provider groq.
    #[arg(long, global = true)]
    groq: bool,

    /// Shortcut for --provider xai.
    #[arg(long, global = true)]
    xai: bool,

    /// Shortcut for --provider google.
    #[arg(long, global = true)]
    google: bool,

    /// Print the resolved command and environment without launching it.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Mirror this session to a browser page (see `alc remote`).
    ///
    /// Must appear before the agent's own arguments; `alc share <agent> --
    /// <args>` is the unambiguous form.
    #[arg(long, global = true, env = "ALC_SHARE")]
    share: bool,

    /// Never mirror this session, whatever the configuration says.
    #[arg(long, global = true, conflicts_with = "share")]
    no_share: bool,

    /// Bind the session page to this machine's network address instead of
    /// loopback, so a phone on the same Wi-Fi can reach it directly.
    ///
    /// A tunnel does not need this: `tailscale serve` and `cloudflared`
    /// both connect to loopback themselves.
    #[arg(long = "bind-lan", global = true)]
    bind_lan: bool,

    /// Name a shared session on the page, instead of `<agent>@<directory>`.
    #[arg(long, global = true, value_name = "NAME")]
    name: Option<String>,

    /// Permission mode a shared session starts in: plan, ask, auto-edit,
    /// auto, or full.
    #[arg(long, global = true, value_name = "RUNG")]
    permission: Option<String>,

    /// Override the alc config directory (also available as ALC_CONFIG_DIR).
    #[arg(long, global = true, env = "ALC_CONFIG_DIR", hide = true)]
    config_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the configuration TUI or use a scripting subcommand.
    ///
    /// The TUI has three screens, named across its header: provider
    /// profiles, per-agent defaults, and sharing & remote control - which is
    /// where share-by-default, the bind address and the permission ceiling
    /// live.
    Config(ConfigArgs),
    /// Check agent binaries, credentials, defaults, and compatibility.
    Doctor,
    /// Show or refresh the GPT models available through the Codex bridge.
    Models(ModelsArgs),
    /// Check for and install the latest alc release.
    Update(UpdateArgs),
    /// Launch a coding agent with its session mirrored to a browser.
    Share(ShareArgs),
    /// Inspect or change the remote-control settings and credentials.
    Remote(RemoteArgs),
    /// Approve a permission change a shared session asked for.
    Confirm(ConfirmArgs),
    /// Start, stop, or inspect the process that owns shared sessions.
    Hub(HubArgs),
    /// List shared sessions.
    Sessions(SessionsArgs),
    /// Put this terminal back on a shared session.
    Attach(SessionRef),
    /// Stop a shared session.
    Kill(SessionRef),
    /// Rename a shared session's card.
    Rename(RenameArgs),
    /// Launch Claude Code.
    Claude(ClaudeArgs),
    /// Launch Codex CLI.
    Codex(Passthrough),
    /// Launch OpenCode.
    Opencode(Passthrough),
    /// Launch Pi.
    Pi(Passthrough),
    /// Launch GitHub Copilot CLI.
    Copilot(Passthrough),
    /// Launch Goose.
    Goose(Passthrough),
    /// Launch Qwen Code.
    Qwen(Passthrough),
    /// Launch Kimi Code CLI.
    Kimi(Passthrough),
}

#[derive(Debug, Args)]
struct Passthrough {
    /// Arguments passed unchanged to the coding agent.
    #[arg(
        value_name = "ARGS",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    args: Vec<OsString>,
}

#[derive(Debug, Args)]
struct ClaudeArgs {
    /// GPT model this session starts on (for example gpt-5.6-terra).
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// Starting Codex reasoning effort: low, medium, high, xhigh, or max.
    #[arg(long, value_name = "LEVEL")]
    effort: Option<ReasoningEffort>,

    /// Deprecated and ignored; Claude Code now picks the model in-session.
    #[arg(long, hide = true)]
    no_picker: bool,

    /// Save this run's model and effort as the provider's defaults.
    #[arg(long)]
    save: bool,

    /// Arguments passed unchanged to Claude Code.
    #[arg(
        value_name = "ARGS",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    args: Vec<OsString>,
}

#[derive(Debug, Args)]
struct ShareArgs {
    /// claude, codex, opencode, pi, copilot, goose, qwen, or kimi.
    agent: Agent,

    /// Arguments passed unchanged to the coding agent.
    #[arg(
        value_name = "ARGS",
        allow_hyphen_values = true,
        trailing_var_arg = true
    )]
    args: Vec<OsString>,
}

#[derive(Debug, Args)]
struct HubArgs {
    #[command(subcommand)]
    command: Option<HubCommand>,
}

#[derive(Debug, Subcommand)]
enum HubCommand {
    /// Start the hub if it is not already running.
    Start(HubStartArgs),
    /// Stop the hub.
    Stop {
        /// Stop the sessions it is running too.
        #[arg(long)]
        drain: bool,
    },
    /// Report whether a hub is running and where its page is.
    Status,
}

#[derive(Debug, Args)]
struct HubStartArgs {
    /// Run in this terminal instead of detaching. Used by alc itself when
    /// it starts a hub, and useful for seeing why one will not come up.
    #[arg(long)]
    foreground: bool,
}

#[derive(Debug, Args)]
struct SessionsArgs {
    /// Print the sessions as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct SessionRef {
    /// A session id, or an unambiguous prefix of one.
    id: String,
}

#[derive(Debug, Args)]
struct RenameArgs {
    /// A session id, or an unambiguous prefix of one.
    id: String,
    /// The new name.
    name: String,
}

#[derive(Debug, Args)]
struct ConfirmArgs {
    /// The ticket the page showed.
    ticket: String,
}

#[derive(Debug, Args)]
struct RemoteArgs {
    #[command(subcommand)]
    command: Option<RemoteSubcommand>,

    /// Report the resolved posture without binding a socket.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Subcommand)]
enum RemoteSubcommand {
    /// Show whether remote control is on, how it binds, and where its files live.
    Status,
    /// Allow sessions to be shared.
    On,
    /// Refuse to share sessions.
    Off,
    /// Replace the tokens, invalidating every link handed out so far.
    Token(RemoteTokenArgs),
    /// Answer to another name, for a tunnel's hostname.
    ///
    /// Takes `host[:port]`, or `*.example.com` for a tunnel that mints a
    /// fresh hostname on every run.
    AllowHost {
        /// For example `box.tail1a2b.ts.net` or `*.trycloudflare.com`.
        host: String,
    },
    /// Print the link to the page, including its token.
    Url,
    /// Share every session without passing --share.
    AutoShare {
        /// on or off.
        state: OnOff,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Debug, Args)]
struct RemoteTokenArgs {
    /// Mint fresh tokens.
    #[arg(long)]
    rotate: bool,
}

#[derive(Debug, Args)]
struct ModelsArgs {
    /// Force an immediate sync from the installed Codex CLI.
    #[arg(long)]
    refresh: bool,

    /// Print the catalog as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    /// Check whether an update is available without installing it.
    #[arg(long, conflicts_with = "force")]
    check: bool,

    /// Reinstall the latest release even when this version is current.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct ConfigArgs {
    #[command(subcommand)]
    command: Option<ConfigCommand>,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Write the starter configuration if no config exists yet.
    Init,
    /// Print the non-secret configuration and credential status.
    Show,
    /// Print alc's configuration paths.
    Path,
    /// Create or update a provider profile without the TUI.
    Upsert(UpsertArgs),
    /// Remove a provider profile.
    Remove {
        /// Provider profile name.
        name: String,
    },
    /// Change an agent's default provider profile.
    SetDefault {
        /// claude, codex, opencode, pi, copilot, goose, qwen, or kimi.
        agent: Agent,
        /// Provider profile name.
        provider: String,
    },
    /// Save, replace, or clear a provider API key.
    Key(KeyArgs),
}

#[derive(Debug, Args)]
struct UpsertArgs {
    /// Provider profile name.
    name: String,

    /// Provider implementation kind.
    #[arg(long)]
    kind: Option<ProviderKind>,

    /// Default model ID.
    #[arg(long)]
    model: Option<String>,

    /// Default Codex reasoning effort.
    #[arg(long, conflicts_with = "clear_effort")]
    effort: Option<ReasoningEffort>,

    /// Follow the selected model or Codex config instead of forcing an effort.
    #[arg(long)]
    clear_effort: bool,

    /// Optional small/fast model ID.
    #[arg(long)]
    small_model: Option<String>,

    /// Provider base URL.
    #[arg(long)]
    base_url: Option<String>,

    /// Separate Anthropic-compatible URL used by Claude Code.
    #[arg(long)]
    anthropic_base_url: Option<String>,

    /// Wire protocol exposed by the provider.
    #[arg(long)]
    protocol: Option<Protocol>,

    /// Authentication header style.
    #[arg(long)]
    auth: Option<AuthStyle>,

    /// Environment variable that may supply the API key.
    #[arg(long)]
    api_key_env: Option<String>,

    /// Named ~/.codex/<name>.config.toml layer.
    #[arg(long)]
    codex_profile: Option<String>,

    /// Disable this profile without deleting it.
    #[arg(long, conflicts_with = "enable")]
    disable: bool,

    /// Re-enable this profile.
    #[arg(long)]
    enable: bool,
}

#[derive(Debug, Args)]
struct KeyArgs {
    /// Provider profile name.
    provider: String,

    /// Read the key from stdin instead of a hidden prompt.
    #[arg(long, conflicts_with = "clear")]
    stdin: bool,

    /// Delete the locally saved key.
    #[arg(long)]
    clear: bool,
}

pub fn run() -> Result<u8> {
    let cli = Cli::parse();
    let requested_provider = provider_selector(&cli)?;
    if let Command::Update(args) = &cli.command {
        return update::run(args.check, args.force);
    }
    let mut store = Store::load(cli.config_dir.clone())?;
    let sharing = Sharing {
        enabled: cli.share && !cli.no_share,
        forced_off: cli.no_share,
        lan: cli.bind_lan,
        name: cli.name.clone(),
        permission: cli.permission.clone(),
    };

    match cli.command {
        Command::Config(args) => run_config(&mut store, args),
        Command::Doctor => Ok(if doctor::run(&store)? { 0 } else { 1 }),
        Command::Models(args) => run_models(&store, args),
        Command::Update(_) => unreachable!("update is handled before config loading"),
        Command::Remote(args) => run_remote(&store, args),
        Command::Confirm(args) => remote::confirm(&store, &args.ticket),
        Command::Hub(args) => {
            let command = match args.command {
                Some(HubCommand::Start(start)) => remote::HubCommand::Start {
                    foreground: start.foreground,
                    bind_lan: cli.bind_lan,
                },
                Some(HubCommand::Stop { drain }) => remote::HubCommand::Stop { drain },
                None | Some(HubCommand::Status) => remote::HubCommand::Status,
            };
            remote::run_hub(&store, command)
        }
        Command::Sessions(args) => {
            remote::run_hub(&store, remote::HubCommand::List { json: args.json })
        }
        Command::Attach(args) => {
            remote::run_hub(&store, remote::HubCommand::Attach { id: args.id })
        }
        Command::Kill(args) => remote::run_hub(&store, remote::HubCommand::Kill { id: args.id }),
        Command::Rename(args) => remote::run_hub(
            &store,
            remote::HubCommand::Rename {
                id: args.id,
                name: args.name,
            },
        ),
        Command::Share(args) => {
            let sharing = Sharing {
                enabled: !cli.no_share,
                forced_off: cli.no_share,
                lan: cli.bind_lan,
                name: cli.name.clone(),
                permission: cli.permission.clone(),
            };
            match args.agent {
                Agent::Claude => run_claude(
                    &mut store,
                    requested_provider.as_deref(),
                    ClaudeArgs {
                        model: None,
                        effort: None,
                        no_picker: false,
                        save: false,
                        args: args.args,
                    },
                    cli.dry_run,
                    sharing,
                ),
                agent => run_agent(
                    &store,
                    agent,
                    requested_provider.as_deref(),
                    args.args,
                    cli.dry_run,
                    sharing,
                ),
            }
        }
        Command::Claude(args) => run_claude(
            &mut store,
            requested_provider.as_deref(),
            args,
            cli.dry_run,
            sharing,
        ),
        Command::Codex(args) => run_agent(
            &store,
            Agent::Codex,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
            sharing,
        ),
        Command::Opencode(args) => run_agent(
            &store,
            Agent::Opencode,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
            sharing,
        ),
        Command::Pi(args) => run_agent(
            &store,
            Agent::Pi,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
            sharing,
        ),
        Command::Copilot(args) => run_agent(
            &store,
            Agent::Copilot,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
            sharing,
        ),
        Command::Goose(args) => run_agent(
            &store,
            Agent::Goose,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
            sharing,
        ),
        Command::Qwen(args) => run_agent(
            &store,
            Agent::Qwen,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
            sharing,
        ),
        Command::Kimi(args) => run_agent(
            &store,
            Agent::Kimi,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
            sharing,
        ),
    }
}

fn provider_selector(cli: &Cli) -> Result<Option<String>> {
    let shortcuts = [
        (cli.codex, "codex"),
        (cli.anthropic, "anthropic"),
        (cli.openai, "openai"),
        (cli.openrouter, "openrouter"),
        (cli.ollama, "ollama"),
        (cli.vllm, "vllm"),
        (cli.deepseek, "deepseek"),
        (cli.moonshot, "moonshot"),
        (cli.zai, "zai"),
        (cli.minimax, "minimax"),
        (cli.groq, "groq"),
        (cli.xai, "xai"),
        (cli.google, "google"),
    ];
    let selected: Vec<_> = shortcuts
        .into_iter()
        .filter_map(|(enabled, name)| enabled.then_some(name))
        .collect();
    if selected.len() > 1 {
        bail!("provider shortcut flags are mutually exclusive");
    }
    if cli.provider.is_some() && !selected.is_empty() {
        bail!("--provider cannot be combined with a provider shortcut flag");
    }
    Ok(cli
        .provider
        .clone()
        .or_else(|| selected.first().map(|name| (*name).to_owned())))
}

/// Whether this launch is mirrored to a browser, and how it binds.
#[derive(Debug, Clone)]
struct Sharing {
    enabled: bool,
    /// `--no-share` was passed, so the standing preference is overridden
    /// for this one launch.
    forced_off: bool,
    lan: bool,
    /// A name for the session card, when the user gave one.
    name: Option<String>,
    /// The permission rung the session starts in, when the user named one.
    permission: Option<String>,
}

/// alc's own flags, which `trailing_var_arg` hands to the agent verbatim
/// once the first passthrough token has been seen.
///
/// Appending a flag to a command you already have is the most natural
/// gesture there is, and without this it silently sends `--share` to the
/// model as prompt text, or exits with the agent's own unknown-flag error
/// naming a flag the agent has never heard of.
const ALC_OWNED_FLAGS: [&str; 5] = [
    "--share",
    "--no-share",
    "--bind-lan",
    "--name",
    "--permission",
];

fn reject_swallowed_flags(args: &[OsString], agent: Agent) -> Result<()> {
    for argument in args {
        let Some(text) = argument.to_str() else {
            continue;
        };
        let name = text.split('=').next().unwrap_or(text);
        if ALC_OWNED_FLAGS.contains(&name) {
            bail!(
                "`{name}` is alc's own flag but it came after the agent's arguments, \
where it would be passed to {agent} instead; put it before the agent name, \
or use `alc share {agent} -- <args>`"
            );
        }
    }
    Ok(())
}

fn run_agent(
    store: &Store,
    agent: Agent,
    requested_provider: Option<&str>,
    args: Vec<OsString>,
    dry_run: bool,
    sharing: Sharing,
) -> Result<u8> {
    reject_swallowed_flags(&args, agent)?;
    let provider = store.config.resolve(agent, requested_provider)?.1.clone();
    let overrides = if provider.kind == ProviderKind::Codex && agent != Agent::Codex {
        codex_launch_overrides(store, &provider, dry_run)?
    } else {
        launch::LaunchOverrides::default()
    };
    let spec = launch::build(store, agent, requested_provider, &args, &overrides)?;
    run_spec(store, spec, dry_run, sharing)
}

fn run_claude(
    store: &mut Store,
    requested_provider: Option<&str>,
    args: ClaudeArgs,
    dry_run: bool,
    sharing: Sharing,
) -> Result<u8> {
    reject_swallowed_flags(&args.args, Agent::Claude)?;
    let (profile_name, provider) = {
        let (name, provider) = store.config.resolve(Agent::Claude, requested_provider)?;
        (name.to_owned(), provider.clone())
    };

    if provider.kind != ProviderKind::Codex {
        if args.effort.is_some() || args.save {
            bail!(
                "--effort and --save are available with a Codex provider; use `alc --codex claude`"
            );
        }
        // A local Ollama server can say how much context it really gives the
        // model, which is worth more than Claude Code's 200k guess for an
        // unknown model id. Skipped silently when the server is not running.
        let context_window = (provider.kind == ProviderKind::Ollama)
            .then(|| {
                ollama::context_window(&provider, args.model.as_deref().unwrap_or(&provider.model))
            })
            .flatten();
        let overrides = launch::LaunchOverrides {
            model: args.model,
            context_window,
            ..launch::LaunchOverrides::default()
        };
        let spec = launch::build(
            store,
            Agent::Claude,
            requested_provider,
            &args.args,
            &overrides,
        )?;
        return run_spec(store, spec, dry_run, sharing);
    }

    let overrides = if args.model.is_some() || args.effort.is_some() || args.save {
        let catalog = load_codex_catalog(store, dry_run);
        let (model, effort) =
            resolve_codex_defaults(&provider, &catalog, args.model.as_deref(), args.effort)?;

        if args.save {
            let entry = store
                .config
                .providers
                .get_mut(&profile_name)
                .context("selected Codex provider disappeared from the config")?;
            entry.model = model.clone();
            entry.reasoning_effort = Some(effort);
            store.save()?;
            println!("Saved {model} / {effort} as the default for '{profile_name}'.");
        }

        let catalog = codex_catalog_for(store, catalog, &model, dry_run);
        let context_window = catalog.find(&model).map(|entry| entry.context_window);
        launch::LaunchOverrides {
            model: Some(model),
            reasoning_effort: Some(effort),
            context_window,
            model_options: routable_model_options(&catalog),
        }
    } else {
        codex_launch_overrides(store, &provider, dry_run)?
    };
    let spec = launch::build(
        store,
        Agent::Claude,
        requested_provider,
        &args.args,
        &overrides,
    )?;
    run_spec(store, spec, dry_run, sharing)
}

/// The model and reasoning effort a Codex-backed Claude Code session starts
/// on. Claude Code switches both during the session, so these are defaults,
/// not a fixed choice.
fn resolve_codex_defaults(
    provider: &Provider,
    catalog: &ModelCatalog,
    model: Option<&str>,
    effort: Option<ReasoningEffort>,
) -> Result<(String, ReasoningEffort)> {
    let model = model
        .map(launch::normalize_codex_model)
        .map_or_else(|| launch::resolve_codex_model(provider), Ok)?;
    let effort = effort
        .or(provider.reasoning_effort)
        .or(launch::resolve_codex_effort(provider)?)
        .or_else(|| catalog.find(&model).map(|entry| entry.default_effort))
        .unwrap_or(ReasoningEffort::Medium);

    if let Some(entry) = catalog.find(&model)
        && !entry.supported_efforts.contains(&effort)
    {
        bail!("model '{model}' does not support reasoning effort '{effort}'");
    }
    Ok((model, clamp_for_bridge(effort)))
}

/// The loosest effort the bundled bridge can actually carry.
///
/// GPT-6 and the newer GPT-5.6 models accept `ultra`, and the bundled
/// claude-codex helper does not - its own effort enum stops at `max`, so a
/// request carrying `ultra` is refused. Every bridged agent's effort reaches
/// that helper one way or another: pinned through `CCP_CODEX_EFFORT` for the
/// Responses and Chat clients, and inside each request for Claude Code,
/// which alc hands `--effort` to directly.
///
/// So it is clamped here, at the one point where a bridged session's effort
/// is decided, and said out loud. Silently downgrading an explicit choice
/// would be worse, and refusing outright would block a session that works
/// perfectly well at `max`.
fn clamp_for_bridge(effort: ReasoningEffort) -> ReasoningEffort {
    if effort != ReasoningEffort::Ultra {
        return effort;
    }
    eprintln!(
        "note: the bundled Codex bridge tops out at 'max', so this session uses max rather \
         than ultra. Native `alc codex` reaches ultra."
    );
    ReasoningEffort::Max
}

/// Loads the Codex model catalog, syncing it in the background unless this
/// is a dry run (which must never touch disk beyond a plain cache read).
fn load_codex_catalog(store: &Store, dry_run: bool) -> ModelCatalog {
    if dry_run {
        ModelCatalog::load(&store.dir)
    } else {
        ModelCatalog::load_and_refresh_if_due(&store.dir)
    }
}

/// The catalog, refreshed early when it has never heard of the model this
/// launch is about to use.
///
/// The catalog is what tells the agent how large the model's context window
/// is, and a cache can be a day older than the model - or, after a release
/// that changed what the catalog keeps, simply missing an entry it used to
/// drop. Launching anyway is not neutral: Claude Code falls back to assuming
/// 200k and starts compacting a 272k session three quarters of the way in,
/// without saying so. Waiting for one `codex debug models` is the cheaper
/// mistake, and only happens when the model really is unknown.
fn codex_catalog_for(
    store: &Store,
    catalog: ModelCatalog,
    model: &str,
    dry_run: bool,
) -> ModelCatalog {
    if dry_run || catalog.find(model).is_some() {
        return catalog;
    }
    ModelCatalog::refresh(&store.dir).unwrap_or(catalog)
}

/// The catalog-backed defaults a Codex-bridged session starts on for any
/// agent: no CLI overrides applied. `run_claude` layers `--model`/`--effort`/
/// `--save` on top of this for Claude Code specifically.
fn codex_launch_overrides(
    store: &Store,
    provider: &Provider,
    dry_run: bool,
) -> Result<launch::LaunchOverrides> {
    let catalog = load_codex_catalog(store, dry_run);
    let (model, effort) = resolve_codex_defaults(provider, &catalog, None, None)?;
    let catalog = codex_catalog_for(store, catalog, &model, dry_run);
    let context_window = catalog.find(&model).map(|entry| entry.context_window);
    Ok(launch::LaunchOverrides {
        model: Some(model),
        reasoning_effort: Some(effort),
        context_window,
        model_options: routable_model_options(&catalog),
    })
}

/// The catalog entries the bundled bridge can actually route.
///
/// These become the agent's own in-session picker, and the catalog on disk
/// can be a day older than the bridge: an entry the bridge cannot route
/// would fail mid-conversation, which is a worse place to find out than at
/// launch. Every path that builds a picker goes through here, because a
/// filter applied on only some of them is the same bug with a narrower
/// trigger. No answer from the bridge means "no opinion" and leaves the
/// list alone.
fn routable_model_options(catalog: &ModelCatalog) -> Vec<ModelInfo> {
    let routable = launch::bridge_codex_models();
    catalog
        .models
        .iter()
        .filter(|entry| {
            routable
                .as_ref()
                .is_none_or(|routable| routable.contains(&entry.id))
        })
        .cloned()
        .collect()
}

fn run_spec(
    store: &Store,
    spec: launch::LaunchSpec,
    dry_run: bool,
    sharing: Sharing,
) -> Result<u8> {
    // Decided once, so `--dry-run` reports what a real run would actually
    // do. An explicit `--share` wins, then the standing preference - and
    // the preference is skipped silently for a scripted run, because it
    // must not be the reason somebody's `alc claude -p "…" > out.txt`
    // starts failing. An explicit flag still says so, loudly, because there
    // the user asked for something alc cannot do.
    let share_now = if sharing.enabled {
        true
    } else {
        !sharing.forced_off && remote::shares_by_default(store) && remote::can_share()
    };

    if dry_run {
        println!(
            "agent: {}\nprovider: {} ({})\ncommand: {}",
            spec.agent,
            spec.provider_name,
            spec.provider_kind,
            spec.redacted_command()
        );
        if let Some(plan) = &spec.bridge {
            println!(
                "adapter: built in ({}), on an ephemeral loopback port",
                launch::bridge_label()
            );
            // A dry run exists to report what a real run would do, so it has
            // to admit the launch it is describing would be refused.
            if launch::bridge_codex_models().is_some_and(|routable| !routable.contains(&plan.model))
            {
                println!(
                    "adapter: WOULD FAIL - the bridge cannot route '{}'; run without --dry-run for the models it does",
                    plan.model
                );
            }
        }
        for entry in &spec.file_setup {
            match entry {
                launch::FileSetup::UpsertJson { path, key, .. } => {
                    println!("setup: would update {} ({key})", path.display());
                }
                launch::FileSetup::WriteTemp { path, .. } => {
                    println!(
                        "setup: temporary config at {} (contents withheld)",
                        path.display()
                    );
                }
            }
        }
        if share_now {
            println!("share: would mirror this session to a browser page");
            // Which process does the work is not a detail here: a shared
            // launch is performed by the hub, from this spec sent over the
            // control socket, and a spec that arrives there incomplete is
            // how a Codex session once ran with no adapter at all. A dry
            // run that named neither would leave a reader debugging the
            // wrong process.
            if spec.bridge.is_some() || !spec.file_setup.is_empty() {
                println!("share: the hub performs the launch, adapter and setup included");
            }
        }
        return Ok(0);
    }
    if share_now {
        return remote::share(store, spec, sharing.lan, sharing.name, sharing.permission);
    }
    launch::execute(spec)
}

fn run_remote(store: &Store, args: RemoteArgs) -> Result<u8> {
    let command = match args.command {
        None if args.dry_run => RemoteCommand::DryRun,
        None | Some(RemoteSubcommand::Status) => RemoteCommand::Status,
        Some(RemoteSubcommand::On) => RemoteCommand::Enable,
        Some(RemoteSubcommand::Off) => RemoteCommand::Disable,
        Some(RemoteSubcommand::AllowHost { host }) => RemoteCommand::AllowHost { host },
        Some(RemoteSubcommand::Url) => RemoteCommand::Url,
        Some(RemoteSubcommand::AutoShare { state }) => RemoteCommand::AutoShare {
            on: state == OnOff::On,
        },
        Some(RemoteSubcommand::Token(token)) => {
            if !token.rotate {
                bail!("`alc remote token` needs --rotate; it never prints a token");
            }
            RemoteCommand::RotateTokens
        }
    };
    remote::run_command(store, command)
}

fn run_models(store: &Store, args: ModelsArgs) -> Result<u8> {
    let catalog = if args.refresh {
        ModelCatalog::refresh(&store.dir)?
    } else {
        ModelCatalog::load_and_refresh_if_due(&store.dir)
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&catalog)?);
        return Ok(0);
    }

    println!("Codex bridge model catalog");
    println!("source: {}", catalog.source);
    for model in &catalog.models {
        let efforts = model
            .supported_efforts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "- {}: {} (Codex context: {}K; default: {}; efforts: {})",
            model.id,
            model.description,
            model.context_window / 1_000,
            model.default_effort,
            efforts
        );
    }
    println!("Auto-sync: once every 24 hours; run `alc models --refresh` to sync now.");
    Ok(0)
}

fn run_config(store: &mut Store, args: ConfigArgs) -> Result<u8> {
    match args.command {
        None => {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                bail!(
                    "`alc config` needs an interactive terminal; use `alc config --help` for scripting commands"
                );
            }
            tui::run(store)?;
            Ok(0)
        }
        Some(ConfigCommand::Init) => {
            store.ensure_saved()?;
            println!("initialized {}", store.config_path().display());
            Ok(0)
        }
        Some(ConfigCommand::Show) => {
            print_config(store)?;
            Ok(0)
        }
        Some(ConfigCommand::Path) => {
            println!("config: {}", store.config_path().display());
            println!("credentials: {}", store.credentials_path().display());
            Ok(0)
        }
        Some(ConfigCommand::Upsert(args)) => {
            upsert(store, args)?;
            store.save()?;
            println!(
                "saved provider configuration to {}",
                store.config_path().display()
            );
            Ok(0)
        }
        Some(ConfigCommand::Remove { name }) => {
            remove(store, &name)?;
            store.save()?;
            println!("removed provider '{name}'");
            Ok(0)
        }
        Some(ConfigCommand::SetDefault { agent, provider }) => {
            let entry = store
                .config
                .providers
                .get(&provider)
                .with_context(|| format!("provider profile '{provider}' does not exist"))?;
            if !entry.supports(agent) {
                bail!(
                    "provider '{provider}' ({}) cannot be used with {agent}",
                    entry.kind
                );
            }
            store.config.defaults.set(agent, &provider);
            store.save()?;
            println!("default {agent} provider: {provider}");
            Ok(0)
        }
        Some(ConfigCommand::Key(args)) => {
            set_key(store, args)?;
            store.save()?;
            Ok(0)
        }
    }
}

fn upsert(store: &mut Store, args: UpsertArgs) -> Result<()> {
    validate_profile_name(&args.name)?;
    let exists = store.config.providers.contains_key(&args.name);
    let kind = args.kind.unwrap_or_else(|| {
        store
            .config
            .providers
            .get(&args.name)
            .map(|provider| provider.kind)
            .unwrap_or(ProviderKind::Custom)
    });
    let provider = store
        .config
        .providers
        .entry(args.name.clone())
        .or_insert_with(|| Provider::for_kind(kind));

    if provider.kind != kind {
        *provider = Provider::for_kind(kind);
    }
    if let Some(model) = args.model {
        provider.model = model;
    }
    if let Some(effort) = args.effort {
        provider.reasoning_effort = Some(effort);
    } else if args.clear_effort {
        provider.reasoning_effort = None;
    }
    if let Some(model) = args.small_model {
        provider.small_model = non_empty(model);
    }
    if let Some(base_url) = args.base_url {
        provider.base_url = non_empty(base_url);
    }
    if let Some(base_url) = args.anthropic_base_url {
        provider.anthropic_base_url = non_empty(base_url);
    }
    if let Some(protocol) = args.protocol {
        provider.protocol = protocol;
    }
    if let Some(auth) = args.auth {
        provider.auth = auth;
    }
    if let Some(name) = args.api_key_env {
        provider.api_key_env = non_empty(name);
    }
    if let Some(profile) = args.codex_profile {
        provider.codex_profile = non_empty(profile);
    }
    if args.disable {
        provider.enabled = false;
    } else if args.enable {
        provider.enabled = true;
    }

    if !exists {
        for agent in Agent::ALL {
            if store.config.defaults.get(agent).is_empty() && provider.supports(agent) {
                store.config.defaults.set(agent, &args.name);
            }
        }
    }
    Ok(())
}

fn remove(store: &mut Store, name: &str) -> Result<()> {
    if !store.config.providers.contains_key(name) {
        bail!("provider profile '{name}' does not exist");
    }
    let defaults: Vec<_> = Agent::ALL
        .into_iter()
        .filter(|agent| {
            store.config.defaults.is_explicit(*agent) && store.config.defaults.get(*agent) == name
        })
        .collect();
    if !defaults.is_empty() {
        let list = defaults
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "provider '{name}' is still the default for {list}; change those defaults before removing it"
        );
    }
    store.config.providers.remove(name);
    store.credentials.api_keys.remove(name);
    Ok(())
}

fn set_key(store: &mut Store, args: KeyArgs) -> Result<()> {
    if !store.config.providers.contains_key(&args.provider) {
        bail!("provider profile '{}' does not exist", args.provider);
    }
    if args.clear {
        store.set_key(&args.provider, String::new());
        println!("cleared the saved key for '{}'", args.provider);
        return Ok(());
    }

    let key = if args.stdin {
        let mut value = String::new();
        io::stdin().read_to_string(&mut value)?;
        value.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        rpassword::prompt_password(format!("API key for {}: ", args.provider))?
    };
    if key.is_empty() {
        bail!("API key cannot be empty; pass --clear to remove it");
    }
    store.set_key(&args.provider, key);
    println!("saved the key for '{}'", args.provider);
    Ok(())
}

fn print_config(store: &Store) -> Result<()> {
    print!("{}", toml::to_string_pretty(&store.config)?);
    println!("\n# Credential status (values are never printed)");
    for (name, provider) in &store.config.providers {
        let status = if provider
            .api_key_env
            .as_deref()
            .and_then(|variable| std::env::var(variable).ok())
            .is_some_and(|value| !value.is_empty())
        {
            "environment"
        } else if store.credentials.api_keys.contains_key(name) {
            "saved-local"
        } else if matches!(provider.auth, AuthStyle::Native | AuthStyle::None) {
            "not-required"
        } else {
            "missing"
        };
        println!("# {name}: {status}");
    }

    // Sharing is the one thing people go looking for in `alc config` and do
    // not find, because it lives in remote.toml rather than in the dump
    // above. Printed as comments, and attributed to its own file, so the
    // TOML half of this output still round-trips as config.toml.
    println!("\n# Remote control (remote.toml; `alc config` → Sharing & remote)");
    match remote::Settings::load(&store.dir) {
        Ok(settings) => {
            // The stored values, not the effective ones: this command reports
            // what is in the files it names, and printing `off` for a file
            // that says `auto_share = true` would misdescribe it. The
            // dependency between the two gets its own line instead.
            println!("# sharing: {}", remote::on_off(settings.enabled));
            println!(
                "# share by default: {}",
                remote::on_off(settings.auto_share)
            );
            if settings.auto_share && !settings.enabled {
                println!("# note: sharing is off, so nothing shares by default yet");
            }
        }
        Err(error) => println!("# unreadable: {error}"),
    }
    Ok(())
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_provider(model: &str, effort: Option<ReasoningEffort>) -> Provider {
        let mut provider = Provider::for_kind(ProviderKind::Codex);
        provider.model = model.to_owned();
        provider.reasoning_effort = effort;
        provider
    }

    #[test]
    fn saved_provider_values_become_the_session_defaults() {
        let provider = codex_provider("gpt-5.6-luna", Some(ReasoningEffort::Low));
        let resolved =
            resolve_codex_defaults(&provider, &ModelCatalog::built_in(), None, None).unwrap();
        assert_eq!(resolved, ("gpt-5.6-luna".to_owned(), ReasoningEffort::Low));
    }

    #[test]
    fn command_line_values_override_the_saved_provider() {
        let provider = codex_provider("gpt-5.6-luna", Some(ReasoningEffort::Low));
        let resolved = resolve_codex_defaults(
            &provider,
            &ModelCatalog::built_in(),
            Some("gpt-5.6"),
            Some(ReasoningEffort::Max),
        )
        .unwrap();
        assert_eq!(resolved, ("gpt-5.6-sol".to_owned(), ReasoningEffort::Max));
    }

    #[test]
    fn an_effort_the_model_rejects_is_reported() {
        let mut catalog = ModelCatalog::built_in();
        let limited = catalog
            .models
            .iter_mut()
            .find(|model| model.id == "gpt-5.6-luna")
            .expect("catalog entry");
        limited.supported_efforts = vec![ReasoningEffort::Low];
        let provider = codex_provider("gpt-5.6-luna", Some(ReasoningEffort::Max));

        let error = resolve_codex_defaults(&provider, &catalog, None, None).unwrap_err();
        assert!(
            error.to_string().contains("does not support"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn gpt_6_is_offered_with_its_own_top_tier() {
        let catalog = ModelCatalog::built_in();
        let astra = catalog
            .find("gpt-6-astra")
            .expect("gpt-6-astra in the catalog");
        assert!(astra.supported_efforts.contains(&ReasoningEffort::Ultra));
        // Most capable first, so a picker's first row is the best model.
        assert_eq!(catalog.models[0].id, "gpt-6-astra");
    }

    #[test]
    fn a_bridged_session_is_clamped_to_what_the_helper_can_carry() {
        // The bundled helper's effort enum stops at max; a request carrying
        // `ultra` is refused, and a refusal mid-session is a far worse way
        // to learn that than a note at launch.
        let provider = codex_provider("gpt-6-astra", None);
        let (model, effort) = resolve_codex_defaults(
            &provider,
            &ModelCatalog::built_in(),
            None,
            Some(ReasoningEffort::Ultra),
        )
        .unwrap();
        assert_eq!(model, "gpt-6-astra");
        assert_eq!(effort, ReasoningEffort::Max);
    }

    #[test]
    fn every_other_effort_passes_through_untouched() {
        for effort in ReasoningEffort::ALL {
            if effort == ReasoningEffort::Ultra {
                continue;
            }
            assert_eq!(clamp_for_bridge(effort), effort);
        }
    }

    #[test]
    fn an_effort_the_model_does_not_have_is_still_refused() {
        // gpt-5.6-luna has no ultra tier, so asking for one is an error
        // rather than something to quietly clamp.
        let provider = codex_provider("gpt-5.6-luna", None);
        let error = resolve_codex_defaults(
            &provider,
            &ModelCatalog::built_in(),
            None,
            Some(ReasoningEffort::Ultra),
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not support"), "{error}");
    }

    #[test]
    fn empty_values_become_none() {
        assert_eq!(non_empty("".into()), None);
        assert_eq!(non_empty("  ".into()), None);
        assert_eq!(non_empty("value".into()), Some("value".into()));
    }
}
