//! Acquire/verify/publish/activate alc updates. No agent, hub, bridge, or user
//! session is stopped, and no loaded generation executable is overwritten.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::file_lock::{FileLock, canonical_path};
use crate::runtime::{self, ActiveManifest, FrontManifest, GenerationRecord};

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/treeleaves30760/all-code/releases/latest";
const API_DOWNLOAD_LIMIT: u64 = 4 * 1024 * 1024;
const CHECKSUM_DOWNLOAD_LIMIT: u64 = 1024 * 1024;
const ARCHIVE_DOWNLOAD_LIMIT: u64 = 256 * 1024 * 1024;
const EXTRACTED_ARCHIVE_LIMIT: u64 = 640 * 1024 * 1024;
const ARCHIVE_ENTRY_LIMIT: usize = 4096;
const VERSION_OUTPUT_LIMIT: u64 = 64 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Deserialize, Serialize)]
struct Release {
    tag_name: String,
    html_url: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BundleManifest {
    schema: u32,
    version: String,
    platform: String,
    asset: String,
    archive_sha256: String,
    binary_sha256: String,
}

struct VerifiedPayload {
    // Keep the fresh, isolated extraction directory alive until publication.
    _extraction: tempfile::TempDir,
    binary: PathBuf,
    version: Version,
    digest: String,
}

struct VerifiedBundle {
    manifest: BundleManifest,
    release: Release,
    checksums: Vec<u8>,
    archive: Vec<u8>,
    payload: VerifiedPayload,
}

/// Local sources are handled before any GitHub lookup, including --check. --from
/// is always zero-network, even when the redundant --offline flag is omitted.
pub fn run(
    check_only: bool,
    force: bool,
    from: Option<&Path>,
    download_only: Option<&Path>,
    offline: bool,
    rollback: Option<&str>,
) -> Result<u8> {
    ensure!(
        !offline || from.is_some() || rollback.is_some(),
        "--offline requires --from or --rollback; no network request was made"
    );
    ensure!(
        !(check_only && download_only.is_some()),
        "--check cannot be combined with --download-only"
    );
    ensure!(
        !(rollback.is_some() && (from.is_some() || download_only.is_some() || check_only)),
        "--rollback cannot be combined with --from, --download-only, or --check"
    );
    if let Some(selector) = rollback {
        let front = runtime::install_front()?;
        rollback_to(&front, selector)?;
        return Ok(0);
    }

    // Validation of an unknown/corrupt/wrong-platform local bundle happens
    // before even resolving or creating any installed runtime paths.
    let bundle = if let Some(directory) = from {
        read_bundle(directory)?
    } else {
        let release = fetch_release()?;
        let latest = parse_release_version(&release.tag_name)?;
        if check_only {
            print_check(&installed_version()?, &latest, &release.html_url);
            return Ok(0);
        }
        if download_only.is_none() {
            let current = installed_version()?;
            if latest < current || (latest == current && !force) {
                print_no_update(&current, &latest);
                return Ok(0);
            }
        }
        acquire_bundle(release)?
    };

    if check_only {
        print_check(
            &installed_version()?,
            &bundle.payload.version,
            &bundle.release.html_url,
        );
        return Ok(0);
    }
    if let Some(directory) = download_only {
        persist_bundle(&bundle, directory)?;
        println!(
            "Verified alc {} bundle saved to {}. No installation was changed.",
            bundle.payload.version,
            directory.display()
        );
        return Ok(0);
    }

    let front = runtime::install_front()?;
    publish_and_activate(&front, &bundle.payload, force)?;
    print_path_note(&front);
    Ok(0)
}

/// Config-independent hidden installer entrypoint. The shell/PowerShell
/// installer verifies its archive, then asks its payload to perform the same
/// locked Rust transaction used by `alc update`.
pub fn install_payload(install_to: &Path, expected_version: &str) -> Result<u8> {
    let expected = parse_release_version(expected_version)?;
    let source = env::current_exe().context("could not locate the installer payload")?;
    let source = fs::canonicalize(source)?;
    let version = packaged_version(&source)?;
    ensure!(
        version == expected,
        "installer expected alc {expected}, but the payload is alc {version}"
    );
    let digest = runtime::file_digest(&source)?;
    let extraction = tempfile::tempdir().context("could not prepare installer verification")?;
    let payload = VerifiedPayload {
        _extraction: extraction,
        binary: source,
        version,
        digest,
    };
    publish_and_activate(install_to, &payload, true)?;
    Ok(0)
}

fn installed_version() -> Result<Version> {
    let front = runtime::install_front()?;
    let root = front
        .parent()
        .context("install entrypoint has no parent directory")?
        .join(".alc");
    if let Some(active) = runtime::read_active(&root)? {
        return Version::parse(&active.current.version).context("invalid installed active version");
    }
    packaged_version(&front)
}

fn print_check(current: &Version, latest: &Version, url: &str) {
    if latest > current {
        println!(
            "Update available: alc {current} -> {latest}\nRun `alc update` to install it.\n{url}"
        );
    } else if latest == current {
        println!("alc {current} is up to date.");
    } else {
        println!("alc {current} is newer than the latest release ({latest}).");
    }
}

fn print_no_update(current: &Version, latest: &Version) {
    if latest < current {
        println!(
            "alc {current} is newer than the requested release ({latest}); nothing to do. Use --rollback for an explicitly retained generation."
        );
    } else {
        println!("alc {current} is already up to date.");
    }
}

fn fetch_release() -> Result<Release> {
    let url = release_api_url();
    let bytes = download(&url, API_DOWNLOAD_LIMIT)
        .with_context(|| format!("could not query the latest alc release at {url}"))?;
    let release: Release =
        serde_json::from_slice(&bytes).context("GitHub returned invalid release metadata")?;
    ensure!(
        release.assets.len() <= ARCHIVE_ENTRY_LIMIT,
        "release contains too many assets"
    );
    Ok(release)
}

fn release_api_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = env::var("ALC_UPDATE_API_URL") {
        return url;
    }
    LATEST_RELEASE_API.to_owned()
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(url)
        .header("User-Agent", concat!("alc/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .with_context(|| format!("GET {url} failed"))?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .with_context(|| format!("could not read {url}"))
}

fn parse_release_version(tag: &str) -> Result<Version> {
    let value = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(value).with_context(|| format!("release tag '{tag}' is not a valid version"))
}

fn platform_asset_name() -> Result<String> {
    let platform = runtime::platform_key()?;
    let extension = if cfg!(windows) { "zip" } else { "tar.gz" };
    Ok(format!("alc-{platform}.{extension}"))
}

fn find_asset<'a>(release: &'a Release, name: &str) -> Result<&'a ReleaseAsset> {
    let mut candidates = release.assets.iter().filter(|asset| asset.name == name);
    let asset = candidates
        .next()
        .with_context(|| format!("release {} does not contain {name}", release.tag_name))?;
    ensure!(
        candidates.next().is_none(),
        "release contains duplicate asset {name}"
    );
    Ok(asset)
}

fn checksum_for(contents: &[u8], asset_name: &str) -> Result<String> {
    ensure!(
        contents.len() as u64 <= CHECKSUM_DOWNLOAD_LIMIT,
        "checksums.txt exceeds its size limit"
    );
    let text = std::str::from_utf8(contents).context("checksums.txt is not valid UTF-8")?;
    let mut found = None;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else { continue };
        let Some(name) = fields.next() else { continue };
        if name.strip_prefix('*').unwrap_or(name) == asset_name {
            ensure!(
                fields.next().is_none(),
                "the published checksum line for {asset_name} is invalid"
            );
            ensure!(
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "the published checksum for {asset_name} is invalid"
            );
            ensure!(
                found.is_none(),
                "checksums.txt contains duplicate checksums for {asset_name}"
            );
            found = Some(hash.to_ascii_lowercase());
        }
    }
    found.with_context(|| format!("checksums.txt does not contain {asset_name}"))
}

fn verify_checksum(bytes: &[u8], expected: &str, name: &str) -> Result<()> {
    let actual = runtime::bytes_digest(bytes);
    ensure!(
        actual == expected,
        "checksum mismatch for {name}: expected {expected}, got {actual}"
    );
    Ok(())
}

fn acquire_bundle(release: Release) -> Result<VerifiedBundle> {
    let version = parse_release_version(&release.tag_name)?;
    let asset = platform_asset_name()?;
    let archive_asset = find_asset(&release, &asset)?;
    let checksum_asset = find_asset(&release, "checksums.txt")?;
    println!(
        "Downloading alc {version} for {}...",
        runtime::platform_key()?
    );
    let checksums = download(
        &checksum_asset.browser_download_url,
        CHECKSUM_DOWNLOAD_LIMIT,
    )
    .context("could not download release checksums")?;
    let expected = checksum_for(&checksums, &asset)?;
    let archive = download(&archive_asset.browser_download_url, ARCHIVE_DOWNLOAD_LIMIT)
        .with_context(|| format!("could not download {asset}"))?;
    verify_checksum(&archive, &expected, &asset)?;
    let payload = verify_archive(&archive, &asset, &version)?;
    println!("Verified {asset} (SHA-256 and alc version).");
    let manifest = BundleManifest {
        schema: runtime::FORMAT_VERSION,
        version: version.to_string(),
        platform: runtime::platform_key()?,
        asset,
        archive_sha256: expected,
        binary_sha256: payload.digest.clone(),
    };
    Ok(VerifiedBundle {
        manifest,
        release,
        checksums,
        archive,
        payload,
    })
}

fn verify_archive(archive: &[u8], asset: &str, expected: &Version) -> Result<VerifiedPayload> {
    let extraction = tempfile::Builder::new()
        .prefix("alc-payload-")
        .tempdir()
        .context("could not create an isolated extraction directory")?;
    let binary = extract_binary(archive, asset, extraction.path())?;
    let digest = runtime::file_digest(&binary)?;
    let version = packaged_version(&binary)?;
    ensure!(
        &version == expected,
        "release metadata says {expected}, but the downloaded binary is {version}"
    );
    Ok(VerifiedPayload {
        _extraction: extraction,
        binary,
        version,
        digest,
    })
}

fn read_bundle(directory: &Path) -> Result<VerifiedBundle> {
    runtime::require_directory(directory)?;
    let manifest_bytes =
        runtime::read_bounded(&directory.join("bundle.json"), runtime::METADATA_LIMIT)?;
    let manifest: BundleManifest =
        serde_json::from_slice(&manifest_bytes).context("invalid offline bundle manifest")?;
    ensure!(
        manifest.schema == runtime::FORMAT_VERSION,
        "unsupported offline bundle schema {}",
        manifest.schema
    );
    ensure!(
        manifest.platform == runtime::platform_key()?,
        "offline bundle targets {}, not {}",
        manifest.platform,
        runtime::platform_key()?
    );
    let asset = platform_asset_name()?;
    ensure!(
        manifest.asset == asset,
        "offline bundle has the wrong target asset"
    );
    ensure!(
        runtime::valid_digest(&manifest.archive_sha256)
            && runtime::valid_digest(&manifest.binary_sha256),
        "offline bundle contains invalid digests"
    );
    let version = Version::parse(&manifest.version).context("invalid offline bundle version")?;
    let release_bytes = runtime::read_bounded(&directory.join("release.json"), API_DOWNLOAD_LIMIT)?;
    let release: Release =
        serde_json::from_slice(&release_bytes).context("invalid offline release metadata")?;
    ensure!(
        parse_release_version(&release.tag_name)? == version,
        "offline bundle release version does not match its manifest"
    );
    ensure!(
        release.assets.len() <= ARCHIVE_ENTRY_LIMIT,
        "offline release contains too many assets"
    );
    find_asset(&release, &asset)?;
    find_asset(&release, "checksums.txt")?;
    let checksums =
        runtime::read_bounded(&directory.join("checksums.txt"), CHECKSUM_DOWNLOAD_LIMIT)?;
    let expected = checksum_for(&checksums, &asset)?;
    ensure!(
        expected == manifest.archive_sha256,
        "offline bundle checksum does not match its manifest"
    );
    let archive = runtime::read_bounded(&directory.join(&asset), ARCHIVE_DOWNLOAD_LIMIT)?;
    verify_checksum(&archive, &expected, &asset)?;
    let payload = verify_archive(&archive, &asset, &version)?;
    ensure!(
        payload.digest == manifest.binary_sha256,
        "offline bundle executable digest does not match its manifest"
    );
    Ok(VerifiedBundle {
        manifest,
        release,
        checksums,
        archive,
        payload,
    })
}

fn persist_bundle(bundle: &VerifiedBundle, directory: &Path) -> Result<()> {
    let directory = canonical_path(directory)?;
    if directory.try_exists()? {
        runtime::require_directory(&directory)?;
        ensure!(
            fs::read_dir(&directory)?.next().is_none(),
            "bundle destination must be new or empty: {}",
            directory.display()
        );
    }
    let parent = directory
        .parent()
        .context("bundle directory has no parent")?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".alc-bundle-")
        .tempdir_in(parent)?;
    write_new(&stage.path().join(&bundle.manifest.asset), &bundle.archive)?;
    write_new(&stage.path().join("checksums.txt"), &bundle.checksums)?;
    write_new(
        &stage.path().join("release.json"),
        &serde_json::to_vec_pretty(&bundle.release)?,
    )?;
    write_new(
        &stage.path().join("bundle.json"),
        &serde_json::to_vec_pretty(&bundle.manifest)?,
    )?;
    runtime::sync_directory(stage.path())?;
    if directory.try_exists()? {
        fs::remove_dir(&directory).context("bundle destination is no longer empty")?;
    }
    fs::rename(stage.path(), &directory).context("could not publish the offline bundle")?;
    runtime::sync_directory(parent)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn extract_binary(archive: &[u8], asset: &str, destination: &Path) -> Result<PathBuf> {
    ensure!(
        archive.len() as u64 <= ARCHIVE_DOWNLOAD_LIMIT,
        "release archive exceeds its size limit"
    );
    runtime::require_directory(destination)?;
    ensure!(
        fs::read_dir(destination)?.next().is_none(),
        "extraction directory must be empty"
    );
    let output = destination.join(runtime::binary_name());
    if asset.ends_with(".zip") {
        extract_zip_file(archive, &output)?;
    } else {
        extract_tar_file(archive, &output)?;
    }
    ensure!(
        output.is_file(),
        "release archive does not contain {}",
        runtime::binary_name()
    );
    runtime::make_executable(&output)?;
    Ok(output)
}

fn extract_zip_file(bytes: &[u8], destination: &Path) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("release is not a valid ZIP archive")?;
    ensure!(
        archive.len() <= ARCHIVE_ENTRY_LIMIT,
        "release ZIP contains too many entries"
    );
    let mut found = false;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("could not read release ZIP entry")?;
        if entry.name() != runtime::binary_name() {
            continue;
        }
        ensure!(!found, "release ZIP contains duplicate executable entries");
        ensure!(
            entry.is_file()
                && entry
                    .unix_mode()
                    .is_none_or(|mode| mode & 0o170000 != 0o120000),
            "release ZIP executable is not a regular file"
        );
        ensure!(
            entry.size() <= runtime::BINARY_LIMIT,
            "release executable exceeds its size limit"
        );
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let size = io::copy(
            &mut entry.by_ref().take(runtime::BINARY_LIMIT + 1),
            &mut output,
        )?;
        ensure!(
            size <= runtime::BINARY_LIMIT,
            "release executable exceeds its size limit"
        );
        output.sync_all()?;
        found = true;
    }
    ensure!(
        found,
        "release ZIP does not contain {}",
        runtime::binary_name()
    );
    Ok(())
}

struct BoundedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(io::Error::other(
                "decompressed release archive exceeds its size limit",
            ));
        }
        let limit = output
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let size = self.inner.read(&mut output[..limit])?;
        self.remaining -= size as u64;
        Ok(size)
    }
}

fn extract_tar_file(bytes: &[u8], destination: &Path) -> Result<()> {
    // Inspect headers first so tar's GNU long-name/PAX parsers never allocate an
    // attacker-advertised, unbounded metadata record. Package archives have
    // short root names and do not need extended headers.
    validate_tar_headers(bytes)?;
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(BoundedReader {
        inner: decoder,
        remaining: EXTRACTED_ARCHIVE_LIMIT,
    });
    let mut found = false;
    for (index, entry) in archive
        .entries()
        .context("release is not a valid tar archive")?
        .enumerate()
    {
        ensure!(
            index < ARCHIVE_ENTRY_LIMIT,
            "release tar contains too many entries"
        );
        let mut entry = entry.context("could not read release archive entry")?;
        ensure!(
            entry.size() <= runtime::BINARY_LIMIT,
            "release archive entry exceeds its size limit"
        );
        let name = entry
            .path()
            .context("release archive contains an invalid path")?;
        // Only the root payload is extracted. Never unpack paths, links, scripts,
        // or filenames supplied by the archive into the install directory.
        if name != Path::new(runtime::binary_name())
            && name != Path::new(&format!("./{}", runtime::binary_name()))
        {
            continue;
        }
        ensure!(!found, "release tar contains duplicate executable entries");
        ensure!(
            entry.header().entry_type().is_file(),
            "release executable is not a regular file"
        );
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let size = io::copy(
            &mut entry.by_ref().take(runtime::BINARY_LIMIT + 1),
            &mut output,
        )?;
        ensure!(
            size <= runtime::BINARY_LIMIT,
            "release executable exceeds its size limit"
        );
        output.sync_all()?;
        found = true;
    }
    ensure!(
        found,
        "release tar does not contain {}",
        runtime::binary_name()
    );
    Ok(())
}

fn validate_tar_headers(bytes: &[u8]) -> Result<()> {
    let mut decoder = BoundedReader {
        inner: GzDecoder::new(Cursor::new(bytes)),
        remaining: EXTRACTED_ARCHIVE_LIMIT,
    };
    for _ in 0..ARCHIVE_ENTRY_LIMIT {
        let mut header = [0_u8; 512];
        decoder
            .read_exact(&mut header)
            .context("truncated release tar header")?;
        if header.iter().all(|byte| *byte == 0) {
            return Ok(());
        }
        ensure!(
            !matches!(header[156], b'L' | b'K' | b'x' | b'g'),
            "release tar uses unsupported extended metadata"
        );
        // Only the traditional bounded octal size field is accepted. GNU binary
        // numbers/PAX overrides are unnecessary for alc's release artifacts.
        let raw = std::str::from_utf8(&header[124..136]).context("invalid tar size field")?;
        let raw = raw.trim_matches(['\0', ' ']);
        let size = if raw.is_empty() {
            0
        } else {
            u64::from_str_radix(raw, 8).context("invalid tar entry size")?
        };
        ensure!(
            size <= runtime::BINARY_LIMIT,
            "release tar entry exceeds its size limit"
        );
        let padded = size.checked_add(511).context("tar entry size overflow")? / 512 * 512;
        let copied = io::copy(&mut decoder.by_ref().take(padded), &mut io::sink())?;
        ensure!(copied == padded, "truncated release tar entry");
    }
    bail!("release tar contains too many entries")
}

fn packaged_version(binary: &Path) -> Result<Version> {
    // Probe output is bounded on disk rather than collected into an unbounded
    // Vec or drained by threads that can hang on an inherited grandchild pipe.
    let probe = tempfile::Builder::new().prefix("alc-version-").tempdir()?;
    let stdout_path = probe.path().join("stdout");
    let stderr_path = probe.path().join("stderr");
    let stdout = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stdout_path)?;
    let stderr = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stderr_path)?;
    let mut command = Command::new(binary);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    runtime::scrub_runtime_env(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("could not run downloaded binary {}", binary.display()))?;
    let start = Instant::now();
    let status = loop {
        if fs::metadata(&stdout_path)?.len() > VERSION_OUTPUT_LIMIT
            || fs::metadata(&stderr_path)?.len() > VERSION_OUTPUT_LIMIT
        {
            let _ = child.kill();
            let _ = child.wait();
            bail!("alc --version exceeded its output limit");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= VERSION_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("alc --version timed out during payload verification");
        }
        thread::sleep(Duration::from_millis(10));
    };
    ensure!(
        status.success(),
        "downloaded alc binary failed its version check"
    );
    let stdout = runtime::read_bounded(&stdout_path, VERSION_OUTPUT_LIMIT)?;
    let stdout = std::str::from_utf8(&stdout).context("alc --version returned invalid UTF-8")?;
    parse_binary_version(stdout.trim())
}

fn parse_binary_version(output: &str) -> Result<Version> {
    let mut fields = output.split_whitespace();
    ensure!(
        fields.next() == Some("alc"),
        "unexpected version output: {output}"
    );
    let version = fields
        .next()
        .with_context(|| format!("unexpected version output: {output}"))?;
    ensure!(
        fields.next().is_none(),
        "unexpected version output: {output}"
    );
    Version::parse(version).with_context(|| format!("invalid packaged alc version: {version}"))
}

fn publish_and_activate(front: &Path, payload: &VerifiedPayload, force: bool) -> Result<()> {
    let front = canonical_path(front)?;
    ensure!(
        front
            .file_name()
            .is_some_and(|name| name == runtime::binary_name()),
        "--install-to must name the full alc executable path, ending in {}",
        runtime::binary_name()
    );
    ensure!(
        runtime::file_digest(&payload.binary)? == payload.digest,
        "verified payload changed before installation"
    );
    let parent = front.parent().context("install entrypoint has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("could not create install directory {}", parent.display()))?;
    let root = parent.join(".alc");
    fs::create_dir_all(&root)?;
    runtime::require_directory(&root)?;
    let _lock = FileLock::acquire(&root.join("publish.lock"))?;

    // Another updater may have activated a newer version while acquisition was
    // in progress. The compiled/running version is not the installed version.
    let active = runtime::read_active(&root)?;
    // A durable marker records the intended activation before an initial front
    // rename. Recover a crash after that rename/metadata publication without
    // mistaking its now-new version for an already completed installation.
    let activation_marker = runtime::read_activation_marker(&root)?;
    if let (Some(marker), Some(active)) = (&activation_marker, &active) {
        ensure!(
            marker.current == active.current || marker.previous.as_ref() == Some(&active.current),
            "pending activation does not extend the installed generation"
        );
    }
    let front_manifest = runtime::read_front(&root, &front)?;
    let existing = if let Some(active) = &active {
        Some(active.current.clone())
    } else if let Some(marker) = &activation_marker {
        marker.previous.clone()
    } else if front.try_exists()? {
        Some(GenerationRecord {
            digest: runtime::file_digest(&front)?,
            version: packaged_version(&front)?.to_string(),
            platform: runtime::platform_key()?,
        })
    } else {
        None
    };
    if let Some(existing) = &existing {
        let installed = Version::parse(&existing.version)?;
        if payload.version < installed || (payload.version == installed && !force) {
            print_no_update(&installed, &payload.version);
            if active.is_some() && activation_marker.is_some() {
                fs::remove_file(root.join("activation.json"))?;
                runtime::sync_directory(&root)?;
            }
            return Ok(());
        }
    }

    if active.is_none()
        && activation_marker.is_none()
        && let Some(existing) = &existing
    {
        runtime::publish_generation(&root, &front, existing)
            .context("could not retain the previous executable for rollback")?;
    }
    let next = GenerationRecord {
        digest: payload.digest.clone(),
        version: payload.version.to_string(),
        platform: runtime::platform_key()?,
    };
    let published = runtime::publish_generation(&root, &payload.binary, &next)?;
    let previous = match &active {
        Some(active) if active.current.digest == next.digest => active.previous.clone(),
        Some(active) => Some(active.current.clone()),
        None => existing.filter(|existing| existing.digest != next.digest),
    };
    let activation = ActiveManifest {
        schema: runtime::FORMAT_VERSION,
        current: next.clone(),
        previous,
    };
    runtime::write_activation_marker(&root, &activation)?;

    if front_manifest.is_none() {
        // A directly running new front is proof that this exact content has a
        // dispatcher. An installer payload must otherwise bootstrap the old
        // front using one atomic rename, not truncate/cp over a loaded image.
        let running = fs::canonicalize(env::current_exe()?)?;
        let front_is_running_dispatcher =
            running == front && runtime::file_digest(&front)? == runtime::running_digest()?;
        let stable = if front_is_running_dispatcher {
            FrontManifest {
                schema: runtime::FORMAT_VERSION,
                dispatcher: 1,
                digest: runtime::file_digest(&front)?,
                version: env!("CARGO_PKG_VERSION").into(),
                platform: runtime::platform_key()?,
            }
        } else {
            bootstrap_front(&front, &root, &published, &next)?;
            FrontManifest {
                schema: runtime::FORMAT_VERSION,
                dispatcher: 1,
                digest: next.digest.clone(),
                version: next.version.clone(),
                platform: next.platform.clone(),
            }
        };
        runtime::write_front(&root, &stable)?;
    }

    runtime::write_active(&root, &activation)?;
    fs::remove_file(root.join("activation.json"))?;
    runtime::sync_directory(&root)?;
    let _ = fs::remove_file(root.join("pending.json"));
    println!("Activated alc {} at {}.", next.version, front.display());
    println!(
        "Existing sessions keep their pinned executables; retained generations are available for --rollback."
    );
    Ok(())
}

fn bootstrap_front(
    front: &Path,
    root: &Path,
    published: &Path,
    next: &GenerationRecord,
) -> Result<()> {
    let parent = front.parent().context("install entrypoint has no parent")?;
    let stage = tempfile::Builder::new()
        .prefix(".alc-front-")
        .tempdir_in(parent)?;
    let staged = stage.path().join(runtime::binary_name());
    runtime::copy_new(published, &staged)?;
    runtime::make_executable(&staged)?;
    if let Err(error) = fs::rename(&staged, front) {
        #[cfg(windows)]
        {
            // A loaded pre-dispatch front cannot be retroactively fixed. Keep
            // the verified immutable payload, do not kill anything or schedule
            // a PowerShell finalizer, and do not claim activation succeeded.
            let pending = serde_json::json!({ "schema": runtime::FORMAT_VERSION, "front": front, "payload": next });
            crate::config::atomic_write(
                &root.join("pending.json"),
                &serde_json::to_vec_pretty(&pending)?,
                true,
            )?;
            runtime::sync_directory(root)?;
            bail!(
                "Windows bootstrap is pending: {} could not be replaced ({error}). The verified payload is preserved at {}. Retry `\"{}\" __install --install-to \"{}\" --version {}` after processes using the old front exit, or choose a different install directory. No process was stopped and active.json was not changed.",
                front.display(),
                published.display(),
                published.display(),
                front.display(),
                next.version
            );
        }
        #[cfg(not(windows))]
        {
            let _ = (root, next);
            return Err(error).with_context(|| format!("could not atomically bootstrap {}; check directory permissions or choose a different install directory", front.display()));
        }
    }
    runtime::sync_directory(parent)
}

fn rollback_to(front: &Path, selector: &str) -> Result<()> {
    ensure!(
        selector == "previous"
            || runtime::valid_digest(selector)
            || RuntimeSelector::is_short(selector),
        "--rollback expects previous, a full retained digest, or a 12-character generation id"
    );
    let front = canonical_path(front)?;
    let root = front
        .parent()
        .context("install entrypoint has no parent")?
        .join(".alc");
    runtime::require_directory(&root)?;
    let _lock = FileLock::acquire(&root.join("publish.lock"))?;
    let active =
        runtime::read_active(&root)?.context("no active immutable installation to roll back")?;
    runtime::read_front(&root, &front)?
        .context("the stable front is not bootstrapped; finish installation before rollback")?;
    let next = if selector == "previous" {
        active
            .previous
            .clone()
            .context("no previous generation is retained in active.json")?
    } else if runtime::valid_digest(selector) {
        runtime::read_generation(&root, selector)?
    } else {
        let mut matching = Vec::new();
        for entry in fs::read_dir(root.join("generations"))? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str()
                && runtime::valid_digest(name)
                && name.starts_with(selector)
            {
                matching.push(name.to_owned());
            }
        }
        ensure!(
            matching.len() == 1,
            "rollback generation id is missing or ambiguous"
        );
        runtime::read_generation(&root, &matching[0])?
    };
    ensure!(
        runtime::read_generation(&root, &next.digest)? == next,
        "retained rollback metadata does not match its executable"
    );
    if next.digest == active.current.digest {
        println!("alc {} is already active.", next.version);
        return Ok(());
    }
    runtime::write_active(
        &root,
        &ActiveManifest {
            schema: runtime::FORMAT_VERSION,
            current: next.clone(),
            previous: Some(active.current),
        },
    )?;
    println!(
        "Rolled back to retained alc {}. Existing sessions were not changed.",
        next.version
    );
    Ok(())
}

struct RuntimeSelector;
impl RuntimeSelector {
    fn is_short(value: &str) -> bool {
        matches!(
            runtime::RuntimeScope::parse(value),
            Ok(runtime::RuntimeScope::Generation(_))
        )
    }
}

fn print_path_note(front: &Path) {
    let Some(directory) = front.parent() else {
        return;
    };
    let found = env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path)
            .any(|entry| canonical_path(&entry).is_ok_and(|entry| entry == directory))
    });
    if !found {
        println!(
            "note: {} is not currently on PATH; add this directory to PATH to run `alc` without its full path.",
            directory.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_release_tags_with_or_without_v() {
        assert_eq!(
            parse_release_version("v1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
        assert_eq!(
            parse_release_version("1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
    }

    #[test]
    fn parses_checksums_from_common_formats() {
        let checksums = b"abcd  unrelated\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *alc-windows-x86_64.zip\n";
        assert_eq!(
            checksum_for(checksums, "alc-windows-x86_64.zip").unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn rejects_invalid_or_duplicate_checksums() {
        assert!(
            checksum_for(
                b"not-a-hash alc-linux-x86_64.tar.gz",
                "alc-linux-x86_64.tar.gz"
            )
            .is_err()
        );
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(
            checksum_for(
                format!("{hash} alc.zip\n{hash} alc.zip\n").as_bytes(),
                "alc.zip"
            )
            .is_err()
        );
    }

    #[test]
    fn parses_packaged_binary_version() {
        assert_eq!(
            parse_binary_version("alc 0.3.0").unwrap(),
            Version::new(0, 3, 0)
        );
        assert!(parse_binary_version("something 0.3.0").is_err());
        assert!(parse_binary_version("alc 0.3.0\nextra").is_err());
    }

    #[test]
    fn current_platform_has_a_release_asset() {
        let name = platform_asset_name().unwrap();
        assert!(name.starts_with("alc-"));
        assert!(name.ends_with(".zip") || name.ends_with(".tar.gz"));
    }

    #[test]
    fn unknown_offline_schema_has_no_install_effects() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("bundle.json"), br#"{"schema":99,"version":"2.0.1","platform":"unknown","asset":"../../alc","archive_sha256":"","binary_sha256":""}"#).unwrap();
        assert!(read_bundle(temp.path()).is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn tar_rejects_duplicate_payloads_and_unbounded_extended_headers() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for _ in 0..2 {
            let bytes = b"fake executable";
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, "alc", &bytes[..]).unwrap();
        }
        let bytes = tar.into_inner().unwrap().finish().unwrap();
        assert!(
            extract_binary(&bytes, "alc-darwin-aarch64.tar.gz", temp.path())
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );

        let mut header = tar::Header::new_gnu();
        header.set_path("metadata").unwrap();
        header.set_entry_type(tar::EntryType::GNULongName);
        header.set_size(runtime::BINARY_LIMIT);
        header.set_cksum();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(header.as_bytes()).unwrap();
        let bytes = encoder.finish().unwrap();
        assert!(
            validate_tar_headers(&bytes)
                .unwrap_err()
                .to_string()
                .contains("unsupported extended")
        );
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_removes_inherited_runtime_origin() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("alc");
        fs::write(
            &binary,
            b"#!/bin/sh\n[ -z \"${ALC_RUNTIME_ORIGIN:-}\" ] || exit 1\nprintf 'alc 2.0.1\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(packaged_version(&binary).unwrap(), Version::new(2, 0, 1));
    }
}
