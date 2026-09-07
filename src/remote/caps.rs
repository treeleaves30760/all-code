//! What each coding agent can be told about permissions, and how.
//!
//! This is the crux of driving eight agents through one control. They do not
//! agree on what a permission mode is, what the modes are called, whether
//! one can be changed mid-session, or even whether the concept exists - so
//! the page cannot show one dropdown and hope. It shows whatever this table
//! says the agent in front of it can actually do.
//!
//! # The ladder, and why the agent's own word is shown next to it
//!
//! `SafetyRung` is alc's five-rung ordering, used for the ceiling check and
//! for "make this stricter" to mean the same thing everywhere. It is NOT
//! shown on its own, because a shared label actively misleads here: `auto`
//! is the most permissive mode Goose has, while Claude Code's `auto` is a
//! mid-tier classifier that is *less* permissive than `bypassPermissions`.
//! The page renders both - `Auto-edit · Claude Code: acceptEdits`.
//!
//! # Verified against a binary, or read from documentation
//!
//! Agent CLIs move faster than a table like this. `--full-auto` is in a lot
//! of Codex documentation and does not exist in codex-cli 0.153.2; Claude
//! Code's `--permission-mode` takes `manual` and has no `default`, which is
//! the opposite of what its own SDK enum accepts. So every row records
//! whether alc confirmed the flags against a real `--help`, and alc only
//! injects a permission flag at launch for the rows where it did. Guessing
//! a flag name into an agent's argv does not produce a tightened session; it
//! produces one that will not start.

use crate::config::Agent;

/// alc's ordering, loosest last. Comparisons drive the ceiling check, so the
/// order of these variants is load-bearing.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SafetyRung {
    /// Reads and plans; writes nothing.
    Plan,
    /// Asks before anything that changes the world.
    Ask,
    /// Edits files without asking; still asks for commands.
    AutoEdit,
    /// Acts without asking, inside whatever sandbox the agent has.
    Auto,
    /// No gate at all.
    Full,
}

impl SafetyRung {
    pub(crate) const ALL: [Self; 5] = [
        Self::Plan,
        Self::Ask,
        Self::AutoEdit,
        Self::Auto,
        Self::Full,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Ask => "ask",
            Self::AutoEdit => "auto-edit",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }
}

impl std::fmt::Display for SafetyRung {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for SafetyRung {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> anyhow::Result<Self> {
        Self::ALL
            .into_iter()
            .find(|rung| rung.as_str() == value.to_ascii_lowercase())
            .ok_or_else(|| {
                let expected = Self::ALL
                    .iter()
                    .map(|rung| rung.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::anyhow!("unknown permission '{value}'; expected one of {expected}")
            })
    }
}

/// One mode an agent actually has, in the agent's own vocabulary.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub(crate) struct NativeMode {
    /// What the agent calls it, shown beside the rung.
    pub label: &'static str,
    /// The launch arguments that select it. A slice because Codex splits the
    /// decision across two independent flags, and collapsing them would make
    /// half its states unreachable.
    #[serde(skip)]
    pub cli: &'static [&'static str],
    /// Environment, for agents that take the mode that way rather than as an
    /// argument.
    #[serde(skip)]
    pub env: Option<(&'static str, &'static str)>,
    pub rung: SafetyRung,
    /// Reaching this from a browser needs a confirmation typed at the host's
    /// own terminal.
    pub escalation: bool,
}

/// How a mode can be changed on a session that is already running.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum SetMethod {
    /// A command that sets the named mode outright. `{}` is replaced with
    /// the native mode's own name.
    Absolute { template: &'static str },
    /// Opens the agent's own picker; a human finishes in the terminal pane.
    /// The page says so rather than pretending the control completed.
    OpenPicker { command: &'static str },
    /// Only relative movement is possible, so the page offers a "cycle"
    /// button and never an absolute dropdown.
    Cycle {
        keys: &'static str,
        order: &'static str,
    },
    /// Nothing can change it after launch.
    RelaunchOnly,
    /// The agent has no such concept. The reason is shown verbatim.
    Unsupported { reason: &'static str },
}

/// Everything the page needs to render one agent's permission control.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub(crate) struct AgentCaps {
    #[serde(serialize_with = "agent_name")]
    pub agent: Agent,
    /// False when the agent gates nothing at all. Drives a visible badge:
    /// a viewer should not have to know Pi's design philosophy to see that
    /// nothing is standing between the model and the filesystem.
    pub sandboxed: bool,
    /// True when alc confirmed these flags against the agent's own `--help`
    /// on a real install. alc injects a permission flag at launch only when
    /// this is true; see the module note.
    pub flags_verified: bool,
    /// True when the in-session mechanism below has been confirmed rather
    /// than read from documentation.
    pub set_verified: bool,
    /// Named when the agent splits the decision across independent axes, so
    /// the page can say why one rung does not describe the whole state.
    pub axes: &'static [&'static str],
    pub modes: &'static [NativeMode],
    pub set: SetMethod,
    /// Screen text that names a mode, lowercased, for reading state back off
    /// the rendered terminal. Version-fragile by construction: used only to
    /// display a mode and to stop a cycle, never to decide anything.
    #[serde(skip)]
    pub probe: &'static [(&'static str, SafetyRung)],
    /// A flag that makes this agent markedly better on a small screen.
    pub mobile_hint: Option<&'static str>,
}

fn agent_name<S: serde::Serializer>(agent: &Agent, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_str(agent.as_str())
}

impl AgentCaps {
    pub(crate) fn mode(&self, rung: SafetyRung) -> Option<&'static NativeMode> {
        self.modes.iter().find(|mode| mode.rung == rung)
    }

    /// The rungs this agent can actually be put in.
    pub(crate) fn rungs(&self) -> Vec<SafetyRung> {
        self.modes.iter().map(|mode| mode.rung).collect()
    }
}

/// How much to trust the mode currently displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Confidence {
    /// alc passed the flag itself and has sent nothing since.
    Launched,
    /// Read back off the rendered screen by the probe.
    Reported,
    /// Neither. Shown with a question mark.
    Assumed,
}

/// What actually happened when a mode change was asked for.
///
/// An enum rather than a `Result<(), _>` because "the keystrokes were sent
/// but alc cannot confirm the agent acted on them" is the honest answer for
/// several agents, and a type that could not express it would force a lie.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub(crate) enum Applied {
    /// Sent and confirmed by re-reading the screen.
    Done {
        rung: SafetyRung,
    },
    /// Sent, but alc cannot see whether it took effect.
    Sent {
        rung: SafetyRung,
        bytes: String,
    },
    /// The agent's own picker is now open in the terminal pane.
    PickerOpen {
        command: String,
    },
    /// Only a relaunch can reach this rung.
    NeedsRelaunch {
        rung: SafetyRung,
        cli: Vec<String>,
    },
    /// Loosening past the ceiling needs a confirmation at the host terminal.
    NeedsConfirmation {
        ticket: String,
        rung: SafetyRung,
    },
    Unsupported {
        reason: String,
    },
}

const CLAUDE_MODES: &[NativeMode] = &[
    NativeMode {
        label: "Plan",
        cli: &["--permission-mode", "plan"],
        env: None,
        rung: SafetyRung::Plan,
        escalation: false,
    },
    // `manual`, not `default`. Claude Code's CLI takes `manual` and does not
    // list `default`; its SDK's control channel is the other way round. A
    // single spelling would silently break one of the two paths.
    NativeMode {
        label: "Ask every time",
        cli: &["--permission-mode", "manual"],
        env: None,
        rung: SafetyRung::Ask,
        escalation: false,
    },
    NativeMode {
        label: "Accept edits",
        cli: &["--permission-mode", "acceptEdits"],
        env: None,
        rung: SafetyRung::AutoEdit,
        escalation: false,
    },
    // Claude Code's `auto` is a classifier, not a free pass - it is *less*
    // permissive than `bypassPermissions`, the opposite of Goose's `auto`.
    NativeMode {
        label: "Auto (model decides)",
        cli: &["--permission-mode", "auto"],
        env: None,
        rung: SafetyRung::Auto,
        escalation: false,
    },
    NativeMode {
        label: "Bypass permissions",
        cli: &["--permission-mode", "bypassPermissions"],
        env: None,
        rung: SafetyRung::Full,
        escalation: true,
    },
];

const CODEX_MODES: &[NativeMode] = &[
    NativeMode {
        label: "Read only",
        cli: &["-s", "read-only", "-a", "on-request"],
        env: None,
        rung: SafetyRung::Plan,
        escalation: false,
    },
    NativeMode {
        label: "Workspace write, ask",
        cli: &["-s", "workspace-write", "-a", "on-request"],
        env: None,
        rung: SafetyRung::Ask,
        escalation: false,
    },
    NativeMode {
        label: "Approve for me",
        cli: &["-s", "workspace-write", "--approve-for-me"],
        env: None,
        rung: SafetyRung::AutoEdit,
        escalation: false,
    },
    NativeMode {
        label: "Workspace write, never ask",
        cli: &["-s", "workspace-write", "-a", "never"],
        env: None,
        rung: SafetyRung::Auto,
        escalation: false,
    },
    NativeMode {
        label: "Full access",
        cli: &["-s", "danger-full-access", "-a", "never"],
        env: None,
        rung: SafetyRung::Full,
        escalation: true,
    },
];

const OPENCODE_MODES: &[NativeMode] = &[
    NativeMode {
        label: "plan agent",
        cli: &["--agent", "plan"],
        env: None,
        rung: SafetyRung::Plan,
        escalation: false,
    },
    NativeMode {
        label: "build agent",
        cli: &["--agent", "build"],
        env: None,
        rung: SafetyRung::Ask,
        escalation: false,
    },
    NativeMode {
        label: "auto-approve",
        cli: &["--auto"],
        env: None,
        rung: SafetyRung::Auto,
        escalation: true,
    },
];

const GOOSE_MODES: &[NativeMode] = &[
    NativeMode {
        label: "chat",
        cli: &[],
        env: Some(("GOOSE_MODE", "chat")),
        rung: SafetyRung::Plan,
        escalation: false,
    },
    NativeMode {
        label: "approve",
        cli: &[],
        env: Some(("GOOSE_MODE", "approve")),
        rung: SafetyRung::Ask,
        escalation: false,
    },
    // Underscored. `smart-approve` is not a mode.
    NativeMode {
        label: "smart_approve",
        cli: &[],
        env: Some(("GOOSE_MODE", "smart_approve")),
        rung: SafetyRung::AutoEdit,
        escalation: false,
    },
    // Goose's own default, and its most permissive setting - the exact
    // inverse of what `auto` means to Claude Code.
    NativeMode {
        label: "auto",
        cli: &[],
        env: Some(("GOOSE_MODE", "auto")),
        rung: SafetyRung::Auto,
        escalation: true,
    },
];

const QWEN_MODES: &[NativeMode] = &[
    NativeMode {
        label: "plan",
        cli: &["--approval-mode", "plan"],
        env: None,
        rung: SafetyRung::Plan,
        escalation: false,
    },
    NativeMode {
        label: "default",
        cli: &["--approval-mode", "default"],
        env: None,
        rung: SafetyRung::Ask,
        escalation: false,
    },
    // Hyphenated, unlike Goose's underscore. Two agents, two spellings of
    // the same idea; a shared constant here would break one of them.
    NativeMode {
        label: "auto-edit",
        cli: &["--approval-mode", "auto-edit"],
        env: None,
        rung: SafetyRung::AutoEdit,
        escalation: false,
    },
    NativeMode {
        label: "auto",
        cli: &["--approval-mode", "auto"],
        env: None,
        rung: SafetyRung::Auto,
        escalation: false,
    },
    NativeMode {
        label: "yolo",
        cli: &["--approval-mode", "yolo"],
        env: None,
        rung: SafetyRung::Full,
        escalation: true,
    },
];

const KIMI_MODES: &[NativeMode] = &[
    NativeMode {
        label: "plan",
        cli: &["--plan"],
        env: None,
        rung: SafetyRung::Plan,
        escalation: false,
    },
    NativeMode {
        label: "Always ask",
        cli: &[],
        env: None,
        rung: SafetyRung::Ask,
        escalation: false,
    },
    NativeMode {
        label: "Ask when needed",
        cli: &["--yolo"],
        env: None,
        rung: SafetyRung::Auto,
        escalation: true,
    },
];

const COPILOT_MODES: &[NativeMode] = &[
    NativeMode {
        label: "plan",
        cli: &["--mode", "plan"],
        env: None,
        rung: SafetyRung::Plan,
        escalation: false,
    },
    NativeMode {
        label: "interactive",
        cli: &["--mode", "interactive"],
        env: None,
        rung: SafetyRung::Ask,
        escalation: false,
    },
    NativeMode {
        label: "allow all tools",
        cli: &["--allow-all-tools"],
        env: None,
        rung: SafetyRung::Full,
        escalation: true,
    },
];

/// The eight rows. Ordered as `Agent::ALL`, and a test asserts that.
pub(crate) const CAPS: [AgentCaps; 8] = [
    AgentCaps {
        agent: Agent::Claude,
        sandboxed: true,
        flags_verified: true,
        set_verified: false,
        axes: &["permission-mode"],
        modes: CLAUDE_MODES,
        // Shift+Tab, and only Shift+Tab. Claude Code has no command that
        // sets a mode outright, so the page offers "cycle" and never a
        // dropdown - a dropdown would imply alc can land on a chosen mode,
        // and from `auto` the first press goes somewhere else entirely.
        set: SetMethod::Cycle {
            keys: "\u{1b}[Z",
            order: "plan → accept edits → ask → (bypass, only if enabled at launch)",
        },
        probe: &[
            ("plan mode", SafetyRung::Plan),
            ("accept edits", SafetyRung::AutoEdit),
            ("bypass permissions", SafetyRung::Full),
        ],
        mobile_hint: None,
    },
    AgentCaps {
        agent: Agent::Codex,
        sandboxed: true,
        flags_verified: true,
        set_verified: false,
        // Two genuinely independent flags. One rung cannot describe a
        // session whose sandbox and approval policy were set separately.
        axes: &["sandbox", "approval"],
        modes: CODEX_MODES,
        set: SetMethod::OpenPicker {
            command: "/permissions\r",
        },
        probe: &[
            ("read only", SafetyRung::Plan),
            ("approve for me", SafetyRung::AutoEdit),
            ("full access", SafetyRung::Full),
        ],
        // Codex draws into the alternate screen by default, which on a phone
        // means no scrollback at all.
        mobile_hint: Some("--no-alt-screen"),
    },
    AgentCaps {
        agent: Agent::Opencode,
        sandboxed: true,
        flags_verified: true,
        set_verified: false,
        // OpenCode's real model is a thirteen-key rule matrix, not a mode
        // enum, so this control is labelled by the agent it selects.
        axes: &["agent"],
        modes: OPENCODE_MODES,
        set: SetMethod::Cycle {
            keys: "\t",
            order: "build ↔ plan",
        },
        probe: &[("plan", SafetyRung::Plan), ("build", SafetyRung::Ask)],
        mobile_hint: Some("--mini"),
    },
    AgentCaps {
        agent: Agent::Pi,
        // Stated plainly rather than dressed up: Pi has no permission modes,
        // no plan mode, no prompts and no sandbox, by design. The control
        // ships disabled with this sentence, and the card carries a badge.
        sandboxed: false,
        flags_verified: false,
        set_verified: false,
        axes: &[],
        modes: &[],
        set: SetMethod::Unsupported {
            reason: "Pi has no permission modes, no plan mode, no permission prompts and no \
                     sandbox, by design. Tool access is fixed at launch with --tools / \
                     --exclude-tools.",
        },
        probe: &[],
        mobile_hint: None,
    },
    AgentCaps {
        agent: Agent::Copilot,
        sandboxed: true,
        flags_verified: false,
        set_verified: false,
        axes: &["mode"],
        modes: COPILOT_MODES,
        set: SetMethod::OpenPicker {
            command: "/permissions\r",
        },
        probe: &[],
        mobile_hint: None,
    },
    AgentCaps {
        agent: Agent::Goose,
        sandboxed: true,
        flags_verified: false,
        set_verified: false,
        axes: &["mode"],
        modes: GOOSE_MODES,
        // The cleanest of the eight: one command, sets the mode outright.
        set: SetMethod::Absolute {
            template: "/mode {}\r",
        },
        // A rustyline REPL with no status line, so there is nothing to read
        // a mode back off.
        probe: &[],
        mobile_hint: None,
    },
    AgentCaps {
        agent: Agent::Qwen,
        sandboxed: true,
        flags_verified: false,
        set_verified: false,
        axes: &["approval-mode"],
        modes: QWEN_MODES,
        set: SetMethod::Absolute {
            template: "/approval-mode {}\r",
        },
        probe: &[],
        mobile_hint: None,
    },
    AgentCaps {
        agent: Agent::Kimi,
        sandboxed: true,
        flags_verified: false,
        set_verified: false,
        axes: &["mode"],
        modes: KIMI_MODES,
        set: SetMethod::RelaunchOnly,
        probe: &[
            ("never ask", SafetyRung::Full),
            ("ask when needed", SafetyRung::Auto),
            ("always ask", SafetyRung::Ask),
        ],
        mobile_hint: None,
    },
];

pub(crate) fn caps(agent: Agent) -> &'static AgentCaps {
    CAPS.iter()
        .find(|caps| caps.agent == agent)
        .unwrap_or(&CAPS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_exactly_one_row_in_agent_order() {
        assert_eq!(CAPS.len(), Agent::ALL.len());
        for (row, agent) in CAPS.iter().zip(Agent::ALL) {
            assert_eq!(row.agent, agent, "the table is out of order at {agent}");
        }
    }

    #[test]
    fn claude_asks_with_manual_because_default_is_not_a_cli_value() {
        // Verified against `claude --help`: the choices are acceptEdits,
        // auto, bypassPermissions, manual, dontAsk, plan. Passing `default`
        // would be rejected outright, and it is what the SDK's control
        // channel wants - so the two spellings must not be shared.
        let mode = caps(Agent::Claude).mode(SafetyRung::Ask).unwrap();
        assert_eq!(mode.cli, &["--permission-mode", "manual"]);
        assert!(!mode.cli.contains(&"default"));
    }

    #[test]
    fn the_table_never_emits_a_codex_flag_that_does_not_exist() {
        // `--full-auto` appears in a lot of Codex documentation and is not
        // in codex-cli 0.153.2. A stale flag here does not tighten a
        // session; it stops the session from starting at all.
        for mode in caps(Agent::Codex).modes {
            assert!(!mode.cli.contains(&"--full-auto"), "{}", mode.label);
        }
    }

    #[test]
    fn goose_and_qwen_keep_their_own_spelling_of_the_same_idea() {
        let goose = caps(Agent::Goose).mode(SafetyRung::AutoEdit).unwrap();
        assert_eq!(goose.env, Some(("GOOSE_MODE", "smart_approve")));
        let qwen = caps(Agent::Qwen).mode(SafetyRung::AutoEdit).unwrap();
        assert_eq!(qwen.cli, &["--approval-mode", "auto-edit"]);
    }

    #[test]
    fn auto_means_opposite_things_and_the_labels_say_so() {
        // Goose's `auto` is its most permissive setting; Claude Code's is a
        // classifier that is stricter than bypassPermissions. A page showing
        // only alc's rung would be actively misleading, which is why every
        // control renders the native label beside it.
        let goose = caps(Agent::Goose).mode(SafetyRung::Auto).unwrap();
        let claude = caps(Agent::Claude).mode(SafetyRung::Auto).unwrap();
        assert!(goose.escalation, "goose auto is a free pass");
        assert!(!claude.escalation, "claude auto still gates");
        assert_ne!(goose.label, claude.label);
    }

    #[test]
    fn pi_is_refused_loudly_rather_than_faked() {
        let pi = caps(Agent::Pi);
        assert!(!pi.sandboxed);
        assert!(pi.modes.is_empty());
        match pi.set {
            SetMethod::Unsupported { reason } => assert!(reason.contains("by design"), "{reason}"),
            other => panic!("Pi must be unsupported, got {other:?}"),
        }
    }

    #[test]
    fn every_full_rung_mode_is_marked_as_needing_confirmation() {
        for row in &CAPS {
            for mode in row.modes {
                if mode.rung == SafetyRung::Full {
                    assert!(
                        mode.escalation,
                        "{} / {} reaches Full without a gate",
                        row.agent, mode.label
                    );
                }
            }
        }
    }

    #[test]
    fn modes_are_listed_in_rung_order_within_each_agent() {
        for row in &CAPS {
            let rungs = row.rungs();
            let mut sorted = rungs.clone();
            sorted.sort();
            assert_eq!(rungs, sorted, "{} lists its modes out of order", row.agent);
        }
    }

    #[test]
    fn an_agent_with_no_verified_flags_is_never_treated_as_confirmed() {
        // alc injects a permission flag at launch only for verified rows.
        // These five come from documentation, and a wrong flag name in argv
        // is a session that will not start.
        for agent in [
            Agent::Pi,
            Agent::Copilot,
            Agent::Goose,
            Agent::Qwen,
            Agent::Kimi,
        ] {
            assert!(!caps(agent).flags_verified, "{agent} claims verification");
        }
        for agent in [Agent::Claude, Agent::Codex, Agent::Opencode] {
            assert!(!caps(agent).modes.is_empty(), "{agent} has no modes");
            assert!(caps(agent).flags_verified, "{agent} was verified by hand");
        }
    }

    #[test]
    fn a_rung_round_trips_through_its_own_spelling() {
        for rung in SafetyRung::ALL {
            assert_eq!(rung.as_str().parse::<SafetyRung>().unwrap(), rung);
        }
        assert!("nonsense".parse::<SafetyRung>().is_err());
    }

    #[test]
    fn the_ladder_orders_from_strictest_to_loosest() {
        assert!(SafetyRung::Plan < SafetyRung::Ask);
        assert!(SafetyRung::Ask < SafetyRung::AutoEdit);
        assert!(SafetyRung::AutoEdit < SafetyRung::Auto);
        assert!(SafetyRung::Auto < SafetyRung::Full);
    }
}
