//! Changing a running session's permission mode, and being honest about
//! whether it worked.
//!
//! Three agents can be told a mode outright, two can only be asked to open
//! their own picker, two can only be cycled relatively, and one has no
//! permission model at all. `caps.rs` says which; this module carries it
//! out and reports what actually happened rather than flattening every case
//! into success.
//!
//! # Reading the mode back
//!
//! There is no interface for asking a terminal program what mode it is in.
//! The probe matches known phrases against the bottom rows of the rendered
//! screen, which is version-fragile by construction: an agent restyling its
//! status line silently stops it working. So the probe is never allowed to
//! *decide* anything - only to label the control and to stop a cycle early.
//! What the page shows alongside is `Confidence`, which says plainly whether
//! alc knows the mode (it passed the flag), read it (the probe matched), or
//! is guessing.
//!
//! # The escalation gate
//!
//! Tightening is always free, from any viewer. Loosening past the configured
//! ceiling is not something a stolen link should be able to do, so it needs a
//! confirmation typed at a terminal on the host itself: `alc confirm
//! <ticket>`. The ticket lives in the 0700 run directory, which only the
//! machine's owner can write to, and `alc confirm` additionally refuses to
//! run without a controlling terminal - so a ticket cannot be redeemed by
//! the agent piping a command into a shell.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};

use std::ffi::OsString;

use crate::launch::LaunchSpec;
use crate::remote::caps::{AgentCaps, Applied, Confidence, SafetyRung, SetMethod, caps};
use crate::remote::session::Session;
use crate::remote::settings::{Secrets, generate_token};

/// How long a minted ticket may go unredeemed.
const TICKET_LIFETIME: Duration = Duration::from_secs(300);

/// How long a redeemed grant stays usable. Short: the point is that a human
/// is at the machine right now, not that they once were.
const GRANT_LIFETIME: Duration = Duration::from_secs(60);

/// How many rows up from the bottom the probe reads. Status lines live at
/// the bottom of every agent that has one.
const PROBE_ROWS: usize = 4;

/// What the page shows for a session's permission mode.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PermState {
    pub rung: Option<SafetyRung>,
    /// The agent's own name for it, shown beside the rung because a shared
    /// label alone misleads - see the note in `caps.rs`. Owned rather than
    /// borrowed because this crosses the hub's control socket.
    pub native: Option<String>,
    pub confidence: Confidence,
}

impl PermState {
    pub(crate) fn unknown() -> Self {
        Self {
            rung: None,
            native: None,
            confidence: Confidence::Assumed,
        }
    }

    /// The state a session starts in when alc passed the flag itself.
    pub(crate) fn launched(caps: &AgentCaps, rung: SafetyRung) -> Self {
        Self {
            rung: Some(rung),
            native: caps.mode(rung).map(|mode| mode.label.to_owned()),
            confidence: Confidence::Launched,
        }
    }
}

/// Matches the agent's own status line against known phrases.
///
/// Returns `None` when nothing matches, which is the common case and is not
/// an error: most agents do not render their mode anywhere.
pub(crate) fn probe(caps: &AgentCaps, screen: &[String]) -> Option<SafetyRung> {
    if caps.probe.is_empty() {
        return None;
    }
    let tail = screen
        .iter()
        .rev()
        .take(PROBE_ROWS)
        .map(|row| row.to_lowercase())
        .collect::<Vec<_>>();
    for (needle, rung) in caps.probe {
        if tail.iter().any(|row| row.contains(needle)) {
            return Some(*rung);
        }
    }
    None
}

/// Carries out a mode change on a running session.
///
/// `ceiling` is the loosest rung a browser may reach without a confirmation
/// at the host terminal. Tightening is never gated - making a session safer
/// is something an operator should be able to do without ceremony - and
/// every rung past the ceiling is gated every time, not only the first.
pub(crate) fn apply(
    session: &Session,
    rung: SafetyRung,
    ceiling: SafetyRung,
    gate: &EscalationGate,
    granted: Option<&str>,
) -> Result<Applied> {
    let caps = caps(session.agent());
    let Some(mode) = caps.mode(rung) else {
        return Ok(Applied::Unsupported {
            reason: format!(
                "{} has no {rung} mode; it offers {}",
                session.agent(),
                describe_rungs(caps)
            ),
        });
    };

    // Gated on the TARGET, never on the direction of travel.
    //
    // A direction check reads more naturally and is wrong: alc's idea of the
    // mode a session is currently in is usually `Assumed` - it sent a slash
    // command and cannot see whether the agent was mid-turn - so "this is
    // not a loosening, it is already there" is a belief, not a fact. A
    // browser that could get alc to believe a session was already loose
    // would then reach that rung for free. Asking about the destination
    // instead needs no belief at all, and tightening stays free because
    // every strict rung sits at or below the ceiling.
    if rung > ceiling || mode.escalation {
        match granted {
            Some(ticket) if gate.redeem(ticket, rung)? => {}
            _ => {
                return Ok(Applied::NeedsConfirmation {
                    ticket: gate.mint(rung)?,
                    rung,
                });
            }
        }
    }

    match caps.set {
        SetMethod::Unsupported { reason } => Ok(Applied::Unsupported {
            reason: reason.to_owned(),
        }),
        SetMethod::RelaunchOnly => Ok(Applied::NeedsRelaunch {
            rung,
            cli: mode.cli.iter().map(|arg| (*arg).to_owned()).collect(),
        }),
        SetMethod::OpenPicker { command } => {
            session.input(command.as_bytes())?;
            session.set_permission(PermState {
                rung: None,
                native: None,
                confidence: Confidence::Assumed,
            });
            Ok(Applied::PickerOpen {
                command: render(command),
            })
        }
        SetMethod::Absolute { template } => {
            let command = template.replace("{}", mode.label);
            session.input(command.as_bytes())?;
            session.set_permission(PermState {
                rung: Some(rung),
                native: Some(mode.label.to_owned()),
                // Assumed until the screen says otherwise: alc cannot see
                // whether the agent was mid-turn, in which case the slash
                // command became part of the prompt instead.
                confidence: Confidence::Assumed,
            });
            if settled_on(session, caps, rung) {
                return Ok(Applied::Done { rung });
            }
            Ok(Applied::Sent {
                rung,
                bytes: render(&command),
            })
        }
        SetMethod::Cycle { keys, .. } => {
            // Relative only. One press, then let the probe say where it
            // landed - alc does not spin the cycle hunting for a target,
            // because the order is version-dependent and a wrong guess
            // would walk the session somewhere looser than asked.
            session.input(keys.as_bytes())?;
            session.set_permission(PermState {
                rung: None,
                native: None,
                confidence: Confidence::Assumed,
            });
            Ok(Applied::Sent {
                rung,
                bytes: render(keys),
            })
        }
    }
}

/// Waits briefly for the agent to redraw, then asks the screen whether the
/// mode actually changed.
///
/// Bounded and best-effort on purpose. Most agents render nothing to read,
/// and this is only ever the difference between telling the user "sent" and
/// "done" - it never gates the change itself, so a slow redraw costs an
/// honest label rather than a broken session.
fn settled_on(session: &Session, caps: &AgentCaps, rung: SafetyRung) -> bool {
    if caps.probe.is_empty() {
        return false;
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        if session.permission().rung == Some(rung) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn describe_rungs(caps: &AgentCaps) -> String {
    let rungs = caps.rungs();
    if rungs.is_empty() {
        return "none".to_owned();
    }
    rungs
        .iter()
        .map(|rung| rung.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders control bytes so the page can show exactly what was sent.
///
/// The page shows this verbatim. alc cannot tell whether an agent was
/// mid-turn when a slash command arrived - in which case it became part of
/// the prompt rather than a command - so the honest thing is to show the
/// keystrokes and let the user see the result in the terminal beside it.
fn render(raw: &str) -> String {
    raw.chars()
        .map(|character| match character {
            '\u{1b}' => "<ESC>".to_owned(),
            '\r' => "<CR>".to_owned(),
            '\t' => "<TAB>".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

/// Adds the launch arguments for `rung` to a spec, and reports what alc can
/// honestly claim about the resulting session.
///
/// Two rules keep this from doing harm:
///
/// * A flag is injected for an agent alc has not verified ONLY when the user
///   asked for a rung by name. Guessing a flag into argv does not produce a
///   tightened session, it produces one that will not start, and silently
///   breaking `alc --share qwen` to improve a label is a bad trade.
/// * If the user's own passthrough already sets the mode, alc leaves it
///   alone entirely - including the claim about what it is.
pub(crate) fn arm_at_launch(spec: &mut LaunchSpec, requested: Option<SafetyRung>) -> PermState {
    let caps = caps(spec.agent);
    if user_already_set_the_mode(caps, &spec.args) {
        return PermState::unknown();
    }
    let rung = match (requested, caps.flags_verified) {
        (Some(rung), _) => rung,
        // A shared session defaults to asking. Goose in particular starts
        // fully autonomous otherwise, which is not what someone opening a
        // link on a phone expects to be looking at.
        (None, true) => SafetyRung::Ask,
        (None, false) => return PermState::unknown(),
    };
    let Some(mode) = caps.mode(rung) else {
        return PermState::unknown();
    };

    for argument in mode.cli {
        spec.args.insert(0, OsString::from(*argument));
    }
    // Inserted at the front one at a time above reverses them; put the run
    // back in order.
    if !mode.cli.is_empty() {
        let count = mode.cli.len();
        spec.args[..count].reverse();
    }
    if let Some((name, value)) = mode.env {
        spec.env.insert(OsString::from(name), OsString::from(value));
    }
    PermState::launched(caps, rung)
}

/// True when the user's own arguments already choose a mode, in which case
/// alc must not add a second, contradictory one.
fn user_already_set_the_mode(caps: &AgentCaps, args: &[OsString]) -> bool {
    let flags: Vec<&str> = caps
        .modes
        .iter()
        .flat_map(|mode| mode.cli.iter())
        .filter(|argument| argument.starts_with('-'))
        .copied()
        .collect();
    args.iter().any(|argument| {
        argument
            .to_str()
            .is_some_and(|text| flags.contains(&text.split('=').next().unwrap_or(text)))
    })
}

/// Tickets for loosening a session past the configured ceiling.
pub(crate) struct EscalationGate {
    dir: PathBuf,
}

impl EscalationGate {
    /// Lives inside the 0700 run directory, so only the machine's owner can
    /// create or read a ticket in the first place.
    pub(crate) fn new(config_dir: &Path) -> Result<Self> {
        let dir = Secrets::run_dir(config_dir).join("confirm");
        crate::remote::settings::restricted_dir(&dir)?;
        Ok(Self { dir })
    }

    pub(crate) fn mint(&self, rung: SafetyRung) -> Result<String> {
        self.sweep();
        let ticket = short_ticket()?;
        crate::config::atomic_write(&self.request_path(&ticket), rung.as_str().as_bytes(), true)?;
        Ok(ticket)
    }

    /// What `alc confirm` grants. Refuses a ticket that was never minted or
    /// has expired, so a guessed one does nothing.
    pub(crate) fn grant(&self, ticket: &str) -> Result<SafetyRung> {
        validate_ticket(ticket)?;
        let request = self.request_path(ticket);
        let rung: SafetyRung = fs::read_to_string(&request)
            .map_err(|_| anyhow::anyhow!("no pending request matches '{ticket}'"))?
            .trim()
            .parse()?;
        if age(&request)? > TICKET_LIFETIME {
            let _ = fs::remove_file(&request);
            bail!("that request has expired; ask for the change again");
        }
        crate::config::atomic_write(&self.grant_path(ticket), rung.as_str().as_bytes(), true)?;
        Ok(rung)
    }

    /// Consumes a grant. Single use: both files go whether or not the rung
    /// matched, so a grant for one change cannot be replayed for another.
    pub(crate) fn redeem(&self, ticket: &str, rung: SafetyRung) -> Result<bool> {
        if validate_ticket(ticket).is_err() {
            return Ok(false);
        }
        let grant = self.grant_path(ticket);
        let Ok(recorded) = fs::read_to_string(&grant) else {
            return Ok(false);
        };
        let fresh = age(&grant)
            .map(|age| age <= GRANT_LIFETIME)
            .unwrap_or(false);
        let _ = fs::remove_file(&grant);
        let _ = fs::remove_file(self.request_path(ticket));
        Ok(fresh && recorded.trim() == rung.as_str())
    }

    /// Removes tickets nobody redeemed, so the directory does not accumulate
    /// a record of every escalation ever asked for.
    fn sweep(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            if age(&entry.path()).is_ok_and(|age| age > TICKET_LIFETIME) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    fn request_path(&self, ticket: &str) -> PathBuf {
        self.dir.join(format!("{ticket}.request"))
    }

    fn grant_path(&self, ticket: &str) -> PathBuf {
        self.dir.join(format!("{ticket}.granted"))
    }
}

/// Ten characters of the same alphabet a session id uses: short enough to
/// read off a phone and type at a keyboard, long enough not to be guessed
/// inside the five minutes it lives.
fn short_ticket() -> Result<String> {
    Ok(generate_token()?
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(10)
        .collect())
}

/// A ticket becomes a file name, so it is held to an identifier charset
/// rather than trusted.
fn validate_ticket(ticket: &str) -> Result<()> {
    if ticket.len() != 10 || !ticket.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("'{ticket}' is not a confirmation ticket");
    }
    Ok(())
}

fn age(path: &Path) -> Result<Duration> {
    let modified = fs::metadata(path)?.modified()?;
    Ok(SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Agent;
    use crate::remote::caps::caps;

    fn gate() -> (tempfile::TempDir, EscalationGate) {
        let temp = tempfile::tempdir().unwrap();
        let gate = EscalationGate::new(temp.path()).unwrap();
        (temp, gate)
    }

    #[test]
    fn a_ticket_is_only_good_once() {
        let (_temp, gate) = gate();
        let ticket = gate.mint(SafetyRung::Full).unwrap();
        assert_eq!(gate.grant(&ticket).unwrap(), SafetyRung::Full);
        assert!(gate.redeem(&ticket, SafetyRung::Full).unwrap());
        assert!(
            !gate.redeem(&ticket, SafetyRung::Full).unwrap(),
            "a redeemed ticket was accepted a second time"
        );
    }

    #[test]
    fn a_grant_for_one_rung_cannot_be_spent_on_a_looser_one() {
        let (_temp, gate) = gate();
        let ticket = gate.mint(SafetyRung::Auto).unwrap();
        gate.grant(&ticket).unwrap();
        assert!(!gate.redeem(&ticket, SafetyRung::Full).unwrap());
    }

    #[test]
    fn a_ticket_nobody_minted_is_refused() {
        let (_temp, gate) = gate();
        assert!(gate.grant("ABCDEFGHIJ").is_err());
        assert!(!gate.redeem("ABCDEFGHIJ", SafetyRung::Full).unwrap());
    }

    #[test]
    fn a_ticket_that_is_not_an_identifier_is_refused_before_it_reaches_the_filesystem() {
        let (_temp, gate) = gate();
        for hostile in ["../../etc/x", "short", "with space", ""] {
            assert!(gate.grant(hostile).is_err(), "{hostile} was accepted");
            assert!(!gate.redeem(hostile, SafetyRung::Full).unwrap());
        }
    }

    #[test]
    fn a_minted_ticket_is_readable_only_by_its_owner() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let (_temp, gate) = gate();
            let ticket = gate.mint(SafetyRung::Full).unwrap();
            let mode = fs::metadata(gate.request_path(&ticket))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn the_probe_reads_a_mode_off_the_bottom_of_the_screen() {
        let screen = vec![
            "some earlier output".to_owned(),
            "".to_owned(),
            "  ⏵⏵ accept edits on (shift+tab to cycle)".to_owned(),
        ];
        assert_eq!(
            probe(caps(Agent::Claude), &screen),
            Some(SafetyRung::AutoEdit)
        );
    }

    #[test]
    fn the_probe_ignores_a_phrase_that_has_scrolled_out_of_reach() {
        // Only the bottom rows are a status line; the same words higher up
        // are just something the agent printed.
        let mut screen = vec!["plan mode".to_owned()];
        screen.extend(std::iter::repeat_n(String::new(), PROBE_ROWS + 2));
        assert_eq!(probe(caps(Agent::Claude), &screen), None);
    }

    #[test]
    fn an_agent_with_no_status_line_never_reports_a_mode() {
        let screen = vec!["goose> auto".to_owned()];
        assert_eq!(probe(caps(Agent::Goose), &screen), None);
    }

    #[test]
    fn control_bytes_are_rendered_so_the_page_can_show_what_was_sent() {
        assert_eq!(render("\u{1b}[Z"), "<ESC>[Z");
        assert_eq!(render("/mode approve\r"), "/mode approve<CR>");
        assert_eq!(render("\t"), "<TAB>");
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;
    use crate::config::Agent;

    fn launch_spec(agent: Agent) -> LaunchSpec {
        let mut spec = LaunchSpec::for_test();
        spec.agent = agent;
        spec
    }

    #[test]
    fn a_verified_agent_starts_at_ask_with_the_flag_alc_passed() {
        let mut spec = launch_spec(Agent::Claude);
        let state = arm_at_launch(&mut spec, None);

        assert_eq!(state.rung, Some(SafetyRung::Ask));
        assert_eq!(state.confidence, Confidence::Launched);
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--permission-mode"),
                OsString::from("manual")
            ]
        );
    }

    #[test]
    fn a_two_axis_agent_keeps_both_flags_in_order() {
        let mut spec = launch_spec(Agent::Codex);
        arm_at_launch(&mut spec, Some(SafetyRung::Plan));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("-s"),
                OsString::from("read-only"),
                OsString::from("-a"),
                OsString::from("on-request"),
            ]
        );
    }

    #[test]
    fn an_unverified_agent_is_left_alone_unless_the_user_asked() {
        // Guessing `--approval-mode` into argv would break the launch
        // outright if the installed qwen spells it differently.
        let mut spec = launch_spec(Agent::Qwen);
        let state = arm_at_launch(&mut spec, None);
        assert!(spec.args.is_empty());
        assert_eq!(state.confidence, Confidence::Assumed);
        assert_eq!(state.rung, None);

        let mut spec = launch_spec(Agent::Qwen);
        let state = arm_at_launch(&mut spec, Some(SafetyRung::Plan));
        assert_eq!(state.rung, Some(SafetyRung::Plan));
        assert_eq!(
            spec.args,
            vec![OsString::from("--approval-mode"), OsString::from("plan")]
        );
    }

    #[test]
    fn an_agent_that_takes_its_mode_from_the_environment_gets_it_there() {
        let mut spec = launch_spec(Agent::Goose);
        arm_at_launch(&mut spec, Some(SafetyRung::Ask));
        assert!(spec.args.is_empty());
        assert_eq!(
            spec.env.get(&OsString::from("GOOSE_MODE")),
            Some(&OsString::from("approve"))
        );
    }

    #[test]
    fn the_users_own_choice_wins_and_alc_claims_nothing() {
        let mut spec = launch_spec(Agent::Claude);
        spec.args = vec![OsString::from("--permission-mode"), OsString::from("plan")];
        let state = arm_at_launch(&mut spec, Some(SafetyRung::Full));

        assert_eq!(spec.args.len(), 2, "alc added a contradictory flag");
        assert_eq!(state.rung, None);
        assert_eq!(state.confidence, Confidence::Assumed);
    }

    #[test]
    fn pi_is_left_completely_untouched() {
        let mut spec = launch_spec(Agent::Pi);
        let state = arm_at_launch(&mut spec, Some(SafetyRung::Plan));
        assert!(spec.args.is_empty());
        assert!(spec.env.is_empty());
        assert_eq!(state.rung, None);
    }
}

#[cfg(test)]
mod gate_direction_tests {
    use super::*;
    use crate::config::Agent;

    /// The rungs a browser may reach on its own, given a ceiling. Mirrors
    /// what `apply` decides, without needing a live session to ask.
    fn needs_ticket(agent: Agent, rung: SafetyRung, ceiling: SafetyRung) -> bool {
        let caps = caps(agent);
        caps.mode(rung)
            .is_some_and(|mode| rung > ceiling || mode.escalation)
    }

    #[test]
    fn tightening_is_never_gated() {
        for rung in [SafetyRung::Plan, SafetyRung::Ask] {
            for agent in [Agent::Claude, Agent::Codex, Agent::Goose, Agent::Qwen] {
                assert!(
                    !needs_ticket(agent, rung, SafetyRung::AutoEdit),
                    "{agent} gated {rung}"
                );
            }
        }
    }

    #[test]
    fn a_rung_past_the_ceiling_is_gated_every_time_not_only_the_first() {
        // The bypass this guards against: a direction check would let a
        // second request through once alc believed the session had already
        // moved - and that belief is usually only `Assumed`.
        assert!(needs_ticket(
            Agent::Claude,
            SafetyRung::Auto,
            SafetyRung::AutoEdit
        ));
        assert!(needs_ticket(
            Agent::Claude,
            SafetyRung::Auto,
            SafetyRung::AutoEdit
        ));
    }

    #[test]
    fn a_mode_marked_escalation_is_gated_even_below_the_ceiling() {
        // Goose's `auto` is a free pass whatever the ceiling says.
        assert!(needs_ticket(
            Agent::Goose,
            SafetyRung::Auto,
            SafetyRung::Full
        ));
    }

    #[test]
    fn raising_the_ceiling_opens_exactly_the_rungs_below_it() {
        assert!(!needs_ticket(
            Agent::Claude,
            SafetyRung::Auto,
            SafetyRung::Auto
        ));
        assert!(needs_ticket(
            Agent::Claude,
            SafetyRung::Full,
            SafetyRung::Auto
        ));
    }
}
