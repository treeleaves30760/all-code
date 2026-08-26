use std::env;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/treeleaves30760/all-code/releases/latest";
const API_DOWNLOAD_LIMIT: u64 = 4 * 1024 * 1024;
const CHECKSUM_DOWNLOAD_LIMIT: u64 = 1024 * 1024;
const ARCHIVE_DOWNLOAD_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug)]
struct ExtractedBinaries {
    alc: PathBuf,
    helper: PathBuf,
}

pub fn run(check_only: bool, force: bool) -> Result<u8> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("the installed alc version is not valid semantic versioning")?;
    let release = fetch_release()?;
    let latest = parse_release_version(&release.tag_name)?;

    if check_only {
        if latest > current {
            println!(
                "Update available: alc {current} -> {latest}\nRun `alc update` to install it.\n{}",
                release.html_url
            );
            return Ok(0);
        }
        if latest == current {
            println!("alc {current} is up to date.");
        } else {
            println!("alc {current} is newer than the latest release ({latest}).");
        }
        return Ok(0);
    }

    if latest < current {
        println!("alc {current} is newer than the latest release ({latest}); nothing to do.");
        return Ok(0);
    }
    if latest == current && !force {
        println!("alc {current} is already up to date.");
        return Ok(0);
    }

    let asset_name = platform_asset_name()?;
    let archive_asset = find_asset(&release, &asset_name)?;
    let checksum_asset = find_asset(&release, "checksums.txt")?;

    println!("Downloading alc {latest} for {}...", platform_label());
    let checksums = download(
        &checksum_asset.browser_download_url,
        CHECKSUM_DOWNLOAD_LIMIT,
    )
    .context("could not download release checksums")?;
    let expected = checksum_for(&checksums, &asset_name)?;
    let archive = download(&archive_asset.browser_download_url, ARCHIVE_DOWNLOAD_LIMIT)
        .with_context(|| format!("could not download {asset_name}"))?;
    verify_checksum(&archive, &expected, &asset_name)?;
    println!("Verified {asset_name} (SHA-256).");

    let temp = tempfile::tempdir().context("could not create a temporary update directory")?;
    let extracted = extract_binaries(&archive, &asset_name, temp.path())?;
    let packaged = packaged_version(&extracted.alc)?;
    ensure!(
        packaged == latest,
        "release metadata says {latest}, but the downloaded binary is {packaged}"
    );

    let current_exe = env::current_exe().context("could not locate the running alc executable")?;
    install(&current_exe, &extracted, &current, &latest)?;
    print_path_note(&current_exe);
    Ok(0)
}

fn fetch_release() -> Result<Release> {
    let url = release_api_url();
    let bytes = download(&url, API_DOWNLOAD_LIMIT)
        .with_context(|| format!("could not query the latest alc release at {url}"))?;
    serde_json::from_slice(&bytes).context("GitHub returned invalid release metadata")
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
    let os = match env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        other => bail!("self-update is not supported on operating system '{other}'"),
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("self-update is not supported on CPU architecture '{other}'"),
    };
    let extension = if os == "windows" { "zip" } else { "tar.gz" };
    Ok(format!("alc-{os}-{arch}.{extension}"))
}

fn platform_label() -> String {
    format!("{} / {}", env::consts::OS, env::consts::ARCH)
}

fn find_asset<'a>(release: &'a Release, name: &str) -> Result<&'a ReleaseAsset> {
    release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .with_context(|| format!("release {} does not contain {name}", release.tag_name))
}

fn checksum_for(contents: &[u8], asset_name: &str) -> Result<String> {
    let text = std::str::from_utf8(contents).context("checksums.txt is not valid UTF-8")?;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        if name.trim_start_matches('*') == asset_name {
            ensure!(
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "the published checksum for {asset_name} is invalid"
            );
            return Ok(hash.to_ascii_lowercase());
        }
    }
    bail!("checksums.txt does not contain {asset_name}")
}

fn verify_checksum(bytes: &[u8], expected: &str, asset_name: &str) -> Result<()> {
    let digest = Sha256::digest(bytes);
    let mut actual = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut actual, "{byte:02x}").expect("writing to a String cannot fail");
    }
    ensure!(
        actual == expected,
        "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
    );
    Ok(())
}

fn extract_binaries(
    archive: &[u8],
    asset_name: &str,
    destination: &Path,
) -> Result<ExtractedBinaries> {
    let suffix = if asset_name.ends_with(".zip") {
        ".exe"
    } else {
        ""
    };
    let alc = destination.join(format!("alc{suffix}"));
    let helper = destination.join(format!("claude-codex{suffix}"));

    if asset_name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive))
            .context("the downloaded release is not a valid ZIP archive")?;
        extract_zip_file(&mut zip, &format!("alc{suffix}"), &alc)?;
        extract_zip_file(&mut zip, &format!("claude-codex{suffix}"), &helper)?;
    } else {
        extract_tar_files(archive, &alc, &helper)?;
    }

    ensure!(
        alc.is_file(),
        "release archive does not contain alc{suffix}"
    );
    ensure!(
        helper.is_file(),
        "release archive does not contain claude-codex{suffix}"
    );
    make_executable(&alc)?;
    make_executable(&helper)?;
    Ok(ExtractedBinaries { alc, helper })
}

fn extract_zip_file(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    destination: &Path,
) -> Result<()> {
    let mut source = archive
        .by_name(name)
        .with_context(|| format!("release ZIP does not contain {name}"))?;
    ensure!(source.is_file(), "release ZIP entry {name} is not a file");
    let mut output = File::create(destination)
        .with_context(|| format!("could not create {}", destination.display()))?;
    io::copy(&mut source, &mut output).with_context(|| format!("could not extract {name}"))?;
    output
        .sync_all()
        .with_context(|| format!("could not flush {}", destination.display()))?;
    Ok(())
}

fn extract_tar_files(archive: &[u8], alc: &Path, helper: &Path) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);
    let mut found_alc = false;
    let mut found_helper = false;

    for entry in tar
        .entries()
        .context("the downloaded release is not a valid tar archive")?
    {
        let mut entry = entry.context("could not read a release archive entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .context("release archive contains an invalid path")?;
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        let destination = match name.as_str() {
            "alc" if !found_alc => {
                found_alc = true;
                alc
            }
            "claude-codex" if !found_helper => {
                found_helper = true;
                helper
            }
            _ => continue,
        };
        let mut output = File::create(destination)
            .with_context(|| format!("could not create {}", destination.display()))?;
        io::copy(&mut entry, &mut output).with_context(|| format!("could not extract {name}"))?;
        output
            .sync_all()
            .with_context(|| format!("could not flush {}", destination.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("could not mark {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn packaged_version(binary: &Path) -> Result<Version> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("could not run downloaded binary {}", binary.display()))?;
    ensure!(
        output.status.success(),
        "downloaded alc binary failed its version check"
    );
    let stdout =
        String::from_utf8(output.stdout).context("alc --version returned invalid UTF-8")?;
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

fn install(
    current_exe: &Path,
    extracted: &ExtractedBinaries,
    current: &Version,
    latest: &Version,
) -> Result<()> {
    let install_dir = current_exe
        .parent()
        .context("the running alc executable has no parent directory")?;
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let executable_suffix = env::consts::EXE_SUFFIX;
    let staged_alc = install_dir.join(format!(".alc-update-{nonce}{executable_suffix}"));
    let staged_helper =
        install_dir.join(format!(".claude-codex-update-{nonce}{executable_suffix}"));
    copy_new(&extracted.alc, &staged_alc)?;
    if let Err(error) = copy_new(&extracted.helper, &staged_helper) {
        let _ = fs::remove_file(&staged_alc);
        return Err(error);
    }
    if let Err(error) = make_executable(&staged_alc).and_then(|()| make_executable(&staged_helper))
    {
        let _ = fs::remove_file(&staged_alc);
        let _ = fs::remove_file(&staged_helper);
        return Err(error);
    }

    let helper_target = install_dir.join(format!("claude-codex{executable_suffix}"));
    let result = install_platform(
        current_exe,
        &helper_target,
        &staged_alc,
        &staged_helper,
        &nonce,
        current,
        latest,
    );
    if result.is_err() {
        let _ = fs::remove_file(&staged_alc);
        let _ = fs::remove_file(&staged_helper);
    }
    result
}

fn copy_new(source: &Path, destination: &Path) -> Result<()> {
    let mut input =
        File::open(source).with_context(|| format!("could not open {}", source.display()))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .with_context(|| {
            format!(
                "cannot stage the update in {}; check directory permissions",
                destination.parent().unwrap_or(destination).display()
            )
        })?;
    io::copy(&mut input, &mut output)
        .with_context(|| format!("could not stage {}", destination.display()))?;
    output
        .sync_all()
        .with_context(|| format!("could not flush {}", destination.display()))?;
    Ok(())
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn install_platform(
    current_exe: &Path,
    helper_target: &Path,
    staged_alc: &Path,
    staged_helper: &Path,
    nonce: &str,
    current: &Version,
    latest: &Version,
) -> Result<()> {
    let helper_backup = helper_target.with_file_name(format!(".claude-codex-backup-{nonce}"));
    let had_helper = helper_target.exists();
    if had_helper {
        fs::rename(helper_target, &helper_backup)
            .with_context(|| format!("could not back up {}", helper_target.display()))?;
    }

    if let Err(error) = fs::rename(staged_helper, helper_target) {
        if had_helper {
            let _ = fs::rename(&helper_backup, helper_target);
        }
        let _ = fs::remove_file(staged_alc);
        return Err(error).context("could not install the updated claude-codex helper");
    }

    if let Err(error) = fs::rename(staged_alc, current_exe) {
        let _ = fs::remove_file(helper_target);
        if had_helper {
            let _ = fs::rename(&helper_backup, helper_target);
        }
        let _ = fs::remove_file(staged_alc);
        return Err(error).with_context(|| {
            format!(
                "could not replace {}; use the one-line installer if this location is managed by an administrator",
                current_exe.display()
            )
        });
    }

    if had_helper {
        let _ = fs::remove_file(helper_backup);
    }
    println!("Updated alc {current} -> {latest}.");
    Ok(())
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn install_platform(
    current_exe: &Path,
    helper_target: &Path,
    staged_alc: &Path,
    staged_helper: &Path,
    nonce: &str,
    current: &Version,
    latest: &Version,
) -> Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SCRIPT: &str = r#"param(
    [int]$ParentProcessId,
    [string]$CurrentAlc,
    [string]$CurrentHelper,
    [string]$StagedAlc,
    [string]$StagedHelper,
    [string]$AlcBackup,
    [string]$HelperBackup,
    [string]$LogPath,
    [string]$ScriptPath
)
$ErrorActionPreference = 'Stop'
$alcInstalled = $false
$helperInstalled = $false
try {
    Wait-Process -Id $ParentProcessId -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 150
    if (Test-Path -LiteralPath $CurrentAlc) {
        Move-Item -LiteralPath $CurrentAlc -Destination $AlcBackup
    }
    if (Test-Path -LiteralPath $CurrentHelper) {
        Move-Item -LiteralPath $CurrentHelper -Destination $HelperBackup
    }
    Move-Item -LiteralPath $StagedHelper -Destination $CurrentHelper
    $helperInstalled = $true
    Move-Item -LiteralPath $StagedAlc -Destination $CurrentAlc
    $alcInstalled = $true
    Remove-Item -Force -LiteralPath $AlcBackup, $HelperBackup -ErrorAction SilentlyContinue
    Remove-Item -Force -LiteralPath $LogPath -ErrorAction SilentlyContinue
} catch {
    $failure = $_ | Out-String
    if ($alcInstalled) {
        Remove-Item -Force -LiteralPath $CurrentAlc -ErrorAction SilentlyContinue
    }
    if ($helperInstalled) {
        Remove-Item -Force -LiteralPath $CurrentHelper -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $AlcBackup) {
        Move-Item -Force -LiteralPath $AlcBackup -Destination $CurrentAlc -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $HelperBackup) {
        Move-Item -Force -LiteralPath $HelperBackup -Destination $CurrentHelper -ErrorAction SilentlyContinue
    }
    Set-Content -LiteralPath $LogPath -Value $failure
} finally {
    Remove-Item -Force -LiteralPath $StagedAlc, $StagedHelper -ErrorAction SilentlyContinue
    Remove-Item -Force -LiteralPath $ScriptPath -ErrorAction SilentlyContinue
}
"#;

    let install_dir = current_exe
        .parent()
        .context("the running alc executable has no parent directory")?;
    let script_path = install_dir.join(format!(".alc-update-{nonce}.ps1"));
    let alc_backup = install_dir.join(format!(".alc-backup-{nonce}.exe"));
    let helper_backup = install_dir.join(format!(".claude-codex-backup-{nonce}.exe"));
    let log_path = install_dir.join("alc-update.log");
    fs::write(&script_path, SCRIPT)
        .with_context(|| format!("could not create {}", script_path.display()))?;

    let spawn = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ])
        .arg(&script_path)
        .arg(std::process::id().to_string())
        .arg(current_exe)
        .arg(helper_target)
        .arg(staged_alc)
        .arg(staged_helper)
        .arg(&alc_backup)
        .arg(&helper_backup)
        .arg(&log_path)
        .arg(&script_path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    if let Err(error) = spawn {
        let _ = fs::remove_file(&script_path);
        let _ = fs::remove_file(staged_alc);
        let _ = fs::remove_file(staged_helper);
        return Err(error).context("could not start the Windows update finalizer");
    }

    println!("Verified alc {latest}. The Windows update will finish after this command exits.");
    println!("Run `alc --version` again in a moment (previous version: {current}).");
    println!(
        "If replacement fails, details will be written to {}.",
        log_path.display()
    );
    Ok(())
}

fn print_path_note(current_exe: &Path) {
    let Some(directory) = current_exe.parent() else {
        return;
    };
    if path_contains(directory) {
        return;
    }
    eprintln!(
        "note: {} is not currently on PATH; add this directory to PATH to run `alc` without its full path.",
        directory.display()
    );
}

fn path_contains(directory: &Path) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|entry| paths_equal(&entry, directory))
}

#[cfg(windows)]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .eq_ignore_ascii_case(right.to_string_lossy().trim_end_matches(['\\', '/']))
}

#[cfg(not(windows))]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
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
    fn rejects_malformed_checksum() {
        let error = checksum_for(
            b"not-a-hash  alc-linux-x86_64.tar.gz",
            "alc-linux-x86_64.tar.gz",
        )
        .unwrap_err();
        assert!(error.to_string().contains("checksum"));
    }

    #[test]
    fn parses_packaged_binary_version() {
        assert_eq!(
            parse_binary_version("alc 0.3.0").unwrap(),
            Version::new(0, 3, 0)
        );
        assert!(parse_binary_version("something 0.3.0").is_err());
    }

    #[test]
    fn current_platform_has_a_release_asset() {
        let name = platform_asset_name().unwrap();
        assert!(name.starts_with("alc-"));
        assert!(name.ends_with(".zip") || name.ends_with(".tar.gz"));
    }
}
