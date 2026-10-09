//! `alc usage` — what is left on every login, and what each agent has spent.
//!
//! # Why alc answers this at all
//!
//! alc is the thing that knows which login each agent is launched on, so it is
//! the only thing that can put "what is left on this account" and "which agent
//! spent it" in one table. Every coding agent can show you its own numbers;
//! none of them can show you the other seven, or the second account.
//!
//! # What it will not do
//!
//! It never writes a credential, never refreshes a token, and never guesses a
//! number. A provider with no published balance endpoint says "no quota API"
//! rather than showing a zero, and a pair of provider and agent whose traffic
//! alc does not carry shows a dash in the token columns rather than a total
//! that would read as "nothing spent".

pub(crate) mod accounts;
mod chart;
pub(crate) mod forward;
pub(crate) mod ledger;
pub(crate) mod native;
pub(crate) mod observer;
pub(crate) mod pricing;
pub(crate) mod query;
pub(crate) mod quota;
pub(crate) mod records;
mod view;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::{ProviderKind, Store};
use crate::doctor::{Cell, INDENT, Issue, Status, Table, Theme, Tone, heading_text, summary_text};
use accounts::{ClaudeStores, Credential, CredentialSource, Discovered, Env};
use ledger::{LedgerSummary, now_unix};

/// A window at or past this share of itself is worth noticing before it runs
/// out. The page uses the same number so the two surfaces agree.
pub(crate) const WARN_PERCENT: f64 = 75.0;
const REPORT_SCHEMA_VERSION: u32 = 1;

/// Which surface built the report.
///
/// Worth saying out loud: the hub resolves its own environment, which belongs
/// to whichever shell started the daemon, so a report it built can name a
/// different account than the same command in your shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResolvedBy {
    Cli,
    Hub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AccountState {
    Ok,
    Warn,
    Exhausted,
    SignedOut,
    Error,
    /// There is nothing to read: no key saved, or a vendor with no endpoint.
    Unavailable,
}

impl AccountState {
    fn status(self) -> Status {
        match self {
            Self::Ok => Status::Good,
            Self::Warn => Status::Warn,
            Self::Exhausted | Self::SignedOut | Self::Error => Status::Bad,
            Self::Unavailable => Status::Off,
        }
    }

    /// Whether a person can do something about it right now. Drives the exit
    /// code, the way `alc doctor`'s does.
    fn is_actionable(self) -> bool {
        matches!(self, Self::SignedOut | Self::Error)
    }
}

/// A rolling limit, named by its length.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Window {
    pub name: String,
    /// The model or feature the window covers, where the vendor scopes it.
    pub scope: Option<String>,
    pub used_percent: f64,
    pub resets_at: Option<u64>,
    pub remaining: Option<f64>,
    pub limit: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Balance {
    pub remaining: Option<f64>,
    pub limit: Option<f64>,
    pub used: Option<f64>,
    /// `USD`, `CNY`, `credits`, or empty where the vendor does not say.
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Credits {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

/// One provider profile's login, as the report shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Account {
    pub profile: String,
    pub kind: ProviderKind,
    /// An email, a key label, or the tail of an account id. Redacted for a
    /// viewer-grade reader of the remote page.
    pub label: Option<String>,
    pub account_id: Option<String>,
    pub source: CredentialSource,
    pub credential_path: Option<String>,
    pub plan: Option<String>,
    pub state: AccountState,
    pub windows: Vec<Window>,
    pub balance: Option<Balance>,
    pub credits: Option<Credits>,
    pub fetched_at: Option<u64>,
    /// The sentence a person reads when the row is not a number.
    pub error: Option<String>,
}

/// What `alc usage --json` prints and `GET /api/usage` answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageReport {
    pub schema_version: u32,
    pub generated_at: u64,
    pub resolved_by: ResolvedBy,
    pub accounts: Vec<Account>,
    pub ledger: LedgerSummary,
}

impl UsageReport {
    /// The same report with everything that names a person or a filesystem
    /// removed.
    ///
    /// A shared link can be handed to somebody who may watch a session but has
    /// no business knowing which email pays for it or where the credential
    /// sits on disk. The numbers stay; the identity does not.
    pub(crate) fn redacted(mut self) -> Self {
        for account in &mut self.accounts {
            account.label = None;
            account.account_id = None;
            account.credential_path = None;
            // A sentence naming a path names the home directory it sits in,
            // which is the operator's account name. The state still says
            // what kind of row this is.
            if account.error.is_some() {
                account.error = Some(match account.state {
                    AccountState::SignedOut => "not signed in on this machine".to_owned(),
                    AccountState::Unavailable => "nothing to read".to_owned(),
                    _ => "could not be read".to_owned(),
                });
            }
        }
        // The page never reads either of these, and both spell out where the
        // config directory is.
        self.ledger.path = String::new();
        self.ledger.error = None;
        self
    }

    fn needs_attention(&self) -> bool {
        self.accounts
            .iter()
            .any(|account| account.state.is_actionable())
    }
}

/// Builds the report: discover every profile's login, ask each vendor once,
/// then fold the ledger in.
///
/// Never fails. A login that cannot be read is a row with a sentence in it,
/// because a report that refuses to print because one of five accounts is
/// signed out is less useful than one that says which.
pub(crate) fn build_report(
    store: &Store,
    requested: Option<&str>,
    resolved_by: ResolvedBy,
    env: &Env,
    stores: ClaudeStores,
) -> UsageReport {
    let found = accounts::discover(store, requested, env);
    let now_ms = now_unix().saturating_mul(1000);
    // Two profiles can name one login. Asking the vendor twice would be rude
    // and would count against the same rate limit twice.
    let mut seen: HashMap<accounts::Identity, String> = HashMap::new();
    let accounts = found
        .iter()
        .map(|discovered| {
            // The entry API rather than `insert`, which would replace the
            // stored name and leave a third profile pointing at the second
            // rather than at the one that actually holds the numbers.
            let duplicate = discovered
                .identity()
                .and_then(|identity| match seen.entry(identity) {
                    std::collections::hash_map::Entry::Occupied(first) => Some(first.get().clone()),
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(discovered.profile.clone());
                        None
                    }
                });
            account_for(discovered, duplicate, env, stores, now_ms)
        })
        .collect();

    UsageReport {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at: now_unix(),
        resolved_by,
        accounts,
        ledger: ledger::summarise(&store.dir),
    }
}

fn account_for(
    discovered: &Discovered,
    duplicate_of: Option<String>,
    env: &Env,
    stores: ClaudeStores,
    now_ms: u64,
) -> Account {
    let mut account = Account {
        profile: discovered.profile.clone(),
        kind: discovered.kind,
        label: None,
        account_id: None,
        source: discovered.source,
        credential_path: None,
        plan: None,
        state: AccountState::Unavailable,
        windows: Vec::new(),
        balance: None,
        credits: None,
        fetched_at: None,
        error: None,
    };

    // A second profile on one login is a fact about the configuration, not a
    // second thing to ask the vendor about.
    if let Some(first) = duplicate_of {
        account.error = Some(format!("same login as {first}"));
        return finish(account, &discovered.credential, env);
    }

    let outcome = match &discovered.credential {
        Credential::None { reason } => {
            account.error = Some(reason.clone());
            return finish(account, &discovered.credential, env);
        }
        Credential::Codex { auth_file } => {
            match accounts::read_codex_login(auth_file, env.home.as_deref()) {
                Ok(login) => {
                    account.account_id = login.account_id.clone();
                    account.label = login.label.clone();
                    quota::fetch_codex(&login, now_ms)
                }
                Err(error) => quota::Outcome {
                    state: Some(AccountState::SignedOut),
                    error: Some(error),
                    ..quota::Outcome::default()
                },
            }
        }
        Credential::Claude { config_dir } => {
            let label = Some(elide_home(config_dir, env.home.as_deref()));
            match accounts::read_claude_login(config_dir, env.home.as_deref(), stores) {
                Ok(login) => quota::fetch_claude(&login, label, now_ms),
                Err(refusal) => quota::Outcome {
                    label,
                    state: Some(if refusal.actionable {
                        AccountState::SignedOut
                    } else {
                        AccountState::Unavailable
                    }),
                    error: Some(refusal.message),
                    ..quota::Outcome::default()
                },
            }
        }
        Credential::ApiKey { key, vendor } => {
            account.label = Some("key".to_owned());
            quota::fetch_key(*vendor, key, &discovered.profile)
        }
    };

    account.plan = outcome.plan;
    account.label = outcome.label.or(account.label);
    account.windows = outcome.windows;
    account.balance = outcome.balance;
    account.credits = outcome.credits;
    account.error = outcome.error;
    account.state = outcome.state.unwrap_or_else(|| {
        state_from(
            &account.windows,
            account.balance.as_ref(),
            account.credits.as_ref(),
        )
    });
    account.fetched_at = Some(now_unix());
    finish(account, &discovered.credential, env)
}

fn finish(mut account: Account, credential: &Credential, env: &Env) -> Account {
    account.credential_path = match credential {
        Credential::Codex { auth_file } => Some(elide_home(auth_file, env.home.as_deref())),
        Credential::Claude { config_dir } => Some(elide_home(config_dir, env.home.as_deref())),
        Credential::ApiKey { .. } | Credential::None { .. } => None,
    };
    if account.error.is_some() && account.state == AccountState::Unavailable {
        // A sentence with no state yet means discovery had nothing to read,
        // which is information rather than a fault.
        account.state = AccountState::Unavailable;
    }
    account
}

/// The state a set of windows implies when the vendor did not say.
fn state_from(
    windows: &[Window],
    balance: Option<&Balance>,
    credits: Option<&Credits>,
) -> AccountState {
    // A plan at its limit with credits behind it can still serve a turn, so
    // it is worth noticing rather than a failure. The mapper leaves the
    // decision here precisely so both paths agree on it.
    let spendable = credits.is_some_and(|credits| credits.has_credits || credits.unlimited);
    let empty = windows.iter().any(|window| window.used_percent >= 100.0)
        || balance.is_some_and(|balance| balance.remaining.is_some_and(|left| left <= 0.0));
    if empty {
        return if spendable {
            AccountState::Warn
        } else {
            AccountState::Exhausted
        };
    }
    if windows
        .iter()
        .any(|window| window.used_percent >= WARN_PERCENT)
    {
        return AccountState::Warn;
    }
    AccountState::Ok
}

/// `~/.codex/auth.json` rather than the full path: shorter, and it keeps a
/// screenshot of the panel from naming the user's home directory.
pub(crate) fn elide_home(path: &Path, home: Option<&Path>) -> String {
    // By component rather than by byte: `/Users/ada-work` starts with
    // `/Users/ada` as a string, and eliding it would print `~-work/…`, a path
    // that names nothing. For a Claude profile this string is the only thing
    // telling two logins apart, so it has to stay a real path.
    home.filter(|home| !home.as_os_str().is_empty())
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|rest| {
            if rest.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rest.display())
            }
        })
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Serialize)]
struct CliUsageReport {
    #[serde(flatten)]
    report: UsageReport,
    statistics: query::Statistics,
}

/// CLI statistics extend the report without teaching the shared browser about
/// native histories, filesystem paths, or per-request account metadata.
pub(crate) fn run_statistics(
    store: &Store,
    requested: Option<&str>,
    options: &query::UsageOptions,
) -> Result<u8> {
    let statistics = query::statistics(&store.dir, &store.config, options)?;
    if let Some(path) = options.chart.as_deref() {
        let path = chart::destination(path)?;
        chart::export(&statistics, &path)?;
        eprintln!("Usage chart: {}", path.display());
    }
    let report = if options.offline {
        UsageReport {
            schema_version: REPORT_SCHEMA_VERSION,
            generated_at: now_unix(),
            resolved_by: ResolvedBy::Cli,
            accounts: Vec::new(),
            ledger: ledger::summarise(&store.dir),
        }
    } else {
        build_report(
            store,
            requested,
            ResolvedBy::Cli,
            &Env::from_process(),
            ClaudeStores::All,
        )
    };
    let code = u8::from(report.needs_attention());
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&CliUsageReport { report, statistics })?
        );
    } else {
        let theme = Theme::detect();
        if options.offline {
            println!("Accounts: not fetched (--offline)");
        } else {
            print!("{}", render(&report, &theme));
        }
        let view = view::View {
            monthly: options.monthly,
            width: view::terminal_width(),
        };
        print!("{}", view::render_statistics(&statistics, &theme, view));
        if options.details {
            print!("{}", query::render_details(&statistics, &theme));
        }
    }
    Ok(code)
}

/// The whole rendered report as one string, so a test can assert on it
/// without capturing stdout.
pub(crate) fn render(report: &UsageReport, theme: &Theme) -> String {
    let mut out = String::new();
    let mut issues = Vec::new();

    out.push_str(&heading_text(theme, "Accounts"));
    let mut table = Table::new(vec!["", "PROFILE", "ACCOUNT", "PLAN", "LEFT", "REMAINING"]);
    for account in &report.accounts {
        let status = account.state.status();
        table.push(vec![
            Cell::left(theme.mark(status), Tone::Plain),
            Cell::left(account.profile.clone(), Tone::Plain),
            Cell::left(
                account
                    .label
                    .clone()
                    .unwrap_or_else(|| theme.dash().to_owned()),
                Tone::Plain,
            ),
            Cell::left(
                account
                    .plan
                    .clone()
                    .unwrap_or_else(|| theme.dash().to_owned()),
                Tone::Dim,
            ),
            Cell::left(quota_bar(account, theme), status.tone()),
            Cell::left(remaining_text(account, theme), status.tone()),
        ]);
        if account.state.is_actionable() {
            issues.push(Issue::new(
                account.profile.clone(),
                account
                    .error
                    .clone()
                    .unwrap_or_else(|| "could not be read".to_owned()),
                fix_for(account),
            ));
        }
    }
    for line in table.render(theme) {
        out.push_str(&format!("{INDENT}{line}\n"));
    }

    out.push_str(&heading_text(theme, "Usage by provider and agent"));
    if report.ledger.rows.is_empty() {
        let note = match &report.ledger.error {
            Some(error) => format!("{}: {error}", report.ledger.path),
            None => format!(
                "no launches recorded yet {} the first `alc <agent>` writes {}",
                theme.dash(),
                report.ledger.path
            ),
        };
        out.push_str(&format!("{INDENT}{}\n", theme.paint(Tone::Dim, &note)));
    } else {
        let width = view::terminal_width();
        out.push_str(&view::fit(theme, width, |short| {
            let mut table = view::BoxTable::new(vec![
                view::Column::left("Provider"),
                view::Column::left("Agent"),
                view::Column::right("Launches"),
                view::Column::right("Turns"),
                view::Column::right("Input"),
                view::Column::right("Cached").optional(1),
                view::Column::right("Cache %").optional(2),
                view::Column::right("Output"),
                view::Column::left("Last").optional(3),
            ]);
            for row in &report.ledger.rows {
                let carried = row.turns > 0;
                let dash = || view::Cell::toned(theme.dash(), Tone::Dim);
                let count = |value: u64| {
                    if carried {
                        view::Cell::plain(view::count(value, short))
                    } else {
                        dash()
                    }
                };
                let cached = row
                    .cached_tokens
                    .filter(|cached| carried && *cached <= row.input_tokens);
                table.push(vec![
                    view::Cell::plain(row.provider.clone()),
                    view::Cell::plain(row.agent.to_string()),
                    view::Cell::plain(view::count(row.launches, short)),
                    count(row.turns),
                    count(row.input_tokens),
                    cached
                        .map_or_else(dash, |cached| view::Cell::plain(view::count(cached, short))),
                    cached
                        .and_then(|cached| cache_read_percent(row.input_tokens, cached))
                        .map_or_else(dash, |percent| view::Cell::plain(format!("{percent}%"))),
                    count(row.output_tokens),
                    view::Cell::toned(format_age(report.generated_at, row.last_at), Tone::Dim),
                ]);
            }
            table
        }));
        out.push_str(&format!(
            "{INDENT}{}\n",
            theme.paint(
                Tone::Dim,
                &format!(
                    "{} {} tokens count only where alc carries the traffic (opt in with --metrics)",
                    report.ledger.path,
                    theme.dash()
                )
            )
        ));
    }
    if report.ledger.skipped_lines > 0 {
        out.push_str(&format!(
            "{INDENT}{}\n",
            theme.paint(
                Tone::Dim,
                &format!(
                    "{} line(s) in the ledger were not written by this alc and were skipped",
                    report.ledger.skipped_lines
                )
            )
        ));
    }

    out.push_str(&summary_text(theme, &issues));
    out
}

/// The command that clears a row, where there is one.
fn fix_for(account: &Account) -> Option<String> {
    if !matches!(account.state, AccountState::SignedOut) {
        return None;
    }
    match account.kind {
        ProviderKind::Codex => Some("codex login".to_owned()),
        ProviderKind::Anthropic => Some("claude".to_owned()),
        _ => Some(format!("alc config key {}", account.profile)),
    }
}

/// The `LEFT` column: the tightest plan window as a ten-step meter.
fn quota_bar(account: &Account, theme: &Theme) -> String {
    let Some(used) = account
        .windows
        .iter()
        .filter(|window| window.scope.is_none())
        .map(|window| quota::clamp_percent(window.used_percent))
        .reduce(f64::max)
    else {
        return theme.dash().to_owned();
    };
    let filled = ((100.0 - used) / 10.0).round().clamp(0.0, 10.0) as usize;
    let (on, off) = if theme.unicode() {
        ("▰", "▱")
    } else {
        ("#", ".")
    };
    format!("{}{}", on.repeat(filled), off.repeat(10 - filled))
}

/// The `REMAINING` column: every window, then whatever balance there is, then
/// the sentence if the row has one instead of numbers.
fn remaining_text(account: &Account, theme: &Theme) -> String {
    if let Some(error) = &account.error
        && account.windows.is_empty()
        && account.balance.is_none()
    {
        return error.clone();
    }

    let mut parts: Vec<String> = account
        .windows
        .iter()
        // A per-model window nobody has touched says nothing a person needs;
        // the plan's own windows always show, and `--json` keeps every one.
        .filter(|window| window.scope.is_none() || window.used_percent > 0.0)
        .map(|window| {
            let name = match &window.scope {
                Some(scope) => format!("{scope} {}", window.name),
                None => window.name.clone(),
            };
            let left = (100.0 - quota::clamp_percent(window.used_percent)).round();
            match window
                .resets_at
                .map(|at| at.saturating_sub(account.fetched_at.unwrap_or(0)))
                .filter(|seconds| *seconds > 0)
            {
                Some(seconds) => format!(
                    "{name} {left:.0}% left, resets in {}",
                    format_countdown(seconds)
                ),
                None => format!("{name} {left:.0}% left"),
            }
        })
        .collect();

    if let Some(balance) = &account.balance
        && let Some(text) = balance_text(balance)
    {
        parts.push(text);
    }
    if let Some(credits) = &account.credits {
        if credits.unlimited {
            parts.push("unlimited credits".to_owned());
        } else if credits.has_credits {
            let amount = credits.balance.clone().unwrap_or_default();
            parts.push(if amount.is_empty() {
                "credits available".to_owned()
            } else {
                format!("credits {amount}")
            });
        } else if !account.windows.is_empty() {
            parts.push("no credits".to_owned());
        }
    }
    if let Some(error) = &account.error {
        parts.push(error.clone());
    }

    if parts.is_empty() {
        theme.dash().to_owned()
    } else {
        parts.join(&format!(" {} ", theme.dash()))
    }
}

/// `None` when the vendor enabled a balance but sent no numbers for it, so
/// the caller pushes nothing rather than an empty part with a separator
/// hanging off it.
fn balance_text(balance: &Balance) -> Option<String> {
    Some(match (balance.remaining, balance.limit, balance.used) {
        (Some(remaining), Some(limit), _) => format!(
            "{} of {} left",
            money(remaining, &balance.unit),
            money(limit, &balance.unit)
        ),
        (Some(remaining), None, _) => format!("{} left", money(remaining, &balance.unit)),
        (None, Some(limit), Some(used)) => format!(
            "{} of {} used",
            money(used, &balance.unit),
            money(limit, &balance.unit)
        ),
        (None, None, Some(used)) => format!("{} used", money(used, &balance.unit)),
        _ => return None,
    })
}

fn money(value: f64, unit: &str) -> String {
    match unit {
        "USD" => format!("${value:.2}"),
        "CNY" => format!("¥{value:.2}"),
        "" => format!("{value:.2}"),
        other => format!("{value:.2} {other}"),
    }
}

/// Coarse on purpose: the exact second a weekly window resets is noise, and
/// the page formats the same values the same way.
pub(crate) fn format_countdown(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
        _ => format!("{}d {}h", seconds / 86_400, (seconds % 86_400) / 3600),
    }
}

fn format_age(now: u64, then: u64) -> String {
    if then == 0 || then > now {
        return "just now".to_owned();
    }
    format!("{} ago", format_countdown(now - then))
}

/// The share of gross input Codex served from its prompt cache.
///
/// Integer arithmetic keeps terminal and JSON consumers independent of float
/// representation; adding half the denominator gives nearest-whole rounding.
pub(crate) fn cache_read_percent(input_tokens: u64, cached_tokens: u64) -> Option<u64> {
    if input_tokens == 0 || cached_tokens > input_tokens {
        return None;
    }
    let numerator = u128::from(cached_tokens) * 100 + u128::from(input_tokens) / 2;
    Some((numerator / u128::from(input_tokens)) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Agent;
    use ledger::LedgerRow;

    fn theme() -> Theme {
        Theme::for_test(false, true)
    }

    fn account(profile: &str, state: AccountState) -> Account {
        Account {
            profile: profile.to_owned(),
            kind: ProviderKind::Codex,
            label: Some("me@example.com".to_owned()),
            account_id: Some("acct_1".to_owned()),
            source: CredentialSource::Default,
            credential_path: Some("~/.codex/auth.json".to_owned()),
            plan: Some("plus".to_owned()),
            state,
            windows: vec![Window {
                name: "5h".to_owned(),
                scope: None,
                used_percent: 37.0,
                resets_at: Some(1_000 + 7_800),
                remaining: None,
                limit: None,
            }],
            balance: None,
            credits: None,
            fetched_at: Some(1_000),
            error: None,
        }
    }

    fn report(accounts: Vec<Account>, rows: Vec<LedgerRow>) -> UsageReport {
        UsageReport {
            schema_version: REPORT_SCHEMA_VERSION,
            generated_at: 1_000,
            resolved_by: ResolvedBy::Cli,
            accounts,
            ledger: LedgerSummary {
                path: "~/.config/alc/usage.jsonl".to_owned(),
                rows,
                first_at: None,
                skipped_lines: 0,
                error: None,
            },
        }
    }

    #[test]
    fn a_healthy_account_renders_its_windows_and_the_time_they_reset() {
        let text = render(
            &report(vec![account("codex", AccountState::Ok)], vec![]),
            &theme(),
        );
        assert!(text.contains("codex"), "{text}");
        assert!(text.contains("me@example.com"), "{text}");
        assert!(text.contains("5h 63% left, resets in 2h 10m"), "{text}");
    }

    /// The panel the request was about: several logins of one kind, each with
    /// its own row.
    #[test]
    fn every_login_gets_its_own_row() {
        let mut second = account("codex-work", AccountState::Exhausted);
        second.label = Some("work@example.com".to_owned());
        second.windows[0].used_percent = 100.0;
        let text = render(
            &report(vec![account("codex", AccountState::Ok), second], vec![]),
            &theme(),
        );
        assert!(text.contains("work@example.com"), "{text}");
        assert!(text.contains("5h 0% left"), "{text}");
    }

    #[test]
    fn a_signed_out_account_becomes_an_issue_with_the_command_that_fixes_it() {
        let mut signed_out = account("codex", AccountState::SignedOut);
        signed_out.windows.clear();
        signed_out.label = None;
        signed_out.error = Some("no Codex credentials at ~/.codex/auth.json".to_owned());

        let text = render(&report(vec![signed_out], vec![]), &theme());
        assert!(text.contains("no Codex credentials"), "{text}");
        assert!(text.contains("codex login"), "{text}");
        assert!(text.contains("needs attention"), "{text}");
    }

    #[test]
    fn a_provider_with_no_quota_api_is_an_off_row_rather_than_a_zero() {
        let mut none = account("ollama", AccountState::Unavailable);
        none.kind = ProviderKind::Ollama;
        none.label = None;
        none.plan = None;
        none.windows.clear();
        none.error = Some("no quota API".to_owned());

        let text = render(&report(vec![none], vec![]), &theme());
        assert!(text.contains("no quota API"), "{text}");
        assert!(!text.contains("needs attention"), "{text}");
    }

    /// The honesty rule: a launch alc did not carry traffic for has unknown
    /// tokens, and an unknown must not render as zero.
    #[test]
    fn a_direct_launch_shows_dashes_in_the_token_columns_and_says_why() {
        let rows = vec![
            LedgerRow {
                provider: "codex".to_owned(),
                kind: ProviderKind::Codex,
                agent: Agent::Claude,
                launches: 14,
                turns: 231,
                input_tokens: 1_200_000,
                cached_tokens: Some(900_000),
                output_tokens: 88_000,
                total_tokens: 1_288_000,
                last_at: 900,
            },
            LedgerRow {
                provider: "ollama".to_owned(),
                kind: ProviderKind::Ollama,
                agent: Agent::Claude,
                launches: 1,
                turns: 0,
                input_tokens: 0,
                cached_tokens: Some(0),
                output_tokens: 0,
                total_tokens: 0,
                last_at: 400,
            },
        ];
        let text = render(&report(vec![], rows), &theme());
        assert!(text.contains("1,200,000"), "{text}");
        assert!(text.contains("900,000"), "{text}");
        assert!(text.contains("75%"), "{text}");
        assert!(text.contains("88,000"), "{text}");
        assert!(
            text.contains("tokens count only where alc carries the traffic"),
            "{text}"
        );
        let ollama = text
            .lines()
            .find(|line| line.contains("ollama"))
            .unwrap_or_default();
        assert!(ollama.contains('—'), "{ollama}");
    }

    #[test]
    fn unknown_cache_totals_render_as_dashes() {
        let row = LedgerRow {
            provider: "codex".to_owned(),
            kind: ProviderKind::Codex,
            agent: Agent::Claude,
            launches: 1,
            turns: 1,
            input_tokens: 100,
            cached_tokens: None,
            output_tokens: 10,
            total_tokens: 110,
            last_at: 900,
        };
        let text = render(&report(vec![], vec![row]), &theme());
        let row = text
            .lines()
            .find(|line| line.contains("codex"))
            .unwrap_or_default();
        assert!(row.contains("100"), "{row}");
        assert_eq!(row.matches('—').count(), 2, "{row}");
    }

    #[test]
    fn json_keeps_schema_one_and_exposes_raw_cache_totals() {
        let row = LedgerRow {
            provider: "codex".to_owned(),
            kind: ProviderKind::Codex,
            agent: Agent::Claude,
            launches: 1,
            turns: 1,
            input_tokens: 100,
            cached_tokens: Some(74),
            output_tokens: 10,
            total_tokens: 110,
            last_at: 900,
        };
        let value = serde_json::to_value(report(vec![], vec![row])).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ledger"]["rows"][0]["cached_tokens"], 74);
        assert!(value["ledger"]["rows"][0].get("cache_percent").is_none());
    }

    #[test]
    fn cache_share_uses_gross_input_and_nearest_whole_rounding() {
        assert_eq!(cache_read_percent(100, 74), Some(74));
        assert_eq!(cache_read_percent(3, 2), Some(67));
        assert_eq!(cache_read_percent(200, 1), Some(1));
        assert_eq!(cache_read_percent(0, 0), None);
        assert_eq!(cache_read_percent(10, 11), None);
    }

    /// A plan with a per-model window for every model it has ever offered
    /// would otherwise render a line of "100% left" nobody reads.
    #[test]
    fn an_untouched_per_model_window_is_left_out_of_the_table() {
        let mut account = account("codex", AccountState::Ok);
        account.windows.push(Window {
            name: "week".to_owned(),
            scope: Some("Spark".to_owned()),
            used_percent: 0.0,
            resets_at: None,
            remaining: None,
            limit: None,
        });
        account.windows.push(Window {
            name: "week".to_owned(),
            scope: Some("Astra".to_owned()),
            used_percent: 4.0,
            resets_at: None,
            remaining: None,
            limit: None,
        });

        let text = render(&report(vec![account], vec![]), &theme());
        assert!(text.contains("Astra week 96% left"), "{text}");
        assert!(!text.contains("Spark"), "{text}");
    }

    #[test]
    fn an_empty_ledger_names_the_file_the_first_launch_writes() {
        let text = render(&report(vec![], vec![]), &theme());
        assert!(text.contains("no launches recorded yet"), "{text}");
        assert!(text.contains("usage.jsonl"), "{text}");
    }

    #[test]
    fn a_countdown_is_coarse_and_a_count_is_exact() {
        assert_eq!(format_countdown(45), "45s");
        assert_eq!(format_countdown(130), "2m");
        assert_eq!(format_countdown(7_800), "2h 10m");
        assert_eq!(format_countdown(3 * 86_400 + 3_600), "3d 1h");
        assert_eq!(query::format_count(999), "999");
        assert_eq!(query::format_count(1_500), "1,500");
        assert_eq!(query::format_count(88_000), "88,000");
        assert_eq!(query::format_count(1_200_000), "1,200,000");
    }

    #[test]
    fn a_home_relative_path_is_elided() {
        let home = Path::new("/home/ada");
        assert_eq!(
            elide_home(Path::new("/home/ada/.codex/auth.json"), Some(home)),
            "~/.codex/auth.json"
        );
        assert_eq!(
            elide_home(Path::new("/opt/codex"), Some(home)),
            "/opt/codex"
        );
    }

    /// A link handed to somebody who may only watch sessions must not tell
    /// them who pays for them or where the credential lives.
    #[test]
    fn a_redacted_report_keeps_the_numbers_and_drops_the_identity() {
        let redacted = report(vec![account("codex", AccountState::Ok)], vec![]).redacted();
        let account = &redacted.accounts[0];
        assert!(account.label.is_none());
        assert!(account.account_id.is_none());
        assert!(account.credential_path.is_none());
        assert_eq!(account.windows.len(), 1);
        assert_eq!(account.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn the_state_a_window_implies_follows_the_warn_threshold() {
        let window = |used: f64| Window {
            name: "5h".to_owned(),
            scope: None,
            used_percent: used,
            resets_at: None,
            remaining: None,
            limit: None,
        };
        assert_eq!(state_from(&[window(74.0)], None, None), AccountState::Ok);
        assert_eq!(state_from(&[window(75.0)], None, None), AccountState::Warn);
        assert_eq!(
            state_from(&[window(100.0)], None, None),
            AccountState::Exhausted
        );
    }

    /// The rule the Codex mapper leaves to `state_from`: a plan at its limit
    /// with credits behind it can still serve a turn, so it is worth
    /// noticing rather than a failure.
    #[test]
    fn a_used_up_plan_with_credits_behind_it_is_a_warning_not_a_failure() {
        let full = Window {
            name: "5h".to_owned(),
            scope: None,
            used_percent: 100.0,
            resets_at: None,
            remaining: None,
            limit: None,
        };
        let credits = |has: bool, unlimited: bool| Credits {
            has_credits: has,
            unlimited,
            balance: None,
        };

        assert_eq!(
            state_from(std::slice::from_ref(&full), None, None),
            AccountState::Exhausted
        );
        assert_eq!(
            state_from(
                std::slice::from_ref(&full),
                None,
                Some(&credits(false, false))
            ),
            AccountState::Exhausted
        );
        assert_eq!(
            state_from(
                std::slice::from_ref(&full),
                None,
                Some(&credits(true, false))
            ),
            AccountState::Warn
        );
        assert_eq!(
            state_from(&[full], None, Some(&credits(false, true))),
            AccountState::Warn
        );
    }

    /// A sibling directory shares a prefix with the home directory as a
    /// string but not as a path, and eliding it would print a `~` path that
    /// names nothing - which for a Claude profile is the only label telling
    /// two logins apart.
    #[test]
    fn a_sibling_of_the_home_directory_is_not_elided() {
        let home = Path::new("/home/ada");
        assert_eq!(
            elide_home(Path::new("/home/ada-work/.claude"), Some(home)),
            "/home/ada-work/.claude"
        );
        assert_eq!(
            elide_home(Path::new("/home/adamant/.codex/auth.json"), Some(home)),
            "/home/adamant/.codex/auth.json"
        );
        assert_eq!(elide_home(home, Some(home)), "~");
    }

    /// A vendor that enables a balance but sends no figures for it would
    /// otherwise leave a separator pointing at nothing.
    #[test]
    fn a_balance_with_no_numbers_adds_nothing_to_the_column() {
        let mut account = account("anthropic", AccountState::Ok);
        account.balance = Some(Balance {
            remaining: None,
            limit: None,
            used: None,
            unit: "USD".to_owned(),
        });

        let text = render(&report(vec![account], vec![]), &theme());
        let row = text
            .lines()
            .find(|line| line.contains("anthropic"))
            .unwrap_or_default()
            .trim_end();
        assert!(
            !row.ends_with('—'),
            "a separator with nothing after it: {row}"
        );
        assert!(row.ends_with("resets in 2h 10m"), "{row}");
    }

    /// Everything a viewer-grade link must not learn: not only who pays for
    /// the plan, but where on disk any of it lives.
    #[test]
    fn a_redacted_report_names_no_path_and_no_person() {
        let mut account = account("codex", AccountState::SignedOut);
        account.error = Some("no Codex credentials at /home/ada/.codex/auth.json".to_owned());
        let mut report = report(vec![account], vec![]);
        report.ledger.path = "/home/ada/.config/alc/usage.jsonl".to_owned();
        report.ledger.error = Some("/home/ada/.config/alc/usage.jsonl: denied".to_owned());

        let json = serde_json::to_string(&report.redacted()).unwrap();
        assert!(!json.contains("/home/ada"), "{json}");
        assert!(!json.contains("me@example.com"), "{json}");
        assert!(json.contains("not signed in on this machine"), "{json}");
    }
}
