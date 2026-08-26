use std::ffi::OsString;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::config::{
    Agent, AuthStyle, Protocol, Provider, ProviderKind, ReasoningEffort, Store,
    validate_profile_name,
};
use crate::model_catalog::ModelCatalog;
use crate::model_picker::{self, PickerRequest};
use crate::{doctor, launch, tui, update};

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

    /// Print the resolved command and environment without launching it.
    #[arg(long, global = true)]
    dry_run: bool,

    /// Override the alc config directory (also available as ALC_CONFIG_DIR).
    #[arg(long, global = true, env = "ALC_CONFIG_DIR", hide = true)]
    config_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the provider configuration TUI or use a scripting subcommand.
    Config(ConfigArgs),
    /// Check agent binaries, credentials, defaults, and compatibility.
    Doctor,
    /// Show or refresh the GPT models available for Codex-to-Claude.
    Models(ModelsArgs),
    /// Check for and install the latest alc release.
    Update(UpdateArgs),
    /// Launch Claude Code.
    Claude(ClaudeArgs),
    /// Launch Codex CLI.
    Codex(Passthrough),
    /// Launch OpenCode.
    Opencode(Passthrough),
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
    /// GPT model for this run (for example gpt-5.6-terra).
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// Codex reasoning effort: low, medium, high, xhigh, or max.
    #[arg(long, value_name = "LEVEL")]
    effort: Option<ReasoningEffort>,

    /// Use configured defaults without opening the model picker.
    #[arg(long)]
    no_picker: bool,

    /// Save the selected model and effort as this provider's defaults.
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
        /// claude, codex, or opencode.
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

    match cli.command {
        Command::Config(args) => run_config(&mut store, args),
        Command::Doctor => Ok(if doctor::run(&store)? { 0 } else { 1 }),
        Command::Models(args) => run_models(&store, args),
        Command::Update(_) => unreachable!("update is handled before config loading"),
        Command::Claude(args) => {
            run_claude(&mut store, requested_provider.as_deref(), args, cli.dry_run)
        }
        Command::Codex(args) => run_agent(
            &store,
            Agent::Codex,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
        ),
        Command::Opencode(args) => run_agent(
            &store,
            Agent::Opencode,
            requested_provider.as_deref(),
            args.args,
            cli.dry_run,
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

fn run_agent(
    store: &Store,
    agent: Agent,
    requested_provider: Option<&str>,
    args: Vec<OsString>,
    dry_run: bool,
) -> Result<u8> {
    let spec = launch::build(
        store,
        agent,
        requested_provider,
        &args,
        &launch::LaunchOverrides::default(),
    )?;
    run_spec(spec, dry_run)
}

fn run_claude(
    store: &mut Store,
    requested_provider: Option<&str>,
    args: ClaudeArgs,
    dry_run: bool,
) -> Result<u8> {
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
        let overrides = launch::LaunchOverrides {
            model: args.model,
            reasoning_effort: None,
            context_window: None,
        };
        let spec = launch::build(
            store,
            Agent::Claude,
            requested_provider,
            &args.args,
            &overrides,
        )?;
        return run_spec(spec, dry_run);
    }

    let catalog = if dry_run {
        ModelCatalog::load(&store.dir)
    } else {
        ModelCatalog::load_and_refresh_if_due(&store.dir)
    };
    let mut model = args
        .model
        .as_deref()
        .map(launch::normalize_codex_model)
        .unwrap_or(launch::resolve_codex_model(&provider)?);
    let mut effort = args
        .effort
        .or(provider.reasoning_effort)
        .or(launch::resolve_codex_effort(&provider)?)
        .or_else(|| catalog.find(&model).map(|entry| entry.default_effort))
        .unwrap_or(ReasoningEffort::Medium);
    let mut remember = args.save;

    let interactive = !dry_run
        && !args.no_picker
        && io::stdin().is_terminal()
        && io::stdout().is_terminal()
        && (args.model.is_none() || args.effort.is_none());
    if interactive {
        let selection = model_picker::run(
            &catalog,
            PickerRequest {
                model,
                effort,
                choose_model: args.model.is_none(),
                choose_effort: args.effort.is_none(),
            },
        )?;
        let Some(selection) = selection else {
            println!("Cancelled.");
            return Ok(0);
        };
        model = selection.model;
        effort = selection.effort;
        remember |= selection.remember;
    }

    if let Some(entry) = catalog.find(&model)
        && !entry.supported_efforts.contains(&effort)
    {
        bail!("model '{model}' does not support reasoning effort '{effort}'");
    }

    if remember {
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

    let context_window = catalog.find(&model).map(|entry| entry.context_window);
    let overrides = launch::LaunchOverrides {
        model: Some(model),
        reasoning_effort: Some(effort),
        context_window,
    };
    let spec = launch::build(
        store,
        Agent::Claude,
        requested_provider,
        &args.args,
        &overrides,
    )?;
    run_spec(spec, dry_run)
}

fn run_spec(spec: launch::LaunchSpec, dry_run: bool) -> Result<u8> {
    if dry_run {
        println!(
            "agent: {}\nprovider: {} ({})\ncommand: {}",
            spec.agent,
            spec.provider_name,
            spec.provider_kind,
            spec.redacted_command()
        );
        if spec.needs_codex_proxy {
            println!(
                "adapter: bundled claude-codex {} on an ephemeral loopback port",
                launch::CLAUDE_CODEX_HELPER_VERSION
            );
        }
        return Ok(0);
    }
    launch::execute(spec)
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

    println!("Codex -> Claude model catalog");
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
        .filter(|agent| store.config.defaults.get(*agent) == name)
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
    Ok(())
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_values_become_none() {
        assert_eq!(non_empty("".into()), None);
        assert_eq!(non_empty("  ".into()), None);
        assert_eq!(non_empty("value".into()), Some("value".into()));
    }
}
