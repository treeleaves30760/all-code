//! What each vendor will tell alc about what is left.
//!
//! # Why every endpoint is a constant
//!
//! A profile's `base_url` is where its *inference* goes, and for a good number
//! of users that is a proxy. Quota lives at the vendor, so these URLs are fixed
//! and a profile pointing somewhere else is reported as having no quota API
//! rather than having its key posted to a host that did not issue it.
//!
//! # Why the parsers are so forgiving
//!
//! Most of these endpoints are undocumented and all of them are outside alc's
//! control. Every wire struct defaults every field, plan names are strings
//! rather than enums, and two of these vendors answer HTTP 200 with the real
//! status buried in the body. A shape that changes should cost one row, not
//! the whole command.

use std::time::Duration;

use serde::Deserialize;

use super::accounts::{ClaudeLogin, CodexLogin, KeyVendor};
use super::{AccountState, Balance, Credits, Window};

/// Long enough for a vendor on a slow link, short enough that one dead host
/// does not make `alc usage` feel hung.
const QUOTA_TIMEOUT: Duration = Duration::from_secs(8);
/// Every payload here is a few KiB. Anything larger is not the endpoint alc
/// thinks it is.
const QUOTA_BODY_LIMIT: u64 = 256 * 1024;

/// Named so the endpoint's own rate limiter buckets alc with the client whose
/// token this is; Anthropic has been observed to throttle unrecognised agents
/// hard. Bumped by hand.
const CLAUDE_USER_AGENT: &str = "claude-code/2.1.0";

const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OPENROUTER_KEY_URL: &str = "https://openrouter.ai/api/v1/key";
const DEEPSEEK_BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
const MOONSHOT_BALANCE_URL: &str = "https://api.moonshot.ai/v1/users/me/balance";
const MINIMAX_REMAINS_URL: &str = "https://www.minimax.io/v1/token_plan/remains";
const ZAI_QUOTA_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

/// What one vendor answered, in the report's own vocabulary.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub plan: Option<String>,
    pub label: Option<String>,
    pub windows: Vec<Window>,
    pub balance: Option<Balance>,
    pub credits: Option<Credits>,
    pub state: Option<AccountState>,
    pub error: Option<String>,
}

impl Outcome {
    fn failed(state: AccountState, error: String) -> Self {
        Self {
            state: Some(state),
            error: Some(error),
            ..Self::default()
        }
    }
}

/// The vendor URL, or the same path on `ALC_USAGE_API_BASE` in a debug build.
///
/// One override for every vendor: each path is distinct, so one fake server
/// can answer all of them by routing on the path. Compiled out of release
/// builds, so a shipped binary cannot be pointed anywhere else.
fn endpoint(url: &'static str) -> String {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("ALC_USAGE_API_BASE")
        && !base.is_empty()
    {
        return rebase(&base, url);
    }
    url.to_owned()
}

/// Splits a URL at the start of its path and puts `base` in front of it.
///
/// Only reachable from the debug-only override above, so a release build has
/// no caller for it.
#[cfg(any(debug_assertions, test))]
fn rebase(base: &str, url: &str) -> String {
    let path_start = url
        .find("://")
        .map(|index| index + 3)
        .and_then(|index| url[index..].find('/').map(|slash| index + slash))
        .unwrap_or(url.len());
    format!("{}{}", base.trim_end_matches('/'), &url[path_start..])
}

struct Fetched {
    status: u16,
    body: String,
}

fn get(url: &str, headers: &[(&str, &str)]) -> Result<Fetched, String> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(QUOTA_TIMEOUT))
        // A 401 body says which login was refused; throwing it away would
        // leave alc guessing at the reason.
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut request = agent.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request
        .call()
        .map_err(|error| format!("could not reach {}: {error}", host_of(url)))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(QUOTA_BODY_LIMIT)
        .read_to_string()
        .map_err(|error| format!("could not read {}: {error}", host_of(url)))?;
    Ok(Fetched { status, body })
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
}

/// Turns a non-2xx answer into the state and the sentence the user reads.
fn refused(status: u16, url: &str, signed_out: &str) -> Option<Outcome> {
    match status {
        200..=299 => None,
        401 | 403 => Some(Outcome::failed(
            AccountState::SignedOut,
            signed_out.to_owned(),
        )),
        429 => Some(Outcome::failed(
            AccountState::Error,
            format!("{} is rate limiting alc; try again later", host_of(url)),
        )),
        other => Some(Outcome::failed(
            AccountState::Error,
            format!("{} answered {other}", host_of(url)),
        )),
    }
}

fn unparsable(url: &str) -> Outcome {
    Outcome::failed(
        AccountState::Error,
        format!(
            "{} sent something that is not the expected JSON",
            host_of(url)
        ),
    )
}

// ------------------------------------------------------------------ codex

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexUsage {
    /// A string, not an enum: a plan name alc has never heard of must not
    /// fail the parse.
    plan_type: Option<String>,
    rate_limit: Option<CodexLimit>,
    credits: Option<CodexCredits>,
    additional_rate_limits: Option<Vec<CodexNamedLimit>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexLimit {
    limit_reached: Option<bool>,
    primary_window: Option<CodexWindow>,
    secondary_window: Option<CodexWindow>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexWindow {
    used_percent: f64,
    limit_window_seconds: u64,
    reset_after_seconds: u64,
    reset_at: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexCredits {
    has_credits: bool,
    unlimited: bool,
    balance: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CodexNamedLimit {
    limit_name: String,
    rate_limit: Option<CodexLimit>,
}

pub(crate) fn fetch_codex(login: &CodexLogin, now_ms: u64) -> Outcome {
    if login.expires_at_ms <= now_ms {
        return Outcome {
            plan: login.plan_hint.clone(),
            ..Outcome::failed(
                AccountState::SignedOut,
                "the Codex token has expired; run `codex login`, or start a bridged session once so the adapter refreshes it"
                    .to_owned(),
            )
        };
    }

    let url = endpoint(CODEX_USAGE_URL);
    let bearer = format!("Bearer {}", login.access_token);
    let mut headers = vec![
        ("authorization", bearer.as_str()),
        ("accept", "application/json"),
        ("user-agent", crate::bridge::upstream::USER_AGENT),
        ("originator", crate::bridge::upstream::USER_AGENT),
    ];
    if let Some(account) = login.account_id.as_deref() {
        headers.push(("chatgpt-account-id", account));
    }

    let fetched = match get(&url, &headers) {
        Ok(fetched) => fetched,
        Err(error) => {
            return Outcome {
                plan: login.plan_hint.clone(),
                ..Outcome::failed(AccountState::Error, error)
            };
        }
    };
    if let Some(outcome) = refused(
        fetched.status,
        &url,
        "the Codex login was refused; run `codex login`",
    ) {
        return Outcome {
            plan: login.plan_hint.clone(),
            ..outcome
        };
    }
    match serde_json::from_str::<CodexUsage>(&fetched.body) {
        Ok(usage) => map_codex(usage, login),
        Err(_) => Outcome {
            plan: login.plan_hint.clone(),
            ..unparsable(&url)
        },
    }
}

fn map_codex(usage: CodexUsage, login: &CodexLogin) -> Outcome {
    let mut windows = Vec::new();
    let mut exhausted = false;
    let mut collect = |limit: Option<CodexLimit>, scope: Option<&str>| {
        let Some(limit) = limit else { return };
        exhausted |= limit.limit_reached == Some(true);
        for window in [limit.primary_window, limit.secondary_window]
            .into_iter()
            .flatten()
        {
            windows.push(Window {
                name: window_name(window.limit_window_seconds),
                scope: scope.map(str::to_owned),
                used_percent: clamp_percent(window.used_percent),
                resets_at: window
                    .reset_at
                    .filter(|at| *at > 0)
                    .or_else(|| Some(super::ledger::now_unix() + window.reset_after_seconds)),
                remaining: None,
                limit: None,
            });
        }
    };
    collect(usage.rate_limit, None);
    for named in usage.additional_rate_limits.unwrap_or_default() {
        let name = named.limit_name.clone();
        collect(named.rate_limit, Some(&name));
    }

    let credits = usage.credits.map(|credits| Credits {
        has_credits: credits.has_credits,
        unlimited: credits.unlimited,
        balance: credits.balance,
    });
    // The bridge's own rule: a plan at its limit is only exhausted when there
    // are no credits behind it to keep the session going.
    let spendable = credits
        .as_ref()
        .is_some_and(|credits| credits.has_credits || credits.unlimited);

    Outcome {
        plan: usage.plan_type.or_else(|| login.plan_hint.clone()),
        label: login.label.clone(),
        state: (exhausted && !spendable).then_some(AccountState::Exhausted),
        windows,
        credits,
        balance: None,
        error: None,
    }
}

// ----------------------------------------------------------------- claude

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClaudeUsage {
    five_hour: Option<ClaudeWindow>,
    seven_day: Option<ClaudeWindow>,
    seven_day_opus: Option<ClaudeWindow>,
    seven_day_sonnet: Option<ClaudeWindow>,
    extra_usage: Option<ClaudeExtra>,
    /// Newer accounts carry their per-model windows here and leave the
    /// `seven_day_*` keys null.
    limits: Option<Vec<ClaudeLimit>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClaudeWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClaudeExtra {
    is_enabled: bool,
    used_credits: Option<f64>,
    monthly_limit: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClaudeLimit {
    kind: Option<String>,
    percent: Option<f64>,
    resets_at: Option<String>,
    is_active: Option<bool>,
    scope: Option<ClaudeScope>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClaudeScope {
    model: Option<ClaudeModel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClaudeModel {
    display_name: Option<String>,
}

pub(crate) fn fetch_claude(login: &ClaudeLogin, label: Option<String>, now_ms: u64) -> Outcome {
    if login.expires_at_ms > 0 && login.expires_at_ms <= now_ms {
        return Outcome {
            plan: login.subscription.clone(),
            label,
            ..Outcome::failed(
                AccountState::SignedOut,
                "the Claude login has expired; open `claude` once so it refreshes".to_owned(),
            )
        };
    }

    let url = endpoint(CLAUDE_USAGE_URL);
    let bearer = format!("Bearer {}", login.access_token);
    let fetched = match get(
        &url,
        &[
            ("authorization", bearer.as_str()),
            ("anthropic-beta", "oauth-2025-04-20"),
            ("accept", "application/json"),
            ("user-agent", CLAUDE_USER_AGENT),
        ],
    ) {
        Ok(fetched) => fetched,
        Err(error) => {
            return Outcome {
                plan: login.subscription.clone(),
                label,
                ..Outcome::failed(AccountState::Error, error)
            };
        }
    };
    if let Some(outcome) = refused(
        fetched.status,
        &url,
        "the Claude login was refused; open `claude` and sign in again",
    ) {
        return Outcome {
            plan: login.subscription.clone(),
            label,
            ..outcome
        };
    }
    match serde_json::from_str::<ClaudeUsage>(&fetched.body) {
        Ok(usage) => {
            let mut outcome = map_claude(usage);
            outcome.plan = login.subscription.clone();
            outcome.label = label;
            outcome
        }
        Err(_) => Outcome {
            plan: login.subscription.clone(),
            label,
            ..unparsable(&url)
        },
    }
}

fn map_claude(usage: ClaudeUsage) -> Outcome {
    let mut windows = Vec::new();
    let mut push = |name: &str, scope: Option<String>, window: Option<ClaudeWindow>| {
        let Some(window) = window else { return };
        let Some(used) = window.utilization else {
            return;
        };
        windows.push(Window {
            name: name.to_owned(),
            scope,
            used_percent: clamp_percent(used),
            resets_at: window.resets_at.as_deref().and_then(parse_rfc3339_utc),
            remaining: None,
            limit: None,
        });
    };
    push("5h", None, usage.five_hour);
    push("week", None, usage.seven_day);

    // Prefer the newer array: on accounts that have it, the legacy per-model
    // keys come back null and only these carry a number.
    let scoped: Vec<&ClaudeLimit> = usage
        .limits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|limit| {
            limit.kind.as_deref() == Some("weekly_scoped") && limit.is_active != Some(false)
        })
        .collect();
    if scoped.is_empty() {
        push("week", Some("Opus".to_owned()), usage.seven_day_opus);
        push("week", Some("Sonnet".to_owned()), usage.seven_day_sonnet);
    } else {
        for limit in scoped {
            let Some(percent) = limit.percent else {
                continue;
            };
            windows.push(Window {
                name: "week".to_owned(),
                scope: limit
                    .scope
                    .as_ref()
                    .and_then(|scope| scope.model.as_ref())
                    .and_then(|model| model.display_name.clone()),
                used_percent: clamp_percent(percent),
                resets_at: limit.resets_at.as_deref().and_then(parse_rfc3339_utc),
                remaining: None,
                limit: None,
            });
        }
    }

    let balance = usage
        .extra_usage
        .filter(|extra| extra.is_enabled)
        .map(|extra| Balance {
            // The endpoint counts in cents.
            used: extra.used_credits.map(|credits| credits / 100.0),
            limit: extra
                .monthly_limit
                .filter(|limit| *limit > 0.0)
                .map(|limit| limit / 100.0),
            remaining: None,
            unit: "USD".to_owned(),
        });

    Outcome {
        windows,
        balance,
        ..Outcome::default()
    }
}

// -------------------------------------------------------------- key kinds

pub(crate) fn fetch_key(vendor: KeyVendor, key: &str, profile: &str) -> Outcome {
    let (url, bearer) = match vendor {
        KeyVendor::Openrouter => (endpoint(OPENROUTER_KEY_URL), true),
        KeyVendor::Deepseek => (endpoint(DEEPSEEK_BALANCE_URL), true),
        KeyVendor::Moonshot => (endpoint(MOONSHOT_BALANCE_URL), true),
        KeyVendor::Minimax => (endpoint(MINIMAX_REMAINS_URL), true),
        KeyVendor::Zai => (endpoint(ZAI_QUOTA_URL), true),
    };
    let authorization = if bearer {
        format!("Bearer {key}")
    } else {
        key.to_owned()
    };
    let fetched = match get(
        &url,
        &[
            ("authorization", authorization.as_str()),
            ("accept", "application/json"),
            ("user-agent", concat!("alc/", env!("CARGO_PKG_VERSION"))),
        ],
    ) {
        Ok(fetched) => fetched,
        Err(error) => return Outcome::failed(AccountState::Error, error),
    };
    let refused_message = format!("the API key was refused; run `alc config key {profile}`");
    if let Some(outcome) = refused(fetched.status, &url, &refused_message) {
        return outcome;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&fetched.body) else {
        return unparsable(&url);
    };
    match vendor {
        KeyVendor::Openrouter => map_openrouter(&value),
        KeyVendor::Deepseek => map_deepseek(&value),
        KeyVendor::Moonshot => map_moonshot(&value),
        KeyVendor::Minimax => map_minimax(&value, &refused_message),
        KeyVendor::Zai => map_zai(&value, &refused_message),
    }
}

fn number(value: &serde_json::Value, key: &str) -> Option<f64> {
    let found = value.get(key)?;
    found
        .as_f64()
        // DeepSeek sends its balances as decimal strings.
        .or_else(|| found.as_str().and_then(|text| text.parse().ok()))
}

fn map_openrouter(value: &serde_json::Value) -> Outcome {
    let data = value.get("data").unwrap_or(value);
    let used = number(data, "usage");
    let limit = number(data, "limit");
    Outcome {
        label: data
            .get("label")
            .and_then(|label| label.as_str())
            .filter(|label| !label.is_empty())
            .map(str::to_owned),
        balance: Some(Balance {
            remaining: number(data, "limit_remaining"),
            limit,
            used,
            unit: "USD".to_owned(),
        }),
        ..Outcome::default()
    }
}

fn map_deepseek(value: &serde_json::Value) -> Outcome {
    let first = value
        .get("balance_infos")
        .and_then(|infos| infos.as_array())
        .and_then(|infos| infos.first());
    let balance = first.map(|info| Balance {
        remaining: number(info, "total_balance"),
        limit: None,
        used: None,
        unit: info
            .get("currency")
            .and_then(|unit| unit.as_str())
            .unwrap_or_default()
            .to_owned(),
    });
    let available = value
        .get("is_available")
        .and_then(|flag| flag.as_bool())
        .unwrap_or(true);
    Outcome {
        state: (!available).then_some(AccountState::Exhausted),
        error: (!available).then(|| "no balance left".to_owned()),
        balance,
        ..Outcome::default()
    }
}

fn map_moonshot(value: &serde_json::Value) -> Outcome {
    if let Some(code) = value.get("code").and_then(|code| code.as_i64())
        && code != 0
    {
        return Outcome::failed(
            AccountState::Error,
            format!("api.moonshot.ai answered code {code}"),
        );
    }
    let data = value.get("data").unwrap_or(value);
    let remaining = number(data, "available_balance");
    Outcome {
        state: remaining
            .filter(|value| *value <= 0.0)
            .map(|_| AccountState::Exhausted),
        balance: Some(Balance {
            remaining,
            limit: None,
            used: None,
            unit: String::new(),
        }),
        ..Outcome::default()
    }
}

/// MiniMax answers HTTP 200 even for a refused key, so the status lives in the
/// body and a naive parser would report a dead key as fully stocked.
fn map_minimax(value: &serde_json::Value, refused_message: &str) -> Outcome {
    let status = value
        .get("base_resp")
        .and_then(|resp| resp.get("status_code"))
        .and_then(|code| code.as_i64())
        .unwrap_or(0);
    if status == 1004 {
        return Outcome::failed(AccountState::SignedOut, refused_message.to_owned());
    }
    if status != 0 {
        let message = value
            .get("base_resp")
            .and_then(|resp| resp.get("status_msg"))
            .and_then(|msg| msg.as_str())
            .unwrap_or("unknown error");
        return Outcome::failed(
            AccountState::Error,
            format!("minimax answered {status}: {message}"),
        );
    }

    let remains = value
        .get("model_remains")
        .or_else(|| value.get("data").and_then(|data| data.get("model_remains")))
        .and_then(|list| list.as_array())
        .cloned()
        .unwrap_or_default();
    let windows = remains
        .iter()
        .filter_map(|entry| {
            let total = number(entry, "current_interval_total_count")?;
            let remaining = number(entry, "current_interval_remaining_count");
            let used = match remaining {
                Some(left) => total - left,
                None => number(entry, "current_interval_used_count")
                    .or_else(|| number(entry, "current_interval_usage_count"))?,
            };
            let start = number(entry, "start_time").unwrap_or(0.0);
            let end = number(entry, "end_time").unwrap_or(0.0);
            Some(Window {
                name: window_name(seconds_between(start, end)),
                scope: None,
                used_percent: if total > 0.0 {
                    clamp_percent(used / total * 100.0)
                } else {
                    0.0
                },
                resets_at: to_unix_seconds(end),
                remaining,
                limit: Some(total),
            })
        })
        .collect();

    Outcome {
        plan: ["plan_name", "current_subscribe_title"]
            .iter()
            .find_map(|key| {
                value
                    .get(*key)
                    .and_then(|name| name.as_str())
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            }),
        windows,
        ..Outcome::default()
    }
}

/// Z.ai is the other vendor whose real status is in the body.
fn map_zai(value: &serde_json::Value, refused_message: &str) -> Outcome {
    let code = value
        .get("code")
        .and_then(|code| code.as_i64())
        .unwrap_or(0);
    if code == 401 {
        return Outcome::failed(AccountState::SignedOut, refused_message.to_owned());
    }
    if code != 0 && code != 200 {
        let message = value
            .get("msg")
            .and_then(|msg| msg.as_str())
            .unwrap_or("unknown error");
        return Outcome::failed(
            AccountState::Error,
            format!("z.ai answered {code}: {message}"),
        );
    }

    let limits = value
        .get("data")
        .and_then(|data| data.get("limits"))
        .and_then(|limits| limits.as_array())
        .cloned()
        .unwrap_or_default();
    let mut windows = Vec::new();
    let mut balance = None;
    for limit in &limits {
        match limit.get("type").and_then(|kind| kind.as_str()) {
            Some("TOKENS_LIMIT") => windows.push(Window {
                name: window_name_from_unit(
                    limit
                        .get("unit")
                        .and_then(|unit| unit.as_str())
                        .unwrap_or_default(),
                    number(limit, "number").unwrap_or(0.0) as u64,
                ),
                scope: None,
                used_percent: clamp_percent(number(limit, "percentage").unwrap_or(0.0)),
                resets_at: number(limit, "nextResetTime").and_then(to_unix_seconds),
                remaining: number(limit, "remaining"),
                // Not `number`: on a TOKENS_LIMIT entry that field is how
                // many `unit`s long the window is, which the name above
                // already carries. Reporting a five-hour window as a quota of
                // five would be a made-up figure in `--json`.
                limit: None,
            }),
            Some("CREDIT_LIMIT") => {
                balance = Some(Balance {
                    remaining: number(limit, "remaining"),
                    limit: number(limit, "number"),
                    used: number(limit, "usage"),
                    unit: "credits".to_owned(),
                });
            }
            // TIME_LIMIT counts web searches, which is not what this panel is
            // about.
            _ => {}
        }
    }

    Outcome {
        windows,
        balance,
        ..Outcome::default()
    }
}

// ---------------------------------------------------------------- helpers

/// A window's name from its length, so `alc usage` says "5h" and "week"
/// rather than repeating a second count at the user.
pub(crate) fn window_name(seconds: u64) -> String {
    match seconds {
        0 => "window".to_owned(),
        604_800 => "week".to_owned(),
        value if value % 86_400 == 0 => format!("{}d", value / 86_400),
        value if value % 3_600 == 0 => format!("{}h", value / 3_600),
        value if value % 60 == 0 => format!("{}m", value / 60),
        value => format!("{value}s"),
    }
}

/// The same idea for a vendor that sends a count and a unit instead.
fn window_name_from_unit(unit: &str, number: u64) -> String {
    let lower = unit.to_ascii_lowercase();
    if lower.starts_with("hour") {
        format!("{number}h")
    } else if lower.starts_with("day") {
        if number == 7 {
            "week".to_owned()
        } else {
            format!("{number}d")
        }
    } else if lower.starts_with("minute") {
        format!("{number}m")
    } else if unit.is_empty() {
        "window".to_owned()
    } else {
        format!("{number} {unit}")
    }
}

/// Milliseconds where the number is too large to be seconds. Vendors here
/// disagree about the unit and say so nowhere.
fn to_unix_seconds(value: f64) -> Option<u64> {
    if value <= 0.0 {
        return None;
    }
    Some(if value > 1e11 {
        (value / 1000.0) as u64
    } else {
        value as u64
    })
}

fn seconds_between(start: f64, end: f64) -> u64 {
    match (to_unix_seconds(start), to_unix_seconds(end)) {
        (Some(start), Some(end)) if end > start => end - start,
        _ => 0,
    }
}

pub(crate) fn clamp_percent(value: f64) -> f64 {
    if value.is_nan() {
        return 0.0;
    }
    value.clamp(0.0, 100.0)
}

/// `2026-09-13T18:04:00Z` and friends, without a date crate.
///
/// Only UTC is accepted: every timestamp these endpoints send is UTC, and a
/// silently mis-parsed offset would show a countdown that is hours wrong.
pub(crate) fn parse_rfc3339_utc(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let rest = rest
        .strip_suffix('Z')
        .or_else(|| rest.strip_suffix("+00:00"))
        .or_else(|| rest.strip_suffix("-00:00"))?;
    let time = rest.split('.').next()?;

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    // Days from civil, Howard Hinnant's algorithm.
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login() -> CodexLogin {
        CodexLogin {
            access_token: "a.b.c".to_owned(),
            account_id: Some("acct_1".to_owned()),
            label: Some("me@example.com".to_owned()),
            plan_hint: Some("plus".to_owned()),
            expires_at_ms: u64::MAX,
        }
    }

    #[test]
    fn the_codex_payload_maps_both_windows_its_plan_and_its_credits() {
        let usage: CodexUsage = serde_json::from_value(serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 63.0, "limit_window_seconds": 18000,
                    "reset_after_seconds": 7800, "reset_at": 2_000_000_000u64
                },
                "secondary_window": {
                    "used_percent": 12.0, "limit_window_seconds": 604800,
                    "reset_after_seconds": 100, "reset_at": 2_000_100_000u64
                }
            },
            "credits": { "has_credits": true, "unlimited": false, "balance": "4.20" }
        }))
        .unwrap();

        let outcome = map_codex(usage, &login());
        assert_eq!(outcome.plan.as_deref(), Some("plus"));
        assert_eq!(outcome.windows.len(), 2);
        assert_eq!(outcome.windows[0].name, "5h");
        assert_eq!(outcome.windows[0].resets_at, Some(2_000_000_000));
        assert_eq!(outcome.windows[1].name, "week");
        assert_eq!(outcome.credits.unwrap().balance.as_deref(), Some("4.20"));
        assert!(outcome.state.is_none());
    }

    /// The bridge's own rule, and the reason the credits block is read at all.
    #[test]
    fn a_codex_plan_at_its_limit_is_exhausted_only_without_credits_behind_it() {
        let payload = |has_credits: bool| {
            serde_json::from_value::<CodexUsage>(serde_json::json!({
                "plan_type": "pro",
                "rate_limit": { "limit_reached": true, "primary_window": {
                    "used_percent": 100.0, "limit_window_seconds": 18000, "reset_after_seconds": 60
                }},
                "credits": { "has_credits": has_credits, "unlimited": false }
            }))
            .unwrap()
        };

        assert_eq!(
            map_codex(payload(false), &login()).state,
            Some(AccountState::Exhausted)
        );
        assert!(map_codex(payload(true), &login()).state.is_none());
    }

    #[test]
    fn a_named_codex_limit_becomes_a_scoped_window() {
        let usage: CodexUsage = serde_json::from_value(serde_json::json!({
            "plan_type": "pro",
            "additional_rate_limits": [{
                "limit_name": "gpt-6-astra",
                "rate_limit": { "primary_window": {
                    "used_percent": 5.0, "limit_window_seconds": 86400, "reset_after_seconds": 10
                }}
            }]
        }))
        .unwrap();

        let outcome = map_codex(usage, &login());
        assert_eq!(outcome.windows[0].scope.as_deref(), Some("gpt-6-astra"));
        assert_eq!(outcome.windows[0].name, "1d");
    }

    /// An expired token is a fact the file already states; asking the endpoint
    /// would only turn it into a 401.
    #[test]
    fn an_expired_codex_token_is_signed_out_without_a_request() {
        let mut expired = login();
        expired.expires_at_ms = 1_000;
        let outcome = fetch_codex(&expired, 2_000);
        assert_eq!(outcome.state, Some(AccountState::SignedOut));
        assert!(outcome.error.unwrap().contains("codex login"));
        assert_eq!(outcome.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn the_claude_payload_prefers_the_limits_array_over_the_legacy_model_keys() {
        let usage: ClaudeUsage = serde_json::from_value(serde_json::json!({
            "five_hour": { "utilization": 81.0, "resets_at": "2026-09-14T03:00:00Z" },
            "seven_day": { "utilization": 22.0, "resets_at": "2026-09-20T00:00:00Z" },
            "seven_day_opus": { "utilization": 9.0, "resets_at": null },
            "limits": [
                { "kind": "weekly_scoped", "percent": 40.0, "is_active": true,
                  "scope": { "model": { "display_name": "Fable" } },
                  "resets_at": "2026-09-20T00:00:00Z" },
                { "kind": "weekly_scoped", "percent": 1.0, "is_active": false,
                  "scope": { "model": { "display_name": "Retired" } } }
            ]
        }))
        .unwrap();

        let outcome = map_claude(usage);
        let names: Vec<_> = outcome
            .windows
            .iter()
            .map(|window| (window.name.as_str(), window.scope.as_deref()))
            .collect();
        assert_eq!(
            names,
            vec![("5h", None), ("week", None), ("week", Some("Fable"))]
        );
        assert_eq!(outcome.windows[0].resets_at, Some(1_789_354_800));
    }

    #[test]
    fn legacy_claude_model_keys_are_used_when_the_array_is_absent() {
        let usage: ClaudeUsage = serde_json::from_value(serde_json::json!({
            "five_hour": { "utilization": 10.0 },
            "seven_day_opus": { "utilization": 9.0 },
            "seven_day_sonnet": { "utilization": 4.0 }
        }))
        .unwrap();

        let scopes: Vec<_> = map_claude(usage)
            .windows
            .into_iter()
            .filter_map(|window| window.scope)
            .collect();
        assert_eq!(scopes, vec!["Opus", "Sonnet"]);
    }

    #[test]
    fn claude_extra_usage_is_read_in_dollars_and_only_when_enabled() {
        let enabled: ClaudeUsage = serde_json::from_value(serde_json::json!({
            "extra_usage": { "is_enabled": true, "used_credits": 1234.0, "monthly_limit": 5000.0 }
        }))
        .unwrap();
        let balance = map_claude(enabled).balance.unwrap();
        assert_eq!(balance.used, Some(12.34));
        assert_eq!(balance.limit, Some(50.0));

        let disabled: ClaudeUsage = serde_json::from_value(serde_json::json!({
            "extra_usage": { "is_enabled": false, "used_credits": 1.0 }
        }))
        .unwrap();
        assert!(map_claude(disabled).balance.is_none());
    }

    #[test]
    fn openrouter_reports_what_is_left_of_the_key_limit() {
        let outcome = map_openrouter(&serde_json::json!({
            "data": { "label": "laptop", "usage": 12.4, "limit": 50.0, "limit_remaining": 37.6 }
        }));
        let balance = outcome.balance.unwrap();
        assert_eq!(outcome.label.as_deref(), Some("laptop"));
        assert_eq!(balance.remaining, Some(37.6));
        assert_eq!(balance.unit, "USD");
    }

    #[test]
    fn deepseek_balance_strings_become_numbers_and_unavailable_is_exhausted() {
        let outcome = map_deepseek(&serde_json::json!({
            "is_available": true,
            "balance_infos": [{ "currency": "USD", "total_balance": "18.20" }]
        }));
        assert_eq!(outcome.balance.unwrap().remaining, Some(18.2));

        let empty = map_deepseek(&serde_json::json!({
            "is_available": false,
            "balance_infos": [{ "currency": "CNY", "total_balance": "0.00" }]
        }));
        assert_eq!(empty.state, Some(AccountState::Exhausted));
    }

    #[test]
    fn moonshot_reports_a_spent_balance_as_exhausted() {
        let outcome = map_moonshot(&serde_json::json!({
            "code": 0, "data": { "available_balance": 0.0 }
        }));
        assert_eq!(outcome.state, Some(AccountState::Exhausted));
    }

    /// A dead key answering 200 would otherwise render as a full plan.
    #[test]
    fn minimax_reads_its_status_from_the_body_rather_than_the_http_code() {
        let refused = map_minimax(
            &serde_json::json!({ "base_resp": { "status_code": 1004, "status_msg": "login fail" } }),
            "fix it",
        );
        assert_eq!(refused.state, Some(AccountState::SignedOut));
        assert_eq!(refused.error.as_deref(), Some("fix it"));

        let broken = map_minimax(
            &serde_json::json!({ "base_resp": { "status_code": 2013, "status_msg": "invalid params" } }),
            "fix it",
        );
        assert_eq!(broken.state, Some(AccountState::Error));
    }

    #[test]
    fn minimax_used_is_the_total_less_what_remains() {
        let outcome = map_minimax(
            &serde_json::json!({
                "base_resp": { "status_code": 0 },
                "plan_name": "coding-max",
                "model_remains": [{
                    "current_interval_total_count": 200.0,
                    "current_interval_remaining_count": 50.0,
                    "start_time": 1_700_000_000.0,
                    "end_time": 1_700_018_000.0
                }]
            }),
            "fix it",
        );
        assert_eq!(outcome.plan.as_deref(), Some("coding-max"));
        assert_eq!(outcome.windows[0].used_percent, 75.0);
        assert_eq!(outcome.windows[0].name, "5h");
        assert_eq!(outcome.windows[0].remaining, Some(50.0));
    }

    #[test]
    fn zai_reads_its_code_from_the_body_and_splits_windows_from_credits() {
        let refused = map_zai(
            &serde_json::json!({ "code": 401, "msg": "token expired" }),
            "fix",
        );
        assert_eq!(refused.state, Some(AccountState::SignedOut));

        let outcome = map_zai(
            &serde_json::json!({
                "code": 200,
                "data": { "limits": [
                    { "type": "TOKENS_LIMIT", "unit": "hours", "number": 5.0,
                      "percentage": 30.0, "nextResetTime": 1_700_000_000_000u64 },
                    { "type": "CREDIT_LIMIT", "number": 100.0, "usage": 25.0, "remaining": 75.0 },
                    { "type": "TIME_LIMIT", "number": 3.0 }
                ]}
            }),
            "fix",
        );
        assert_eq!(outcome.windows.len(), 1);
        assert_eq!(outcome.windows[0].name, "5h");
        assert_eq!(outcome.windows[0].resets_at, Some(1_700_000_000));
        assert_eq!(outcome.balance.unwrap().remaining, Some(75.0));
    }

    #[test]
    fn a_window_is_named_after_its_length() {
        assert_eq!(window_name(18_000), "5h");
        assert_eq!(window_name(604_800), "week");
        assert_eq!(window_name(86_400), "1d");
        assert_eq!(window_name(300), "5m");
        assert_eq!(window_name(0), "window");
        assert_eq!(window_name_from_unit("days", 7), "week");
        assert_eq!(window_name_from_unit("weeks", 2), "2 weeks");
    }

    #[test]
    fn a_percentage_outside_its_range_is_clamped_and_a_nan_is_zero() {
        assert_eq!(clamp_percent(140.0), 100.0);
        assert_eq!(clamp_percent(-3.0), 0.0);
        assert_eq!(clamp_percent(f64::NAN), 0.0);
    }

    #[test]
    fn resets_parse_in_utc_and_any_other_offset_is_refused() {
        assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_utc("2026-09-13T18:04:05Z"),
            Some(1_789_322_645)
        );
        assert_eq!(
            parse_rfc3339_utc("2026-09-13T18:04:05.123456Z"),
            Some(1_789_322_645)
        );
        assert_eq!(
            parse_rfc3339_utc("2026-09-13T18:04:05+00:00"),
            Some(1_789_322_645)
        );
        assert_eq!(parse_rfc3339_utc("2026-09-13T18:04:05+02:00"), None);
        assert_eq!(parse_rfc3339_utc("not a timestamp"), None);
    }

    /// The override is what lets every `alc usage` test answer from a loopback
    /// listener; it must keep the path so one server can route on it.
    #[test]
    fn the_debug_endpoint_override_replaces_the_host_and_keeps_the_path() {
        assert_eq!(
            rebase("http://127.0.0.1:8080", CODEX_USAGE_URL),
            "http://127.0.0.1:8080/backend-api/wham/usage"
        );
        assert_eq!(
            rebase("http://127.0.0.1:8080/", CLAUDE_USAGE_URL),
            "http://127.0.0.1:8080/api/oauth/usage"
        );
    }

    #[test]
    fn a_refusal_names_the_command_that_fixes_it_and_a_rate_limit_does_not() {
        let signed_out = refused(401, CODEX_USAGE_URL, "run `codex login`").unwrap();
        assert_eq!(signed_out.state, Some(AccountState::SignedOut));

        let throttled = refused(429, CODEX_USAGE_URL, "unused").unwrap();
        assert_eq!(throttled.state, Some(AccountState::Error));
        assert!(throttled.error.unwrap().contains("rate limiting"));

        assert!(refused(200, CODEX_USAGE_URL, "unused").is_none());
    }

    #[test]
    fn an_unreachable_host_is_a_sentence_rather_than_a_panic() {
        // Bound and dropped, so the port is closed but routable.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/nothing", listener.local_addr().unwrap());
        drop(listener);

        let Err(error) = get(&url, &[]) else {
            panic!("a closed port cannot answer");
        };
        assert!(error.starts_with("could not reach 127.0.0.1:"), "{error}");
    }
}
