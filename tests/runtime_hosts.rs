//! Runtime ownership is exercised against isolated roots and fake agents only.
//! Legacy uses its original paths and protocol; unscoped new launches use the
//! executable's generation, never an active owner's namespace.

use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::io::{BufRead, BufReader};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TIMEOUT: Duration = Duration::from_secs(45);

fn root() -> tempfile::TempDir {
    // macOS's ordinary temporary root leaves no room for a unix socket name.
    #[cfg(unix)]
    return tempfile::Builder::new()
        .prefix("alch-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    tempfile::Builder::new().prefix("alch-").tempdir().unwrap()
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alc"))
}

fn generation() -> String {
    generation_for(&binary())
}

fn generation_for(binary: &Path) -> String {
    let digest = Sha256::digest(fs::read(binary).unwrap());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn command(config: &Path, runtime: Option<&str>) -> Command {
    command_at(&binary(), config, runtime)
}

fn command_at(binary: &Path, config: &Path, runtime: Option<&str>) -> Command {
    let mut command = Command::new(binary);
    command.arg("--config-dir").arg(config);
    if let Some(runtime) = runtime {
        command.args(["--runtime", runtime]);
    }
    command
        .env("CLAUDE_CONFIG_DIR", config.join("claude-history"))
        .env("CODEX_HOME", config.join("codex-history"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ALC_REMOTE_CTL")
        .env_remove("ALC_REMOTE_OPERATOR")
        .env_remove("ALC_REMOTE_VIEWER")
        .timeout(TIMEOUT);
    command
}

fn initialize(config: &Path) {
    command(config, None)
        .args(["config", "init"])
        .assert()
        .success();
    fs::write(config.join("remote.toml"), "port = 0\n").unwrap();
}

struct StopHosts {
    config: PathBuf,
    runtime: String,
}

impl StopHosts {
    fn new(config: &Path, runtime: &str) -> Self {
        Self {
            config: config.to_owned(),
            runtime: runtime.to_owned(),
        }
    }
}

impl Drop for StopHosts {
    fn drop(&mut self) {
        let _ = command(&self.config, Some(&self.runtime))
            .args(["hub", "stop", "--drain"])
            .output();
        let _ = command(&self.config, Some(&self.runtime))
            .args(["bridge", "stop"])
            .output();
    }
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    files
}

fn fake_claude(work: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = work.join("claude");
        fs::write(&path, "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> \"$ALC_FAKE_ARGS\"; done\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
    #[cfg(windows)]
    {
        let path = work.join("claude.cmd");
        fs::write(&path, "@echo off\r\nsetlocal\r\n:next\r\nif \"%~1\"==\"\" goto done\r\n>>\"%ALC_FAKE_ARGS%\" echo(%~1\r\nshift\r\ngoto next\r\n:done\r\nexit /b 0\r\n").unwrap();
        path
    }
}

fn settings_from(args: &Path) -> (PathBuf, Value) {
    let args = fs::read_to_string(args).unwrap();
    let lines: Vec<_> = args.lines().collect();
    let index = lines.iter().position(|arg| *arg == "--settings").unwrap();
    let path = PathBuf::from(lines[index + 1]);
    let settings = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    (path, settings)
}

fn helper(config: &Path, work: &Path, line: &str) -> String {
    #[cfg(unix)]
    let mut command = Command::new("/bin/sh");
    #[cfg(unix)]
    command.args(["-c", line]);
    #[cfg(windows)]
    let mut command = Command::new("cmd");
    #[cfg(windows)]
    command.args(["/D", "/S", "/C", line]);
    let output = command
        .current_dir(work)
        .env("CLAUDE_CONFIG_DIR", config.join("claude-history"))
        .env("CODEX_HOME", config.join("codex-history"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_BASE_URL")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .timeout(TIMEOUT)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).unwrap().trim().to_owned()
}

fn read_http(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    let mut expected = None;
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            // A connect-only liveness probe is not an incomplete HTTP request.
            // Bound its idle socket, but do not panic the fake server on it.
            Err(error)
                if bytes.is_empty()
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
            {
                return String::new();
            }
            Err(error) => panic!("incomplete fake HTTP request: {error}"),
        };
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        if expected.is_none()
            && let Some(head) = bytes.windows(4).position(|part| part == b"\r\n\r\n")
        {
            let header = String::from_utf8_lossy(&bytes[..head]);
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            expected = Some(head + 4 + length);
        }
        if expected.is_some_and(|length| bytes.len() >= length) {
            break;
        }
        assert!(bytes.len() < 2 * 1024 * 1024);
    }
    String::from_utf8(bytes).unwrap()
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    stream.flush().unwrap();
}

struct Upstream {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Upstream {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/prefix", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&requests);
        let halt = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !halt.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(peer) => peer,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("fake upstream accept: {error}"),
                };
                let request = read_http(&mut stream);
                if request.is_empty() {
                    continue;
                }
                let index = seen.lock().unwrap().len();
                seen.lock().unwrap().push(request);
                let start = format!(
                    "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_generation_{index}\",\"model\":\"test-model\",\"usage\":{{\"input_tokens\":10,\"cache_read_input_tokens\":2,\"cache_creation_input_tokens\":0,\"output_tokens\":0}}}}}}\n\n"
                );
                let content = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n";
                let end = "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":4}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
                let length = start.len() + content.len() + end.len();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{start}").unwrap();
                stream.flush().unwrap();
                thread::sleep(Duration::from_millis(40));
                stream.write_all(content.as_bytes()).unwrap();
                stream.flush().unwrap();
                thread::sleep(Duration::from_millis(40));
                stream.write_all(end.as_bytes()).unwrap();
                stream.flush().unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn profile(config: &Path, url: &str, key: &str) {
    command(config, None)
        .args([
            "config",
            "upsert",
            "runtime-test",
            "--kind",
            "custom",
            "--model",
            "test-model",
            "--auth",
            "bearer",
            "--protocol",
            "anthropic-messages",
            "--base-url",
            url,
            "--anthropic-base-url",
            url,
        ])
        .assert()
        .success();
    command(config, None)
        .args(["config", "key", "runtime-test", "--stdin"])
        .write_stdin(key)
        .assert()
        .success();
}

fn launch_metrics(config: &Path, work: &Path, runtime: Option<&str>) -> (PathBuf, Value) {
    let args = work.join("args.txt");
    let _ = fs::remove_file(&args);
    command(config, runtime)
        .env("ALC_CLAUDE_BIN", fake_claude(work))
        .env("ALC_FAKE_ARGS", &args)
        .args([
            "--metrics",
            "--provider",
            "runtime-test",
            "--no-share",
            "claude",
        ])
        .assert()
        .success();
    settings_from(&args)
}

fn model_request(base: &str, credential: &str) -> String {
    let url = reqwest::Url::parse(base).unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = r#"{"model":"test-model","messages":[{"role":"user","content":"private-runtime-prompt"}],"stream":true}"#;
    write!(stream, "POST {}/v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {credential}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", url.path(), url.port().unwrap(), body.len()).unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn dry_run_names_pinned_generation_helper_without_creating_runtime_state() {
    let config = root();
    initialize(config.path());
    profile(config.path(), "https://example.test", "dry-run-test-key");
    let before = tree(config.path());
    let output = command(config.path(), None)
        .args([
            "--dry-run",
            "--provider",
            "runtime-test",
            "--no-share",
            "claude",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains(&format!("--runtime {}", generation())),
        "{text}"
    );
    assert!(text.contains(".alc/generations/"), "{text}");
    assert!(!text.contains("dry-run-test-key"));
    assert_eq!(before, tree(config.path()));
    assert!(!config.path().join("run").exists());
    assert!(!config.path().join("runtime").exists());
}

#[test]
fn legacy_and_generation_hosts_coexist_without_rewriting_legacy_state() {
    let config = root();
    let work = root();
    initialize(config.path());
    let upstream = Upstream::start();
    profile(config.path(), &upstream.url, "runtime-first-key");
    let id = generation();
    let _old = StopHosts::new(config.path(), "legacy");
    let _new = StopHosts::new(config.path(), &id);
    command(config.path(), Some("legacy"))
        .args(["hub", "start"])
        .assert()
        .success();
    let (old_path, old_settings) = launch_metrics(config.path(), work.path(), Some("legacy"));
    let old_helper = old_settings["apiKeyHelper"].as_str().unwrap();
    assert!(!old_helper.contains("--runtime"));
    let old_credential = helper(config.path(), work.path(), old_helper);
    let old_bytes = fs::read(&old_path).unwrap();
    let legacy_state = tree(&config.path().join("run"));
    let credentials = fs::read(config.path().join("credentials.toml")).unwrap();

    command(config.path(), None)
        .args(["hub", "start"])
        .assert()
        .success();
    let (new_path, new_settings) = launch_metrics(config.path(), work.path(), None);
    assert!(new_path.starts_with(config.path().join("run/g").join(&id).join("claude")));
    let new_helper = new_settings["apiKeyHelper"].as_str().unwrap();
    assert!(
        new_helper.contains(&format!("--runtime {id}")),
        "{new_helper}"
    );
    assert!(new_helper.contains(".alc/generations/"));
    let new_credential = helper(config.path(), work.path(), new_helper);
    let old_base = old_settings["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap();
    let new_base = new_settings["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap();
    assert_ne!(old_base, new_base);
    assert!(model_request(old_base, &old_credential).starts_with("HTTP/1.1 200"));
    assert!(model_request(new_base, &new_credential).starts_with("HTTP/1.1 200"));
    assert!(model_request(new_base, &old_credential).starts_with("HTTP/1.1 401"));
    assert_eq!(fs::read(&old_path).unwrap(), old_bytes);
    assert_eq!(
        fs::read(config.path().join("credentials.toml")).unwrap(),
        credentials
    );
    for (path, bytes) in legacy_state {
        assert_eq!(
            fs::read(config.path().join("run").join(path)).unwrap(),
            bytes
        );
    }
    let stop = command(config.path(), None)
        .args(["hub", "stop"])
        .output()
        .unwrap();
    assert!(!stop.status.success());
    assert!(String::from_utf8_lossy(&stop.stderr).contains("--runtime"));
    let stop = command(config.path(), None)
        .args(["bridge", "stop"])
        .output()
        .unwrap();
    assert!(!stop.status.success());
    assert!(String::from_utf8_lossy(&stop.stderr).contains("--runtime"));
    command(config.path(), Some("legacy"))
        .args(["hub", "status"])
        .assert()
        .success();
    command(config.path(), Some(&id))
        .args(["hub", "status"])
        .assert()
        .success();
    let report = command(config.path(), None)
        .args(["tps", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(report["summary"]["ttft_samples"], 2);
    assert_eq!(report["summary"]["stream_tps_samples"], 2);
    let ledger = fs::read_to_string(config.path().join("usage.jsonl")).unwrap();
    assert!(
        ledger
            .lines()
            .any(|line| serde_json::from_str::<Value>(line).unwrap()["v"] == 3)
    );
    assert!(!ledger.contains("runtime-first-key") && !ledger.contains("private-runtime-prompt"));
}

#[test]
fn coexisting_forward_credentials_remain_accepted_for_the_same_frozen_route() {
    let config = root();
    let work = root();
    initialize(config.path());
    let upstream = Upstream::start();
    profile(config.path(), &upstream.url, "runtime-first-key");
    let id = generation();
    let _stop = StopHosts::new(config.path(), &id);
    let (_, settings) = launch_metrics(config.path(), work.path(), None);
    let line = settings["apiKeyHelper"].as_str().unwrap();
    let first = helper(config.path(), work.path(), line);
    command(config.path(), None)
        .args(["config", "key", "runtime-test", "--stdin"])
        .write_stdin("runtime-second-key")
        .assert()
        .success();
    let second = helper(config.path(), work.path(), line);
    let base = settings["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap();
    assert!(model_request(base, &first).starts_with("HTTP/1.1 200"));
    assert!(model_request(base, &second).starts_with("HTTP/1.1 200"));
    let requests = upstream.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].contains("runtime-first-key"));
    assert!(requests[1].contains("runtime-second-key"));
}

#[test]
fn generation_port_collision_does_not_rotate_token_or_rewrite_settings() {
    let config = root();
    initialize(config.path());
    let id = generation();
    let run = config.path().join("run/g").join(&id);
    fs::create_dir_all(run.join("claude")).unwrap();
    let listener = (20_000..30_000)
        .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    fs::write(run.join("bridge.port"), port.to_string()).unwrap();
    fs::write(run.join("bridge.token"), "generation-fixed-token").unwrap();
    let settings =
        format!("{{\"env\":{{\"ANTHROPIC_BASE_URL\":\"http://127.0.0.1:{port}/r/codex-test\"}}}}");
    fs::write(run.join("claude/settings-unchanged.json"), &settings).unwrap();
    let before = tree(&run);
    let stop = Arc::new(AtomicBool::new(false));
    let halt = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !halt.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            let request = read_http(&mut stream);
            if request.is_empty() {
                continue;
            }
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            assert!(!request.contains("generation-fixed-token"));
            respond(&mut stream, "404 Not Found", "{}");
        }
    });
    let output = command(config.path(), Some(&id))
        .args(["bridge", "serve"])
        .output()
        .unwrap();
    stop.store(true, Ordering::Release);
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("left unchanged"));
    for (path, bytes) in before {
        assert_eq!(fs::read(run.join(path)).unwrap(), bytes);
    }
}

#[test]
fn legacy_occupied_bridge_accepts_old_authenticated_greeting_without_relocation() {
    let config = root();
    initialize(config.path());
    let run = config.path().join("run");
    fs::create_dir_all(config.path().join("claude")).unwrap();
    fs::create_dir_all(&run).unwrap();
    let listener = (20_000..30_000)
        .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = "legacy-original-token";
    fs::write(run.join("bridge.port"), port.to_string()).unwrap();
    fs::write(run.join("bridge.token"), token).unwrap();
    let settings =
        format!("{{\"env\":{{\"ANTHROPIC_BASE_URL\":\"http://127.0.0.1:{port}/r/codex-old\"}}}}");
    fs::write(config.path().join("claude/settings-old.json"), &settings).unwrap();
    let before = tree(config.path());
    let stop = Arc::new(AtomicBool::new(false));
    let halt = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !halt.load(Ordering::Acquire) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            let request = read_http(&mut stream);
            if request.is_empty() {
                continue;
            }
            if !request.starts_with("GET /alc/hello ") {
                respond(&mut stream, "404 Not Found", "{}");
            } else if !request
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {token}"))
            {
                respond(&mut stream, "401 Unauthorized", "{}");
            } else {
                respond(
                    &mut stream,
                    "200 OK",
                    &json!({ "alc":"2.0.0", "pid":100, "port":port }).to_string(),
                );
            }
        }
    });
    let output = command(config.path(), Some("legacy"))
        .args(["bridge", "serve"])
        .output()
        .unwrap();
    stop.store(true, Ordering::Release);
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already serving"));
    for (path, bytes) in before {
        assert_eq!(fs::read(config.path().join(path)).unwrap(), bytes);
    }
}

#[cfg(unix)]
fn ctl(run_dir: &Path, body: Value) -> Value {
    let secret = fs::read_to_string(run_dir.join("ctl.token")).unwrap();
    let mut stream = std::os::unix::net::UnixStream::connect(run_dir.join("ctl.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    writeln!(stream, "{}", json!({ "secret":secret, "request":body })).unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[cfg(unix)]
fn create_session(run_dir: &Path, work: &Path, fake: &Path, runtime: Option<&str>) -> String {
    let mut body = json!({
        "op":"create", "alc":env!("CARGO_PKG_VERSION"),
        "spec": {
            "program":fake, "args":[], "env":[], "env_remove":[],
            "provider_name":"runtime-test", "provider_kind":"custom", "agent":"claude",
            "model":null, "effort":null, "secret_values":[], "secret_env":[],
        },
        "cwd":work, "environ":[["ALC_CLAUDE_BIN",fake]], "name":"runtime-test", "cols":80, "rows":24,
        "scrollback_bytes":4096, "permission":{"rung":null,"native":null,"confidence":"assumed"},
    });
    if let Some(runtime) = runtime {
        body["generation"] = json!(runtime);
        body["protocol"] = json!(1);
    }
    let reply = ctl(run_dir, body);
    assert_eq!(reply["reply"], "created", "{reply}");
    reply["id"].as_str().unwrap().to_owned()
}

#[cfg(unix)]
#[test]
fn control_management_does_not_require_overridden_browser_role_files() {
    use std::os::unix::fs::PermissionsExt;

    for role in ["ALC_REMOTE_OPERATOR", "ALC_REMOTE_VIEWER"] {
        let config = root();
        let work = root();
        initialize(config.path());
        let fake = work.path().join("claude");
        fs::write(&fake, "#!/bin/sh\nexec cat\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let _host = StopHosts::new(config.path(), "legacy");
        command(config.path(), Some("legacy"))
            .env(role, "synthetic-browser-role-override")
            .args(["hub", "start"])
            .assert()
            .success();
        let run_dir = config.path().join("run");
        let absent_role = if role == "ALC_REMOTE_OPERATOR" {
            "operator.token"
        } else {
            "viewer.token"
        };
        assert!(!run_dir.join(absent_role).exists());
        let session = create_session(&run_dir, work.path(), &fake, None);
        let before = tree(&run_dir);
        let output = command(config.path(), None)
            .args(["sessions", "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert!(
            report
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == session && row["runtime"] == "legacy"),
            "a browser-only override must not hide a ctl-authenticated owner"
        );
        assert_eq!(
            tree(&run_dir),
            before,
            "discovery must not mint credentials"
        );
        command(config.path(), None)
            .args(["hub", "status"])
            .assert()
            .success();
        command(config.path(), None)
            .args(["rename", &session, "renamed-with-ctl-only"])
            .assert()
            .success();
        command(config.path(), None)
            .args(["kill", &session])
            .assert()
            .success();
        command(config.path(), Some("legacy"))
            .args(["hub", "stop", "--drain"])
            .assert()
            .success();
        assert!(!run_dir.join(absent_role).exists());
    }
}

#[cfg(unix)]
#[test]
fn session_management_routes_to_the_real_owner_and_cleanup_stays_in_scope() {
    use std::os::unix::fs::PermissionsExt;
    let config = root();
    let work = root();
    initialize(config.path());
    let fake = work.path().join("claude");
    fs::write(
        &fake,
        "#!/bin/sh\nprintf 'runtime-session-ready\\n'\nexec cat\n",
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let id = generation();
    let _old = StopHosts::new(config.path(), "legacy");
    let _new = StopHosts::new(config.path(), &id);
    command(config.path(), Some("legacy"))
        .args(["hub", "start"])
        .assert()
        .success();
    let old_run = config.path().join("run");
    let old_session = create_session(&old_run, work.path(), &fake, None);
    let old_live_file = work.path().join("legacy-session-file");
    fs::write(&old_live_file, "still-held-by-legacy").unwrap();
    fs::create_dir_all(old_run.join("sessions")).unwrap();
    fs::write(
        old_run.join("sessions/legacy-extra.json"),
        json!({"id":"old", "cleanup":[old_live_file]}).to_string(),
    )
    .unwrap();
    command(config.path(), None)
        .args(["hub", "start"])
        .assert()
        .success();
    assert!(old_live_file.exists());
    assert!(old_run.join("sessions/legacy-extra.json").exists());
    let new_run = config.path().join("run/g").join(&id);
    let new_session = create_session(&new_run, work.path(), &fake, Some(&id));
    let hello = ctl(&new_run, json!({"op":"hello"}));
    assert_eq!(hello["generation"], id);
    assert_eq!(hello["protocol"], 1);
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "request-metrics-v3")
    );
    let report = command(config.path(), None)
        .args(["sessions", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(report.as_array().unwrap().len(), 2);
    assert!(
        report
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == old_session && row["runtime"] == "legacy")
    );
    assert!(
        report
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == new_session && row["runtime"] == id)
    );
    let ambiguous = command(config.path(), None)
        .args(["kill", "claude"])
        .output()
        .unwrap();
    assert!(!ambiguous.status.success());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("more than one"));
    assert_eq!(
        ctl(&old_run, json!({"op":"list"}))["sessions"][0]["state"],
        "running"
    );
    assert_eq!(
        ctl(&new_run, json!({"op":"list"}))["sessions"][0]["state"],
        "running"
    );
    command(config.path(), None)
        .args(["rename", &old_session, "old-renamed"])
        .assert()
        .success();
    assert_eq!(
        ctl(&old_run, json!({"op":"list"}))["sessions"][0]["name"],
        "old-renamed"
    );
    // Attach uses the owning socket even though this caller's default scope
    // is Generation. The out-of-band resize must go back to that same owner.
    let secret = fs::read_to_string(old_run.join("ctl.token")).unwrap();
    let mut attached = std::os::unix::net::UnixStream::connect(old_run.join("ctl.sock")).unwrap();
    attached
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    writeln!(
        attached,
        "{}",
        json!({"secret":secret, "request":{"op":"attach","id":old_session,"cols":91,"rows":31}})
    )
    .unwrap();
    let mut answer = Vec::new();
    let mut byte = [0_u8; 1];
    while attached.read_exact(&mut byte).is_ok() && byte[0] != b'\n' {
        answer.push(byte[0]);
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&answer).unwrap()["reply"],
        "ok"
    );
    attached.write_all(b"owner-specific-input\n").unwrap();
    let mut output = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut buffer = [0_u8; 1024];
    while !String::from_utf8_lossy(&output).contains("owner-specific-input")
        && std::time::Instant::now() < deadline
    {
        let read = attached.read(&mut buffer).unwrap();
        assert!(read > 0);
        output.extend_from_slice(&buffer[..read]);
    }
    assert!(String::from_utf8_lossy(&output).contains("owner-specific-input"));
    assert_eq!(
        ctl(
            &old_run,
            json!({"op":"resize","id":old_session,"cols":99,"rows":33})
        )["reply"],
        "ok"
    );
    assert_eq!(
        ctl(&old_run, json!({"op":"list"}))["sessions"][0]["cols"],
        99
    );
    assert_eq!(
        ctl(&new_run, json!({"op":"list"}))["sessions"][0]["cols"],
        80
    );
    drop(attached);
    let marker = work.path().join("new-session-file");
    fs::write(&marker, "still-held-by-new").unwrap();
    fs::write(
        new_run.join("sessions/active-extra.json"),
        json!({"id":"new", "cleanup":[marker]}).to_string(),
    )
    .unwrap();
    let second = command(config.path(), Some(&id))
        .args(["hub", "start", "--foreground"])
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(
        marker.exists(),
        "a losing foreground start must not reap an active owner's files"
    );
    command(config.path(), None)
        .args(["kill", &old_session])
        .assert()
        .success();
    assert_eq!(
        ctl(&new_run, json!({"op":"list"}))["sessions"][0]["state"],
        "running"
    );
    command(config.path(), None)
        .args(["kill", &new_session])
        .assert()
        .success();
}

/// The coordinator/CI may supply another genuinely linked same-version build.
/// Ordinary local runs still cover Legacy plus the current generation above.
#[cfg(unix)]
#[test]
fn independently_linked_same_version_generations_are_distinct_owners() {
    use std::os::unix::fs::PermissionsExt;
    let Some(other) = std::env::var_os("ALC_TEST_OTHER_BINARY").map(PathBuf::from) else {
        eprintln!(
            "ALC_TEST_OTHER_BINARY not supplied; second independently linked generation branch skipped"
        );
        return;
    };
    assert!(
        other.is_absolute() && other.is_file(),
        "ALC_TEST_OTHER_BINARY must name an absolute executable"
    );
    let own = generation();
    let foreign = generation_for(&other);
    assert_ne!(
        own, foreign,
        "fixture must be an independently linked executable"
    );
    let version = command_at(&other, root().path(), None)
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&version.stdout).contains(env!("CARGO_PKG_VERSION")),
        "fixtures must carry the same version"
    );
    let config = root();
    let work = root();
    initialize(config.path());
    let fake = work.path().join("claude");
    fs::write(
        &fake,
        "#!/bin/sh\nprintf 'two-generation-ready\\n'\nexec cat\n",
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let _own = StopHosts::new(config.path(), &own);
    let _other = StopHosts::new(config.path(), &foreign);
    command(config.path(), None)
        .args(["hub", "start"])
        .assert()
        .success();
    command_at(&other, config.path(), None)
        .args(["hub", "start"])
        .assert()
        .success();
    let own_run = config.path().join("run/g").join(&own);
    let other_run = config.path().join("run/g").join(&foreign);
    let one = create_session(&own_run, work.path(), &fake, Some(&own));
    let two = create_session(&other_run, work.path(), &fake, Some(&foreign));
    let own_hello = ctl(&own_run, json!({"op":"hello"}));
    let other_hello = ctl(&other_run, json!({"op":"hello"}));
    assert_eq!(own_hello["alc"], other_hello["alc"]);
    assert_ne!(own_hello["generation"], other_hello["generation"]);
    assert_ne!(own_hello["pid"], other_hello["pid"]);
    let report = command(config.path(), None)
        .args(["sessions", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(report.as_array().unwrap().len(), 2);
    assert!(
        report
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == one && row["runtime"] == own)
    );
    assert!(
        report
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == two && row["runtime"] == foreign)
    );
    let attempt = command(config.path(), Some(&foreign))
        .args(["hub", "start"])
        .output()
        .unwrap();
    assert!(
        !attempt.status.success(),
        "a selector cannot create this build's launch under another owner"
    );
    command(config.path(), None)
        .args(["rename", &two, "other-renamed"])
        .assert()
        .success();
    assert_eq!(
        ctl(&other_run, json!({"op":"list"}))["sessions"][0]["name"],
        "other-renamed"
    );
    command(config.path(), None)
        .args(["kill", &two])
        .assert()
        .success();
    assert_eq!(
        ctl(&own_run, json!({"op":"list"}))["sessions"][0]["state"],
        "running"
    );
}
