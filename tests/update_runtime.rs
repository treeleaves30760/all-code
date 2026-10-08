//! Isolated update/runtime integration tests. Every executable/install/config
//! root belongs to the test; fixtures are native, compile-stamped binaries, not
//! user-installed alc or running user daemons. Local bundles never use network.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn binary_name() -> &'static str {
    if cfg!(windows) { "alc.exe" } else { "alc" }
}

fn platform() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

fn asset_name() -> String {
    format!(
        "alc-{}.{}",
        platform(),
        if cfg!(windows) { "zip" } else { "tar.gz" }
    )
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn file_digest(path: &Path) -> String {
    digest(&fs::read(path).unwrap())
}

struct StampedFixtures {
    _directory: TempDir,
    old: PathBuf,
    next: PathBuf,
    newer: PathBuf,
}

fn fixtures() -> &'static StampedFixtures {
    static FIXTURES: OnceLock<StampedFixtures> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let old = compile_fixture(directory.path(), "old", "1.0.0");
        let next = compile_fixture(directory.path(), "next", "3.0.0");
        let newer = compile_fixture(directory.path(), "newer", "4.0.0");
        StampedFixtures {
            _directory: directory,
            old,
            next,
            newer,
        }
    })
}

fn compile_fixture(root: &Path, stamp: &str, version: &str) -> PathBuf {
    // Source/build tools are never run from an extracted/downloaded directory.
    let scripts = root.join(format!("source-{stamp}"));
    let binaries = root.join(format!("binary-{stamp}"));
    fs::create_dir(&scripts).unwrap();
    fs::create_dir(&binaries).unwrap();
    let source = scripts.join("fixture.rs");
    fs::write(&source, format!(r#"
        const VERSION: &str = "{version}";
        const BUILD: &str = "{stamp}";
        fn main() {{
            let args: Vec<_> = std::env::args().skip(1).collect();
            if args.first().map(String::as_str) == Some("--version") {{
                for key in ["ALC_RUNTIME_ORIGIN", "ALC_RUNTIME_SELECTOR"] {{
                    if std::env::var_os(key).is_some() {{ std::process::exit(23); }}
                }}
                println!("alc {{VERSION}}");
            }} else if args.first().map(String::as_str) == Some("hold") {{
                std::fs::write(&args[1], format!("{{VERSION}} {{BUILD}} {{}}", std::process::id())).unwrap();
                std::thread::sleep(std::time::Duration::from_secs(60));
            }} else {{
                println!("fixture {{VERSION}} {{BUILD}} {{args:?}}");
            }}
        }}
    "#)).unwrap();
    let binary = binaries.join(binary_name());
    let rustc = std::env::var_os("RUSTC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let cargo_home = std::env::var_os("CARGO_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                        .map(|home| PathBuf::from(home).join(".cargo"))
                });
            cargo_home
                .map(|home| {
                    home.join("bin")
                        .join(if cfg!(windows) { "rustc.exe" } else { "rustc" })
                })
                .filter(|path| path.is_file())
                .unwrap_or_else(|| PathBuf::from("rustc"))
        });
    let output = Command::new(rustc)
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .current_dir(&scripts)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

struct Install {
    directory: TempDir,
    front: PathBuf,
    pinned: PathBuf,
    config: PathBuf,
}

impl Install {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let payload_dir = directory.path().join("verified-payload");
        fs::create_dir(&payload_dir).unwrap();
        let payload = payload_dir.join(binary_name());
        fs::copy(env!("CARGO_BIN_EXE_alc"), &payload).unwrap();
        let front = directory.path().join("bin").join(binary_name());
        let config = directory.path().join("config");
        fs::create_dir(&config).unwrap();
        // Installer/update must bypass user config/provider parsing entirely.
        fs::write(
            config.join("config.toml"),
            "deliberately invalid config = [",
        )
        .unwrap();
        let output = command(&payload, &config)
            .arg("__install")
            .arg("--install-to")
            .arg(&front)
            .arg("--version")
            .arg(VERSION)
            .output()
            .unwrap();
        success(&output);
        let hash = file_digest(&payload);
        let pinned = front
            .parent()
            .unwrap()
            .join(".alc/generations")
            .join(hash)
            .join(binary_name());
        assert!(pinned.is_file());
        assert_eq!(version(&front, &config), format!("alc {VERSION}"));
        Self {
            directory,
            front,
            pinned,
            config,
        }
    }

    fn bundle(&self, name: &str, payload: &Path, version: &str) -> PathBuf {
        let directory = self.directory.path().join(name);
        write_bundle(&directory, payload, version);
        directory
    }

    fn active(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.front.parent().unwrap().join(".alc/active.json")).unwrap(),
        )
        .unwrap()
    }

    fn apply(&self, bundle: &Path, force: bool) -> Output {
        let mut command = command(&self.pinned, &self.config);
        command
            .args(["update", "--from"])
            .arg(bundle)
            .arg("--offline");
        if force {
            command.arg("--force");
        }
        command.output().unwrap()
    }
}

fn command(executable: &Path, config: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .env("ALC_CONFIG_DIR", config)
        .env("ALC_UPDATE_API_URL", "http://127.0.0.1:9/no-network")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("ALC_RUNTIME_ORIGIN")
        .env_remove("ALC_RUNTIME_SELECTOR")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdin(Stdio::null());
    command
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn version(executable: &Path, config: &Path) -> String {
    let output = command(executable, config)
        .arg("--version")
        .output()
        .unwrap();
    success(&output);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn archive(payload: &Path) -> Vec<u8> {
    let bytes = fs::read(payload).unwrap();
    if cfg!(windows) {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            binary_name(),
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
        zip.write_all(&bytes).unwrap();
        zip.finish().unwrap().into_inner()
    } else {
        let encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "./alc", &bytes[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }
}

fn write_bundle(directory: &Path, payload: &Path, version: &str) {
    fs::create_dir(directory).unwrap();
    let archive = archive(payload);
    let asset = asset_name();
    let archive_sha256 = digest(&archive);
    let release = json!({ "tag_name": format!("v{version}"), "html_url": "https://example.invalid/release", "assets": [
        { "name": asset, "browser_download_url": "https://example.invalid/archive" },
        { "name": "checksums.txt", "browser_download_url": "https://example.invalid/checksums" }
    ] });
    let manifest = json!({ "schema": 1, "version": version, "platform": platform(), "asset": asset, "archive_sha256": archive_sha256, "binary_sha256": file_digest(payload) });
    fs::write(directory.join(&asset), archive).unwrap();
    fs::write(
        directory.join("checksums.txt"),
        format!("{archive_sha256}  {asset}\n"),
    )
    .unwrap();
    fs::write(
        directory.join("release.json"),
        serde_json::to_vec(&release).unwrap(),
    )
    .unwrap();
    fs::write(
        directory.join("bundle.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn edit_json(path: &Path, edit: impl FnOnce(&mut Value)) {
    let mut json: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    edit(&mut json);
    fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
}

#[test]
fn full_front_dispatches_future_cli_and_pinned_generation_bypasses_active() {
    let install = Install::new();
    let original_front = fs::read(&install.front).unwrap();
    let bundle = install.bundle("next-bundle", &fixtures().next, "3.0.0");
    success(&install.apply(&bundle, false));
    assert_eq!(
        fs::read(&install.front).unwrap(),
        original_front,
        "stable front was rewritten"
    );
    assert_eq!(version(&install.front, &install.config), "alc 3.0.0");
    assert_eq!(
        version(&install.pinned, &install.config),
        format!("alc {VERSION}")
    );
    let output = command(&install.front, &install.config)
        .args(["future-command", "space argument", "--new-option"])
        .env("ALC_RUNTIME_ORIGIN", "/does/not/exist")
        .env("ALC_RUNTIME_SELECTOR", "bogus")
        .output()
        .unwrap();
    success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("future-command")
            && stdout.contains("space argument")
            && stdout.contains("--new-option")
    );
    let active = install.active();
    assert_eq!(active["current"]["digest"], file_digest(&fixtures().next));
    assert_eq!(active["previous"]["digest"], file_digest(&install.pinned));
    assert!(
        !install.config.join("run").exists(),
        "update initialized daemons"
    );
    assert_eq!(
        fs::read_to_string(install.config.join("config.toml")).unwrap(),
        "deliberately invalid config = ["
    );
}

#[test]
fn rollback_retains_generations_and_rechecks_installed_not_running_version() {
    let install = Install::new();
    let next = install.bundle("next", &fixtures().next, "3.0.0");
    let newer = install.bundle("newer", &fixtures().newer, "4.0.0");
    success(&install.apply(&newer, false));
    success(&install.apply(&next, true)); // --force is not an implicit downgrade.
    assert_eq!(install.active()["current"]["version"], "4.0.0");
    let rollback = command(&install.pinned, &install.config)
        .args(["update", "--rollback", "previous", "--offline"])
        .output()
        .unwrap();
    success(&rollback);
    assert_eq!(
        version(&install.front, &install.config),
        format!("alc {VERSION}")
    );
    let newer_hash = file_digest(&fixtures().newer);
    let rollback = command(&install.pinned, &install.config)
        .args(["update", "--rollback"])
        .arg(&newer_hash[..12])
        .output()
        .unwrap();
    success(&rollback);
    assert_eq!(version(&install.front, &install.config), "alc 4.0.0");
    assert!(install.pinned.is_file());
    assert!(
        install
            .front
            .parent()
            .unwrap()
            .join(".alc/generations")
            .join(newer_hash)
            .join(binary_name())
            .is_file()
    );
}

#[test]
fn concurrent_offline_apply_serializes_and_cannot_downgrade_newer_activation() {
    let install = Install::new();
    let next = install.bundle("next", &fixtures().next, "3.0.0");
    let newer = install.bundle("newer", &fixtures().newer, "4.0.0");
    let mut first = command(&install.pinned, &install.config);
    let mut second = command(&install.pinned, &install.config);
    first
        .args(["update", "--from"])
        .arg(next)
        .args(["--offline", "--force"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    second
        .args(["update", "--from"])
        .arg(newer)
        .args(["--offline", "--force"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let first = first.spawn().unwrap();
    let second = second.spawn().unwrap();
    success(&first.wait_with_output().unwrap());
    success(&second.wait_with_output().unwrap());
    assert_eq!(install.active()["current"]["version"], "4.0.0");
    assert_eq!(version(&install.front, &install.config), "alc 4.0.0");
}

struct TestServer {
    address: String,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn new(routes: impl Fn(&str, &str) -> Vec<u8> + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_count = count.clone();
        let worker_stop = stop.clone();
        let base = address.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("test server accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let request = read_request(&mut stream);
                if request.is_empty() {
                    continue; // A connector may discard its first address probe.
                }
                worker_count.fetch_add(1, Ordering::SeqCst);
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let body = routes(path, &base);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                let _ = stream.write_all(&body);
            }
        });
        Self {
            address,
            count,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Err(error) = self.worker.take().unwrap().join()
            && !thread::panicking()
        {
            std::panic::resume_unwind(error);
        }
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    // Accepted macOS sockets inherit O_NONBLOCK from the listener; the read
    // timeout must wait for request bytes instead of closing a real connection.
    stream.set_nonblocking(false).unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    while bytes.len() < 8192 && !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
        let size = stream.read(&mut buffer).unwrap_or(0);
        if size == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..size]);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn offline_rejects_unknown_corrupt_wrong_target_and_oversized_metadata_without_effects() {
    let install = Install::new();
    let network = TestServer::new(|_, _| panic!("offline update attempted a network request"));
    let before_active = install.active();
    let before_front = fs::read(&install.front).unwrap();
    let scenarios = [
        "unknown-schema",
        "unknown-field",
        "wrong-platform",
        "archive-corrupt",
        "wrong-version",
        "bad-digest",
        "oversized",
    ];
    for scenario in scenarios {
        let bundle = install.bundle(scenario, &fixtures().next, "3.0.0");
        match scenario {
            "unknown-schema" => edit_json(&bundle.join("bundle.json"), |json| {
                json["schema"] = json!(999)
            }),
            "unknown-field" => edit_json(&bundle.join("bundle.json"), |json| {
                json["redirect"] = json!("../")
            }),
            "wrong-platform" => edit_json(&bundle.join("bundle.json"), |json| {
                json["platform"] = json!("not-this-platform")
            }),
            "archive-corrupt" => fs::write(bundle.join(asset_name()), b"corrupt").unwrap(),
            "wrong-version" => {
                edit_json(&bundle.join("bundle.json"), |json| {
                    json["version"] = json!("5.0.0")
                });
                edit_json(&bundle.join("release.json"), |json| {
                    json["tag_name"] = json!("v5.0.0")
                });
            }
            "bad-digest" => edit_json(&bundle.join("bundle.json"), |json| {
                json["binary_sha256"] = json!("0".repeat(64))
            }),
            "oversized" => {
                fs::write(bundle.join("bundle.json"), vec![b' '; 64 * 1024 + 1]).unwrap()
            }
            _ => unreachable!(),
        }
        let output = command(&install.pinned, &install.config)
            .args(["update", "--from"])
            .arg(bundle)
            .arg("--offline")
            .env("ALC_UPDATE_API_URL", format!("{}/latest", network.address))
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {scenario}");
        assert_eq!(
            install.active(),
            before_active,
            "{scenario} changed activation"
        );
        assert_eq!(
            fs::read(&install.front).unwrap(),
            before_front,
            "{scenario} rewrote stable front"
        );
        assert!(
            !install
                .front
                .parent()
                .unwrap()
                .join(".alc/generations")
                .join(file_digest(&fixtures().next))
                .exists(),
            "{scenario} published an unverified payload"
        );
    }
    assert_eq!(network.count.load(Ordering::SeqCst), 0);
}

#[test]
fn download_only_persists_verified_bundle_and_offline_reuses_it_without_network() {
    let install = Install::new();
    let bytes = archive(&fixtures().next);
    let asset = asset_name();
    let checksum = digest(&bytes);
    let server = TestServer::new(move |path, base| {
        match path {
        "/latest" => serde_json::to_vec(&json!({ "tag_name": "v3.0.0", "html_url": "https://example.invalid/release", "assets": [
            { "name": asset, "browser_download_url": format!("{base}/archive") },
            { "name": "checksums.txt", "browser_download_url": format!("{base}/checksums") }
        ] })).unwrap(),
        "/checksums" => format!("{checksum}  {asset}\n").into_bytes(),
        "/archive" => bytes.clone(),
        _ => panic!("unexpected updater path {path}"),
    }
    });
    let before = install.active();
    let bundle = install.directory.path().join("download-only");
    let output = command(&install.pinned, &install.config)
        .args(["update", "--download-only"])
        .arg(&bundle)
        .env("ALC_UPDATE_API_URL", format!("{}/latest", server.address))
        .output()
        .unwrap();
    success(&output);
    assert_eq!(install.active(), before);
    assert!(bundle.join("bundle.json").is_file());
    assert!(bundle.join("release.json").is_file());
    assert!(bundle.join("checksums.txt").is_file());
    assert!(bundle.join(asset_name()).is_file());
    assert_eq!(server.count.load(Ordering::SeqCst), 3);
    success(&install.apply(&bundle, false));
    assert_eq!(version(&install.front, &install.config), "alc 3.0.0");
    assert_eq!(server.count.load(Ordering::SeqCst), 3);
    // Persistent bundles are retained after activation for transfer/retry.
    assert!(bundle.join(asset_name()).is_file());
}

#[test]
fn interrupted_initial_activation_recovers_marker_and_preserves_rollback_metadata() {
    let install = Install::new();
    // Publish the old fixture directly through an isolated bootstrap first.
    let old_root = install.directory.path().join("old-front");
    fs::create_dir(&old_root).unwrap();
    let old_front = old_root.join(binary_name());
    fs::copy(&fixtures().old, &old_front).unwrap();
    let output = command(Path::new(env!("CARGO_BIN_EXE_alc")), &install.config)
        .arg("__install")
        .arg("--install-to")
        .arg(&old_front)
        .arg("--version")
        .arg(VERSION)
        .output()
        .unwrap();
    success(&output);
    let root = old_root.join(".alc");
    let activation = fs::read(root.join("active.json")).unwrap();
    fs::write(root.join("activation.json"), &activation).unwrap();
    fs::remove_file(root.join("active.json")).unwrap();
    // This is the crash window after front/front.json publication but before
    // the very first active.json rename. A retry must finish, not early-return.
    let output = command(Path::new(env!("CARGO_BIN_EXE_alc")), &install.config)
        .arg("__install")
        .arg("--install-to")
        .arg(&old_front)
        .arg("--version")
        .arg(VERSION)
        .output()
        .unwrap();
    success(&output);
    assert!(!root.join("activation.json").exists());
    let recovered: Value =
        serde_json::from_slice(&fs::read(root.join("active.json")).unwrap()).unwrap();
    assert_eq!(recovered["current"]["version"], VERSION);
    assert_eq!(recovered["previous"]["version"], "1.0.0");
}

#[test]
fn online_same_version_retry_finishes_initial_activation_without_downloading() {
    let install = Install::new();
    let root = install.front.parent().unwrap().join(".alc");
    let activation = fs::read(root.join("active.json")).unwrap();
    fs::write(root.join("activation.json"), &activation).unwrap();
    fs::remove_file(root.join("active.json")).unwrap();
    let server = TestServer::new(|path, _| {
        assert_eq!(path, "/latest", "recovery must not download a payload");
        serde_json::to_vec(&json!({
            "tag_name": format!("v{VERSION}"),
            "html_url": "https://example.invalid/release",
            "assets": []
        }))
        .unwrap()
    });
    let output = command(&install.front, &install.config)
        .arg("update")
        .env("ALC_UPDATE_API_URL", format!("{}/latest", server.address))
        .output()
        .unwrap();
    success(&output);
    assert_eq!(fs::read(root.join("active.json")).unwrap(), activation);
    assert!(!root.join("activation.json").exists());
    assert_eq!(server.count.load(Ordering::SeqCst), 1);
}

#[test]
fn rollback_cancels_interrupted_activation_before_changing_active() {
    let install = Install::new();
    let next = install.bundle("next", &fixtures().next, "3.0.0");
    let newer = install.bundle("newer", &fixtures().newer, "4.0.0");
    success(&install.apply(&next, false));
    let previous = install.active();
    success(&install.apply(&newer, false));
    let root = install.front.parent().unwrap().join(".alc");
    // Crash immediately before the newer active.json rename: both generations
    // are verified, but active still selects next and marker proposes newer.
    fs::copy(root.join("active.json"), root.join("activation.json")).unwrap();
    fs::write(
        root.join("active.json"),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();
    fs::write(root.join("pending.json"), b"{}").unwrap();
    let output = command(&install.pinned, &install.config)
        .args(["update", "--rollback", "previous", "--offline"])
        .output()
        .unwrap();
    success(&output);
    assert_eq!(install.active()["current"]["version"], VERSION);
    assert!(!root.join("activation.json").exists());
    assert!(!root.join("pending.json").exists());
    success(&install.apply(&newer, false));
    assert_eq!(install.active()["current"]["version"], "4.0.0");
}

#[test]
fn offline_same_version_retry_completes_pending_activation_before_noop() {
    let install = Install::new();
    let bundle = install.bundle("same", &install.pinned, VERSION);
    let root = install.front.parent().unwrap().join(".alc");
    let activation = fs::read(root.join("active.json")).unwrap();
    fs::write(root.join("activation.json"), &activation).unwrap();
    fs::remove_file(root.join("active.json")).unwrap();
    success(&install.apply(&bundle, false));
    assert_eq!(fs::read(root.join("active.json")).unwrap(), activation);
    assert!(!root.join("activation.json").exists());
}

#[test]
fn damaged_generation_and_unknown_active_manifest_fail_closed() {
    let install = Install::new();
    let active_path = install.front.parent().unwrap().join(".alc/active.json");
    let saved = fs::read(&active_path).unwrap();
    edit_json(&active_path, |json| {
        json["current"]["version"] = json!("99.0.0")
    });
    let output = command(&install.front, &install.config)
        .arg("--version")
        .output()
        .unwrap();
    assert!(!output.status.success());
    fs::write(&active_path, &saved).unwrap();
    edit_json(&active_path, |json| json["schema"] = json!(999));
    assert!(
        !command(&install.front, &install.config)
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::write(&active_path, saved).unwrap();
    fs::write(&install.pinned, b"damaged immutable binary").unwrap();
    assert!(
        !command(&install.front, &install.config)
            .arg("--version")
            .output()
            .unwrap()
            .status
            .success()
    );
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn await_ready(path: &Path, child: &mut OwnedChild) {
    let start = Instant::now();
    while !path.is_file() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "fixture exited before ready"
        );
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "fixture did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn activation_does_not_stop_loaded_front_or_previous_payload() {
    let install = Install::new();
    let next = install.bundle("next", &fixtures().next, "3.0.0");
    let newer = install.bundle("newer", &fixtures().newer, "4.0.0");
    success(&install.apply(&next, false));
    let ready = install.directory.path().join("holding-ready");
    let mut holding = OwnedChild(
        command(&install.front, &install.config)
            .arg("hold")
            .arg(&ready)
            .spawn()
            .unwrap(),
    );
    await_ready(&ready, &mut holding);
    let original_front = file_digest(&install.front);
    success(&install.apply(&newer, false));
    assert!(
        holding.0.try_wait().unwrap().is_none(),
        "update stopped a loaded executable"
    );
    assert_eq!(file_digest(&install.front), original_front);
    assert_eq!(version(&install.front, &install.config), "alc 4.0.0");
    assert!(
        install
            .front
            .parent()
            .unwrap()
            .join(".alc/generations")
            .join(file_digest(&fixtures().next))
            .join(binary_name())
            .is_file()
    );
}

#[test]
fn old_front_bootstrap_retains_old_content_and_uses_atomic_publication() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let front = bin.join(binary_name());
    fs::copy(&fixtures().old, &front).unwrap();
    let old_digest = file_digest(&front);
    let config = directory.path().join("config");
    let output = command(Path::new(env!("CARGO_BIN_EXE_alc")), &config)
        .arg("__install")
        .arg("--install-to")
        .arg(&front)
        .arg("--version")
        .arg(VERSION)
        .output()
        .unwrap();
    success(&output);
    assert_eq!(version(&front, &config), format!("alc {VERSION}"));
    assert_eq!(
        file_digest(&front),
        file_digest(Path::new(env!("CARGO_BIN_EXE_alc")))
    );
    assert!(
        bin.join(".alc/generations")
            .join(old_digest)
            .join(binary_name())
            .is_file()
    );
    assert!(
        !config.exists(),
        "hidden installer loaded/wrote user config"
    );
}

#[cfg(windows)]
#[test]
fn locked_legacy_windows_front_reports_pending_preserves_payload_and_retries_synchronously() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let front = bin.join(binary_name());
    fs::copy(&fixtures().old, &front).unwrap();
    let original = file_digest(&front);
    let config = directory.path().join("config");
    let ready = directory.path().join("ready");
    let mut holding = OwnedChild(
        command(&front, &config)
            .arg("hold")
            .arg(&ready)
            .spawn()
            .unwrap(),
    );
    await_ready(&ready, &mut holding);
    let output = command(Path::new(env!("CARGO_BIN_EXE_alc")), &config)
        .arg("__install")
        .arg("--install-to")
        .arg(&front)
        .arg("--version")
        .arg(VERSION)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "loaded old Windows front must not claim completed installation"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("pending"));
    assert_eq!(file_digest(&front), original);
    assert!(holding.0.try_wait().unwrap().is_none());
    assert!(!bin.join(".alc/active.json").exists());
    assert!(bin.join(".alc/pending.json").is_file());
    let preserved = bin
        .join(".alc/generations")
        .join(file_digest(Path::new(env!("CARGO_BIN_EXE_alc"))))
        .join(binary_name());
    assert!(preserved.is_file());
    drop(holding); // only our own fixture process, never user processes.
    let output = command(&preserved, &config)
        .arg("__install")
        .arg("--install-to")
        .arg(&front)
        .arg("--version")
        .arg(VERSION)
        .output()
        .unwrap();
    success(&output);
    assert_eq!(version(&front, &config), format!("alc {VERSION}"));
    assert!(!bin.join(".alc/pending.json").exists());
}
