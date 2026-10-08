use assert_cmd::Command;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PRICE_HEADER: &str = "version=1\ncurrency=\"USD\"\nunits=\"USD-per-million-tokens\"\n";
const CUSTOM_PRICES: &str = r#"
[[models]]
provider="custom"
model="fixture-model"
input="2.5"
output="10.25"
cache_read="0.125"
cache_write="3.5"
"#;

/// Never inherit credentials, native histories, or a caller's CLI settings.
struct Fixture {
    temp: tempfile::TempDir,
    alc: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let fixture = Self {
            alc: temp.path().join("alc"),
            claude: temp.path().join("claude"),
            codex: temp.path().join("codex"),
            temp,
        };
        for path in [
            fixture.alc.clone(),
            fixture.claude.join("projects"),
            fixture.codex.join("sessions"),
            fixture.codex.join("archived_sessions"),
            fixture.temp.path().join("home"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        // These must not be parsed by tps or usage --offline.
        fs::write(fixture.alc.join("credentials.toml"), "not valid TOML [").unwrap();
        fs::write(fixture.claude.join(".credentials.json"), "not valid JSON {").unwrap();
        fs::write(fixture.codex.join("auth.json"), "not valid JSON {").unwrap();
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("alc").expect("alc binary");
        command.env_clear();
        // Windows still needs its runtime environment; no API variables survive.
        for (key, value) in std::env::vars_os() {
            if matches!(
                key.to_string_lossy().to_ascii_uppercase().as_str(),
                "PATH" | "SYSTEMROOT" | "WINDIR" | "TEMP" | "TMP"
            ) {
                command.env(key, value);
            }
        }
        let home = self.temp.path().join("home");
        command
            .current_dir(self.temp.path())
            .env("ALC_CONFIG_DIR", &self.alc)
            .env("CLAUDE_CONFIG_DIR", &self.claude)
            .env("CODEX_HOME", &self.codex)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("APPDATA", &home)
            .env("LOCALAPPDATA", &home)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", &home)
            .env("NO_COLOR", "1")
            .timeout(Duration::from_secs(45));
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self
            .command()
            .args(args)
            .arg("--json")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).expect("one JSON report on stdout")
    }

    fn usage(&self, args: &[&str]) -> Value {
        let mut command = self.command();
        command.args(["usage", "--offline", "--json"]).args(args);
        let output = command.assert().success().get_output().stdout.clone();
        serde_json::from_slice(&output).expect("offline usage JSON")
    }

    fn ledger(&self, rows: &[Value]) {
        write_jsonl(&self.alc.join("usage.jsonl"), rows);
    }

    fn native_claude(&self, rows: &[Value]) {
        write_jsonl(&self.claude.join("projects/fixture/session.jsonl"), rows);
    }

    fn native_codex(&self, rows: &[Value]) {
        write_jsonl(&self.codex.join("sessions/fixture/session.jsonl"), rows);
    }

    fn prices(&self, models: &str) {
        fs::write(
            self.alc.join("pricing.toml"),
            format!("{PRICE_HEADER}{models}"),
        )
        .unwrap();
    }
}

fn write_jsonl(path: &Path, rows: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    fs::write(path, text).unwrap();
}

fn timestamp(text: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .timestamp_millis()
        .try_into()
        .unwrap()
}

fn request(at: &str) -> Value {
    json!({
        "t": "request", "v": 3,
        "record": {
            "source": "alc", "timestamp_ms": timestamp(at), "agent": "claude",
            "profile": "fixture", "provider": "custom", "model": "fixture-model",
            "outcome": "completed", "granularity": "request", "billing": "api",
            "tokens": { "input_basis": "inclusive", "input_tokens": 100,
                "cache_read_tokens": 20, "cache_write_tokens": 10,
                "output_tokens": 10, "reasoning_tokens": 2 }
        }
    })
}

fn timing(streaming: bool, first: u64, terminal: u64) -> Value {
    json!({ "client_streaming": streaming, "first_content_us": first,
        "first_visible_us": first, "last_content_us": terminal - 1,
        "terminal_us": terminal, "elapsed_us": terminal, "output_basis": "gross" })
}

fn claude(at: &str, id: Option<&str>, output: u64, final_block: bool) -> Value {
    json!({ "type": "assistant", "timestamp": at, "sessionId": "fixture-session",
        "requestId": id.map(|id| format!("req-{id}")),
        "message": { "id": id, "model": "fixture-model",
            "stop_reason": if final_block { Some("end_turn") } else { None },
            "usage": { "input_tokens": 70, "cache_read_input_tokens": 20,
                "cache_creation_input_tokens": 10, "output_tokens": output } } })
}

fn codex_start(session: &str) -> Vec<Value> {
    vec![
        json!({ "type": "session_meta", "timestamp": "2026-01-01T22:00:00Z",
            "payload": { "id": session, "model_provider": "custom" } }),
        json!({ "type": "turn_context", "timestamp": "2026-01-01T22:00:01Z",
            "payload": { "turn_id": "fixture-turn", "model": "fixture-model" } }),
    ]
}

fn codex_total(at: &str, input: u64, cached: u64, output: u64) -> Value {
    let usage = json!({ "input_tokens": input, "cached_input_tokens": cached,
        "cache_write_input_tokens": 0, "output_tokens": output,
        "reasoning_output_tokens": 0, "total_tokens": input + output });
    json!({ "type": "event_msg", "timestamp": at,
        "payload": { "type": "token_count", "info": {
            "total_token_usage": usage, "last_token_usage": usage,
            "model_context_window": 1000 } } })
}

fn only_row(report: &Value) -> &Value {
    let rows = report["statistics"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{report}");
    &rows[0]
}

fn assert_null(value: &Value, key: &str) {
    assert_eq!(value.get(key), Some(&Value::Null), "{key}: {value}");
}

fn assert_close(value: &Value, expected: f64) {
    let actual = value.as_f64().expect("numeric metric");
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

#[test]
fn tps_defaults_to_alc_latest_twenty_and_honors_limit() {
    let fixture = Fixture::new();
    let rows: Vec<_> = (0..25)
        .map(|second| {
            let mut row = request(&format!("2026-01-02T00:00:{second:02}Z"));
            row["record"]["timing"] = timing(true, 100_000, 1_000_000);
            row
        })
        .collect();
    fixture.ledger(&rows);
    fixture.native_claude(&[claude("2026-01-03T00:00:00Z", Some("native"), 10, true)]);
    let report = fixture.json(&["tps"]);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["summary"]["requests"], 20);
    let shown = report["rows"].as_array().unwrap();
    assert_eq!(shown.len(), 20);
    assert!(shown.iter().all(|row| row["source"] == "alc"));
    let times: Vec<_> = shown
        .iter()
        .map(|row| row["timestamp_ms"].as_u64().unwrap())
        .collect();
    let expected: Vec<_> = rows[5..]
        .iter()
        .rev()
        .map(|row| row["record"]["timestamp_ms"].as_u64().unwrap())
        .collect();
    assert_eq!(times, expected);
    assert_eq!(report["sources"].as_array().unwrap().len(), 1);
    assert_eq!(report["sources"][0]["source"], "alc");
    let limited = fixture.json(&["tps", "--limit", "3"]);
    assert_eq!(limited["rows"], json!(shown[..3]));
    let excluded = fixture.json(&["tps", "--source", "claude", "--limit", "1"]);
    assert_eq!(excluded["rows"], json!([]));
    assert_eq!(excluded["coverage"]["excluded_unmeasured_records"], 1);
    let native = fixture.json(&[
        "tps",
        "--source",
        "claude",
        "--limit",
        "1",
        "--include-unmeasured",
    ]);
    assert_eq!(native["rows"][0]["source"], "claude");
}

#[test]
fn historical_timing_is_null_in_json_and_na_in_text() {
    let fixture = Fixture::new();
    fixture.ledger(&[json!({ "t": "turn", "v": 2, "ts": 1767312000,
        "agent": "claude", "provider": "fixture", "kind": "codex",
        "model": "fixture-model", "input_tokens": 100, "cached_tokens": 20,
        "output_tokens": 10, "reasoning_tokens": 2, "total_tokens": 110 })]);
    let excluded = fixture.json(&["tps"]);
    assert_eq!(excluded["rows"], json!([]));
    assert_eq!(excluded["coverage"]["excluded_legacy_records"], 1);
    let report = fixture.json(&["tps", "--include-unmeasured"]);
    assert_eq!(report["summary"]["requests"], 1);
    assert_null(&report["rows"][0], "timing");
    for metric in ["ttft_ms", "stream_tps", "e2e_tps"] {
        assert_null(&report["rows"][0]["metrics"], metric);
    }
    assert_eq!(report["summary"]["ttft_samples"], 0);
    assert_eq!(report["summary"]["stream_tps_samples"], 0);
    assert_eq!(report["summary"]["e2e_tps_samples"], 0);
    for metric in [
        "ttft_mean_ms",
        "ttft_p50_ms",
        "ttft_p95_ms",
        "weighted_stream_tps",
        "weighted_e2e_tps",
    ] {
        assert_null(&report["summary"], metric);
    }
    let text = fixture
        .command()
        .args(["tps", "--include-unmeasured"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(text).unwrap();
    assert!(text.contains("N/A"));
    assert!(text.contains("fixture-model"));
}

#[test]
fn tps_uses_first_token_ttft_and_duration_weighted_rates_not_mean_speeds() {
    let fixture = Fixture::new();
    let mut fast = request("2026-01-02T00:00:01Z");
    fast["record"]["tokens"]["output_tokens"] = json!(11);
    fast["record"]["timing"] = timing(true, 100_000, 1_100_000);
    let mut slow = request("2026-01-02T00:00:02Z");
    slow["record"]["tokens"]["output_tokens"] = json!(16);
    slow["record"]["timing"] = timing(true, 200_000, 3_200_000);
    let mut nonstream = request("2026-01-02T00:00:03Z");
    nonstream["record"]["tokens"]["output_tokens"] = json!(10);
    nonstream["record"]["timing"] = timing(false, 500_000, 2_000_000);
    fixture.ledger(&[fast, slow, nonstream]);
    let report = fixture.json(&["tps"]);
    let summary = &report["summary"];
    assert_eq!(summary["requests"], 3);
    assert_eq!(summary["ttft_samples"], 2);
    assert_close(&summary["ttft_mean_ms"], 150.0);
    assert_close(&summary["ttft_p50_ms"], 100.0);
    assert_close(&summary["ttft_p95_ms"], 200.0);
    assert_eq!(summary["stream_tps_samples"], 2);
    assert_close(&summary["weighted_stream_tps"], 25.0 / 4.0);
    assert_eq!(summary["e2e_tps_samples"], 3);
    assert_close(&summary["weighted_e2e_tps"], 37.0 / 6.3);
    let rows = report["rows"].as_array().unwrap();
    assert_null(&rows[0]["metrics"], "ttft_ms");
    assert_null(&rows[0]["metrics"], "stream_tps");
    assert_close(&rows[0]["metrics"]["e2e_tps"], 5.0);
    assert_close(&rows[1]["metrics"]["ttft_ms"], 200.0);
    assert_close(&rows[1]["metrics"]["stream_tps"], 5.0);
    assert_close(&rows[2]["metrics"]["stream_tps"], 10.0);
}

#[test]
fn offline_usage_ignores_malformed_logins_and_extends_schema_one() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    fixture.ledger(&[request("2026-01-02T00:00:00Z")]);
    let report = fixture.usage(&[]);
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["resolved_by"], "cli");
    assert!(report["generated_at"].as_u64().is_some());
    assert_eq!(report["accounts"], json!([]));
    assert_eq!(report["ledger"]["skipped_lines"], 0);
    assert_eq!(report["ledger"]["rows"][0]["provider"], "fixture");
    assert_eq!(report["ledger"]["rows"][0]["turns"], 1);
    assert_eq!(report["ledger"]["rows"][0]["input_tokens"], 100);
    assert_eq!(report["statistics"]["schema_version"], 1);
    assert_eq!(report["statistics"]["timezone"], "UTC");
    assert_eq!(report["statistics"]["records"], 1);
    assert_eq!(report["statistics"]["sources"].as_array().unwrap().len(), 3);
    assert_eq!(only_row(&report)["requests"], 1);
    assert_eq!(only_row(&report)["known_requests"], 1);
    assert_eq!(only_row(&report)["granularity"], "request");
    assert_eq!(report["statistics"]["total_usd"], "0.000315");
    fixture
        .command()
        .args(["usage", "--offline"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Accounts: not fetched (--offline)",
        ));
    assert_eq!(
        fs::read_to_string(fixture.alc.join("credentials.toml")).unwrap(),
        "not valid TOML ["
    );
    assert_eq!(
        fs::read_to_string(fixture.claude.join(".credentials.json")).unwrap(),
        "not valid JSON {"
    );
    assert_eq!(
        fs::read_to_string(fixture.codex.join("auth.json")).unwrap(),
        "not valid JSON {"
    );
}

#[test]
fn source_dates_offsets_filters_and_utc_periods_select_exact_boundaries() {
    let fixture = Fixture::new();
    let mut rows = vec![
        request("2026-01-01T23:59:59.999Z"),
        request("2026-01-02T00:00:00Z"),
        request("2026-01-02T15:59:59.999Z"),
        request("2026-01-02T16:00:00Z"),
        request("2026-01-03T00:00:00Z"),
    ];
    for (profile, agent, model) in [
        ("other", "claude", "fixture-model"),
        ("fixture", "codex", "fixture-model"),
        ("fixture", "claude", "fixture-model-extra"),
    ] {
        let mut row = request("2026-01-02T01:00:00Z");
        row["record"]["profile"] = json!(profile);
        row["record"]["agent"] = json!(agent);
        row["record"]["model"] = json!(model);
        rows.push(row);
    }
    fixture.ledger(&rows);
    fixture.native_claude(&[claude(
        "2026-01-02T08:00:00+08:00",
        Some("boundary"),
        10,
        true,
    )]);
    let filters = [
        "--source",
        "alc",
        "--since",
        "2026-01-02",
        "--until",
        "2026-01-03",
        "--filter-profile",
        "fixture",
        "--filter-agent",
        "claude",
        "--filter-model",
        "fixture-model",
    ];
    let report = fixture.usage(&filters);
    assert_eq!(report["statistics"]["records"], 3);
    assert_eq!(only_row(&report)["requests"], 3);
    assert_eq!(report["statistics"]["sources"].as_array().unwrap().len(), 1);
    let offset = fixture.usage(&[
        "--source",
        "alc",
        "--since",
        "2026-01-02T08:00:00+08:00",
        "--until",
        "2026-01-03T00:00:00+08:00",
        "--filter-profile",
        "fixture",
        "--filter-agent",
        "claude",
        "--filter-model",
        "fixture-model",
    ]);
    assert_eq!(offset["statistics"]["records"], 2);
    let native = fixture.usage(&[
        "--source",
        "claude",
        "--since",
        "2026-01-02",
        "--until",
        "2026-01-03",
        "--daily",
    ]);
    assert_eq!(only_row(&native)["period"], "2026-01-02");
    assert_eq!(only_row(&native)["source"], "claude");
    let excluded = fixture.usage(&["--source", "claude", "--filter-profile", "fixture"]);
    assert_eq!(excluded["statistics"]["records"], 0);
    assert_null(&excluded["statistics"], "total_usd");
    let selected = fixture.usage(&["--source", "alc,codex"]);
    let sources = selected["statistics"]["sources"].as_array().unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| source["source"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["alc", "codex"]
    );
    let monthly = fixture.usage(&["--source", "alc", "--monthly"]);
    assert!(
        monthly["statistics"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["period"] == "2026-01")
    );
    let tps = fixture.json(&[
        "tps",
        "--include-unmeasured",
        "--source",
        "alc",
        "--since",
        "2026-01-02",
        "--until",
        "2026-01-03",
        "--filter-profile",
        "fixture",
        "--filter-agent",
        "claude",
        "--filter-model",
        "fixture-model",
        "--limit",
        "2",
    ]);
    assert_eq!(tps["rows"].as_array().unwrap().len(), 2);
    assert_eq!(
        tps["rows"][0]["timestamp_ms"],
        timestamp("2026-01-02T16:00:00Z")
    );
    assert_eq!(
        tps["rows"][1]["timestamp_ms"],
        timestamp("2026-01-02T15:59:59.999Z")
    );
}

#[test]
fn invalid_sources_dates_limits_and_conflicting_periods_are_errors() {
    let fixture = Fixture::new();
    for command in ["tps", "usage"] {
        for (args, message) in [
            (vec!["--source", "bogus"], "unknown usage source"),
            (vec!["--source", "alc,bogus"], "unknown usage source"),
            (vec!["--since", "2026-02-30"], "valid YYYY-MM-DD"),
            (vec!["--until", "not-a-date"], "valid YYYY-MM-DD"),
            (vec!["--until", "2026-01-02T25:00:00Z"], "RFC3339"),
            (vec!["--since", "1969-12-31"], "1970-01-01"),
            (
                vec!["--since", "2026-01-03", "--until", "2026-01-03"],
                "must precede",
            ),
            (
                vec!["--since", "2026-01-04", "--until", "2026-01-03"],
                "must precede",
            ),
        ] {
            let mut invocation = fixture.command();
            invocation.arg(command);
            if command == "usage" {
                invocation.arg("--offline");
            }
            invocation
                .args(args)
                .assert()
                .failure()
                .stderr(predicates::str::contains(message));
        }
    }
    fixture
        .command()
        .args(["tps", "--limit", "0"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--limit"));
    fixture
        .command()
        .args(["usage", "--offline", "--daily", "--monthly"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("cannot be used with"));
    fixture
        .command()
        .args(["usage", "--offline", "--timezone", "Not/A_Zone"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("unknown timezone"));
    for args in [
        vec!["weekly", "--since", "2026-01-01"],
        vec!["monthly", "--until", "2026-02-01"],
        vec!["yearly", "--daily"],
        vec!["weekly", "--monthly"],
    ] {
        fixture
            .command()
            .args(["usage", "--offline"])
            .args(args)
            .assert()
            .failure()
            .stderr(predicates::str::contains("cannot be used with"));
    }
}

#[test]
fn custom_decimal_prices_are_exact_and_missing_models_counters_or_rates_are_not_free() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    fixture.ledger(&[request("2026-01-02T00:00:00Z")]);
    let complete = fixture.usage(&["--source", "alc"]);
    assert_eq!(only_row(&complete)["known_subtotal_usd"], "0.000315");
    assert_eq!(only_row(&complete)["total_usd"], "0.000315");
    assert_eq!(complete["statistics"]["priced_records"], 1);
    for model in [json!("fixture-model-extra"), Value::Null] {
        let mut row = request("2026-01-02T00:00:00Z");
        row["record"]["model"] = model;
        fixture.ledger(&[row]);
        let unknown = fixture.usage(&["--source", "alc"]);
        assert_eq!(unknown["statistics"]["unpriced_records"], 1);
        assert_eq!(only_row(&unknown)["known_subtotal_usd"], "0");
        assert_null(only_row(&unknown), "total_usd");
        assert_null(&unknown["statistics"], "total_usd");
    }
    let mut zero = request("2026-01-02T00:00:00Z");
    zero["record"]["tokens"] = json!({ "input_tokens": 0, "output_tokens": 0,
        "cache_read_tokens": 0, "cache_write_tokens": 0 });
    fixture.ledger(&[zero.clone()]);
    let measured = fixture.usage(&["--source", "alc"]);
    assert_eq!(only_row(&measured)["output_tokens"], 0);
    assert_eq!(measured["statistics"]["total_usd"], "0");
    zero["record"]["tokens"]
        .as_object_mut()
        .unwrap()
        .remove("output_tokens");
    fixture.ledger(&[zero]);
    let absent = fixture.usage(&["--source", "alc"]);
    assert_null(only_row(&absent), "output_tokens");
    assert_null(only_row(&absent), "total_usd");
    assert_null(&absent["statistics"], "total_usd");
    fixture.prices("[[models]]\nprovider=\"custom\"\nmodel=\"fixture-model\"\ninput=\"2.5\"\n");
    fixture.ledger(&[request("2026-01-02T00:00:00Z")]);
    let rates = fixture.usage(&["--source", "alc"]);
    assert_eq!(only_row(&rates)["known_subtotal_usd"], "0.000175");
    assert_null(only_row(&rates), "total_usd");
    assert_null(&rates["statistics"], "total_usd");
}

#[test]
fn legacy_mixed_zero_fields_are_unknown_but_new_measured_zeroes_remain_known() {
    let fixture = Fixture::new();
    fixture.ledger(&[json!({
        "t":"turn", "v":2, "ts":1767312000, "agent":"claude",
        "provider":"codex", "kind":"codex", "model":"gpt-4.1",
        "input_tokens":100, "cached_tokens":20, "output_tokens":0,
        "reasoning_tokens":0, "total_tokens":100
    })]);
    let report = fixture.usage(&["--source", "alc"]);
    let row = only_row(&report);
    assert_null(row, "output_tokens");
    assert_null(row, "total_usd");
    assert_null(&report["statistics"], "total_usd");
    assert_eq!(row["known_subtotal_usd"], "0.00017");
    assert_eq!(report["ledger"]["rows"][0]["output_tokens"], 0);
    fixture.prices(CUSTOM_PRICES);
    let mut measured = request("2026-01-02T00:00:00Z");
    measured["record"]["tokens"]["output_tokens"] = json!(0);
    measured["record"]["tokens"]["reasoning_tokens"] = json!(0);
    fixture.ledger(&[measured]);
    let report = fixture.usage(&["--source", "alc"]);
    assert_eq!(only_row(&report)["output_tokens"], 0);
    assert_eq!(report["statistics"]["total_usd"], "0.0002125");
}

#[test]
fn recorded_priority_tiers_never_use_a_standard_reference_price() {
    let fixture = Fixture::new();
    let mut row = request("2026-01-02T00:00:00Z");
    row["record"]["provider"] = json!("anthropic");
    row["record"]["model"] = json!("claude-sonnet-4-6");
    row["record"]["service_tier"] = json!("priority");
    row["record"]["tokens"] = json!({
        "input_basis":"separate", "input_tokens":100, "output_tokens":10,
        "cache_read_tokens":0, "cache_write_tokens":0
    });
    fixture.ledger(&[row]);
    let report = fixture.usage(&["--source", "alc"]);
    assert_null(only_row(&report), "total_usd");
    assert_null(&report["statistics"], "total_usd");
    assert_ne!(only_row(&report)["cost_status"], "complete");
}

#[test]
fn skipped_or_unsupported_source_records_make_grand_total_unknown() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    let valid = request("2026-01-02T00:00:00Z");
    for extra in [
        "{broken JSON",
        r#"{"t":"request","v":99,"record":{"source":"alc","timestamp_ms":1,"agent":"claude"}}"#,
    ] {
        fs::write(
            fixture.alc.join("usage.jsonl"),
            format!("{valid}\n{extra}\n"),
        )
        .unwrap();
        let report = fixture.usage(&["--source", "alc"]);
        assert_eq!(report["statistics"]["records"], 1);
        assert_eq!(report["statistics"]["known_subtotal_usd"], "0.000315");
        assert_null(&report["statistics"], "total_usd");
        let source = &report["statistics"]["sources"][0];
        assert!(
            source["skipped_lines"].as_u64().unwrap()
                + source["unsupported_records"].as_u64().unwrap()
                > 0
        );
    }
}

#[test]
fn claude_blocks_and_copies_use_one_snapshot_and_exact_ids_join_alc() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    let mut early = claude("2026-01-02T00:00:00Z", Some("msg-fixture"), 1, false);
    early["uuid"] = json!("block-one");
    let mut final_block = claude("2026-01-02T00:00:01Z", Some("msg-fixture"), 10, true);
    final_block["uuid"] = json!("block-two");
    fixture.native_claude(&[early, final_block.clone()]);
    final_block["sessionId"] = json!("copied-session");
    final_block["uuid"] = json!("copied-block");
    write_jsonl(
        &fixture.claude.join("projects/fixture/subagents/copy.jsonl"),
        &[final_block],
    );
    let native = fixture.usage(&["--source", "claude"]);
    assert_eq!(native["statistics"]["records"], 1);
    assert_eq!(only_row(&native)["requests"], 1);
    assert_eq!(only_row(&native)["input_tokens"], 100);
    assert_eq!(only_row(&native)["output_tokens"], 10);
    let mut observed = request("2026-01-02T00:00:01Z");
    observed["record"]["ids"] = json!([{ "protocol": "messages", "id": "msg-fixture" }]);
    observed["record"]["timing"] = timing(true, 100_000, 1_000_000);
    fixture.ledger(&[observed]);
    let joined = fixture.usage(&["--source", "alc,claude"]);
    assert_eq!(joined["statistics"]["records"], 1);
    assert_eq!(joined["statistics"]["deduplicated_records"], 1);
    assert_eq!(joined["statistics"]["possible_overlap"], false);
    assert_eq!(only_row(&joined)["source"], "alc");
    assert_eq!(joined["statistics"]["total_usd"], "0.000315");
    let tps = fixture.json(&["tps", "--source", "alc,claude"]);
    assert_eq!(tps["rows"].as_array().unwrap().len(), 1);
    assert_eq!(tps["rows"][0]["provenance"], json!(["alc", "claude"]));
    fixture.ledger(&[request("2026-01-02T00:00:01Z")]);
    let unjoined = fixture.usage(&["--source", "alc,claude"]);
    assert_eq!(unjoined["statistics"]["records"], 2);
    assert_eq!(unjoined["statistics"]["deduplicated_records"], 0);
    assert_eq!(unjoined["statistics"]["possible_overlap"], true);
    assert_null(&unjoined["statistics"], "known_subtotal_usd");
    assert_null(&unjoined["statistics"], "total_usd");
    // Identical counters/timestamps are not identities. With the same model,
    // anonymous snapshots must also remain a distinct granularity group.
    fixture.native_claude(&[
        claude("2026-01-02T00:00:01Z", Some("msg-fixture"), 10, true),
        claude("2026-01-02T00:00:01Z", None, 10, true),
        claude("2026-01-02T00:00:01Z", None, 10, true),
    ]);
    let anonymous = fixture.usage(&["--source", "alc,claude"]);
    assert_eq!(anonymous["statistics"]["records"], 4);
    assert_eq!(anonymous["statistics"]["possible_overlap"], true);
    assert_null(&anonymous["statistics"], "known_subtotal_usd");
    assert_null(&anonymous["statistics"], "total_usd");
    let native_rows: Vec<_> = anonymous["statistics"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["source"] == "claude")
        .collect();
    assert_eq!(native_rows.len(), 2);
    let checkpoint = native_rows
        .iter()
        .find(|row| row["granularity"] == "checkpoint")
        .unwrap();
    let identified = native_rows
        .iter()
        .find(|row| row["granularity"] == "request")
        .unwrap();
    assert_eq!(checkpoint["model"], identified["model"]);
    assert_eq!(checkpoint["records"], 2);
    assert_null(checkpoint, "requests");
    assert_null(checkpoint, "input_tokens");
    assert_null(checkpoint, "total_usd");
    assert_eq!(checkpoint["known_requests"], 0);
    assert_eq!(identified["requests"], 1);
    assert_eq!(identified["known_requests"], 1);
}

#[test]
fn codex_cumulative_deltas_fold_baselines_before_date_filter_and_are_not_requests() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    let mut rows = codex_start("fixture-cumulative");
    rows.extend([
        codex_total("2026-01-01T22:00:02Z", 0, 0, 0),
        codex_total("2026-01-01T23:00:00Z", 100, 20, 10),
        codex_total("2026-01-01T23:01:00Z", 100, 20, 10),
        codex_total("2026-01-02T01:00:00Z", 200, 40, 20),
        codex_total("2026-01-02T01:01:00Z", 200, 40, 20),
    ]);
    fixture.native_codex(&rows);
    write_jsonl(&fixture.codex.join("archived_sessions/copy.jsonl"), &rows);
    let report = fixture.usage(&[
        "--source",
        "codex",
        "--since",
        "2026-01-02",
        "--until",
        "2026-01-03",
        "--daily",
    ]);
    assert_eq!(report["statistics"]["records"], 1);
    let row = only_row(&report);
    assert_eq!(row["granularity"], "cumulative-delta");
    assert_eq!(row["period"], "2026-01-02");
    assert_eq!(row["input_tokens"], 100);
    assert_eq!(row["cache_read_tokens"], 20);
    assert_eq!(row["output_tokens"], 10);
    assert_null(row, "requests");
    assert_eq!(row["known_requests"], 0);
    assert_eq!(report["statistics"]["total_usd"], "0.000305");
    let all = fixture.usage(&["--source", "codex"]);
    assert_eq!(only_row(&all)["records"], 2);
    assert_eq!(only_row(&all)["input_tokens"], 200);
    assert_eq!(all["statistics"]["total_usd"], "0.00061");
}

#[test]
fn tps_filters_before_limit_and_preserves_failed_cancelled_and_no_usage_requests() {
    let fixture = Fixture::new();
    let mut observed = Vec::new();
    for (index, outcome) in ["failed", "cancelled", "completed"].into_iter().enumerate() {
        let mut row = request(&format!("2026-01-01T00:00:0{index}Z"));
        row["record"]["outcome"] = json!(outcome);
        row["record"]["timing"] = timing(true, 100_000, 1_000_000);
        if outcome == "completed" {
            row["record"]["tokens"] = json!({});
        }
        observed.push(row);
    }
    for second in 0..25 {
        observed.push(json!({ "t": "turn", "v": 2, "ts": 1767312000 + second,
            "agent": "claude", "provider": "fixture", "kind": "codex",
            "model": "fixture-model", "input_tokens": 100, "cached_tokens": 20,
            "output_tokens": 10, "reasoning_tokens": 2, "total_tokens": 110 }));
    }
    observed.push(request("2026-01-03T00:00:00Z"));
    let mut checkpoint = request("2026-01-04T00:00:00Z");
    checkpoint["record"]["granularity"] = json!("checkpoint");
    observed.push(checkpoint);
    fixture.ledger(&observed);
    let report = fixture.json(&["tps", "--limit", "3"]);
    assert_eq!(report["rows"].as_array().unwrap().len(), 3);
    assert_eq!(report["rows"][0]["outcome"], "completed");
    assert_eq!(report["rows"][1]["outcome"], "cancelled");
    assert_eq!(report["rows"][2]["outcome"], "failed");
    assert_eq!(report["coverage"]["matching_records"], 30);
    assert_eq!(report["coverage"]["measured_requests"], 3);
    assert_eq!(report["coverage"]["excluded_legacy_records"], 25);
    assert_eq!(report["coverage"]["excluded_unmeasured_records"], 1);
    assert_eq!(report["coverage"]["excluded_nonrequest_records"], 1);
    assert_eq!(report["summary"]["ttft_samples"], 3);
    assert_eq!(report["summary"]["e2e_tps_samples"], 2);
    assert_null(&report["rows"][0]["metrics"], "e2e_tps");
    let inclusive = fixture.json(&["tps", "--limit", "2", "--include-unmeasured"]);
    assert_eq!(inclusive["rows"][0]["granularity"], "checkpoint");
    assert_eq!(inclusive["coverage"]["excluded_legacy_records"], 0);
    assert_eq!(inclusive["coverage"]["limited_records"], 28);
    assert_eq!(
        fs::read_to_string(fixture.alc.join("credentials.toml")).unwrap(),
        "not valid TOML ["
    );
}

#[test]
fn daily_rollups_preserve_gross_input_and_expose_exact_disjoint_tokens_and_fees() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    fixture.ledger(&[
        request("2026-01-02T00:00:00Z"),
        request("2026-01-02T12:00:00Z"),
        request("2026-01-03T00:00:00Z"),
    ]);
    let report = fixture.usage(&["--source", "alc"]);
    let row = only_row(&report);
    assert_eq!(row["period"], "all-time");
    assert_eq!(row["input_tokens"], 300);
    assert_eq!(row["uncached_input_tokens"], 210);
    let daily = report["statistics"]["daily_rollups"].as_array().unwrap();
    assert_eq!(daily.len(), 2);
    assert_eq!(daily[0]["period"], "2026-01-02");
    assert_eq!(daily[0]["records"], 2);
    assert_eq!(daily[0]["input_tokens"], 200);
    assert_eq!(daily[0]["uncached_input_tokens"], 140);
    assert_eq!(daily[0]["cache_read_tokens"], 40);
    assert_eq!(daily[0]["cache_write_tokens"], 20);
    assert_eq!(daily[0]["output_tokens"], 20);
    let fees = &daily[0]["cost_components"];
    assert_eq!(fees["uncached_input"]["total_usd"], "0.00035");
    assert_eq!(fees["cache_read"]["total_usd"], "0.000005");
    assert_eq!(fees["cache_write"]["total_usd"], "0.00007");
    assert_eq!(fees["output"]["total_usd"], "0.000205");
    assert_eq!(daily[0]["total_usd"], "0.00063");
    assert_eq!(report["statistics"]["total_usd"], "0.000945");
    assert_null(&report["statistics"]["range"], "start");
    assert_null(&report["statistics"]["range"], "end");
    assert_null(&report["statistics"], "window");
    assert_eq!(report["statistics"]["uncached_input_tokens"], 210);
    fixture.prices("[[models]]\nprovider=\"custom\"\nmodel=\"fixture-model\"\ninput=\"2.5\"\n");
    let partial = fixture.usage(&["--source", "alc"]);
    let first_day = &partial["statistics"]["daily_rollups"][0];
    assert_eq!(first_day["known_subtotal_usd"], "0.00035");
    assert_null(first_day, "total_usd");
    assert_eq!(
        first_day["cost_components"]["cache_read"]["known_subtotal_usd"],
        "0"
    );
    assert_null(&first_day["cost_components"]["cache_read"], "total_usd");
    for preset in ["weekly", "monthly", "yearly"] {
        let current = fixture.usage(&[preset, "--source", "alc"]);
        assert_eq!(current["statistics"]["window"], preset);
        assert!(current["statistics"]["range"]["start"].as_str().is_some());
        assert!(current["statistics"]["range"]["end"].as_str().is_some());
    }
}

#[test]
fn per_record_tier_and_ttl_fees_are_summed_before_daily_formatting() {
    let fixture = Fixture::new();
    fixture.prices(
        r#"
[[models]]
provider="custom"
model="fixture-model"
input="1"
output="2"
cache_read="0.1"
cache_write_5m="3"
cache_write_1h="6"
[[models]]
provider="custom"
model="fixture-model"
tier="priority"
input="4"
output="5"
cache_read="0.2"
cache_write_5m="7"
cache_write_1h="9"
"#,
    );
    let mut standard = request("2026-01-02T00:00:00Z");
    standard["record"]["tokens"] = json!({ "input_basis": "separate", "input_tokens": 1,
        "cache_read_tokens": 1, "cache_write_5m_tokens": 1, "cache_write_1h_tokens": 1,
        "output_tokens": 1 });
    let mut priority = standard.clone();
    priority["record"]["timestamp_ms"] = json!(timestamp("2026-01-02T12:00:00Z"));
    priority["record"]["service_tier"] = json!("priority");
    fixture.ledger(&[standard, priority]);
    let report = fixture.usage(&["--source", "alc"]);
    assert_eq!(report["statistics"]["total_usd"], "0.0000373");
    let day = &report["statistics"]["daily_rollups"][0];
    assert_eq!(day["input_tokens"], 8);
    assert_eq!(day["uncached_input_tokens"], 2);
    assert_eq!(day["cache_write_tokens"], 4);
    assert_eq!(
        day["cost_components"]["uncached_input"]["total_usd"],
        "0.000005"
    );
    assert_eq!(
        day["cost_components"]["cache_read"]["total_usd"],
        "0.0000003"
    );
    assert_eq!(
        day["cost_components"]["cache_write"]["total_usd"],
        "0.000025"
    );
    assert_eq!(day["cost_components"]["output"]["total_usd"], "0.000007");
}

#[test]
fn timezone_controls_date_bounds_daily_buckets_and_legacy_monthly_grouping() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    fixture.ledger(&[
        request("2024-03-10T04:59:59.999Z"),
        request("2024-03-10T05:00:00Z"),
        request("2024-03-11T03:59:59.999Z"),
        request("2024-03-11T04:00:00Z"),
    ]);
    let report = fixture.usage(&[
        "--source",
        "alc",
        "--timezone",
        "America/New_York",
        "--since",
        "2024-03-10",
        "--until",
        "2024-03-11",
        "--daily",
    ]);
    assert_eq!(report["statistics"]["records"], 2);
    assert_eq!(report["statistics"]["timezone"], "America/New_York");
    assert_eq!(
        report["statistics"]["range"]["start"],
        "2024-03-10T00:00:00-05:00"
    );
    assert_eq!(
        report["statistics"]["range"]["end"],
        "2024-03-11T00:00:00-04:00"
    );
    assert_eq!(only_row(&report)["period"], "2024-03-10");
    assert_eq!(
        report["statistics"]["daily_rollups"][0]["period"],
        "2024-03-10"
    );
    fixture.ledger(&[request("2024-01-31T16:00:00Z")]);
    let monthly = fixture.usage(&["--source", "alc", "--timezone", "Asia/Taipei", "--monthly"]);
    assert_eq!(only_row(&monthly)["period"], "2024-02");
    assert_eq!(
        monthly["statistics"]["daily_rollups"][0]["period"],
        "2024-02-01"
    );
}

#[test]
fn overlap_scope_is_filtered_then_recomputed_per_day_without_fake_pooled_sums() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    fixture.ledger(&[request("2026-01-02T00:00:00Z")]);
    fixture.native_claude(&[claude("2026-01-03T00:00:00Z", Some("different"), 10, true)]);
    let all = fixture.usage(&["--source", "alc,claude"]);
    assert_eq!(all["statistics"]["possible_overlap"], true);
    assert_null(&all["statistics"], "uncached_input_tokens");
    assert_null(&all["statistics"], "known_subtotal_usd");
    let daily = all["statistics"]["daily_rollups"].as_array().unwrap();
    assert_eq!(daily.len(), 2);
    assert!(daily.iter().all(|day| day["possible_overlap"] == false));
    assert!(daily.iter().all(|day| day["uncached_input_tokens"] == 70));
    let filtered = fixture.usage(&[
        "--source",
        "alc,claude",
        "--since",
        "2026-01-02",
        "--until",
        "2026-01-03",
    ]);
    assert_eq!(filtered["statistics"]["possible_overlap"], false);
    assert_eq!(filtered["statistics"]["total_usd"], "0.000315");
    fixture.native_claude(&[claude("2026-01-02T01:00:00Z", Some("different"), 10, true)]);
    let same_day = fixture.usage(&["--source", "alc,claude"]);
    let day = &same_day["statistics"]["daily_rollups"][0];
    assert_eq!(day["possible_overlap"], true);
    for field in [
        "input_tokens",
        "uncached_input_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "output_tokens",
        "known_subtotal_usd",
        "total_usd",
    ] {
        assert_null(day, field);
    }
    assert_null(
        &day["cost_components"]["uncached_input"],
        "known_subtotal_usd",
    );
    assert_null(&day["cost_components"]["output"], "total_usd");
    let profile = fixture.usage(&["--source", "alc,claude", "--filter-profile", "fixture"]);
    assert_eq!(profile["statistics"]["possible_overlap"], false);
    assert_eq!(profile["statistics"]["total_usd"], "0.000315");
}

#[test]
fn exact_full_integer_text_and_daily_fee_headers_do_not_round_large_values() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    let mut row = request("2026-01-02T00:00:00Z");
    row["record"]["profile"] = json!("模型");
    row["record"]["tokens"] = json!({ "input_tokens": 9007199254740993_u64,
        "cache_read_tokens": 0, "cache_write_tokens": 0, "output_tokens": 1234567 });
    fixture.ledger(&[row]);
    let output = fixture
        .command()
        .args(["usage", "--offline", "--source", "alc"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    for expected in [
        "9,007,199,254,740,993",
        "1,234,567",
        "Daily totals",
        "PROVIDER",
        "GRANULARITY",
        "CACHE WRITE USD",
        "OUTPUT USD",
        "TOTAL",
        "模型",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("1.2M"));
    let report = fixture.usage(&["--source", "alc"]);
    assert_eq!(
        report["statistics"]["daily_rollups"][0]["uncached_input_tokens"],
        9007199254740993_u64
    );
    assert_eq!(report["statistics"]["total_usd"], "22517998149.50679425");
}

#[test]
fn codex_checkpoints_are_grouped_separately_and_never_summed_as_tokens_or_requests() {
    let fixture = Fixture::new();
    fixture.prices(CUSTOM_PRICES);
    let mut rows = codex_start("fixture-checkpoints");
    rows.extend([
        codex_total("2026-01-02T00:00:00Z", 100, 20, 10),
        codex_total("2026-01-02T01:00:00Z", 200, 40, 20),
        codex_total("2026-01-02T02:00:00Z", 5, 0, 1),
    ]);
    fixture.native_codex(&rows);
    let report = fixture.usage(&["--source", "codex"]);
    assert_eq!(report["statistics"]["records"], 3);
    let rows = report["statistics"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let delta = rows
        .iter()
        .find(|row| row["granularity"] == "cumulative-delta")
        .unwrap();
    let checkpoints = rows
        .iter()
        .find(|row| row["granularity"] == "checkpoint")
        .unwrap();
    assert_eq!(delta["records"], 1);
    assert_eq!(delta["input_tokens"], 100);
    assert_eq!(delta["output_tokens"], 10);
    assert_eq!(checkpoints["records"], 2);
    for field in [
        "input_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "output_tokens",
        "requests",
        "total_usd",
    ] {
        assert_null(checkpoints, field);
    }
    assert_eq!(checkpoints["known_requests"], 0);
    assert_eq!(checkpoints["known_subtotal_usd"], "0");
    assert_null(delta, "requests");
    assert_eq!(delta["known_requests"], 0);
    assert_eq!(report["statistics"]["known_subtotal_usd"], "0.000305");
    assert_null(&report["statistics"], "total_usd");
}
