use assert_cmd::Command;
use predicates::prelude::*;

fn alc(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::cargo_bin("alc").expect("alc binary");
    command.env("ALC_CONFIG_DIR", temp.path());
    command
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
            "Claude Code needs Anthropic Messages",
        ));
}
