use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// Every alc invocation in this file is bounded.
///
/// Not a nicety: a command that blocks here stalls CI for as long as the job
/// is allowed to run, and reports nothing about where it stopped. With a
/// deadline the same bug is a failure with output attached.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

fn alc(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::cargo_bin("alc").expect("alc binary");
    command.env("ALC_CONFIG_DIR", temp.path());
    command.timeout(COMMAND_TIMEOUT);
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

/// A provider profile can name any environment variable through
/// `api_key_env`, so redaction cannot rely on recognising the conventional
/// spellings. alc marks what it actually wrote instead.
#[test]
fn dry_run_redacts_a_key_stored_under_a_custom_env_name() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args([
            "config",
            "upsert",
            "gateway",
            "--kind",
            "custom",
            "--base-url",
            "https://gateway.example/v1",
            "--model",
            "some-model",
            "--api-key-env",
            "MY_GATEWAY_CREDENTIAL",
        ])
        .assert()
        .success();
    alc(&temp)
        .args(["config", "key", "gateway", "--stdin"])
        .write_stdin("never-print-this")
        .assert()
        .success();

    alc(&temp)
        .args(["--provider", "gateway", "--dry-run", "opencode"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MY_GATEWAY_CREDENTIAL=<redacted>"))
        .stdout(predicate::str::contains("never-print-this").not());
}

/// `--share` needs a terminal on both ends. A scripted `alc claude -p … >
/// out.txt` must fail loudly rather than fill that file with the escape
/// sequences a mirrored TUI session emits.
#[test]
fn share_refuses_when_stdio_is_redirected() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--share", "claude", "--print", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("interactive terminal"));
}

/// `trailing_var_arg` hands everything after the first agent argument to the
/// agent, so appending alc's own flag - the most natural gesture there is -
/// would silently send `--share` to the model as prompt text.
#[test]
fn a_share_flag_after_the_agents_arguments_is_rejected_with_guidance() {
    let temp = tempfile::tempdir().unwrap();
    for agent in ["claude", "codex", "opencode", "goose"] {
        alc(&temp)
            .env("OPENROUTER_API_KEY", "key-for-this-test-only")
            .args(["--openrouter", agent, "review this", "--share"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("alc share"))
            .stderr(predicate::str::contains("before the agent name"));
    }
}

#[test]
fn a_dry_run_says_it_would_share_and_binds_nothing() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--share", "--dry-run", "opencode"])
        .assert()
        .success()
        .stdout(predicate::str::contains("share:"))
        .stdout(predicate::str::contains("<redacted>").or(predicate::str::contains("command:")));
}

#[test]
fn the_share_subcommand_takes_the_agent_as_an_argument() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--dry-run", "share", "opencode"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agent: opencode"));
}

#[test]
fn remote_status_reports_the_posture_without_binding_anything() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bind:"))
        .stdout(predicate::str::contains("loopback"));
}

#[test]
fn remote_can_be_turned_off_and_back_on() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp).args(["remote", "off"]).assert().success();
    alc(&temp)
        .args(["remote", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("remote control: off"));
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--share", "opencode"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("turned off"));
    alc(&temp).args(["remote", "on"]).assert().success();
}

/// alc has no command that prints a token: a link is minted at launch and
/// handed over once. `--rotate` is the only thing this subcommand does.
#[test]
fn remote_token_never_prints_a_token() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "token"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--rotate"));
    alc(&temp)
        .args(["remote", "token", "--rotate"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rotated"))
        .stdout(
            predicate::str::is_match("[A-Za-z0-9_-]{40,}")
                .unwrap()
                .not(),
        );
}

/// Reaching a session from a phone on the same Wi-Fi is the shape of the
/// request, so the flag alone is enough. The token guards the socket either
/// way; requiring a config edit as well was friction with no security to
/// show for it.
#[test]
fn the_bind_lan_flag_is_enough_on_its_own() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args([
            "--openrouter",
            "--dry-run",
            "--share",
            "--bind-lan",
            "opencode",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("share:"));
}

/// A tunnel's hostname is not knowable until the tunnel is up, and a
/// `cloudflared` quick tunnel renames itself every run - so this has to be
/// reachable from the command line rather than a file edit.
#[test]
fn a_tunnel_hostname_can_be_allowed_from_the_command_line() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "allow-host", "box.tail1a2b.ts.net"])
        .assert()
        .success()
        .stdout(predicate::str::contains("box.tail1a2b.ts.net"));
    alc(&temp)
        .args(["remote", "allow-host", "*.trycloudflare.com"])
        .assert()
        .success();

    alc(&temp)
        .args(["remote", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("box.tail1a2b.ts.net"))
        .stdout(predicate::str::contains("*.trycloudflare.com"));
}

#[test]
fn a_host_entry_that_is_not_a_host_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "allow-host", "https://box.example/path"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a host name"));
}

/// The confirmation for loosening a session's permissions has to come from a
/// person at the machine - not from whoever holds the link, and not from the
/// agent piping a command into a shell.
#[test]
fn confirm_refuses_without_a_terminal() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["confirm", "ABCDEFGHIJ"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at a terminal on this machine"));
}

#[test]
fn an_unknown_permission_rung_is_named_with_the_ones_that_exist() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args([
            "--openrouter",
            "--share",
            "--permission",
            "nonsense",
            "opencode",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("auto-edit"));
}

/// alc adds a permission flag only for the agents it has confirmed against a
/// real `--help`. A guessed flag name does not tighten a session, it stops
/// the session from starting.
#[test]
fn a_verified_agent_gets_a_permission_flag_and_an_unverified_one_does_not() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--dry-run", "--share", "opencode"])
        .assert()
        .success()
        .stdout(predicate::str::contains("share:"));

    // Qwen's flags are documentation-derived, so an unasked-for launch adds
    // nothing to its arguments.
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--dry-run", "qwen"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--approval-mode").not());
}

#[test]
fn remote_status_reports_the_permission_ceiling() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ceiling:"))
        .stdout(predicate::str::contains("alc confirm"));
}

/// Writes a remote.toml that keeps a test hub off the default port, so two
/// tests running at once cannot collide and neither can touch a real hub.
#[cfg(unix)]
fn ephemeral_remote(temp: &tempfile::TempDir) {
    std::fs::write(
        temp.path().join("remote.toml"),
        "port = 0\nmax_permission = \"auto-edit\"\n",
    )
    .expect("write remote.toml");
}

/// The link `alc <agent> --share` prints scrolls away the moment the agent
/// draws its own interface, so it has to be recoverable.
#[cfg(unix)]
#[test]
fn the_page_link_is_recoverable_after_it_scrolls_away() {
    let temp = tempfile::tempdir().unwrap();
    ephemeral_remote(&temp);
    alc(&temp).args(["hub", "start"]).assert().success();

    alc(&temp)
        .args(["remote", "url"])
        .assert()
        .success()
        .stdout(predicate::str::contains("http://127.0.0.1:"))
        .stdout(predicate::str::contains("/#k="));

    // And `alc sessions` leads with it, because that is where a user looks.
    alc(&temp)
        .args(["sessions"])
        .assert()
        .success()
        .stdout(predicate::str::contains("page "))
        .stdout(predicate::str::contains("/#k="));

    alc(&temp).args(["hub", "stop"]).assert().success();
}

#[cfg(unix)]
#[test]
fn a_tunnel_hostname_is_offered_as_a_link_too() {
    let temp = tempfile::tempdir().unwrap();
    ephemeral_remote(&temp);
    alc(&temp)
        .args(["remote", "allow-host", "box.tail1a2b.ts.net"])
        .assert()
        .success();
    // A wildcard is a pattern, not a name, so it cannot become a link.
    alc(&temp)
        .args(["remote", "allow-host", "*.trycloudflare.com"])
        .assert()
        .success();
    alc(&temp).args(["hub", "start"]).assert().success();

    alc(&temp)
        .args(["remote", "url"])
        .assert()
        .success()
        .stdout(predicate::str::contains("https://box.tail1a2b.ts.net/#k="))
        .stdout(predicate::str::contains("trycloudflare").not());

    alc(&temp).args(["hub", "stop"]).assert().success();
}

#[test]
fn asking_for_the_link_without_a_hub_says_how_to_get_one() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "url"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--share"));
}

/// A standing preference must not be the reason a scripted run starts
/// failing, so a redirected invocation quietly does not share.
#[test]
fn sharing_by_default_stays_out_of_the_way_of_a_scripted_run() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "auto-share", "on"])
        .assert()
        .success()
        .stdout(predicate::str::contains("on"));

    // assert_cmd pipes stdio, which is exactly the scripted case.
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--dry-run", "opencode"])
        .assert()
        .success()
        .stdout(predicate::str::contains("share:").not());

    alc(&temp)
        .args(["remote", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("share by default: on"));
}

/// An explicit `--share` still fails loudly, because there the user asked
/// for something alc cannot do.
#[test]
fn an_explicit_share_still_refuses_a_scripted_run() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["remote", "auto-share", "on"])
        .assert()
        .success();
    alc(&temp)
        .env("OPENROUTER_API_KEY", "key-for-this-test-only")
        .args(["--openrouter", "--share", "opencode"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("interactive terminal"));
}

#[cfg(unix)]
#[test]
fn hub_status_says_so_when_nothing_is_running() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["hub", "status"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not running"));
}

#[cfg(unix)]
#[test]
fn session_commands_explain_themselves_when_no_hub_is_running() {
    let temp = tempfile::tempdir().unwrap();
    alc(&temp)
        .args(["sessions"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("alc hub start"));
}

/// The hub is what makes one page show every session and lets a session
/// outlive its terminal, so it has to come up, answer, and go away again
/// without one.
#[cfg(unix)]
#[test]
fn a_hub_starts_answers_and_stops() {
    let temp = tempfile::tempdir().unwrap();
    ephemeral_remote(&temp);

    alc(&temp)
        .args(["hub", "start"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hub running"));

    alc(&temp)
        .args(["hub", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("running"))
        .stdout(predicate::str::contains("sessions: 0"));

    alc(&temp)
        .args(["sessions"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no shared sessions"));

    alc(&temp).args(["hub", "stop"]).assert().success();
    alc(&temp)
        .args(["hub", "status"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not running"));
}

/// Two clients racing to start a hub must end up with one, not with a
/// spurious error for whichever lost.
#[cfg(unix)]
#[test]
fn concurrent_starts_produce_exactly_one_hub() {
    let temp = tempfile::tempdir().unwrap();
    ephemeral_remote(&temp);

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let dir = temp.path().to_owned();
            std::thread::spawn(move || {
                Command::cargo_bin("alc")
                    .expect("alc binary")
                    .env("ALC_CONFIG_DIR", &dir)
                    .timeout(COMMAND_TIMEOUT)
                    .args(["hub", "start"])
                    .assert()
                    .success();
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("a hub start panicked");
    }

    alc(&temp)
        .args(["hub", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("running"));
    alc(&temp).args(["hub", "stop"]).assert().success();
}

/// A hub killed outright leaves its record behind; the next one must not
/// report a dead pid as running.
#[cfg(unix)]
#[test]
fn a_stale_hub_record_is_not_mistaken_for_a_running_hub() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("run")).unwrap();
    std::fs::write(
        temp.path().join("run/hub.json"),
        r#"{"pid":999999,"port":8787,"instance":"ghost","alc":"1.2.0"}"#,
    )
    .unwrap();

    alc(&temp)
        .args(["hub", "status"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not running"))
        .stdout(predicate::str::contains("stale"));
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
