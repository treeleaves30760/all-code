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

/// A stand-in Ollama server that answers the metadata calls alc makes:
/// `/api/version`, `/api/show`, and `/api/ps` (nothing loaded). It serves
/// until the test process exits.
fn serve_ollama() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Ollama");
    let address = listener.local_addr().expect("fake Ollama address");
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let mut request = [0_u8; 8192];
            let read = stream.read(&mut request).unwrap_or(0);
            let head = String::from_utf8_lossy(&request[..read]);
            let path = head.split_whitespace().nth(1).unwrap_or("");
            let (status, body) = if path.starts_with("/api/version") {
                ("200 OK", r#"{"version":"0.33.3"}"#)
            } else if path.starts_with("/api/show") {
                (
                    "200 OK",
                    r#"{"capabilities":["completion","tools","thinking"],"model_info":{"general.architecture":"gemma4","gemma4.context_length":131072},"parameters":"temperature 1"}"#,
                )
            } else if path.starts_with("/api/ps") {
                ("200 OK", r#"{"models":[]}"#)
            } else {
                ("404 Not Found", r#"{"error":"not found"}"#)
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{address}")
}

fn ollama_profile(temp: &tempfile::TempDir, base_url: &str) {
    alc(temp).args(["config", "init"]).assert().success();
    alc(temp)
        .args([
            "config",
            "upsert",
            "ollama",
            "--kind",
            "ollama",
            "--model",
            "gemma4:12b",
            "--base-url",
            base_url,
        ])
        .assert()
        .success();
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
fn ollama_claude_dry_run_pins_aliases_and_reports_the_servers_context() {
    let temp = tempfile::tempdir().unwrap();
    ollama_profile(&temp, &serve_ollama());
    let assert = alc(&temp)
        .env_remove("API_FORCE_IDLE_TIMEOUT")
        .env_remove("API_TIMEOUT_MS")
        .args(["--ollama", "--dry-run", "claude"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    for expected in [
        "API_FORCE_IDLE_TIMEOUT=0",
        "API_TIMEOUT_MS=1800000",
        "ANTHROPIC_MODEL=gemma4:12b",
        "ANTHROPIC_DEFAULT_MODEL=gemma4:12b",
        "ANTHROPIC_DEFAULT_SONNET_MODEL=gemma4:12b",
        "ANTHROPIC_DEFAULT_OPUS_MODEL=gemma4:12b",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL=gemma4:12b",
        "ANTHROPIC_SMALL_FAST_MODEL=gemma4:12b",
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1",
        "CLAUDE_CODE_MAX_CONTEXT_TOKENS=131072",
    ] {
        assert!(output.contains(expected), "missing {expected} in {output}");
    }
}

#[test]
fn ollama_claude_keeps_the_users_own_timeout() {
    let temp = tempfile::tempdir().unwrap();
    ollama_profile(&temp, "http://127.0.0.1:9");
    alc(&temp)
        .env("API_TIMEOUT_MS", "42")
        .env_remove("API_FORCE_IDLE_TIMEOUT")
        .args(["--ollama", "--dry-run", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("API_FORCE_IDLE_TIMEOUT=0"))
        .stdout(predicate::str::contains("API_TIMEOUT_MS=").not());
}

#[test]
fn ollama_claude_dry_run_works_without_a_running_server() {
    let temp = tempfile::tempdir().unwrap();
    // Nothing listens here, so the context probe must give up quietly.
    ollama_profile(&temp, "http://127.0.0.1:9");
    alc(&temp)
        .args(["--ollama", "--dry-run", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL=gemma4:12b",
        ))
        .stdout(predicate::str::contains("CLAUDE_CODE_MAX_CONTEXT_TOKENS").not());
}

#[test]
fn doctor_checks_the_ollama_model() {
    let temp = tempfile::tempdir().unwrap();
    ollama_profile(&temp, &serve_ollama());
    let assert = alc(&temp).args(["doctor"]).assert();
    let output = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(output.contains("Ollama 0.33.3"), "{output}");
    assert!(output.contains("gemma4:12b"), "{output}");
    assert!(output.contains("tools"), "{output}");
    assert!(output.contains("131072 tokens of context"), "{output}");
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
fn doctor_lists_every_agent() {
    let temp = tempfile::tempdir().unwrap();
    let assert = alc(&temp).args(["doctor"]).assert();
    let output = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    for agent in [
        "claude", "codex", "opencode", "pi", "copilot", "goose", "qwen", "kimi",
    ] {
        assert!(output.contains(agent), "{agent} missing from doctor output");
    }
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
fn pi_dry_run_selects_the_alc_provider_entry() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "secret")
        .args(["--openrouter", "--dry-run", "pi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--provider alc-openrouter"))
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("secret").not());
}

#[test]
fn codex_to_pi_dry_run_reports_bridge_and_setup() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["--codex", "--dry-run", "pi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("claude-codex"))
        .stdout(predicate::str::contains("--provider alc-codex"));
}

#[test]
fn copilot_dry_run_uses_byok_env() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "secret")
        .args(["--openrouter", "--dry-run", "copilot"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "COPILOT_PROVIDER_BASE_URL=https://openrouter.ai/api/v1",
        ))
        .stdout(predicate::str::contains("COPILOT_PROVIDER_TYPE=openai"))
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("secret").not());
}

#[test]
fn goose_dry_run_defaults_to_session_with_env_config() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "secret")
        .args(["--openrouter", "--dry-run", "goose"])
        .assert()
        .success()
        .stdout(predicate::str::contains("GOOSE_PROVIDER=openrouter"))
        .stdout(predicate::str::contains(" session"))
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("secret").not());
}

#[test]
fn qwen_dry_run_uses_auth_type_flags_and_env() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENAI_API_KEY", "secret")
        .args(["--openai", "--dry-run", "qwen"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--auth-type openai"))
        .stdout(predicate::str::contains(
            "OPENAI_BASE_URL=https://api.openai.com/v1",
        ))
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("secret").not());
}

#[test]
fn kimi_dry_run_uses_a_temporary_config_file() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("kimi-config.toml");
    std::fs::write(&config, "default_thinking = true\n").unwrap();
    alc(&temp)
        .env("OPENAI_API_KEY", "secret")
        .env("ALC_KIMI_CONFIG", &config)
        .args(["--openai", "--dry-run", "kimi"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--config-file"))
        .stdout(predicate::str::contains("setup: temporary config"))
        .stdout(predicate::str::contains("secret").not());
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

#[test]
fn removing_a_provider_only_blocks_on_explicit_defaults() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp).args(["config", "init"]).assert().success();
    // Fresh config: qwen/kimi fall back to 'openai' implicitly — removal must succeed.
    alc(&temp)
        .args(["config", "remove", "openai"])
        .assert()
        .success();
    // An explicit default still blocks removal.
    alc(&temp)
        .args(["config", "set-default", "opencode", "openrouter"])
        .assert()
        .success();
    alc(&temp)
        .args(["config", "remove", "openrouter"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("still the default"));
}
