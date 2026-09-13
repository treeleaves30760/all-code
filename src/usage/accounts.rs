//! Which login each provider profile spends, and how to find it.
//!
//! # Why a profile is the unit
//!
//! alc already models "a provider you can launch against" as a profile, and a
//! second ChatGPT or Claude login is exactly that: the same kind, a different
//! credential directory. So two logins are two profiles, and the fields that
//! pin their directories live on `Provider` next to the rest of what a launch
//! resolves. The alternative - a separate account list - would let the account
//! you read a quota for drift away from the account your session spends.
//!
//! # Why nothing here is refreshed
//!
//! Reading a quota is a diagnostic. Rotating somebody else's credential file
//! from a status command is not: OpenAI's refresh tokens are single-use, so a
//! rotation here races the Codex CLI and every running bridge, and Anthropic's
//! token endpoint is not documented at all. An expired token is reported with
//! the command that renews it, and the next real session renews it anyway.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bridge::auth::{self, AuthManager};
use crate::config::{Provider, ProviderKind, Store};

/// Where a credential was found, so a report can say whether it came from
/// something durable (the profile) or from whichever shell happened to be
/// running (an environment variable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CredentialSource {
    /// Pinned by `codex_home` / `claude_config_dir` on the profile.
    Profile,
    /// From `CODEX_HOME`, `CCP_CODEX_AUTH_FILE` or `CLAUDE_CONFIG_DIR`.
    Env,
    /// The conventional location under the user's home directory.
    Default,
    /// An API key, from the keyring file or the profile's `api_key_env`.
    Key,
    /// Nothing to read.
    None,
}

/// The vendors whose remaining balance alc can ask for with an API key.
///
/// Deliberately short: every entry here is an endpoint alc has a documented or
/// verified shape for. A kind that is missing renders "no quota API" rather
/// than a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyVendor {
    Openrouter,
    Deepseek,
    Moonshot,
    Minimax,
    Zai,
}

impl KeyVendor {
    fn for_kind(kind: ProviderKind) -> Option<Self> {
        match kind {
            ProviderKind::Openrouter => Some(Self::Openrouter),
            ProviderKind::Deepseek => Some(Self::Deepseek),
            ProviderKind::Moonshot => Some(Self::Moonshot),
            ProviderKind::Minimax => Some(Self::Minimax),
            ProviderKind::Zai => Some(Self::Zai),
            _ => None,
        }
    }
}

/// What alc will read for one profile.
pub(crate) enum Credential {
    Codex {
        auth_file: PathBuf,
    },
    Claude {
        config_dir: PathBuf,
    },
    ApiKey {
        key: String,
        vendor: KeyVendor,
    },
    /// Nothing to read, and the sentence that says why.
    None {
        reason: String,
    },
}

/// One enabled profile, resolved to the credential it would launch with.
pub(crate) struct Discovered {
    pub profile: String,
    pub kind: ProviderKind,
    pub credential: Credential,
    pub source: CredentialSource,
}

impl Discovered {
    /// The key two profiles share when they point at one login, so the second
    /// can say so instead of asking the vendor twice.
    pub(crate) fn identity(&self) -> Option<String> {
        match &self.credential {
            Credential::Codex { auth_file } => Some(format!("codex:{}", auth_file.display())),
            Credential::Claude { config_dir } => Some(format!("claude:{}", config_dir.display())),
            // Keyed by the key itself, never shown: two profiles holding one
            // key are one account at the vendor.
            Credential::ApiKey { key, .. } => Some(format!("key:{key}")),
            Credential::None { .. } => None,
        }
    }
}

/// The environment inputs discovery reads, taken rather than read, so a test
/// can pin one arrangement without a process-wide variable every other test in
/// the binary would share.
#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    pub ccp_codex_auth_file: Option<OsString>,
    pub codex_home: Option<OsString>,
    pub claude_config_dir: Option<OsString>,
    pub home: Option<PathBuf>,
}

impl Env {
    pub(crate) fn from_process() -> Self {
        Self {
            ccp_codex_auth_file: std::env::var_os("CCP_CODEX_AUTH_FILE"),
            codex_home: std::env::var_os("CODEX_HOME"),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
            home: crate::launch::home_dir(),
        }
    }

    /// The environment a daemon is allowed to trust: its own home directory
    /// and nothing else.
    ///
    /// The hub inherited its environment from whichever shell started it,
    /// possibly days ago, so a `CODEX_HOME` in there answers for that shell
    /// rather than for the user asking now. Profiles are environment-free and
    /// stay authoritative.
    pub(crate) fn hub_safe() -> Self {
        Self {
            home: crate::launch::home_dir(),
            ..Self::default()
        }
    }
}

/// Every enabled profile, resolved to the login it would use.
///
/// `requested` filters the way `Config::resolve` does: an exact profile name
/// first, then a kind shortcut, so `alc --codex usage` shows the Codex rows.
pub(crate) fn discover(store: &Store, requested: Option<&str>, env: &Env) -> Vec<Discovered> {
    store
        .config
        .providers
        .iter()
        .filter(|(_, provider)| provider.enabled)
        .filter(|(name, provider)| match requested {
            None => true,
            Some(wanted) => name.as_str() == wanted || provider.kind.as_str() == wanted,
        })
        .map(|(name, provider)| {
            let (credential, source) = resolve(store, name, provider, env);
            Discovered {
                profile: name.clone(),
                kind: provider.kind,
                credential,
                source,
            }
        })
        .collect()
}

fn resolve(
    store: &Store,
    name: &str,
    provider: &Provider,
    env: &Env,
) -> (Credential, CredentialSource) {
    match provider.kind {
        ProviderKind::Codex => codex_credential(provider, env),
        ProviderKind::Anthropic if store.credentials.key_for(name, provider).is_none() => {
            claude_credential(provider, env)
        }
        // An Anthropic profile with a key talks to the API, which bills
        // per token and publishes no remaining-balance endpoint.
        ProviderKind::Anthropic => (
            Credential::None {
                reason: "API key; no quota API".to_owned(),
            },
            CredentialSource::Key,
        ),
        kind => match KeyVendor::for_kind(kind) {
            None => (
                Credential::None {
                    reason: "no quota API".to_owned(),
                },
                CredentialSource::None,
            ),
            Some(vendor) => match store.credentials.key_for(name, provider) {
                Some(key) => (Credential::ApiKey { key, vendor }, CredentialSource::Key),
                None => (
                    Credential::None {
                        reason: format!("no API key; run `alc config key {name}`"),
                    },
                    CredentialSource::None,
                ),
            },
        },
    }
}

fn codex_credential(provider: &Provider, env: &Env) -> (Credential, CredentialSource) {
    if let Some(home) = provider.pinned_codex_home() {
        return (
            Credential::Codex {
                auth_file: Path::new(home).join("auth.json"),
            },
            CredentialSource::Profile,
        );
    }
    if let Some(path) = env
        .ccp_codex_auth_file
        .as_ref()
        .filter(|value| !value.is_empty())
    {
        return (
            Credential::Codex {
                auth_file: PathBuf::from(path),
            },
            CredentialSource::Env,
        );
    }
    if let Some(home) = env.codex_home.as_ref().filter(|value| !value.is_empty()) {
        return (
            Credential::Codex {
                auth_file: PathBuf::from(home).join("auth.json"),
            },
            CredentialSource::Env,
        );
    }
    match &env.home {
        Some(home) => (
            Credential::Codex {
                auth_file: home.join(".codex/auth.json"),
            },
            CredentialSource::Default,
        ),
        None => (
            Credential::None {
                reason: "could not resolve the Codex auth path; set CODEX_HOME".to_owned(),
            },
            CredentialSource::None,
        ),
    }
}

fn claude_credential(provider: &Provider, env: &Env) -> (Credential, CredentialSource) {
    let source = if provider.pinned_claude_config_dir().is_some() {
        CredentialSource::Profile
    } else if env
        .claude_config_dir
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        CredentialSource::Env
    } else {
        CredentialSource::Default
    };
    match crate::agents::claude::resolve_claude_config_dir(
        provider.pinned_claude_config_dir(),
        env.claude_config_dir.clone(),
        env.home.clone(),
    ) {
        Some(config_dir) => (Credential::Claude { config_dir }, source),
        None => (
            Credential::None {
                reason: "could not resolve the Claude config directory; it must be absolute"
                    .to_owned(),
            },
            CredentialSource::None,
        ),
    }
}

/// A Codex login as `auth.json` states it.
///
/// No `Debug`, `Clone` or `Serialize`: the access token in here must not be
/// able to reach a log line, a report or a `{:?}`, and the compiler is a
/// better guarantee of that than a review.
pub(crate) struct CodexLogin {
    pub access_token: String,
    pub account_id: Option<String>,
    /// The `email` claim of the id token, or the tail of the account id.
    pub label: Option<String>,
    /// The plan the id token claims, used only to fill the column when the
    /// live fetch fails.
    pub plan_hint: Option<String>,
    pub expires_at_ms: u64,
}

/// Reads `auth.json` through the bridge's own parser, so alc has exactly one
/// idea of what that file looks like.
pub(crate) fn read_codex_login(auth_file: &Path) -> Result<CodexLogin, String> {
    let file = AuthManager::new(auth_file.to_path_buf())
        .read_file()
        .map_err(|error| error.to_string())?;
    let tokens = file
        .tokens
        .filter(|tokens| !tokens.access_token.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "{} holds no Codex access token; run `codex login`",
                auth_file.display()
            )
        })?;

    let account_id = tokens
        .account_id
        .clone()
        .filter(|id| !id.is_empty())
        .or_else(|| {
            tokens
                .id_token
                .as_deref()
                .and_then(auth::account_id_from_token)
        })
        .or_else(|| auth::account_id_from_token(&tokens.access_token));
    let claims = tokens
        .id_token
        .as_deref()
        .and_then(auth::decode_jwt_payload);
    let claim = |key: &str| {
        claims
            .as_ref()
            .and_then(|claims| claims.get("https://api.openai.com/auth"))
            .and_then(|auth| auth.get(key))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let email = claims
        .as_ref()
        .and_then(|claims| claims.get("email"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| claim("email"));

    Ok(CodexLogin {
        expires_at_ms: auth::token_expiry_ms(&tokens.access_token).unwrap_or(0),
        label: email.or_else(|| account_id.as_deref().map(elide_id)),
        plan_hint: claim("chatgpt_plan_type"),
        account_id,
        access_token: tokens.access_token,
    })
}

/// A Claude Code login as its credential store states it.
///
/// Holds a token, so it carries no `Debug`/`Serialize` either. See
/// [`CodexLogin`].
pub(crate) struct ClaudeLogin {
    pub access_token: String,
    pub expires_at_ms: u64,
    pub subscription: Option<String>,
}

/// Where a Claude login may be read from, so the hub can refuse the one that
/// would raise a dialog on somebody's desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeStores {
    /// The credentials file and, on macOS, the Keychain.
    All,
    /// The credentials file only.
    FileOnly,
}

/// Why a Claude login could not be read.
///
/// The distinction matters for the exit code: "you are not signed in" is a
/// thing the person reading can fix, and "this process may not open the
/// Keychain" is not.
pub(crate) struct ClaudeRefusal {
    pub message: String,
    /// Whether the reader could do something about it where they are.
    pub actionable: bool,
}

/// Reads Claude Code's login for one config directory, without writing it.
///
/// On macOS the Keychain is the store and the file exists only where the
/// Keychain refused the write, so the Keychain is asked first. Everywhere else
/// there is only the file. A daemon passes [`ClaudeStores::FileOnly`]: a
/// Keychain read from a background process raises a dialog on a desktop
/// nobody is looking at.
pub(crate) fn read_claude_login(
    config_dir: &Path,
    home: Option<&Path>,
    stores: ClaudeStores,
) -> Result<ClaudeLogin, ClaudeRefusal> {
    let missing = || {
        // On macOS the login normally lives in the Keychain, and a daemon is
        // not allowed to open it. Saying "not signed in" there would send
        // somebody looking for a login they have already made.
        if cfg!(target_os = "macos") && stores == ClaudeStores::FileOnly {
            return ClaudeRefusal {
                message: "the Claude login is in the macOS Keychain, which the hub does not \
                    open; run `alc usage` in a terminal for this row"
                    .to_owned(),
                actionable: false,
            };
        }
        ClaudeRefusal {
            message: format!(
                "no Claude Code login in {}; run `claude` and sign in",
                crate::usage::elide_home(config_dir, home)
            ),
            actionable: true,
        }
    };

    let mut raw = None;
    if stores == ClaudeStores::All {
        raw = read_keychain(config_dir, home);
    }
    if raw.is_none() {
        raw = std::fs::read_to_string(config_dir.join(".credentials.json")).ok();
    }
    let raw = raw.ok_or_else(missing)?;

    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|error| ClaudeRefusal {
        message: format!("the stored Claude login is not valid JSON: {error}"),
        actionable: true,
    })?;
    // Claude Code 2.1.x has been seen storing an item that holds only its MCP
    // OAuth state, with no login in it at all.
    let oauth = value.get("claudeAiOauth").ok_or_else(|| {
        let refusal = missing();
        ClaudeRefusal {
            message: format!("{} (the stored item holds no login)", refusal.message),
            ..refusal
        }
    })?;
    let text = |key: &str| {
        oauth
            .get(key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let access_token = text("accessToken").ok_or_else(missing)?;

    Ok(ClaudeLogin {
        access_token,
        expires_at_ms: oauth.get("expiresAt").and_then(|v| v.as_u64()).unwrap_or(0),
        subscription: text("subscriptionType"),
    })
}

/// The Keychain item Claude Code writes, read with the system tool.
///
/// The service name is keyed to the config directory, so a second login under
/// its own `CLAUDE_CONFIG_DIR` has its own item. A custom directory never
/// falls back to the default item: showing the wrong account's quota is worse
/// than showing none.
#[cfg(target_os = "macos")]
fn read_keychain(config_dir: &Path, home: Option<&Path>) -> Option<String> {
    use std::process::Command;

    let default_dir = home.map(|home| home.join(".claude"));
    let service = if default_dir.as_deref() == Some(config_dir) {
        "Claude Code-credentials".to_owned()
    } else {
        // Community-observed, not documented by Anthropic: a miss simply
        // reads as "not signed in" for that directory.
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(config_dir.to_string_lossy().as_bytes());
        format!("Claude Code-credentials-{}", hex8(&digest))
    };
    // Absolute, so a test that empties PATH still finds it.
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", &service, "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(not(target_os = "macos"))]
fn read_keychain(_config_dir: &Path, _home: Option<&Path>) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn hex8(digest: &[u8]) -> String {
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// An account id is long and not worth reading in full; its tail is enough to
/// tell two logins apart.
fn elide_id(id: &str) -> String {
    let tail: String = id
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Credentials, Provider};
    use std::collections::BTreeMap;

    fn store(providers: Vec<(&str, Provider)>, keys: Vec<(&str, &str)>) -> Store {
        Store {
            dir: PathBuf::from("/tmp/alc-test"),
            config: Config {
                version: crate::config::CONFIG_VERSION,
                defaults: Default::default(),
                providers: providers
                    .into_iter()
                    .map(|(name, provider)| (name.to_owned(), provider))
                    .collect(),
            },
            credentials: Credentials {
                version: crate::config::CONFIG_VERSION,
                api_keys: keys
                    .into_iter()
                    .map(|(name, key)| (name.to_owned(), key.to_owned()))
                    .collect::<BTreeMap<_, _>>(),
            },
        }
    }

    fn env() -> Env {
        Env {
            home: Some(PathBuf::from("/home/ada")),
            ..Env::default()
        }
    }

    fn codex_path(found: &Discovered) -> &Path {
        match &found.credential {
            Credential::Codex { auth_file } => auth_file,
            _ => panic!("expected a codex credential"),
        }
    }

    /// The whole point of the field: an ambient `CODEX_HOME` must not move the
    /// account a named profile points at.
    #[test]
    fn a_pinned_codex_home_beats_every_environment_variable() {
        let mut provider = Provider::for_kind(ProviderKind::Codex);
        provider.codex_home = Some("/work/codex".to_owned());
        let store = store(vec![("codex", provider)], vec![]);
        let env = Env {
            ccp_codex_auth_file: Some(OsString::from("/shell/auth.json")),
            codex_home: Some(OsString::from("/shell/codex")),
            ..env()
        };

        let found = discover(&store, None, &env);
        assert_eq!(codex_path(&found[0]), Path::new("/work/codex/auth.json"));
        assert_eq!(found[0].source, CredentialSource::Profile);
    }

    #[test]
    fn the_environment_ladder_runs_explicit_file_then_home_then_the_default() {
        let store = store(
            vec![("codex", Provider::for_kind(ProviderKind::Codex))],
            vec![],
        );

        let explicit = Env {
            ccp_codex_auth_file: Some(OsString::from("/shell/auth.json")),
            codex_home: Some(OsString::from("/shell/codex")),
            ..env()
        };
        assert_eq!(
            codex_path(&discover(&store, None, &explicit)[0]),
            Path::new("/shell/auth.json")
        );

        let home_only = Env {
            codex_home: Some(OsString::from("/shell/codex")),
            ..env()
        };
        let found = discover(&store, None, &home_only);
        assert_eq!(codex_path(&found[0]), Path::new("/shell/codex/auth.json"));
        assert_eq!(found[0].source, CredentialSource::Env);

        let found = discover(&store, None, &env());
        assert_eq!(
            codex_path(&found[0]),
            Path::new("/home/ada/.codex/auth.json")
        );
        assert_eq!(found[0].source, CredentialSource::Default);
    }

    /// A daemon must answer from durable configuration, never from the shell
    /// that happened to start it.
    #[test]
    fn the_hub_environment_carries_no_shell_variables() {
        let env = Env::hub_safe();
        assert!(env.codex_home.is_none());
        assert!(env.ccp_codex_auth_file.is_none());
        assert!(env.claude_config_dir.is_none());
    }

    #[test]
    fn an_anthropic_profile_with_a_key_has_no_quota_api_and_one_without_reads_a_login() {
        let keyed = store(
            vec![("anthropic", Provider::for_kind(ProviderKind::Anthropic))],
            vec![("anthropic", "sk-test")],
        );
        match &discover(&keyed, None, &env())[0].credential {
            Credential::None { reason } => assert_eq!(reason, "API key; no quota API"),
            _ => panic!("expected no credential"),
        }

        let native = store(
            vec![("anthropic", Provider::for_kind(ProviderKind::Anthropic))],
            vec![],
        );
        match &discover(&native, None, &env())[0].credential {
            Credential::Claude { config_dir } => {
                assert_eq!(config_dir, Path::new("/home/ada/.claude"));
            }
            _ => panic!("expected a claude credential"),
        }
    }

    #[test]
    fn a_key_vendor_without_a_saved_key_says_which_command_saves_one() {
        let store = store(
            vec![("openrouter", Provider::for_kind(ProviderKind::Openrouter))],
            vec![],
        );
        match &discover(&store, None, &env())[0].credential {
            Credential::None { reason } => {
                assert_eq!(reason, "no API key; run `alc config key openrouter`");
            }
            _ => panic!("expected no credential"),
        }
    }

    #[test]
    fn a_kind_with_no_balance_endpoint_says_so_rather_than_guessing() {
        let store = store(
            vec![("groq", Provider::for_kind(ProviderKind::Groq))],
            vec![("groq", "gsk-test")],
        );
        match &discover(&store, None, &env())[0].credential {
            Credential::None { reason } => assert_eq!(reason, "no quota API"),
            _ => panic!("expected no credential"),
        }
    }

    #[test]
    fn the_requested_selector_matches_a_profile_name_then_a_kind() {
        let store = store(
            vec![
                ("codex", Provider::for_kind(ProviderKind::Codex)),
                ("codex-work", Provider::for_kind(ProviderKind::Codex)),
                ("ollama", Provider::for_kind(ProviderKind::Ollama)),
            ],
            vec![],
        );
        let names = |requested| {
            discover(&store, requested, &env())
                .into_iter()
                .map(|found| found.profile)
                .collect::<Vec<_>>()
        };

        assert_eq!(names(Some("codex-work")), vec!["codex-work"]);
        assert_eq!(names(Some("codex")), vec!["codex", "codex-work"]);
        assert_eq!(names(None).len(), 3);
    }

    #[test]
    fn a_disabled_profile_is_not_an_account() {
        let mut disabled = Provider::for_kind(ProviderKind::Codex);
        disabled.enabled = false;
        let store = store(vec![("codex", disabled)], vec![]);
        assert!(discover(&store, None, &env()).is_empty());
    }

    #[test]
    fn two_profiles_on_one_auth_file_share_an_identity() {
        let mut pinned = Provider::for_kind(ProviderKind::Codex);
        pinned.codex_home = Some("/home/ada/.codex".to_owned());
        let store = store(
            vec![
                ("codex", Provider::for_kind(ProviderKind::Codex)),
                ("codex-alias", pinned),
            ],
            vec![],
        );
        let found = discover(&store, None, &env());
        assert_eq!(found[0].identity(), found[1].identity());
    }

    #[test]
    fn a_codex_login_without_an_id_token_is_labelled_by_the_account_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "tokens": { "access_token": "a.b.c", "refresh_token": "r", "account_id": "acct_abcdef123456" }
            })
            .to_string(),
        )
        .unwrap();

        let Ok(login) = read_codex_login(&path) else {
            panic!("the auth file should have been read");
        };
        assert_eq!(login.label.as_deref(), Some("…123456"));
        assert_eq!(
            login.expires_at_ms, 0,
            "an unreadable exp counts as expired"
        );
    }

    #[test]
    fn a_missing_auth_file_names_the_command_that_writes_one() {
        let dir = tempfile::tempdir().unwrap();
        let Err(error) = read_codex_login(&dir.path().join("auth.json")) else {
            panic!("a missing auth file cannot be a login");
        };
        assert!(error.contains("codex login"), "{error}");
    }

    #[test]
    fn a_claude_credentials_file_is_read_and_an_item_without_a_login_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".credentials.json"),
            serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": "sk-ant-oat01-x",
                    "expiresAt": 4_000_000_000_000u64,
                    "subscriptionType": "max"
                }
            })
            .to_string(),
        )
        .unwrap();
        let Ok(login) = read_claude_login(dir.path(), None, ClaudeStores::FileOnly) else {
            panic!("the credentials file should have been read");
        };
        assert_eq!(login.subscription.as_deref(), Some("max"));
        assert_eq!(login.expires_at_ms, 4_000_000_000_000);

        std::fs::write(
            dir.path().join(".credentials.json"),
            serde_json::json!({ "mcpOAuth": {} }).to_string(),
        )
        .unwrap();
        let Err(refusal) = read_claude_login(dir.path(), None, ClaudeStores::FileOnly) else {
            panic!("an item with no login cannot be a login");
        };
        assert!(
            refusal.message.contains("holds no login"),
            "{}",
            refusal.message
        );
    }

    #[test]
    fn a_directory_with_no_claude_login_names_the_command_that_makes_one() {
        let dir = tempfile::tempdir().unwrap();
        let Err(refusal) = read_claude_login(dir.path(), None, ClaudeStores::FileOnly) else {
            panic!("an empty directory holds no login");
        };
        // On macOS a file-only read means the hub, which cannot open the
        // Keychain, so the row says that rather than blaming the user.
        if cfg!(target_os = "macos") {
            assert!(refusal.message.contains("Keychain"), "{}", refusal.message);
            assert!(!refusal.actionable);
        } else {
            assert!(
                refusal.message.contains("run `claude` and sign in"),
                "{}",
                refusal.message
            );
            assert!(refusal.actionable);
        }
    }
}
