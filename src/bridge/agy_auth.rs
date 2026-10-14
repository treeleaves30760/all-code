//! agy's Google login, borrowed.
//!
//! The Antigravity CLI keeps an OAuth access token where its keyring library
//! puts it: the Windows Credential Manager entry `gemini:antigravity` (on
//! macOS and Linux the same service and account in the system keyring), or,
//! where no keyring was usable, `~/.gemini/antigravity-cli/antigravity-oauth-token`.
//! alc reads that token and nothing else. It never writes the store, never
//! holds the refresh token (the serde struct below has no field for it) and
//! never runs an OAuth flow: a stale token is agy's to renew.
//!
//! Every error is a fixed code with a fixed sentence. None carries the bytes
//! that failed to parse, so a malformed store cannot put a token into an
//! error message, a log line or a failing assertion.

#![allow(dead_code, reason = "wired in by Plan C Tasks 3-12")]

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The Windows Credential Manager target agy's keyring library writes:
/// go-keyring's `service:user`, with service `gemini` and user `antigravity`.
pub(crate) const KEYRING_TARGET: &str = "gemini:antigravity";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYRING_SERVICE: &str = "gemini";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYRING_ACCOUNT: &str = "antigravity";
/// A login is a few hundred bytes; anything this large is not one.
pub(crate) const MAX_CREDENTIAL_BYTES: usize = 64 * 1024;
const BASE64_PREFIX: &str = "go-keyring-base64:";
/// A token this close to its expiry is renewed before use rather than sent:
/// a turn that starts on it can outlive it.
pub(crate) const FRESHNESS_MARGIN_SECS: i64 = 60;

/// Why agy's login could not be used. The sentence names the fix; the code
/// is what KohakuTerrarium calls the same condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgyAuthError {
    LoginRequired,
    AmbiguousSources,
    MalformedCredential,
    CredentialStoreUnavailable,
    AgyNotInstalled,
    RefreshFailed,
    RefreshBusy,
    RefreshRequired,
}

impl AgyAuthError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::LoginRequired => "login_required",
            Self::AmbiguousSources => "ambiguous_sources",
            Self::MalformedCredential => "malformed_credential",
            Self::CredentialStoreUnavailable => "credential_store_unavailable",
            Self::AgyNotInstalled => "agy_not_installed",
            Self::RefreshFailed => "agy_refresh_failed",
            Self::RefreshBusy => "refresh_busy",
            Self::RefreshRequired => "refresh_required",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::LoginRequired => {
                "not signed in to Antigravity (login_required); run `agy` and sign in"
            }
            Self::AmbiguousSources => {
                "agy's login is stored both in the system keyring and in \
                 ~/.gemini/antigravity-cli/antigravity-oauth-token (ambiguous sources); \
                 resolve it in agy: sign out, then sign in again"
            }
            Self::MalformedCredential => {
                "agy's stored login is not in a shape alc can read (malformed_credential); \
                 sign in again with `agy`"
            }
            Self::CredentialStoreUnavailable => {
                "the system credential store could not be read (credential_store_unavailable); \
                 make sure this desktop session is signed in, then retry"
            }
            Self::AgyNotInstalled => {
                "the Antigravity login has expired and agy is not on PATH to renew it \
                 (agy_not_installed); install agy, run it and sign in"
            }
            Self::RefreshFailed => {
                "agy could not renew the Antigravity login (agy_refresh_failed); run `agy` and sign in"
            }
            Self::RefreshBusy => {
                "another alc is renewing the Antigravity login (refresh_busy); try again in a moment"
            }
            Self::RefreshRequired => "refresh required; run `agy` and sign in",
        }
    }
}

impl fmt::Display for AgyAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for AgyAuthError {}

/// Where agy's fallback file lives under `home`.
pub(crate) fn default_credential_file(home: &Path) -> PathBuf {
    home.join(".gemini")
        .join("antigravity-cli")
        .join("antigravity-oauth-token")
}

/// Where a launch, a route or a report reads agy's login: the fixed keyring
/// entry plus the fallback file, resolved in the shell the user typed into -
/// the background bridge's own HOME may belong to another shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgyLocator {
    pub file: PathBuf,
}

impl AgyLocator {
    pub(crate) fn resolve() -> anyhow::Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(file) = std::env::var_os("ALC_AGY_CREDENTIAL_FILE").filter(|v| !v.is_empty()) {
            return Ok(Self {
                file: PathBuf::from(file),
            });
        }
        let home = crate::launch::home_dir().context(
            "could not resolve the home directory where agy keeps its login; set HOME \
             (USERPROFILE on Windows)",
        )?;
        Ok(Self {
            file: default_credential_file(&home),
        })
    }

    pub(crate) fn store(&self) -> SystemStore {
        SystemStore::new(self.file.clone())
    }
}

/// A store's bytes before parsing: a login, so `Debug` shows only how many
/// bytes there are. Formatting a store's result with `{:?}` cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RawCredential(Vec<u8>);

impl RawCredential {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for RawCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RawCredential(<{} bytes>)", self.0.len())
    }
}

/// The two places agy can have put its login. A trait so tests inject bytes
/// instead of touching a real keyring.
pub(crate) trait CredentialStore: Send + Sync {
    /// The keyring entry's bytes, `Ok(None)` when there is none.
    fn keyring(&self) -> Result<Option<RawCredential>, AgyAuthError>;
    /// The fallback file's bytes, `Ok(None)` when it does not exist.
    fn file(&self) -> Result<Option<RawCredential>, AgyAuthError>;
}

/// The real stores.
pub(crate) struct SystemStore {
    file: PathBuf,
    keyring: bool,
}

impl SystemStore {
    pub(crate) fn new(file: PathBuf) -> Self {
        Self {
            file,
            keyring: keyring_enabled(),
        }
    }

    /// The file alone: tests, and readers that may not open a keyring.
    pub(crate) fn file_only(file: PathBuf) -> Self {
        Self {
            file,
            keyring: false,
        }
    }
}

/// `ALC_AGY_NO_KEYRING` exists so `tests/cli.rs` can never read the login of
/// the machine running the suite. Compiled out of release builds.
fn keyring_enabled() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("ALC_AGY_NO_KEYRING").is_some_and(|value| !value.is_empty()) {
        return false;
    }
    true
}

impl CredentialStore for SystemStore {
    fn keyring(&self) -> Result<Option<RawCredential>, AgyAuthError> {
        if !self.keyring {
            return Ok(None);
        }
        read_keyring()
    }

    fn file(&self) -> Result<Option<RawCredential>, AgyAuthError> {
        use std::io::Read as _;
        let file = match std::fs::File::open(&self.file) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(AgyAuthError::CredentialStoreUnavailable),
        };
        let mut bytes = Vec::new();
        file.take(MAX_CREDENTIAL_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AgyAuthError::CredentialStoreUnavailable)?;
        if bytes.len() > MAX_CREDENTIAL_BYTES {
            return Err(AgyAuthError::MalformedCredential);
        }
        Ok(Some(RawCredential(bytes)))
    }
}

#[cfg(windows)]
fn read_keyring() -> Result<Option<RawCredential>, AgyAuthError> {
    read_windows_credential(KEYRING_TARGET)
}

/// `CredReadW`, read-only. `ERROR_NOT_FOUND` is "not logged in"; any other
/// failure is a store alc could not read, which is a different sentence.
#[cfg(windows)]
fn read_windows_credential(target: &str) -> Result<Option<RawCredential>, AgyAuthError> {
    use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };

    struct Freed(*mut CREDENTIALW);
    impl Drop for Freed {
        fn drop(&mut self) {
            // SAFETY: the pointer came from a successful CredReadW and is freed once.
            unsafe { CredFree(self.0.cast()) };
        }
    }

    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated and outlives the call; `credential` is
    // an out pointer CredReadW fills on success.
    let found = unsafe { CredReadW(wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
    if found == 0 {
        // SAFETY: plain thread-local read, immediately after the failing call.
        let error = unsafe { GetLastError() };
        return if error == ERROR_NOT_FOUND {
            Ok(None)
        } else {
            Err(AgyAuthError::CredentialStoreUnavailable)
        };
    }
    let freed = Freed(credential);
    // SAFETY: CredReadW succeeded, so `credential` points at a CREDENTIALW
    // whose blob stays valid until CredFree, which `freed` runs after the copy.
    let (size, blob) = unsafe {
        (
            (*freed.0).CredentialBlobSize as usize,
            (*freed.0).CredentialBlob,
        )
    };
    if size == 0 || blob.is_null() || size > MAX_CREDENTIAL_BYTES {
        return Err(AgyAuthError::MalformedCredential);
    }
    // SAFETY: `blob` is valid for `size` bytes (see above).
    let bytes = unsafe { std::slice::from_raw_parts(blob, size) }.to_vec();
    drop(freed);
    Ok(Some(RawCredential(bytes)))
}

#[cfg(target_os = "macos")]
fn read_keyring() -> Result<Option<RawCredential>, AgyAuthError> {
    keyring_tool(
        "/usr/bin/security",
        &[
            "find-generic-password",
            "-s",
            KEYRING_SERVICE,
            "-a",
            KEYRING_ACCOUNT,
            "-w",
        ],
    )
}

#[cfg(target_os = "linux")]
fn read_keyring() -> Result<Option<RawCredential>, AgyAuthError> {
    // The distribution's own copy first, so a `secret-tool` earlier on PATH
    // is not handed the lookup.
    const SYSTEM_SECRET_TOOL: &str = "/usr/bin/secret-tool";
    let program = if Path::new(SYSTEM_SECRET_TOOL).is_file() {
        SYSTEM_SECRET_TOOL
    } else {
        "secret-tool"
    };
    keyring_tool(
        program,
        &[
            "lookup",
            "service",
            KEYRING_SERVICE,
            "username",
            KEYRING_ACCOUNT,
        ],
    )
}

/// How long a keyring tool may take. A locked keyring can answer a lookup
/// with an unlock prompt (gnome-keyring) or an access dialog (Keychain) and
/// wait for a person who is not there; past this the lookup reads as absent.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYRING_TOOL_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn keyring_tool(program: &str, args: &[&str]) -> Result<Option<RawCredential>, AgyAuthError> {
    keyring_tool_within(program, args, KEYRING_TOOL_TIMEOUT)
}

/// Best effort, unverified (go-keyring's convention): a missing tool, a
/// failing one, a locked keyring, no item, or no answer within `timeout` all
/// read as "no keyring entry", and the file decides. Stdout is read on a
/// thread, never past [`MAX_CREDENTIAL_BYTES`] + 1; a tool still running at
/// the deadline is killed and its reader left to finish on its own.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn keyring_tool_within(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Option<RawCredential>, AgyAuthError> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    let deadline = Instant::now() + timeout;
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ok(None);
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout
            .take(MAX_CREDENTIAL_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(read);
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
        }
    };
    // The tool has exited, so its answer is already in the pipe. A floor keeps
    // a tool that exits right at the deadline from losing an answer the reader
    // has not handed over yet.
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(100));
    let Ok(Ok(mut bytes)) = receiver.recv_timeout(remaining) else {
        return Ok(None);
    };
    // Before the exit status: a tool cut off mid-answer fails on the closed
    // pipe, and what it was printing was still too large to be a login.
    if bytes.len() > MAX_CREDENTIAL_BYTES {
        return Err(AgyAuthError::MalformedCredential);
    }
    if !status.success() {
        return Ok(None);
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(RawCredential(bytes)))
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn read_keyring() -> Result<Option<RawCredential>, AgyAuthError> {
    Ok(None)
}

/// agy's stored login. No `refresh_token`: alc never holds agy's refresh
/// token, not even parsed into memory it owns.
#[derive(Deserialize)]
struct StoredLogin {
    auth_method: String,
    token: StoredToken,
}

#[derive(Deserialize)]
struct StoredToken {
    access_token: String,
    token_type: String,
    expiry: String,
}

/// An access token and when it stops working. Never `Clone`d into a log:
/// `Debug` redacts it.
pub(crate) struct AgyToken {
    access_token: String,
    expires_at: i64,
}

impl AgyToken {
    pub(crate) fn access_token(&self) -> &str {
        &self.access_token
    }

    pub(crate) fn expires_at(&self) -> i64 {
        self.expires_at
    }

    pub(crate) fn is_fresh(&self, now: i64) -> bool {
        self.expires_at > now + FRESHNESS_MARGIN_SECS
    }

    /// Which login this is, without holding the token: a change of
    /// fingerprint is a change of account, or a renewal.
    pub(crate) fn fingerprint(&self) -> Fingerprint {
        let digest = Sha256::digest(self.access_token.as_bytes());
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&digest);
        Fingerprint(bytes)
    }
}

impl fmt::Debug for AgyToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgyToken")
            .field("access_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// SHA-256 of an access token.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// What the usage ledger records as the account: twelve hex digits, enough
    /// to tell logins apart and useless for anything else.
    pub(crate) fn account_id(&self) -> String {
        self.0
            .iter()
            .take(6)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Fingerprint({})", self.account_id())
    }
}

pub(crate) fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Reads one stored login. The checks are KohakuTerrarium's: a consumer
/// login, a bearer token in the OAuth charset, an expiry with an offset.
pub(crate) fn parse_credential(raw: &[u8]) -> Result<AgyToken, AgyAuthError> {
    use AgyAuthError::MalformedCredential as Malformed;
    if raw.len() > MAX_CREDENTIAL_BYTES {
        return Err(Malformed);
    }
    let text = std::str::from_utf8(raw).map_err(|_| Malformed)?.trim();
    let decoded: Vec<u8>;
    let json = match text.strip_prefix(BASE64_PREFIX) {
        Some(encoded) => {
            decoded = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|_| Malformed)?;
            std::str::from_utf8(&decoded).map_err(|_| Malformed)?
        }
        None => text,
    };
    let login: StoredLogin = serde_json::from_str(json).map_err(|_| Malformed)?;
    if login.auth_method != "consumer" || !login.token.token_type.eq_ignore_ascii_case("bearer") {
        return Err(Malformed);
    }
    let access_token = login.token.access_token;
    let charset = |byte: u8| byte.is_ascii_alphanumeric() || b"._~+/=-".contains(&byte);
    if access_token.is_empty() || !access_token.bytes().all(charset) {
        return Err(Malformed);
    }
    let expires_at = chrono::DateTime::parse_from_rfc3339(login.token.expiry.trim())
        .map_err(|_| Malformed)?
        .timestamp();
    Ok(AgyToken {
        access_token,
        expires_at,
    })
}

/// The login as stored now. Both sources at once is refused even when they
/// agree: which one agy updates next is not alc's to guess.
pub(crate) fn load(store: &dyn CredentialStore) -> Result<AgyToken, AgyAuthError> {
    let keyring = store.keyring()?;
    let file = store.file()?;
    match (keyring, file) {
        (Some(_), Some(_)) => Err(AgyAuthError::AmbiguousSources),
        (Some(raw), None) | (None, Some(raw)) => parse_credential(raw.as_bytes()),
        (None, None) => Err(AgyAuthError::LoginRequired),
    }
}

/// The login's state for a report: offline, and never the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginState {
    Ready { expires_at: i64 },
    Expired,
    Missing(AgyAuthError),
}

pub(crate) fn status(store: &dyn CredentialStore, now: i64) -> LoginState {
    match load(store) {
        Ok(token) if token.is_fresh(now) => LoginState::Ready {
            expires_at: token.expires_at(),
        },
        Ok(_) => LoginState::Expired,
        Err(error) => LoginState::Missing(error),
    }
}

/// How long agy gets to renew its login before alc kills it.
pub(crate) const REFRESH_TIMEOUT: Duration = Duration::from_secs(25);
/// How long a renewal waits for another process's.
pub(crate) const LOCK_WAIT: Duration = Duration::from_secs(30);

/// One lock file for every alc on the machine, so two alc processes (a
/// background bridge and an in-process one, say) never run agy at the same
/// time. Always the legacy run directory, whatever this process's runtime
/// scope: agy's login is one per user, so alc processes on different
/// generations renew the same login and must take the same lock.
pub(crate) fn refresh_lock_path(config_dir: &Path) -> PathBuf {
    crate::bridge_host::files::run_dir_for(config_dir, &crate::runtime::RuntimeScope::Legacy)
        .join("agy-refresh.lock")
}

/// agy's login as a bridge uses it: read fresh from the store on every call,
/// renewed by agy when stale or refused.
pub(crate) struct AgyCredentials {
    store: Arc<dyn CredentialStore>,
    lock_file: PathBuf,
    agy: Option<PathBuf>,
    refresh_timeout: Duration,
    lock_wait: Duration,
    /// One renewal in flight per process. Waiters re-read the store once they
    /// get it, so they take the leader's token instead of running agy again.
    flight: tokio::sync::Mutex<()>,
}

impl AgyCredentials {
    /// agy is resolved here, once (`ALC_AGY_BIN`, else `agy` on PATH): an agy
    /// installed after the bridge started is used only once it restarts.
    pub(crate) fn new(locator: &AgyLocator, config_dir: &Path) -> Self {
        Self::with_parts(
            Arc::new(locator.store()),
            refresh_lock_path(config_dir),
            crate::doctor::resolve(&crate::doctor::binary_for(crate::config::Agent::Agy)),
        )
    }

    pub(crate) fn with_parts(
        store: Arc<dyn CredentialStore>,
        lock_file: PathBuf,
        agy: Option<PathBuf>,
    ) -> Self {
        Self {
            store,
            lock_file,
            agy,
            refresh_timeout: REFRESH_TIMEOUT,
            lock_wait: LOCK_WAIT,
            flight: tokio::sync::Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_timeouts(mut self, refresh: Duration, lock: Duration) -> Self {
        self.refresh_timeout = refresh;
        self.lock_wait = lock;
        self
    }

    /// The login as stored now. Never renews; there is no cache to be stale.
    /// Blocking: on macOS and Linux it can wait on the keyring tool, so async
    /// code reads through [`Self::read`] instead.
    pub(crate) fn current(&self) -> Result<AgyToken, AgyAuthError> {
        load(self.store.as_ref())
    }

    /// A token to sign the next request with, renewed first when stale.
    pub(crate) async fn token(&self) -> Result<AgyToken, AgyAuthError> {
        let token = self.read().await?;
        if token.is_fresh(now_secs()) {
            return Ok(token);
        }
        self.renew(None).await
    }

    /// Renews after upstream refused `rejected` with a 401: the result must be
    /// fresh *and* a different token, or the same refusal would follow.
    pub(crate) async fn renew_after_rejection(
        &self,
        rejected: Fingerprint,
    ) -> Result<AgyToken, AgyAuthError> {
        self.renew(Some(rejected)).await
    }

    async fn renew(&self, rejected: Option<Fingerprint>) -> Result<AgyToken, AgyAuthError> {
        let _flight = self.flight.lock().await;
        if let Some(token) = self.usable(rejected).await? {
            return Ok(token);
        }
        // Before the lock: with no agy, waiting out another process's renewal
        // could only end in `refresh_busy` instead of the actionable answer.
        let agy = self.agy.clone().ok_or(AgyAuthError::AgyNotInstalled)?;
        let lock_file = self.lock_file.clone();
        let wait = self.lock_wait;
        let lock = tokio::task::spawn_blocking(move || {
            crate::file_lock::FileLock::acquire_timeout(&lock_file, wait)
        })
        .await
        .map_err(|_| AgyAuthError::RefreshFailed)?
        .map_err(|error| {
            // Busy only when someone else holds the lock; a lock that cannot
            // be taken at all will not be fixed by trying again.
            if error.is::<crate::file_lock::LockTimeout>() {
                AgyAuthError::RefreshBusy
            } else {
                AgyAuthError::RefreshFailed
            }
        })?;
        // Another process may have renewed while this one waited for the lock.
        if let Some(token) = self.usable(rejected).await? {
            return Ok(token);
        }
        let timeout = self.refresh_timeout;
        // The lock travels with agy, not with this future: a caller that
        // gives up mid-renewal must not free the lock while agy still runs.
        tokio::task::spawn_blocking(move || {
            let _lock = lock;
            run_agy(&agy, timeout)
        })
        .await
        .map_err(|_| AgyAuthError::RefreshFailed)??;
        self.usable(rejected)
            .await?
            .ok_or(AgyAuthError::RefreshRequired)
    }

    /// The stored token when it is fresh and not the one just refused. A
    /// missing or unreadable login is an error here, which is what keeps agy
    /// from ever being run for a user who is not signed in.
    async fn usable(
        &self,
        rejected: Option<Fingerprint>,
    ) -> Result<Option<AgyToken>, AgyAuthError> {
        let token = self.read().await?;
        let refused = rejected == Some(token.fingerprint());
        Ok((token.is_fresh(now_secs()) && !refused).then_some(token))
    }

    /// [`Self::current`] on the blocking pool, so a keyring tool that waits
    /// for an unlock prompt holds a blocking thread and not an async worker.
    /// Never renews, which is what the transport's account-switch guard
    /// needs: it asks who is signed in now, not for a usable token.
    pub(crate) async fn read(&self) -> Result<AgyToken, AgyAuthError> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || load(store.as_ref()))
            .await
            .map_err(|_| AgyAuthError::CredentialStoreUnavailable)?
    }
}

/// `agy --output-format json models`: a command that makes agy refresh its
/// own login before answering. No stdin, no output kept, no auto-update, no
/// console window, and killed at `timeout`.
fn run_agy(binary: &Path, timeout: Duration) -> Result<(), AgyAuthError> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(binary);
    command
        .args(["--output-format", "json", "models"])
        .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|_| AgyAuthError::RefreshFailed)?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(AgyAuthError::RefreshFailed),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            // Past the deadline, or a child that can no longer be waited on:
            // either way it is not left running.
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgyAuthError::RefreshFailed);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::{AgyAuthError, CredentialStore, RawCredential};

    type Slot = Result<Option<RawCredential>, AgyAuthError>;

    /// Credential bytes held in memory, settable mid-test.
    pub(crate) struct MemoryStore {
        keyring: Mutex<Slot>,
        file: Mutex<Slot>,
    }

    impl MemoryStore {
        pub(crate) fn empty() -> Self {
            Self {
                keyring: Mutex::new(Ok(None)),
                file: Mutex::new(Ok(None)),
            }
        }

        pub(crate) fn with_file(bytes: Vec<u8>) -> Self {
            let store = Self::empty();
            store.set_file(bytes);
            store
        }

        pub(crate) fn with_keyring(bytes: Vec<u8>) -> Self {
            let store = Self::empty();
            *store.keyring.lock().unwrap() = Ok(Some(RawCredential::new(bytes)));
            store
        }

        pub(crate) fn with_both(keyring: Vec<u8>, file: Vec<u8>) -> Self {
            let store = Self::with_keyring(keyring);
            store.set_file(file);
            store
        }

        pub(crate) fn failing_keyring(file: Vec<u8>) -> Self {
            let store = Self::with_file(file);
            *store.keyring.lock().unwrap() = Err(AgyAuthError::CredentialStoreUnavailable);
            store
        }

        pub(crate) fn set_file(&self, bytes: Vec<u8>) {
            *self.file.lock().unwrap() = Ok(Some(RawCredential::new(bytes)));
        }
    }

    impl CredentialStore for MemoryStore {
        fn keyring(&self) -> Slot {
            self.keyring.lock().unwrap().clone()
        }

        fn file(&self) -> Slot {
            self.file.lock().unwrap().clone()
        }
    }

    /// A synthetic login in agy's shape, expiring `expires_in_secs` from now.
    /// Carries a refresh token so tests can prove it is never held.
    pub(crate) fn credential(token: &str, expires_in_secs: i64) -> Vec<u8> {
        let expiry = chrono::DateTime::from_timestamp(super::now_secs() + expires_in_secs, 0)
            .expect("a representable time")
            .to_rfc3339();
        serde_json::json!({
            "auth_method": "consumer",
            "token": {
                "access_token": token,
                "token_type": "Bearer",
                "refresh_token": "never-read-refresh-token",
                "expiry": expiry,
            }
        })
        .to_string()
        .into_bytes()
    }

    /// A stand-in agy: counts its runs, records its arguments and the
    /// auto-update switch, sleeps, then (when given) installs `renewed` as the
    /// stored login.
    ///
    /// It never leaves a process behind. The sleep is the script's own
    /// process, so the kill at a refresh timeout reaches it: on Windows a
    /// `cmd` loop on `%TIME%` (a `ping` grandchild would outlive the kill and
    /// hold the test runner's output open), on Unix `exec sleep` when nothing
    /// follows it.
    pub(crate) fn fake_agy(
        dir: &Path,
        renewed: Option<&[u8]>,
        target: &Path,
        sleep_secs: u32,
    ) -> PathBuf {
        if let Some(renewed) = renewed {
            std::fs::write(dir.join("renewed.json"), renewed).unwrap();
        }
        let d = dir.display();
        let t = target.display();
        #[cfg(windows)]
        {
            let mut script = format!(
                "@echo off\r\n>>\"{d}\\calls\" echo called\r\n>\"{d}\\args\" echo %*\r\n\
                 >\"{d}\\env\" echo %AGY_CLI_DISABLE_AUTO_UPDATE%\r\n"
            );
            if sleep_secs > 0 {
                // Centiseconds since midnight, wrapped at midnight.
                let centis = sleep_secs * 100;
                script.push_str(&format!(
                    "call :now start\r\n:wait\r\ncall :now now\r\n\
                     set /a \"elapsed=(now-start+8640000)%%8640000\"\r\n\
                     if %elapsed% lss {centis} goto wait\r\n"
                ));
            }
            if renewed.is_some() {
                script.push_str(&format!("copy /Y \"{d}\\renewed.json\" \"{t}\" >NUL\r\n"));
            }
            script.push_str(
                "exit /b 0\r\n:now\r\nset \"clock=%TIME: =0%\"\r\n\
                 set /a \"%1=((1%clock:~0,2%-100)*3600+(1%clock:~3,2%-100)*60\
                 +(1%clock:~6,2%-100))*100+(1%clock:~9,2%-100)\"\r\nexit /b 0\r\n",
            );
            let path = dir.join("agy.cmd");
            std::fs::write(&path, script).unwrap();
            path
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut script = format!(
                "#!/bin/sh\necho called >> '{d}/calls'\necho \"$@\" > '{d}/args'\n\
                 echo \"$AGY_CLI_DISABLE_AUTO_UPDATE\" > '{d}/env'\n"
            );
            if sleep_secs > 0 {
                let exec = if renewed.is_some() { "" } else { "exec " };
                script.push_str(&format!("{exec}sleep {sleep_secs}\n"));
            }
            if renewed.is_some() {
                script.push_str(&format!("cp '{d}/renewed.json' '{t}'\n"));
            }
            // Written here, installed by `cp`: a file this multi-threaded
            // process held open for writing can be ETXTBSY to exec while
            // another test's fork still carries the descriptor.
            let source = dir.join("agy.sh");
            std::fs::write(&source, script).unwrap();
            let path = dir.join("agy");
            let copied = std::process::Command::new("cp")
                .arg(&source)
                .arg(&path)
                .status()
                .unwrap();
            assert!(copied.success(), "cp {copied}");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    /// How many times the fake agy in `dir` ran.
    pub(crate) fn agy_calls(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("calls")).map_or(0, |text| text.lines().count())
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::test_support::{MemoryStore, agy_calls, credential, fake_agy};
    use super::*;

    #[test]
    fn a_plain_credential_parses_and_never_shows_its_token() {
        let token = parse_credential(&credential("tok-a", 3600)).unwrap();
        assert_eq!(token.access_token(), "tok-a");
        assert!(token.is_fresh(now_secs()));
        let shown = format!("{token:?}");
        assert!(!shown.contains("tok-a"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
    }

    #[test]
    fn the_go_keyring_base64_prefix_is_decoded() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(credential("tok-b", 3600));
        let token = parse_credential(format!("go-keyring-base64:{encoded}").as_bytes()).unwrap();
        assert_eq!(token.access_token(), "tok-b");
    }

    #[test]
    fn a_refresh_token_is_parsed_past_and_never_held() {
        // `credential` writes a refresh_token; nothing in AgyToken can hold it.
        let raw = credential("tok-a", 3600);
        assert!(String::from_utf8_lossy(&raw).contains("never-read-refresh-token"));
        let token = parse_credential(&raw).unwrap();
        assert!(!format!("{token:?}").contains("never-read-refresh-token"));
    }

    #[test]
    fn validation_refuses_by_code_without_echoing_the_input() {
        let shape = |auth_method: &str, token_type: &str, token: &str, expiry: &str| {
            serde_json::json!({"auth_method": auth_method, "token": {
                "access_token": token, "token_type": token_type, "expiry": expiry}})
            .to_string()
            .into_bytes()
        };
        let cases: Vec<Vec<u8>> = vec![
            shape("oauth", "Bearer", "secret-one", "2099-01-01T00:00:00Z"),
            shape("consumer", "MAC", "secret-two", "2099-01-01T00:00:00Z"),
            shape("consumer", "Bearer", "secret three", "2099-01-01T00:00:00Z"),
            shape("consumer", "Bearer", "", "2099-01-01T00:00:00Z"),
            shape("consumer", "Bearer", "secret-four", "2099-01-01T00:00:00"),
            b"secret-five is not json".to_vec(),
            b"go-keyring-base64:!!!secret-six".to_vec(),
            vec![b' '; MAX_CREDENTIAL_BYTES + 1],
            vec![0xff, 0xfe, b's'],
        ];
        for raw in cases {
            let error = parse_credential(&raw).unwrap_err();
            assert_eq!(error, AgyAuthError::MalformedCredential);
            assert_eq!(error.code(), "malformed_credential");
            assert!(!error.to_string().contains("secret"), "{error}");
        }
    }

    #[test]
    fn freshness_has_a_sixty_second_margin() {
        let now = now_secs();
        let at = |seconds: i64| parse_credential(&credential("tok-a", seconds)).unwrap();
        assert!(at(120).is_fresh(now));
        assert!(!at(30).is_fresh(now));
        assert!(!at(-10).is_fresh(now));
    }

    #[test]
    fn the_fingerprint_is_sha256_and_the_account_id_its_first_twelve_hex_digits() {
        let token = parse_credential(&credential("abc", 3600)).unwrap();
        // sha256("abc") = ba7816bf8f01cfea…
        assert_eq!(token.fingerprint().account_id(), "ba7816bf8f01");
        assert_eq!(
            format!("{:?}", token.fingerprint()),
            "Fingerprint(ba7816bf8f01)"
        );
    }

    #[test]
    fn both_sources_present_is_ambiguous_even_when_identical() {
        let raw = credential("tok-a", 3600);
        let store = MemoryStore::with_both(raw.clone(), raw);
        assert_eq!(load(&store).unwrap_err(), AgyAuthError::AmbiguousSources);
        let message = AgyAuthError::AmbiguousSources.message();
        assert!(message.contains("ambiguous sources"), "{message}");
        assert!(message.contains("resolve it in agy"), "{message}");
        assert_eq!(
            status(&store, now_secs()),
            LoginState::Missing(AgyAuthError::AmbiguousSources)
        );
    }

    #[test]
    fn either_source_alone_loads_and_neither_is_login_required() {
        let raw = credential("tok-a", 3600);
        assert_eq!(
            load(&MemoryStore::with_keyring(raw.clone()))
                .unwrap()
                .access_token(),
            "tok-a"
        );
        assert_eq!(
            load(&MemoryStore::with_file(raw)).unwrap().access_token(),
            "tok-a"
        );
        let error = load(&MemoryStore::empty()).unwrap_err();
        assert_eq!(error, AgyAuthError::LoginRequired);
        assert!(error.message().contains("run `agy` and sign in"), "{error}");
    }

    #[test]
    fn a_store_failure_is_credential_store_unavailable() {
        let store = MemoryStore::failing_keyring(credential("tok-a", 3600));
        assert_eq!(
            load(&store).unwrap_err(),
            AgyAuthError::CredentialStoreUnavailable
        );
    }

    #[test]
    fn the_fallback_file_is_read_and_capped_at_64_kib() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("antigravity-oauth-token");
        let store = SystemStore::file_only(path.clone());
        assert_eq!(store.file().unwrap(), None);
        std::fs::write(&path, credential("tok-a", 3600)).unwrap();
        assert_eq!(load(&store).unwrap().access_token(), "tok-a");
        std::fs::write(&path, vec![b'x'; MAX_CREDENTIAL_BYTES + 1]).unwrap();
        assert_eq!(store.file().unwrap_err(), AgyAuthError::MalformedCredential);
    }

    #[test]
    fn raw_store_bytes_show_only_their_length() {
        let raw = RawCredential::new(credential("tok-a", 3600));
        let shown = format!("{:?}", Some(raw.clone()));
        assert!(!shown.contains("tok-a"), "{shown}");
        assert!(
            shown.contains(&format!("<{} bytes>", raw.as_bytes().len())),
            "{shown}"
        );
        let store = MemoryStore::with_both(credential("tok-k", 3600), credential("tok-f", 3600));
        let shown = format!("{:?} {:?}", store.keyring(), store.file());
        assert!(!shown.contains("tok-"), "{shown}");
    }

    /// A stand-in for `security` / `secret-tool`. Unix runs the script through
    /// `/bin/sh` rather than executing it, so a freshly written file cannot hit
    /// ETXTBSY; Windows runs a `.cmd`.
    fn fake_tool(dir: &Path, unix: &str, windows: &str) -> (String, Vec<String>) {
        if cfg!(windows) {
            let script = dir.join("keyring-tool.cmd");
            std::fs::write(&script, format!("@echo off\r\n{windows}\r\n")).unwrap();
            (script.display().to_string(), Vec::new())
        } else {
            let script = dir.join("keyring-tool.sh");
            std::fs::write(&script, format!("{unix}\n")).unwrap();
            ("/bin/sh".to_owned(), vec![script.display().to_string()])
        }
    }

    fn run_fake(
        tool: &(String, Vec<String>),
        timeout: Duration,
    ) -> Result<Option<RawCredential>, AgyAuthError> {
        let args: Vec<&str> = tool.1.iter().map(String::as_str).collect();
        keyring_tool_within(&tool.0, &args, timeout)
    }

    /// A tool that prints the file at `answer`.
    fn printing_tool(dir: &Path, answer: &Path) -> (String, Vec<String>) {
        let answer = answer.display();
        fake_tool(
            dir,
            &format!("cat '{answer}'"),
            &format!("type \"{answer}\""),
        )
    }

    #[test]
    fn a_keyring_tool_answer_is_read_and_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let answer = dir.path().join("answer");
        let raw = credential("tok-a", 3600);
        let mut printed = raw.clone();
        printed.extend_from_slice(b"\r\n");
        std::fs::write(&answer, printed).unwrap();
        let tool = printing_tool(dir.path(), &answer);
        let read = run_fake(&tool, Duration::from_secs(20)).unwrap().unwrap();
        assert_eq!(read.as_bytes(), raw.as_slice());
    }

    #[test]
    fn an_oversized_keyring_answer_is_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let answer = dir.path().join("answer");
        std::fs::write(&answer, vec![b'x'; MAX_CREDENTIAL_BYTES + 100]).unwrap();
        let tool = printing_tool(dir.path(), &answer);
        assert_eq!(
            run_fake(&tool, Duration::from_secs(20)).unwrap_err(),
            AgyAuthError::MalformedCredential
        );
    }

    #[test]
    fn a_failing_silent_or_missing_keyring_tool_reads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let failing = fake_tool(
            dir.path(),
            "echo token; exit 44",
            "echo token\r\nexit /b 44",
        );
        assert_eq!(run_fake(&failing, Duration::from_secs(20)).unwrap(), None);
        let silent = fake_tool(dir.path(), "exit 0", "exit /b 0");
        assert_eq!(run_fake(&silent, Duration::from_secs(20)).unwrap(), None);
        let missing = (
            "alc-test-no-such-keyring-tool-7f3c9e1d".to_owned(),
            Vec::new(),
        );
        assert_eq!(run_fake(&missing, Duration::from_secs(20)).unwrap(), None);
    }

    /// A keyring that shows an unlock prompt nobody answers must not hold a
    /// request forever: past the deadline the tool is killed and the keyring
    /// reads as absent.
    ///
    /// The sleeper is the direct child on both systems, so the kill reaches
    /// it: a `.cmd` around `ping` would leave `ping` holding the test
    /// runner's output for ten seconds after the test ended.
    #[test]
    fn a_keyring_tool_that_hangs_reads_as_absent_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let hanging = if cfg!(windows) {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let ping = Path::new(&root).join("System32").join("PING.EXE");
            (
                ping.display().to_string(),
                vec!["-n".to_owned(), "11".to_owned(), "127.0.0.1".to_owned()],
            )
        } else {
            fake_tool(dir.path(), "exec sleep 10", "")
        };
        let started = Instant::now();
        assert_eq!(
            run_fake(&hanging, Duration::from_millis(500)).unwrap(),
            None
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn status_reports_ready_expired_and_missing() {
        let now = now_secs();
        assert!(matches!(
            status(&MemoryStore::with_file(credential("tok-a", 3600)), now),
            LoginState::Ready { .. }
        ));
        assert_eq!(
            status(&MemoryStore::with_file(credential("tok-a", -5)), now),
            LoginState::Expired
        );
        assert_eq!(
            status(&MemoryStore::empty(), now),
            LoginState::Missing(AgyAuthError::LoginRequired)
        );
    }

    #[test]
    fn the_default_file_is_under_dot_gemini() {
        let file = default_credential_file(Path::new("/home/ada"));
        assert!(
            file.ends_with(".gemini/antigravity-cli/antigravity-oauth-token"),
            "{}",
            file.display()
        );
    }

    /// Exercises the real CredReadW path without reading anybody's login: the
    /// target cannot exist.
    #[cfg(windows)]
    #[test]
    fn a_missing_windows_credential_reads_as_absent() {
        assert_eq!(
            read_windows_credential("alc-test:no-such-target-7f3c9e1d").unwrap(),
            None
        );
    }

    /// A file-backed login and a fake agy that replaces it with `renewed`.
    fn renewing(
        dir: &Path,
        stored: Vec<u8>,
        renewed: Option<Vec<u8>>,
        sleep_secs: u32,
    ) -> AgyCredentials {
        let file = dir.join("login");
        std::fs::write(&file, stored).unwrap();
        let agy = fake_agy(dir, renewed.as_deref(), &file, sleep_secs);
        AgyCredentials::with_parts(
            Arc::new(SystemStore::file_only(file)),
            refresh_lock_path(dir),
            Some(agy),
        )
    }

    #[test]
    fn the_refresh_lock_is_the_legacy_run_dir_lock() {
        let dir = Path::new("/config/alc");
        assert_eq!(
            refresh_lock_path(dir),
            dir.join("run").join("agy-refresh.lock")
        );
    }

    /// Run in a child by the test below: a runtime scope is set once per
    /// process, so a generation scope cannot be switched on in this one
    /// without changing every other test's run directory.
    #[test]
    fn refresh_lock_scope_worker() {
        let Some(dir) = std::env::var_os("ALC_TEST_AGY_LOCK_SCOPE_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let generation = crate::runtime::RuntimeScope::parse("0123456789ab").unwrap();
        crate::runtime::initialize(generation.clone(), true).unwrap();
        // The scope is active: the bridge's own run directory moved...
        assert_eq!(
            crate::bridge_host::files::run_dir(&dir),
            crate::runtime::run_dir_for(&dir, &generation)
        );
        // ...and agy's lock did not, because agy's login is one per user.
        assert_eq!(
            refresh_lock_path(&dir),
            dir.join("run").join("agy-refresh.lock")
        );
        std::fs::write(dir.join("worker-ran"), b"ok").unwrap();
    }

    #[test]
    fn the_refresh_lock_ignores_a_generation_scope() {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "bridge::agy_auth::tests::refresh_lock_scope_worker",
                "--nocapture",
            ])
            .env("ALC_TEST_AGY_LOCK_SCOPE_DIR", dir.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{status}");
        // A worker filtered out by a renamed test would also exit 0.
        assert!(dir.path().join("worker-ran").is_file());
    }

    #[test]
    fn the_renewal_errors_have_fixed_codes_and_name_the_fix() {
        let cases = [
            (AgyAuthError::AgyNotInstalled, "agy_not_installed"),
            (AgyAuthError::RefreshFailed, "agy_refresh_failed"),
            (AgyAuthError::RefreshBusy, "refresh_busy"),
        ];
        for (error, code) in cases {
            assert_eq!(error.code(), code);
            assert!(error.message().contains(&format!("({code})")), "{error}");
        }
        assert_eq!(AgyAuthError::RefreshRequired.code(), "refresh_required");
        let failed = AgyAuthError::RefreshFailed.message();
        assert!(failed.contains("run `agy` and sign in"), "{failed}");
        let busy = AgyAuthError::RefreshBusy.message();
        assert!(busy.contains("try again"), "{busy}");
    }

    #[tokio::test]
    async fn a_fresh_token_is_returned_without_running_agy() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = renewing(dir.path(), credential("tok-a", 3600), None, 0);
        assert_eq!(credentials.token().await.unwrap().access_token(), "tok-a");
        assert_eq!(agy_calls(dir.path()), 0);
    }

    #[tokio::test]
    async fn a_stale_token_is_renewed_by_agy_with_auto_update_off() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = renewing(
            dir.path(),
            credential("tok-a", 10),
            Some(credential("tok-b", 3600)),
            0,
        );
        assert_eq!(credentials.token().await.unwrap().access_token(), "tok-b");
        assert_eq!(agy_calls(dir.path()), 1);
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert_eq!(args.trim(), "--output-format json models");
        let env = std::fs::read_to_string(dir.path().join("env")).unwrap();
        assert_eq!(env.trim(), "true");
    }

    #[tokio::test]
    async fn no_login_never_runs_agy() {
        let dir = tempfile::tempdir().unwrap();
        let agy = fake_agy(dir.path(), None, &dir.path().join("login"), 0);
        let credentials = AgyCredentials::with_parts(
            Arc::new(SystemStore::file_only(dir.path().join("login"))),
            dir.path().join("agy-refresh.lock"),
            Some(agy),
        );
        assert_eq!(
            credentials.token().await.unwrap_err(),
            AgyAuthError::LoginRequired
        );
        assert_eq!(
            credentials
                .renew_after_rejection(
                    parse_credential(&credential("tok-a", 3600))
                        .unwrap()
                        .fingerprint()
                )
                .await
                .unwrap_err(),
            AgyAuthError::LoginRequired
        );
        assert_eq!(agy_calls(dir.path()), 0);
    }

    #[tokio::test]
    async fn an_expired_login_with_no_agy_asks_to_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = AgyCredentials::with_parts(
            Arc::new(MemoryStore::with_file(credential("tok-a", -100))),
            dir.path().join("agy-refresh.lock"),
            None,
        );
        let error = credentials.token().await.unwrap_err();
        assert_eq!(error, AgyAuthError::AgyNotInstalled);
        assert_eq!(error.code(), "agy_not_installed");
        assert!(error.message().contains("sign in"), "{error}");
        // Decided before the lock: nothing was created to take it.
        assert!(!dir.path().join("agy-refresh.lock").exists());
    }

    /// No agy is the actionable answer even while another process renews:
    /// waiting out its lock would only end in `refresh_busy`.
    #[tokio::test]
    async fn a_missing_agy_is_reported_without_waiting_for_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = refresh_lock_path(dir.path());
        let _held = crate::file_lock::FileLock::acquire(&lock).unwrap();
        let credentials = AgyCredentials::with_parts(
            Arc::new(MemoryStore::with_file(credential("tok-a", -100))),
            lock,
            None,
        );
        let started = Instant::now();
        assert_eq!(
            credentials.token().await.unwrap_err(),
            AgyAuthError::AgyNotInstalled
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    /// Only a lock someone else holds is `refresh_busy`; a lock that cannot
    /// be taken at all is a failed renewal, not a reason to try again.
    #[tokio::test]
    async fn a_lock_that_cannot_be_created_is_refresh_failed_not_busy() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = renewing(dir.path(), credential("tok-a", 10), None, 0);
        // A lock under a regular file: its directory can never be created.
        let credentials = AgyCredentials {
            lock_file: dir.path().join("login").join("agy-refresh.lock"),
            ..credentials
        };
        assert_eq!(
            credentials.token().await.unwrap_err(),
            AgyAuthError::RefreshFailed
        );
        assert_eq!(agy_calls(dir.path()), 0);
    }

    #[tokio::test]
    async fn after_a_401_the_same_token_is_not_accepted_back() {
        let dir = tempfile::tempdir().unwrap();
        // agy runs but leaves the login as it was.
        let credentials = renewing(dir.path(), credential("tok-a", 3600), None, 0);
        let rejected = credentials.current().unwrap().fingerprint();
        let error = credentials
            .renew_after_rejection(rejected)
            .await
            .unwrap_err();
        assert_eq!(error, AgyAuthError::RefreshRequired);
        assert_eq!(error.message(), "refresh required; run `agy` and sign in");
        assert_eq!(agy_calls(dir.path()), 1);
    }

    #[tokio::test]
    async fn a_401_after_another_process_renewed_takes_the_new_token_without_agy() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = renewing(dir.path(), credential("tok-b", 3600), None, 0);
        let rejected = parse_credential(&credential("tok-a", 3600))
            .unwrap()
            .fingerprint();
        let token = credentials.renew_after_rejection(rejected).await.unwrap();
        assert_eq!(token.access_token(), "tok-b");
        assert_eq!(agy_calls(dir.path()), 0);
    }

    #[tokio::test]
    async fn an_agy_that_fails_is_refresh_failed() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = renewing(dir.path(), credential("tok-a", 10), None, 0);
        // A missing program is the same failure as a failing one.
        let credentials = AgyCredentials {
            agy: Some(dir.path().join("alc-test-no-such-agy-7f3c9e1d.exe")),
            ..credentials
        };
        assert_eq!(
            credentials.token().await.unwrap_err(),
            AgyAuthError::RefreshFailed
        );
    }

    #[tokio::test]
    async fn concurrent_renewals_run_agy_once() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = Arc::new(renewing(
            dir.path(),
            credential("tok-a", 10),
            Some(credential("tok-b", 3600)),
            1,
        ));
        let mut waiting = Vec::new();
        for _ in 0..4 {
            let credentials = Arc::clone(&credentials);
            waiting.push(tokio::spawn(async move { credentials.token().await }));
        }
        for task in waiting {
            assert_eq!(task.await.unwrap().unwrap().access_token(), "tok-b");
        }
        assert_eq!(agy_calls(dir.path()), 1);
    }

    #[tokio::test]
    async fn a_hung_agy_is_killed_at_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = renewing(dir.path(), credential("tok-a", 10), None, 20)
            .with_timeouts(Duration::from_secs(1), LOCK_WAIT);
        let started = Instant::now();
        assert_eq!(
            credentials.token().await.unwrap_err(),
            AgyAuthError::RefreshFailed
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(agy_calls(dir.path()), 1);
    }

    #[tokio::test]
    async fn a_refresh_lock_held_elsewhere_is_refresh_busy() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = renewing(dir.path(), credential("tok-a", 10), None, 0)
            .with_timeouts(REFRESH_TIMEOUT, Duration::from_millis(200));
        let _held = crate::file_lock::FileLock::acquire(&refresh_lock_path(dir.path())).unwrap();
        assert_eq!(
            credentials.token().await.unwrap_err(),
            AgyAuthError::RefreshBusy
        );
        assert_eq!(agy_calls(dir.path()), 0);
    }
}
