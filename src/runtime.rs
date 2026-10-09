//! Immutable executable generations and the stable `alc` entrypoint.
//!
//! Dispatch is path-based, never controlled by inherited environment variables.
//! A generation executable is already pinned and must not follow `active.json`.

use std::env;
use std::fmt;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::file_lock::{FileLock, canonical_path};

pub(crate) const FORMAT_VERSION: u32 = 1;
pub(crate) const METADATA_LIMIT: u64 = 64 * 1024;
pub(crate) const BINARY_LIMIT: u64 = 512 * 1024 * 1024;
const SHORT_ID_LEN: usize = 12;

/// A validated, short, filesystem-safe namespace. Keep socket paths short on
/// macOS; the full digest is checked in executable manifests before spawning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenerationId(String);

impl GenerationId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimeScope {
    Legacy,
    Generation(GenerationId),
}

impl RuntimeScope {
    pub fn parse(value: &str) -> Result<Self> {
        if value == "legacy" {
            return Ok(Self::Legacy);
        }
        ensure!(
            value.len() == SHORT_ID_LEN && is_lower_hex(value),
            "invalid runtime '{value}'; expected legacy or a {SHORT_ID_LEN}-character lowercase generation id"
        );
        Ok(Self::Generation(GenerationId(value.to_owned())))
    }

    pub fn identity(&self) -> &str {
        match self {
            Self::Legacy => "legacy",
            Self::Generation(id) => id.as_str(),
        }
    }
}

impl fmt::Display for RuntimeScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.identity())
    }
}

struct ProcessScope {
    scope: RuntimeScope,
    explicit: bool,
}

static PROCESS_SCOPE: OnceLock<ProcessScope> = OnceLock::new();
static DEFAULT_SCOPE: RuntimeScope = RuntimeScope::Legacy;

/// Called once after CLI parsing. Runtime identity is immutable across threads;
/// management of another owner must use explicit `run_dir_for` paths instead.
pub fn initialize(scope: RuntimeScope, explicit: bool) -> Result<()> {
    if let Some(existing) = PROCESS_SCOPE.get() {
        ensure!(
            existing.scope == scope && existing.explicit == explicit,
            "the process runtime scope is already initialized"
        );
        return Ok(());
    }
    PROCESS_SCOPE
        .set(ProcessScope { scope, explicit })
        .map_err(|_| anyhow::anyhow!("the process runtime scope is already initialized"))
}

pub fn scope() -> &'static RuntimeScope {
    PROCESS_SCOPE
        .get()
        .map(|context| &context.scope)
        .unwrap_or(&DEFAULT_SCOPE)
}

pub fn scope_is_explicit() -> bool {
    PROCESS_SCOPE.get().is_some_and(|context| context.explicit)
}

pub fn run_dir(config_dir: &Path) -> PathBuf {
    run_dir_for(config_dir, scope())
}

pub fn run_dir_for(config_dir: &Path, scope: &RuntimeScope) -> PathBuf {
    match scope {
        RuntimeScope::Legacy => config_dir.join("run"),
        RuntimeScope::Generation(id) => config_dir.join("run/g").join(id.as_str()),
    }
}

/// Read-only discovery. Invalid names and symlink/reparse directories are not
/// owners. No runtime directory is created merely to list or stop sessions.
pub fn discover_scopes(config_dir: &Path) -> Result<Vec<RuntimeScope>> {
    let mut scopes = vec![RuntimeScope::Legacy];
    let directory = config_dir.join("run/g");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(scopes),
        Err(error) => return Err(error).context("could not discover runtime generations"),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || is_link(&fs::symlink_metadata(entry.path())?) {
            continue;
        }
        if let Some(name) = entry.file_name().to_str()
            && let Ok(RuntimeScope::Generation(id)) = RuntimeScope::parse(name)
        {
            scopes.push(RuntimeScope::Generation(id));
        }
    }
    scopes[1..].sort();
    Ok(scopes)
}

#[derive(Debug, Clone)]
struct ExecutableIdentity {
    path: PathBuf,
    digest: String,
}

static EXECUTABLE_IDENTITY: OnceLock<std::result::Result<ExecutableIdentity, String>> =
    OnceLock::new();

fn executable_identity() -> Result<&'static ExecutableIdentity> {
    EXECUTABLE_IDENTITY
        .get_or_init(|| {
            let result = (|| -> Result<ExecutableIdentity> {
                let path =
                    env::current_exe().context("could not locate the running alc executable")?;
                let path = fs::canonicalize(path)
                    .context("could not resolve the running alc executable")?;
                let digest = file_digest(&path)?;
                Ok(ExecutableIdentity { path, digest })
            })();
            result.map_err(|error| format!("{error:#}"))
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
}

pub(crate) fn running_digest() -> Result<&'static str> {
    Ok(executable_identity()?.digest.as_str())
}

pub fn generation_scope() -> Result<RuntimeScope> {
    let identity = executable_identity()?;
    RuntimeScope::parse(&identity.digest[..SHORT_ID_LEN])
}

/// Selectors may target foreign owners for management, but must not launch this
/// executable's protocol under another generation's namespace.
pub fn require_own_generation() -> Result<()> {
    if matches!(scope(), RuntimeScope::Generation(_)) {
        ensure!(
            scope() == &generation_scope()?,
            "cannot launch this executable in another generation's runtime; invoke that owner's pinned executable instead"
        );
    }
    Ok(())
}

/// Claim a runtime's short name with its full executable identity. Different
/// install roots can share a config directory, so per-install publication alone
/// cannot protect a truncated-name collision. Call only at mutation boundaries.
pub fn claim_namespace(config_dir: &Path) -> Result<()> {
    if scope() == &RuntimeScope::Legacy {
        return Ok(());
    }
    require_own_generation()?;
    claim_namespace_for(config_dir, running_digest()?)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamespaceManifest {
    schema: u32,
    generation: GenerationRecord,
}

fn claim_namespace_for(config_dir: &Path, digest: &str) -> Result<()> {
    ensure!(valid_digest(digest), "invalid runtime namespace digest");
    let scope = RuntimeScope::parse(&digest[..SHORT_ID_LEN])?;
    let config_dir = canonical_path(config_dir)?;
    let run = config_dir.join("run");
    fs::create_dir_all(&run)?;
    require_directory(&run)?;
    let generations = run.join("g");
    fs::create_dir_all(&generations)?;
    require_directory(&generations)?;
    let namespace = run_dir_for(&config_dir, &scope);
    fs::create_dir_all(&namespace)?;
    require_directory(&namespace)?;
    let _lock = FileLock::acquire(&namespace.join("generation.lock"))?;
    let path = namespace.join("generation.json");
    let expected = GenerationRecord {
        digest: digest.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: platform_key()?,
    };
    if let Some(bytes) = read_optional(&path, METADATA_LIMIT)? {
        let manifest: NamespaceManifest =
            serde_json::from_slice(&bytes).context("invalid runtime namespace manifest")?;
        ensure!(
            manifest.schema == FORMAT_VERSION,
            "unsupported runtime namespace schema"
        );
        manifest.generation.validate()?;
        ensure!(
            manifest.generation == expected,
            "runtime namespace collision: {} already belongs to a different full executable digest; no host credentials or settings were changed",
            scope.identity()
        );
        return Ok(());
    }
    let manifest = NamespaceManifest {
        schema: FORMAT_VERSION,
        generation: expected,
    };
    crate::config::atomic_write(&path, &serde_json::to_vec_pretty(&manifest)?, true)?;
    sync_directory(&namespace)
}

pub fn planned_exe(config_dir: &Path) -> Result<PathBuf> {
    let identity = executable_identity()?;
    if let Some(root) = generation_root(&identity.path) {
        let record = read_generation(&root, &identity.digest)?;
        ensure!(
            record.digest == identity.digest,
            "running generation digest changed"
        );
        return Ok(identity.path.clone());
    }
    let root = canonical_path(&config_dir.join("runtime/.alc"))?;
    Ok(generation_exe(&root, &identity.digest))
}

/// Pin a standalone/development binary only at the actual spawn boundary, never
/// in target/debug and never during dry-run/path planning. Installed generations
/// already have an immutable, verified path and require no writes.
pub fn materialize_exe(config_dir: &Path) -> Result<PathBuf> {
    let identity = executable_identity()?;
    if let Some(root) = generation_root(&identity.path) {
        read_generation(&root, &identity.digest)?;
        return Ok(identity.path.clone());
    }
    let root = canonical_path(&config_dir.join("runtime/.alc"))?;
    fs::create_dir_all(&root)?;
    require_directory(&root)?;
    let _lock = FileLock::acquire(&root.join("publish.lock"))?;
    let record = GenerationRecord {
        digest: identity.digest.clone(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: platform_key()?,
    };
    publish_generation(&root, &identity.path, &record)
}

/// Invoke before parsing flags, including --version. The front's compiled-in
/// version must not hide the selected future binary's version/CLI semantics.
pub fn early_dispatch() -> Result<Option<ExitCode>> {
    let current = executable_identity()?.path.clone();
    if let Some(root) = generation_root(&current) {
        let digest = current
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .context("generation executable has no digest")?;
        read_generation(&root, digest)?;
        return Ok(None);
    }
    let Some(parent) = current.parent() else {
        return Ok(None);
    };
    let Some(active) = read_active(&parent.join(".alc"))? else {
        return Ok(None);
    };
    let target = generation_exe(&parent.join(".alc"), &active.current.digest);
    read_generation(&parent.join(".alc"), &active.current.digest)?;
    let mut command = Command::new(&target);
    command.args(env::args_os().skip(1));
    scrub_runtime_env(&mut command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).with_context(|| format!("could not dispatch to {}", target.display()))
    }
    #[cfg(windows)]
    {
        let status = command
            .status()
            .with_context(|| format!("could not dispatch to {}", target.display()))?;
        Ok(Some(ExitCode::from(
            status.code().unwrap_or(1).clamp(0, 255) as u8,
        )))
    }
}

/// Ignore/remove historical or caller-provided selectors during verification
/// and dispatch. Only an explicit CLI --runtime selects daemon ownership.
pub(crate) fn scrub_runtime_env(command: &mut Command) {
    for key in [
        "ALC_RUNTIME_ORIGIN",
        "ALC_RUNTIME_SELECTOR",
        "ALC_RUNTIME_SCOPE",
        "ALC_RUNTIME_GENERATION",
        "ALC_RUNTIME_ID",
        "ALC_RUNTIME_INSTALL",
    ] {
        command.env_remove(key);
    }
}

pub(crate) fn binary_name() -> &'static str {
    if cfg!(windows) { "alc.exe" } else { "alc" }
}

pub(crate) fn platform_key() -> Result<String> {
    let os = match env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        os => bail!("immutable alc runtimes are not supported on '{os}'"),
    };
    let arch = match env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        arch => bail!("immutable alc runtimes are not supported on '{arch}'"),
    };
    Ok(format!("{os}-{arch}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GenerationRecord {
    pub digest: String,
    pub version: String,
    pub platform: String,
}

impl GenerationRecord {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(valid_digest(&self.digest), "invalid generation digest");
        Version::parse(&self.version).context("invalid generation version")?;
        ensure!(
            self.platform == platform_key()?,
            "generation targets {}, not {}",
            self.platform,
            platform_key()?
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationManifest {
    schema: u32,
    record: GenerationRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActiveManifest {
    pub schema: u32,
    pub current: GenerationRecord,
    pub previous: Option<GenerationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrontManifest {
    pub schema: u32,
    pub dispatcher: u32,
    pub digest: String,
    pub version: String,
    pub platform: String,
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64 && is_lower_hex(value)
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Returns the .alc metadata root only for a syntactically pinned executable.
/// Its full manifest and digest are then checked by every consumer.
pub(crate) fn generation_root(executable: &Path) -> Option<PathBuf> {
    if executable.file_name()? != binary_name() {
        return None;
    }
    let generation = executable.parent()?;
    if !valid_digest(generation.file_name()?.to_str()?) {
        return None;
    }
    let generations = generation.parent()?;
    if generations.file_name()? != "generations" {
        return None;
    }
    let root = generations.parent()?;
    if root.file_name()? != ".alc" {
        return None;
    }
    Some(root.to_path_buf())
}

pub(crate) fn install_front() -> Result<PathBuf> {
    let executable = &executable_identity()?.path;
    if let Some(root) = generation_root(executable) {
        let front = root
            .parent()
            .context("generation has no install root")?
            .join(binary_name());
        ensure!(
            front.is_file(),
            "this pinned runtime has no installed front; run update using the original alc executable"
        );
        return Ok(front);
    }
    ensure!(
        executable
            .file_name()
            .is_some_and(|name| name == binary_name()),
        "update requires the alc install entrypoint, not a renamed executable"
    );
    Ok(executable.clone())
}

pub(crate) fn generation_exe(root: &Path, digest: &str) -> PathBuf {
    root.join("generations").join(digest).join(binary_name())
}

pub(crate) fn read_active(root: &Path) -> Result<Option<ActiveManifest>> {
    read_active_at(root, &root.join("active.json"))
}

pub(crate) fn read_activation_marker(root: &Path) -> Result<Option<ActiveManifest>> {
    read_active_at(root, &root.join("activation.json"))
}

fn read_active_at(root: &Path, path: &Path) -> Result<Option<ActiveManifest>> {
    if !root.try_exists()? {
        return Ok(None);
    }
    require_directory(root)?;
    let Some(bytes) = read_optional(path, METADATA_LIMIT)? else {
        return Ok(None);
    };
    let active: ActiveManifest =
        serde_json::from_slice(&bytes).context("invalid active runtime manifest")?;
    ensure!(
        active.schema == FORMAT_VERSION,
        "unsupported active runtime manifest schema {}",
        active.schema
    );
    active.current.validate()?;
    if let Some(previous) = &active.previous {
        previous.validate()?;
    }
    let actual = read_generation(root, &active.current.digest)?;
    ensure!(
        actual == active.current,
        "active manifest does not match its generation"
    );
    Ok(Some(active))
}

pub(crate) fn read_front(root: &Path, front: &Path) -> Result<Option<FrontManifest>> {
    let Some(bytes) = read_optional(&root.join("front.json"), METADATA_LIMIT)? else {
        return Ok(None);
    };
    let record: FrontManifest =
        serde_json::from_slice(&bytes).context("invalid stable front manifest")?;
    ensure!(
        record.schema == FORMAT_VERSION && record.dispatcher == 1,
        "unsupported stable front manifest"
    );
    ensure!(valid_digest(&record.digest), "invalid stable front digest");
    Version::parse(&record.version).context("invalid stable front version")?;
    ensure!(
        record.platform == platform_key()?,
        "stable front has the wrong platform"
    );
    ensure!(
        file_digest(front)? == record.digest,
        "stable front checksum mismatch"
    );
    Ok(Some(record))
}

pub(crate) fn read_generation(root: &Path, digest: &str) -> Result<GenerationRecord> {
    ensure!(valid_digest(digest), "invalid generation digest");
    require_directory(root)?;
    require_directory(&root.join("generations"))?;
    let directory = root.join("generations").join(digest);
    require_directory(&directory)?;
    let bytes = read_bounded(&directory.join("manifest.json"), METADATA_LIMIT)?;
    let manifest: GenerationManifest =
        serde_json::from_slice(&bytes).context("invalid generation manifest")?;
    ensure!(
        manifest.schema == FORMAT_VERSION,
        "unsupported generation manifest schema {}",
        manifest.schema
    );
    manifest.record.validate()?;
    ensure!(
        manifest.record.digest == digest,
        "generation directory does not match its manifest digest"
    );
    ensure!(
        file_digest(&directory.join(binary_name()))? == digest,
        "generation executable checksum mismatch"
    );
    Ok(manifest.record)
}

/// Caller holds this root's publish.lock. Nothing in a published generation is
/// changed; a crash leaves either the old directory or a complete new one.
pub(crate) fn publish_generation(
    root: &Path,
    source: &Path,
    record: &GenerationRecord,
) -> Result<PathBuf> {
    record.validate()?;
    let generations = root.join("generations");
    fs::create_dir_all(&generations)?;
    require_directory(&generations)?;
    let destination = generation_exe(root, &record.digest);
    if destination
        .parent()
        .context("generation path has no parent")?
        .try_exists()?
    {
        ensure!(
            read_generation(root, &record.digest)? == *record,
            "existing generation metadata does not match the payload"
        );
        return Ok(destination);
    }
    // A namespace collision is extremely unlikely but must not silently share
    // sockets/credentials with another full-digest executable.
    for entry in fs::read_dir(&generations)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str()
            && valid_digest(name)
            && name[..SHORT_ID_LEN] == record.digest[..SHORT_ID_LEN]
            && name != record.digest
        {
            bail!(
                "generation namespace collision; full digests differ for {}",
                &record.digest[..SHORT_ID_LEN]
            );
        }
    }
    let stage = tempfile::Builder::new()
        .prefix(".publish-")
        .tempdir_in(&generations)
        .context("could not stage an immutable executable generation")?;
    let staged_exe = stage.path().join(binary_name());
    copy_new(source, &staged_exe)?;
    ensure!(
        file_digest(&staged_exe)? == record.digest,
        "payload changed while publishing its generation"
    );
    make_executable(&staged_exe)?;
    let manifest = GenerationManifest {
        schema: FORMAT_VERSION,
        record: record.clone(),
    };
    crate::config::atomic_write(
        &stage.path().join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
        true,
    )?;
    sync_directory(stage.path())?;
    fs::rename(stage.path(), destination.parent().unwrap())
        .context("could not publish immutable generation")?;
    sync_directory(&generations)?;
    Ok(destination)
}

pub(crate) fn write_active(root: &Path, active: &ActiveManifest) -> Result<()> {
    active.current.validate()?;
    ensure!(
        read_generation(root, &active.current.digest)? == active.current,
        "cannot activate an unverified generation"
    );
    // The secret=true variant also uses an unguessable create_new temp path,
    // avoiding a predictable temporary-file symlink for this public metadata.
    crate::config::atomic_write(
        &root.join("active.json"),
        &serde_json::to_vec_pretty(active)?,
        true,
    )?;
    sync_directory(root)
}

pub(crate) fn write_activation_marker(root: &Path, active: &ActiveManifest) -> Result<()> {
    active.current.validate()?;
    crate::config::atomic_write(
        &root.join("activation.json"),
        &serde_json::to_vec_pretty(active)?,
        true,
    )?;
    sync_directory(root)
}

pub(crate) fn write_front(root: &Path, front: &FrontManifest) -> Result<()> {
    crate::config::atomic_write(
        &root.join("front.json"),
        &serde_json::to_vec_pretty(front)?,
        true,
    )?;
    sync_directory(root)
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut input = open_regular(path)?;
    ensure!(
        input.metadata()?.len() <= limit,
        "{} exceeds its {limit}-byte limit",
        path.display()
    );
    let mut bytes = Vec::new();
    Read::by_ref(&mut input)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "{} exceeds its {limit}-byte limit",
        path.display()
    );
    Ok(bytes)
}

pub(crate) fn read_optional(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    match read_bounded(path, limit) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn file_digest(path: &Path) -> Result<String> {
    let mut file = open_regular(path)?;
    ensure!(
        file.metadata()?.len() <= BINARY_LIMIT,
        "executable {} is too large",
        path.display()
    );
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        total += size as u64;
        ensure!(
            total <= BINARY_LIMIT,
            "executable {} is too large",
            path.display()
        );
        digest.update(&buffer[..size]);
    }
    Ok(hex_digest(&digest.finalize()))
}

pub(crate) fn bytes_digest(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("writing a String cannot fail");
    }
    text
}

pub(crate) fn open_regular(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    ensure!(
        metadata.is_file() && !is_link(&metadata),
        "{} is not a regular file",
        path.display()
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(path)
        .with_context(|| format!("could not open {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && !is_link(&metadata),
        "{} is not a regular file",
        path.display()
    );
    Ok(file)
}

pub(crate) fn require_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect {}", path.display()))?;
    ensure!(
        metadata.is_dir() && !is_link(&metadata),
        "{} is not a regular directory",
        path.display()
    );
    Ok(())
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub(crate) fn copy_new(source: &Path, destination: &Path) -> Result<()> {
    let mut input = open_regular(source)?;
    ensure!(
        input.metadata()?.len() <= BINARY_LIMIT,
        "payload exceeds its size limit"
    );
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;
    let size = io::copy(
        &mut Read::by_ref(&mut input).take(BINARY_LIMIT + 1),
        &mut output,
    )?;
    ensure!(size <= BINARY_LIMIT, "payload exceeds its size limit");
    output.sync_all()?;
    Ok(())
}

pub(crate) fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?
        .sync_all()
        .with_context(|| format!("could not flush directory {}", path.display()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_reject_path_and_selector_injection() {
        for value in [
            "",
            ".",
            "../legacy",
            "abc",
            "123456789ABC",
            "123456789abc/",
            "generation",
        ] {
            assert!(RuntimeScope::parse(value).is_err(), "{value}");
        }
        assert_eq!(RuntimeScope::parse("legacy").unwrap(), RuntimeScope::Legacy);
        assert_eq!(
            RuntimeScope::parse("123456789abc").unwrap().identity(),
            "123456789abc"
        );
    }

    #[test]
    fn owner_paths_do_not_change_shared_config_roots() {
        let config = Path::new("/tmp/config");
        let generation = RuntimeScope::parse("123456789abc").unwrap();
        assert_eq!(
            run_dir_for(config, &RuntimeScope::Legacy),
            config.join("run")
        );
        assert_eq!(
            run_dir_for(config, &generation),
            config.join("run/g/123456789abc")
        );
    }

    #[test]
    fn planning_and_discovery_do_not_write() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("missing");
        let planned = planned_exe(&config).unwrap();
        assert!(planned.ends_with(binary_name()));
        assert!(!config.exists());
        assert_eq!(
            discover_scopes(&config).unwrap(),
            vec![RuntimeScope::Legacy]
        );
        assert!(!config.exists());
    }

    #[test]
    fn manifests_are_bounded_and_unknown_fields_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".alc");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("active.json"),
            vec![b' '; METADATA_LIMIT as usize + 1],
        )
        .unwrap();
        assert!(
            read_active(&root)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        fs::write(
            root.join("active.json"),
            br#"{"schema":99,"current":{},"previous":null,"unknown":true}"#,
        )
        .unwrap();
        assert!(read_active(&root).is_err());
    }

    #[test]
    fn same_version_different_builds_get_different_namespaces() {
        let first = bytes_digest(b"alc 2.0.1 build one");
        let second = bytes_digest(b"alc 2.0.1 build two");
        assert_ne!(&first[..SHORT_ID_LEN], &second[..SHORT_ID_LEN]);
    }

    #[test]
    fn config_namespace_claim_checks_full_digest_across_install_roots() {
        let temp = tempfile::tempdir().unwrap();
        let first = format!("123456789abc{}", "0".repeat(52));
        let collision = format!("123456789abc{}", "f".repeat(52));
        claim_namespace_for(temp.path(), &first).unwrap();
        let namespace = temp.path().join("run/g/123456789abc");
        let path = namespace.join("generation.json");
        let before = fs::read(&path).unwrap();
        claim_namespace_for(temp.path(), &first).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
        let error = claim_namespace_for(temp.path(), &collision).unwrap_err();
        assert!(error.to_string().contains("namespace collision"));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!namespace.join("bridge.token").exists());
        assert!(!namespace.join("ctl.token").exists());
    }

    #[test]
    fn concurrent_namespace_claims_cannot_replace_the_first_identity() {
        let temp = tempfile::tempdir().unwrap();
        let first = format!("abcdef123456{}", "0".repeat(52));
        let second = format!("abcdef123456{}", "f".repeat(52));
        let handles = [first, second]
            .into_iter()
            .map(|digest| {
                let config = temp.path().to_owned();
                std::thread::spawn(move || claim_namespace_for(&config, &digest).is_ok())
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|success| **success).count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn namespace_claim_does_not_follow_a_generation_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let victim = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("run/g")).unwrap();
        std::os::unix::fs::symlink(victim.path(), temp.path().join("run/g/123456789abc")).unwrap();
        let digest = format!("123456789abc{}", "0".repeat(52));
        assert!(claim_namespace_for(temp.path(), &digest).is_err());
        assert_eq!(fs::read_dir(victim.path()).unwrap().count(), 0);
    }

    #[test]
    fn publish_reuses_only_fully_verified_content() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"a test executable").unwrap();
        let root = temp.path().join(".alc");
        fs::create_dir(&root).unwrap();
        let record = GenerationRecord {
            digest: file_digest(&source).unwrap(),
            version: "2.0.1".into(),
            platform: platform_key().unwrap(),
        };
        let _lock = FileLock::acquire(&root.join("publish.lock")).unwrap();
        let published = publish_generation(&root, &source, &record).unwrap();
        assert_eq!(read_generation(&root, &record.digest).unwrap(), record);
        assert_eq!(
            publish_generation(&root, &source, &record).unwrap(),
            published
        );
        fs::write(&published, b"corrupt").unwrap();
        assert!(publish_generation(&root, &source, &record).is_err());
    }
}
