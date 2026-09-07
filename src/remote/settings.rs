//! The remote-control settings sidecar and the three session credentials.
//!
//! Both live beside `config.toml` rather than inside it. `Config` is
//! `deny_unknown_fields` and `validate()` refuses any version but its own
//! before every save, so a `[remote]` table added to config.toml would make
//! every older alc sharing the config dir fail to parse the file it is about
//! to rewrite. `remote.toml` carries neither guard: `#[serde(default)]` with
//! unknown keys ignored means an older alc reads a newer one's file, keeps the
//! keys it understands, and drops the rest instead of erroring out.
//!
//! The tokens stay out of `credentials.toml` for a different reason. They are
//! not user-entered API keys but per-machine session material that
//! `Secrets::rotate` is expected to throw away, so they live one directory
//! down in `run/`, where deleting the whole directory is a safe thing for a
//! user to do when a token has leaked.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::atomic_write;

/// 32 bytes is the width every token in this feature is minted at: below a
/// browser-reachable secret's useful floor there is no point, and above it the
/// string stops fitting in a phone's address bar next to the URL.
const TOKEN_BYTES: usize = 32;

/// Environment name and file name for each credential, in `ctl`, `operator`,
/// `viewer` order. Kept as one table so the three roles cannot drift apart.
const ROLES: [(&str, &str); 3] = [
    ("ALC_REMOTE_CTL", "ctl.token"),
    ("ALC_REMOTE_OPERATOR", "operator.token"),
    ("ALC_REMOTE_VIEWER", "viewer.token"),
];

/// Which interface the mirror's listener binds.
///
/// `Loopback` is the default and is all a tunnel needs - `tailscale serve`
/// and `cloudflared` both connect to 127.0.0.1 and terminate TLS themselves,
/// so alc is never the thing facing the network. `Lan` is for reaching a
/// session from a phone on the same Wi-Fi with nothing else installed; it is
/// one deliberate flag, because that is the shape of the request, and the
/// token is what actually guards the socket either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Bind {
    Loopback,
    Lan,
}

impl Bind {
    pub(crate) const ALL: [Self; 2] = [Self::Loopback, Self::Lan];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Lan => "lan",
        }
    }
}

impl std::fmt::Display for Bind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for Bind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "loopback" | "local" | "localhost" => Ok(Self::Loopback),
            "lan" | "all" | "network" => Ok(Self::Lan),
            _ => {
                let expected = Self::ALL
                    .iter()
                    .map(|bind| bind.as_str())
                    .collect::<Vec<_>>()
                    .join(" or ");
                bail!("unknown bind '{value}'; expected {expected}")
            }
        }
    }
}

/// Everything the mirror reads out of `<config_dir>/remote.toml`.
///
/// Every field is a posture decision rather than a preference, which is why
/// `load` refuses a file it cannot parse instead of quietly substituting these
/// defaults: a typo in `bind` must not be the reason a session ends up
/// answering the network.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RemoteSettings {
    /// Whether a launch offers the mirror at all. Off leaves the PTY and the
    /// user's own terminal untouched; only the listener is skipped.
    pub enabled: bool,
    pub bind: Bind,

    /// 0 asks the operating system for an ephemeral port, which is what
    /// several concurrent sessions on one machine (and the tests) want.
    pub port: u16,
    /// Names this server answers to on top of its own address.
    ///
    /// Needed behind a tunnel, where the name the browser used is not the
    /// socket's: `box.tail1a2b.ts.net`, or `*.trycloudflare.com` for a quick
    /// tunnel that mints a fresh hostname every run. One list serves both the
    /// `Host` check and the `Origin` check - an origin is allowed exactly
    /// when its host part is - so the two can never drift apart.
    pub allowed_hosts: Vec<String>,
    /// How much recent output a newly attached browser is replayed, so a phone
    /// joining mid-session sees context rather than a blank screen. Bounded
    /// because the buffer is held in memory for the life of the session.
    pub scrollback_bytes: usize,
    /// Ceiling on simultaneous connections, so a client that opens sockets and
    /// never reads cannot exhaust a thread-per-connection server.
    pub max_connections: usize,
    /// Whether the session card may probe the running agent for its current
    /// mode. It costs a round trip through the PTY, so an agent that reacts
    /// badly to one can have it turned off without losing the mirror.
    pub mode_probe: bool,
    /// The loosest permission rung a browser may reach on its own. Anything
    /// past this needs `alc confirm` typed at a terminal on this machine, so
    /// a stolen link cannot quietly take a session's brakes off.
    pub max_permission: String,
}

impl Default for RemoteSettings {
    /// Hand-written because most of these are not the zero value: a derived
    /// `Default` would ship a disabled mirror with no scrollback and no
    /// connection ceiling, and `#[serde(default)]` fills every absent key from
    /// here, so a file that omits a key would get that instead.
    fn default() -> Self {
        Self {
            enabled: true,
            bind: Bind::Loopback,
            port: 8787,
            allowed_hosts: Vec::new(),
            scrollback_bytes: 1_048_576,
            max_connections: 64,
            mode_probe: true,
            max_permission: "auto-edit".to_owned(),
        }
    }
}

impl RemoteSettings {
    /// Reads `<config_dir>/remote.toml`, returning defaults when absent.
    ///
    /// A malformed file is an error naming the path and is never silently
    /// defaulted: these values are a security posture, and falling back would
    /// turn an unreadable `bind = "loopbcak"` into a working config that
    /// says something other than what the user believes it says.
    pub(crate) fn load(config_dir: &Path) -> Result<Self> {
        let path = Self::path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&text)
            .with_context(|| format!("failed to parse {}; fix or delete it", path.display()))
    }

    /// Adds a name this server should answer to, and remembers it.
    ///
    /// A tunnel hostname is not knowable until the tunnel is up, and for a
    /// `cloudflared` quick tunnel it changes every run - so this exists to
    /// be called from the command line rather than requiring a file edit at
    /// exactly the moment the user is trying to get connected.
    pub(crate) fn allow_host(config_dir: &Path, entry: &str) -> Result<Self> {
        let entry = entry.trim().to_ascii_lowercase();
        if entry.is_empty() || entry.contains('/') || entry.contains(' ') {
            bail!("'{entry}' is not a host name; expected `host[:port]` or `*.example.com`");
        }
        let mut settings = Self::load(config_dir)?;
        if !settings.allowed_hosts.contains(&entry) {
            settings.allowed_hosts.push(entry);
            settings.save(config_dir)?;
        }
        Ok(settings)
    }

    pub(crate) fn save(&self, config_dir: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("failed to encode remote settings")?;
        atomic_write(&Self::path(config_dir), text.as_bytes(), false)
    }

    pub(crate) fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("remote.toml")
    }
}

/// The three credentials. `ctl` never reaches a browser.
///
/// They are separate secrets rather than one token with a role attached so
/// that handing a colleague the viewer link cannot be escalated by editing it:
/// a browser only ever learns the token it was given.
#[derive(Clone)]
#[allow(
    dead_code,
    reason = "`ctl` gates process creation over the hub's control socket, next milestone"
)]
pub(crate) struct Secrets {
    /// The local control channel, used by alc's own processes.
    pub ctl: String,
    /// Watch and type.
    pub operator: String,
    /// Watch only.
    pub viewer: String,
}

/// Redacts all three fields. These strings are the whole of the mirror's
/// authentication, so a `{:?}` in some future error path must not be the thing
/// that writes one to a log.
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("ctl", &"<redacted>")
            .field("operator", &"<redacted>")
            .field("viewer", &"<redacted>")
            .finish()
    }
}

impl Secrets {
    /// Loads the three token files from `<config_dir>/run/`, creating any that
    /// are missing. The directory is 0700 and the files 0600 on unix.
    ///
    /// An environment override (`ALC_REMOTE_CTL`, `ALC_REMOTE_OPERATOR`,
    /// `ALC_REMOTE_VIEWER`) wins over the file, mirroring the env-first
    /// contract `Credentials::key_for` already has: a CI job or a wrapper
    /// script can pin the tokens it will hand out without writing anything to
    /// the user's config dir. An overridden role's file is left alone — not
    /// created, not read — so an override never mints a spare secret on disk.
    pub(crate) fn load_or_create(config_dir: &Path) -> Result<Self> {
        let dir = ensure_run_dir(config_dir)?;
        let secrets = Self {
            ctl: role_token(&dir, ROLES[0])?,
            operator: role_token(&dir, ROLES[1])?,
            viewer: role_token(&dir, ROLES[2])?,
        };
        // Two roles sharing a value is not a harmless coincidence: the guard
        // resolves a tie in favour of the more powerful role, so a viewer
        // link that happens to equal the operator's would silently grant
        // keystroke injection into a shell. Exporting one variable to two
        // names is all it takes.
        let roles = [
            (ROLES[0].0, &secrets.ctl),
            (ROLES[1].0, &secrets.operator),
            (ROLES[2].0, &secrets.viewer),
        ];
        for (index, (name, value)) in roles.iter().enumerate() {
            for (other_name, other) in roles.iter().skip(index + 1) {
                if value == other {
                    bail!(
                        "the remote-control tokens for {name} and {other_name} are the same; \
                         they grant different things, so they must differ - unset one of the \
                         environment overrides, or run `alc remote token --rotate`"
                    );
                }
            }
        }
        Ok(secrets)
    }

    /// Mints and stores three fresh tokens, invalidating every link and every
    /// browser tab already holding an old one.
    pub(crate) fn rotate(config_dir: &Path) -> Result<Self> {
        let dir = ensure_run_dir(config_dir)?;
        for (_, file_name) in ROLES {
            let token = generate_token()?;
            atomic_write(&dir.join(file_name), token.as_bytes(), true)?;
        }
        // Re-read instead of returning what was just written: an environment
        // override still outranks the file, and a caller who rotated while one
        // was exported has to be told the token the server will really accept,
        // not the one that landed in the file it will ignore.
        Self::load_or_create(config_dir)
    }

    pub(crate) fn run_dir(config_dir: &Path) -> PathBuf {
        config_dir.join("run")
    }
}

/// One credential: the environment wins, then the file, and only then is a
/// fresh token minted and written.
///
/// A value that is empty or all whitespace counts as absent in both places —
/// an exported-but-empty variable is a shell accident, and a zero-length token
/// file would otherwise become a credential that authorises everyone. Reads
/// are trimmed because a hand-made token file (`echo … > ctl.token`) ends in a
/// newline that is not part of the secret.
fn role_token(run_dir: &Path, (env_name, file_name): (&str, &str)) -> Result<String> {
    if let Some(value) = env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let value = value.trim().to_owned();
        // The generated path mints 256 bits deliberately; an override has to
        // clear a floor too, or `ALC_REMOTE_OPERATOR=x` quietly becomes a
        // one-character credential for typing into a shell.
        if value.len() < MIN_TOKEN_CHARS {
            bail!(
                "{env_name} is only {} characters; a token that can type into a session \
                 needs at least {MIN_TOKEN_CHARS}",
                value.len()
            );
        }
        return Ok(value);
    }

    let path = run_dir.join(file_name);
    match fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => return Ok(text.trim().to_owned()),
        // Unambiguously stale: `create_token` never publishes an empty file,
        // so nothing is mid-write here and clearing it is safe.
        Ok(_) => {
            let _ = fs::remove_file(&path);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    }
    create_token(&path)
}

/// Mints a token, or adopts the one another process minted first.
///
/// The token is written to a temporary file in full and only then linked
/// into place. `hard_link` refuses to replace an existing name, which makes
/// publishing both atomic and exclusive: two processes starting at the same
/// moment - two `alc --share` invocations launched together is an ordinary
/// thing to do - converge on whichever won, and the loser adopts it rather
/// than keeping a token that is no longer on disk and having every request
/// refused for a mismatched secret.
///
/// The property that matters beyond the race: a token file is never
/// observable in a half-written state, because it does not exist under its
/// real name until it is complete. That is what lets `role_token` treat an
/// empty one as stale and clear it, rather than having to wonder whether
/// somebody is mid-write.
fn create_token(path: &Path) -> Result<String> {
    let token = generate_token()?;
    let mut suffix = [0_u8; 9];
    getrandom::fill(&mut suffix)
        .map_err(|error| anyhow::anyhow!("failed to read system randomness: {error}"))?;
    let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
    let staged = path.with_extension(format!("{suffix}.staged"));

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    {
        use std::io::Write;
        let mut file = options
            .open(&staged)
            .with_context(|| format!("failed to create {}", staged.display()))?;
        file.write_all(token.as_bytes())
            .with_context(|| format!("failed to write {}", staged.display()))?;
        file.sync_all().ok();
    }

    let published = fs::hard_link(&staged, path);
    let _ = fs::remove_file(&staged);
    match published {
        Ok(()) => Ok(token),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()))?
                .trim()
                .to_owned();
            if existing.is_empty() {
                bail!(
                    "{} exists but is empty; delete it and try again",
                    path.display()
                );
            }
            Ok(existing)
        }
        Err(error) => Err(error).with_context(|| format!("failed to create {}", path.display())),
    }
}

/// Creates `<config_dir>/run`, owner-only.
fn ensure_run_dir(config_dir: &Path) -> Result<PathBuf> {
    let dir = Secrets::run_dir(config_dir);
    restricted_dir(&dir)?;
    Ok(dir)
}

/// Creates a directory that only its owner may enter, and re-applies that
/// mode on every call.
///
/// Re-applying rather than only setting it at creation time is deliberate:
/// the files inside are 0600, but a group- or world-writable directory lets
/// another account rename one out of the way and drop its own in, which the
/// mode bits on the file do nothing to stop. Idempotent, so every entry
/// point can call it without checking first.
pub(crate) fn restricted_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to restrict {} to its owner", dir.display()))?;
    }
    Ok(())
}

/// The shortest token an environment override may supply. Well under the 43
/// characters the generated path produces, but far enough above a typo that
/// no accident lands here.
const MIN_TOKEN_CHARS: usize = 16;

/// 32 bytes of OS randomness, base64url without padding.
pub(crate) fn generate_token() -> Result<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)
        .context("failed to read operating-system randomness for a remote-control token")?;
    Ok(base64url(&bytes))
}

/// The RFC 4648 §5 URL-safe alphabet: `-` and `_` in place of `+` and `/`, so
/// a token survives being pasted into a URL or a query string untouched.
const ALPHABET: [u8; 64] = *b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url without padding. Hand-rolled to keep a whole crate out of the
/// dependency tree for fifteen lines that only ever see a 32-byte token.
fn base64url(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        // Three bytes make four characters; a one- or two-byte tail makes two
        // or three, and the '=' that would pad it out is dropped.
        for index in 0..=chunk.len() {
            let sextet = (packed >> (18 - 6 * index)) & 0b11_1111;
            encoded.push(ALPHABET[sextet as usize] as char);
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Mutex, PoisonError};

    /// `role_token` reads process-wide environment variables, so every test
    /// that creates tokens has to be kept away from the one that sets an
    /// override — cargo runs them on threads of one process.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_base64url(token: &str) -> bool {
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    }

    #[test]
    fn load_with_no_file_gives_the_documented_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let settings = RemoteSettings::load(temp.path()).unwrap();

        assert!(settings.enabled);
        assert_eq!(settings.bind, Bind::Loopback);
        assert_eq!(settings.port, 8787);
        assert!(settings.allowed_hosts.is_empty());
        assert_eq!(settings.scrollback_bytes, 1_048_576);
        assert_eq!(settings.max_connections, 64);
        assert!(settings.mode_probe);
    }

    #[test]
    fn save_then_load_round_trips_every_field() {
        let temp = tempfile::tempdir().unwrap();
        let settings = RemoteSettings {
            enabled: false,
            bind: Bind::Lan,
            port: 0,
            allowed_hosts: vec![
                "box.tail1a2b.ts.net".to_owned(),
                "*.trycloudflare.com".to_owned(),
            ],
            scrollback_bytes: 4096,
            max_connections: 2,
            max_permission: "plan".to_owned(),
            mode_probe: false,
        };
        settings.save(temp.path()).unwrap();

        let loaded = RemoteSettings::load(temp.path()).unwrap();
        assert!(!loaded.enabled);
        assert_eq!(loaded.bind, Bind::Lan);
        assert_eq!(loaded.port, 0);
        assert_eq!(loaded.allowed_hosts, settings.allowed_hosts);
        assert_eq!(loaded.scrollback_bytes, 4096);
        assert_eq!(loaded.max_connections, 2);
        assert!(!loaded.mode_probe);
    }

    /// The whole reason for the sidecar: a key written by a newer alc must not
    /// stop an older one from reading the keys it does understand.
    #[test]
    fn an_unknown_key_in_the_file_is_ignored_rather_than_rejected() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            RemoteSettings::path(temp.path()),
            "port = 9000\nsome-key-from-the-future = \"whatever\"\n",
        )
        .unwrap();

        let settings = RemoteSettings::load(temp.path()).unwrap();
        assert_eq!(settings.port, 9000);
        // Absent keys still come from `Default`, not from zero.
        assert_eq!(settings.max_connections, 64);
    }

    #[test]
    fn a_malformed_file_is_an_error_naming_the_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = RemoteSettings::path(temp.path());
        fs::write(&path, "bind = yes please\n").unwrap();

        let error = RemoteSettings::load(temp.path()).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(&path.display().to_string()),
            "error should name the file: {message}"
        );
    }

    #[test]
    fn bind_round_trips_through_its_string_form() {
        for bind in Bind::ALL {
            assert_eq!(bind.as_str().parse::<Bind>().unwrap(), bind);
            assert_eq!(bind.to_string(), bind.as_str());
        }
        assert!("wan".parse::<Bind>().is_err());
    }

    #[test]
    fn load_or_create_twice_returns_the_same_three_tokens() {
        let _guard = lock();
        let temp = tempfile::tempdir().unwrap();

        let first = Secrets::load_or_create(temp.path()).unwrap();
        let second = Secrets::load_or_create(temp.path()).unwrap();

        assert_eq!(first.ctl, second.ctl);
        assert_eq!(first.operator, second.operator);
        assert_eq!(first.viewer, second.viewer);
    }

    #[test]
    fn the_three_tokens_differ_from_each_other() {
        let _guard = lock();
        let temp = tempfile::tempdir().unwrap();

        let secrets = Secrets::load_or_create(temp.path()).unwrap();
        assert_ne!(secrets.ctl, secrets.operator);
        assert_ne!(secrets.ctl, secrets.viewer);
        assert_ne!(secrets.operator, secrets.viewer);
    }

    /// 32 bytes encode to 43 characters plus one '=' of padding, which this
    /// encoder drops.
    #[test]
    fn a_token_is_forty_three_characters_of_base64url() {
        let token = generate_token().unwrap();
        assert_eq!(token.len(), 43, "{token}");
        assert!(is_base64url(&token), "{token}");
    }

    #[test]
    fn base64url_matches_the_rfc_4648_vectors_without_padding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        // The two characters that separate this alphabet from plain base64.
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[cfg(unix)]
    #[test]
    fn the_run_dir_is_0700_and_each_token_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = lock();
        let temp = tempfile::tempdir().unwrap();
        Secrets::load_or_create(temp.path()).unwrap();

        let dir = Secrets::run_dir(temp.path());
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{} should be owner-only", dir.display());
        for (_, file_name) in ROLES {
            let path = dir.join(file_name);
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} should be owner-only", path.display());
        }
    }

    #[test]
    fn an_environment_override_wins_over_the_token_file() {
        let _guard = lock();
        let temp = tempfile::tempdir().unwrap();
        let stored = Secrets::load_or_create(temp.path()).unwrap();

        // SAFETY: the ENV_LOCK guard keeps every other reader of this variable
        // out for the duration, and it is removed before the assertions.
        unsafe { env::set_var("ALC_REMOTE_OPERATOR", "from-the-environment") };
        let overridden = Secrets::load_or_create(temp.path());
        // SAFETY: same guarded variable, removed before anything can fail.
        unsafe { env::remove_var("ALC_REMOTE_OPERATOR") };

        let overridden = overridden.unwrap();
        assert_eq!(overridden.operator, "from-the-environment");
        assert_eq!(overridden.ctl, stored.ctl, "other roles keep their files");
    }

    #[test]
    fn rotate_replaces_all_three_tokens() {
        let _guard = lock();
        let temp = tempfile::tempdir().unwrap();

        let before = Secrets::load_or_create(temp.path()).unwrap();
        let after = Secrets::rotate(temp.path()).unwrap();

        assert_ne!(before.ctl, after.ctl);
        assert_ne!(before.operator, after.operator);
        assert_ne!(before.viewer, after.viewer);
        // And the new ones are what a fresh load reads back.
        let reloaded = Secrets::load_or_create(temp.path()).unwrap();
        assert_eq!(reloaded.ctl, after.ctl);
    }

    /// A hand-made token file ends in a newline that is not part of the
    /// secret, and an empty one has to be replaced rather than handed out.
    #[test]
    fn a_stored_token_is_trimmed_and_an_empty_file_is_replaced() {
        let _guard = lock();
        let temp = tempfile::tempdir().unwrap();
        let dir = ensure_run_dir(temp.path()).unwrap();
        fs::write(dir.join("ctl.token"), "hand-written\n").unwrap();
        fs::write(dir.join("viewer.token"), "   \n").unwrap();

        let secrets = Secrets::load_or_create(temp.path()).unwrap();
        assert_eq!(secrets.ctl, "hand-written");
        assert_eq!(secrets.viewer.len(), 43);
    }
}

#[cfg(test)]
mod race_tests {
    use super::*;

    /// Two `alc --share` invocations launched together is an ordinary thing
    /// to do. If each minted its own token and kept it, the one whose write
    /// lost would present a secret that is no longer on disk, and every
    /// request it made would be refused for a mismatch.
    #[test]
    fn processes_racing_to_create_the_tokens_converge_on_one_set() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_owned();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || Secrets::load_or_create(&dir).unwrap())
            })
            .collect();

        let all: Vec<Secrets> = handles
            .into_iter()
            .map(|handle| handle.join().expect("a loader panicked"))
            .collect();
        let first = &all[0];
        for secrets in &all[1..] {
            assert_eq!(secrets.ctl, first.ctl);
            assert_eq!(secrets.operator, first.operator);
            assert_eq!(secrets.viewer, first.viewer);
        }

        // And what is on disk is what they all hold.
        let reloaded = Secrets::load_or_create(&dir).unwrap();
        assert_eq!(reloaded.ctl, first.ctl);
    }
}
