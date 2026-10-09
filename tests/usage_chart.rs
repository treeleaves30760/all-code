//! PNG reporting uses only isolated, synthetic history and an embedded font.

use std::fs;
use std::path::Path;
use std::time::Duration;

use assert_cmd::Command;
use serde_json::{Value, json};

struct Fixture {
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for directory in [
            "home",
            "alc",
            "claude/projects",
            "codex/sessions",
            "codex/archived_sessions",
        ] {
            fs::create_dir_all(temp.path().join(directory)).unwrap();
        }
        fs::write(temp.path().join("alc/credentials.toml"), "not TOML [").unwrap();
        fs::write(temp.path().join("alc/pricing.toml"), "version=1\ncurrency=\"USD\"\nunits=\"USD-per-million-tokens\"\n[[models]]\nprovider=\"custom\"\nmodel=\"fixture\"\ninput=\"2.5\"\noutput=\"10.25\"\ncache_read=\"0.125\"\ncache_write=\"3.5\"\n").unwrap();
        Self { temp }
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("alc").unwrap();
        command.env_clear();
        for (name, value) in std::env::vars_os() {
            if matches!(
                name.to_string_lossy().to_ascii_uppercase().as_str(),
                "PATH" | "SYSTEMROOT" | "WINDIR" | "TEMP" | "TMP"
            ) {
                command.env(name, value);
            }
        }
        command
            .current_dir(self.temp.path())
            .env("ALC_CONFIG_DIR", self.temp.path().join("alc"))
            .env("CLAUDE_CONFIG_DIR", self.temp.path().join("claude"))
            .env("CODEX_HOME", self.temp.path().join("codex"))
            .env("HOME", self.temp.path().join("home"))
            .env("USERPROFILE", self.temp.path().join("home"))
            .env("NO_COLOR", "1")
            .timeout(Duration::from_secs(45));
        command
    }

    fn seed(&self) {
        let mut text = String::new();
        for (day, scale) in [(1, 1), (2, 2), (4, 1), (5, 3)] {
            let timestamp =
                chrono::DateTime::parse_from_rfc3339(&format!("2026-10-{day:02}T12:00:00Z"))
                    .unwrap()
                    .timestamp_millis();
            let row = json!({"t":"request","v":3,"record":{
                "source":"alc","timestamp_ms":timestamp,"agent":"claude","profile":"fixture","provider":"custom","model":"fixture","outcome":"completed","granularity":"request","billing":"api",
                "tokens":{"input_basis":"inclusive","input_tokens":1000*scale,"cache_read_tokens":200*scale,"cache_write_tokens":100*scale,"output_tokens":100*scale,"reasoning_tokens":0}
            }});
            text.push_str(&format!("{row}\n"));
        }
        fs::write(self.temp.path().join("alc/usage.jsonl"), text).unwrap();
    }
}

fn assert_png(path: &Path) {
    let bytes = fs::read(path).unwrap();
    assert!(bytes.len() > 10_000, "chart is empty or suspiciously small");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 1560);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 1280);
}

#[test]
fn export_is_opt_in_and_json_stdout_stays_one_document() {
    let fixture = Fixture::new();
    fixture.seed();
    let ledger_before = fs::read(fixture.temp.path().join("alc/usage.jsonl")).unwrap();
    fixture
        .command()
        .args(["usage", "--offline", "--source", "alc", "--json"])
        .assert()
        .success();
    let default = fixture.temp.path().join("home/ai-usage.png");
    assert!(!default.exists());
    let output = fixture
        .command()
        .args(["usage", "--offline", "--source", "alc", "--json", "--chart"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["statistics"]["records"], 4);
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage chart:"));
    assert_png(&default);
    assert_eq!(
        fs::read(fixture.temp.path().join("alc/usage.jsonl")).unwrap(),
        ledger_before
    );
    assert_eq!(
        fs::read_to_string(fixture.temp.path().join("alc/credentials.toml")).unwrap(),
        "not TOML ["
    );
    assert!(!fixture.temp.path().join("alc/run").exists());
}

#[test]
fn explicit_path_replacement_is_atomic_and_no_data_is_a_valid_chart() {
    let fixture = Fixture::new();
    let path = fixture.temp.path().join("report.png");
    fs::write(&path, "previous artifact").unwrap();
    fixture
        .command()
        .args([
            "usage",
            "--offline",
            "--source",
            "alc",
            &format!("--chart={}", path.display()),
        ])
        .assert()
        .success();
    assert_png(&path);
    assert!(!fixture.temp.path().join("home/ai-usage.png").exists());
}

#[test]
fn missing_destination_directory_fails_without_changing_history() {
    let fixture = Fixture::new();
    fixture.seed();
    let before = fs::read(fixture.temp.path().join("alc/usage.jsonl")).unwrap();
    fixture
        .command()
        .args([
            "usage",
            "--offline",
            "--source",
            "alc",
            "--chart=missing-dir/report.png",
        ])
        .assert()
        .failure();
    assert_eq!(
        fs::read(fixture.temp.path().join("alc/usage.jsonl")).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn export_refuses_a_symlink_destination() {
    let fixture = Fixture::new();
    let target = fixture.temp.path().join("keep.txt");
    fs::write(&target, "untouched").unwrap();
    std::os::unix::fs::symlink(&target, fixture.temp.path().join("report.png")).unwrap();
    fixture
        .command()
        .args([
            "usage",
            "--offline",
            "--source",
            "alc",
            "--chart=report.png",
        ])
        .assert()
        .failure();
    assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
}

#[test]
fn wrapped_writes_one_image_and_skips_the_text_report() {
    let fixture = Fixture::new();
    fixture.seed();
    let output = fixture
        .command()
        .args(["usage", "--offline", "--source", "alc", "--wrapped"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let image = fixture.temp.path().join("home/alc-wrapped.png");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Wrapped image:"));
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let bytes = fs::read(&image).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 1600);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 1440);
}
