use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn alc(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::cargo_bin("alc").expect("alc binary");
    command.env("ALC_CONFIG_DIR", temp.path());
    command
}

fn serve_once(body: String) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP server");
    let address = listener.local_addr().expect("test server address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept updater request");
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request).expect("read updater request");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write updater response");
    });
    (format!("http://{address}/latest"), handle)
}

#[test]
fn initializes_and_prints_default_config() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["config", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("initialized"));

    alc(&temp)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[providers.codex]"))
        .stdout(predicate::str::contains("# openai: missing"));
}

#[test]
fn dry_run_preserves_agent_arguments() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["--codex", "--dry-run", "codex", "exec", "hello world"])
        .assert()
        .success()
        .stdout(predicate::str::contains("provider: codex (codex)"))
        .stdout(predicate::str::contains("exec 'hello world'"));
}

#[test]
fn openrouter_claude_dry_run_redacts_key() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "never-print-this")
        .args(["--openrouter", "--dry-run", "claude", "--print", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("https://openrouter.ai/api"))
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("never-print-this").not());
}

#[test]
fn saved_keys_are_not_printed() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp).args(["config", "init"]).assert().success();
    alc(&temp)
        .args(["config", "key", "openai", "--stdin"])
        .write_stdin("super-secret\n")
        .assert()
        .success();

    alc(&temp)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("# openai: saved-local"))
        .stdout(predicate::str::contains("super-secret").not());
}

#[test]
fn incompatible_provider_has_actionable_error() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENAI_API_KEY", "test")
        .args(["--openai", "--dry-run", "claude"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Claude Code needs an Anthropic-compatible endpoint",
        ));
}

#[test]
fn codex_to_claude_accepts_explicit_model_and_effort() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args([
            "--codex",
            "--dry-run",
            "claude",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "max",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("--model gpt-5.6-sol"))
        .stdout(predicate::str::contains("--effort max"))
        .stdout(predicate::str::contains("claude-codex"));
}

#[test]
fn generic_gpt_56_alias_uses_bridge_supported_sol() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args([
            "--codex",
            "--dry-run",
            "claude",
            "--model",
            "gpt-5.6",
            "--effort",
            "medium",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("--model gpt-5.6-sol"));
}

#[test]
fn codex_to_claude_offers_every_gpt_model_inside_claude_code() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["--codex", "--dry-run", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"model\":\"gpt-5.6-luna\""))
        .stdout(predicate::str::contains("\"model\":\"gpt-5.6-terra\""))
        .stdout(predicate::str::contains("\"model\":\"gpt-5.6-sol\""))
        .stdout(predicate::str::contains("\"replaceBuiltInOptions\":true"));
}

#[test]
fn codex_to_claude_can_save_defaults_without_picker() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args([
            "--codex",
            "--dry-run",
            "claude",
            "--model",
            "gpt-5.6-luna",
            "--effort",
            "low",
            "--save",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Saved gpt-5.6-luna / low as the default",
        ));

    alc(&temp)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("model = \"gpt-5.6-luna\""))
        .stdout(predicate::str::contains("reasoning_effort = \"low\""));
}

#[test]
fn codex_to_opencode_dry_run_reports_the_bridge() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["--codex", "--dry-run", "opencode"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude-codex"));
}

#[test]
fn bundled_model_catalog_is_available_offline() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("PATH", "")
        .args(["models"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-5.6-luna"))
        .stdout(predicate::str::contains("gpt-5.6-terra"))
        .stdout(predicate::str::contains("gpt-5.6-sol"))
        .stdout(predicate::str::contains("low, medium, high, xhigh, max"));
}

#[test]
fn new_agent_subcommands_exist_and_fail_cleanly_before_wiring() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["--dry-run", "pi"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not wired up yet"));
}

#[test]
fn preset_kind_upsert_prefills_urls_and_supports_claude() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["config", "upsert", "ds", "--kind", "deepseek"])
        .assert()
        .success();
    alc(&temp)
        .env("DEEPSEEK_API_KEY", "k")
        .args(["--provider", "ds", "--dry-run", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://api.deepseek.com/anthropic",
        ));
}

#[test]
fn update_check_does_not_require_a_valid_provider_config() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("config.toml"),
        "this is not valid toml = [",
    )
    .unwrap();
    let release = format!(
        r#"{{"tag_name":"v{}","html_url":"https://example.test/release","assets":[]}}"#,
        env!("CARGO_PKG_VERSION")
    );
    let (url, server) = serve_once(release);

    alc(&temp)
        .env("ALC_UPDATE_API_URL", url)
        .args(["update", "--check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("is up to date"));

    server.join().expect("test HTTP server");
}
