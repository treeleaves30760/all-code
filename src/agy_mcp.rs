//! `alc mcp agy`: the Antigravity CLI as a tool other agents can call.
//!
//! alc used to run every agent on agy's Google login by borrowing the token
//! agy stores and presenting itself to Google as agy. Antigravity's terms
//! name that as a breach, so it is gone (see
//! [`crate::config::agy_login_is_agy_only`]). What is left is what those
//! terms leave room for: the user's own, unmodified `agy` binary, run on the
//! user's own machine under the user's own sign-in.
//!
//! This is a stdio MCP server with one tool, `agy`. A call runs
//! `agy --print <prompt> --output-format stream-json` in the server's working
//! directory - the project the calling agent was started in - and answers
//! with agy's final message and its conversation id, which a later call can
//! pass back to continue the same conversation. alc reads no credential of
//! agy's and sees none of its traffic: agy signs in, renews and talks to
//! Google itself, exactly as when the user types `agy -p` by hand.
//!
//! agy is a whole coding agent, not a model: it reads files, runs commands
//! and, outside `plan` mode, edits files in the workspace on its own (in
//! print mode it does not stop to ask). The tool says so, so the calling
//! agent can choose `plan` for a read-only second opinion.
//!
//! The wire is newline-delimited JSON-RPC 2.0. Each call runs on a thread of
//! its own, so a long agy run does not hold up `ping` or a second call, and
//! `notifications/cancelled` stops the agy it names.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::config::{Agent, ProviderKind, ReasoningEffort, Store};

/// The protocol revision answered when the client names none alc knows.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions this server's subset of the protocol is valid for.
const KNOWN_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
/// The tool's name, as the calling agent sees it (`mcp__agy__agy` in
/// Claude Code).
const TOOL: &str = "agy";
/// The most of agy's stderr a failed call relays, in characters.
const STDERR_LIMIT: usize = 2_000;

/// The model and effort a call starts on when it names none: the
/// antigravity profile's, as `alc agy` would launch.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Defaults {
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}

impl Defaults {
    /// From the profile `alc agy` resolves to, when that is an antigravity
    /// one; nothing otherwise (agy then picks for itself).
    pub(crate) fn from_store(store: &Store) -> Self {
        match store.config.resolve(Agent::Agy, None) {
            Ok((_, provider)) if provider.kind == ProviderKind::Antigravity => Self {
                model: Some(provider.model.trim().to_owned()).filter(|model| !model.is_empty()),
                effort: provider.reasoning_effort,
            },
            _ => Self::default(),
        }
    }
}

/// One `tools/call` of `agy`, read from its arguments.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Call {
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
    pub mode: Option<String>,
    pub conversation_id: Option<String>,
}

impl Call {
    pub(crate) fn parse(arguments: &Value) -> Result<Self, String> {
        let text = |name: &str| -> Result<Option<String>, String> {
            match arguments.get(name) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) => {
                    Ok(Some(value.trim().to_owned()).filter(|value| !value.is_empty()))
                }
                Some(_) => Err(format!("`{name}` must be a string")),
            }
        };
        let prompt = text("prompt")?.ok_or("`prompt` is required: the task to hand to agy")?;
        let effort = text("effort")?
            .map(|effort| {
                effort
                    .parse::<ReasoningEffort>()
                    .ok()
                    .filter(|effort| *effort != ReasoningEffort::Ultra)
                    .ok_or(format!(
                        "`effort` '{effort}' is not one of low, medium, high, xhigh, max"
                    ))
            })
            .transpose()?;
        let mode = text("mode")?;
        if let Some(mode) = &mode
            && !matches!(mode.as_str(), "plan" | "accept-edits" | "auto")
        {
            return Err(format!(
                "`mode` '{mode}' is not one of plan, accept-edits, auto"
            ));
        }
        let conversation_id = text("conversation_id")?;
        if let Some(id) = &conversation_id
            && !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("`conversation_id` is the id an earlier answer gave".to_owned());
        }
        Ok(Self {
            prompt,
            model: text("model")?,
            effort,
            mode,
            conversation_id,
        })
    }

    /// agy's arguments for this call. The prompt is `--print`'s own value,
    /// which agy takes as the next word whatever it starts with, so a prompt
    /// beginning with `-` is still a prompt.
    ///
    /// Slash commands are off, so a prompt beginning with `/` is a prompt
    /// too - except with a `mode`, which agy applies through that same
    /// expansion and ignores, with a warning, while it is off (agy 1.5.18).
    pub(crate) fn args(&self, defaults: &Defaults) -> Vec<OsString> {
        let effort = self
            .effort
            .or(defaults.effort)
            .map(crate::agents::agy::clamp_for_agy);
        let mut args: Vec<OsString> = vec![
            "--print".into(),
            self.prompt.clone().into(),
            "--output-format".into(),
            "stream-json".into(),
        ];
        if self.mode.is_none() {
            args.push("--disable-slash-commands".into());
        }
        let model = self.model.clone().or_else(|| {
            defaults.model.as_deref().map(|model| {
                if crate::antigravity::family(model).is_some() {
                    crate::antigravity::wire_model_for(model, effort)
                } else {
                    model.to_owned()
                }
            })
        });
        if let Some(model) = model {
            args.extend(["--model".into(), model.into()]);
        }
        if let Some(effort) = effort {
            args.extend(["--effort".into(), effort.as_str().into()]);
        }
        if let Some(mode) = &self.mode {
            args.extend(["--mode".into(), mode.into()]);
        }
        if let Some(id) = &self.conversation_id {
            args.extend(["--conversation".into(), id.into()]);
        }
        args
    }
}

/// What one agy run came back with, read from its stream-json output.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Outcome {
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub response: String,
    pub usage: Option<Value>,
    pub duration_seconds: Option<f64>,
    /// What headless agy refused for want of a prompt it cannot show, by
    /// display name (`Bash`): its run still reads as a success.
    pub denied: Vec<String>,
    /// Lines that were not stream-json events (agy's own errors land here
    /// when it fails before the stream starts).
    pub stray: Vec<String>,
}

impl Outcome {
    /// Folds one line of agy's stdout in, returning a short note on what agy
    /// is doing for a progress notification.
    pub(crate) fn read_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            self.stray.push(line.to_owned());
            return None;
        };
        let text =
            |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        match event.get("event").and_then(Value::as_str) {
            Some("init") => {
                self.conversation_id =
                    text(&event, "conversation_id").or(self.conversation_id.take());
                self.model = event.get("init").and_then(|init| text(init, "model"));
                None
            }
            Some("step_update") => {
                let step = event.get("step_update")?;
                if self.conversation_id.is_none() {
                    self.conversation_id = text(step, "conversation_id");
                }
                if text(step, "state").as_deref() != Some("ACTIVE") {
                    return None;
                }
                match text(step, "step_type").as_deref() {
                    Some("tool") => text(step, "tool_name").map(|tool| format!("agy: {tool}")),
                    Some("agent_response") => Some("agy: writing".to_owned()),
                    _ => None,
                }
            }
            Some("result") => {
                let result = event.get("result")?;
                if let Some(id) = text(result, "conversation_id") {
                    self.conversation_id = Some(id);
                }
                self.status = text(result, "status");
                self.response = text(result, "response").unwrap_or_default();
                self.usage = result.get("usage").cloned();
                self.duration_seconds = result.get("duration_seconds").and_then(Value::as_f64);
                self.denied = result
                    .get("denied_actions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|action| {
                        text(action, "display_name").or_else(|| text(action, "action"))
                    })
                    .collect();
                None
            }
            _ => {
                self.stray.push(line.to_owned());
                None
            }
        }
    }

    /// The `tools/call` result: agy's answer, with what is needed to carry
    /// on, as text for any client and as structured content for the ones
    /// that read it.
    pub(crate) fn to_result(&self, exit: Option<i32>, stderr: &str) -> Value {
        let succeeded = exit == Some(0) && self.status.as_deref() == Some("SUCCESS");
        let mut text = if succeeded {
            self.response.trim_end().to_owned()
        } else {
            let mut why = format!(
                "agy did not finish ({}).",
                match (&self.status, exit) {
                    (Some(status), _) => format!("status {status}"),
                    (None, Some(code)) => format!("exit code {code}"),
                    (None, None) => "stopped by a signal".to_owned(),
                }
            );
            let detail: Vec<&str> = self
                .stray
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(stderr.trim()))
                .filter(|line| !line.is_empty())
                .collect();
            if !detail.is_empty() {
                let detail = detail.join("\n");
                let detail: String = detail.chars().take(STDERR_LIMIT).collect();
                why.push_str("\n\n");
                why.push_str(&detail);
            }
            if !self.response.trim().is_empty() {
                why.push_str("\n\nPartial answer:\n");
                why.push_str(self.response.trim_end());
            }
            why
        };
        if !self.denied.is_empty() {
            // agy cannot ask in print mode, so it refused; the user decides
            // what agy may run, in agy's own settings - never alc.
            if text.trim().is_empty() {
                text = "agy produced no answer.".to_owned();
            }
            text.push_str(&format!(
                "\n\n[agy was denied: {}. Headless agy cannot ask for permission; the user can \
                 allow it under permissions.allow in agy's settings.json]",
                self.denied.join(", ")
            ));
        }
        if let Some(id) = &self.conversation_id {
            text.push_str(&format!(
                "\n\n[agy conversation_id: {id}; pass it back to continue]"
            ));
        }
        json!({
            "content": [{"type": "text", "text": text}],
            "structuredContent": {
                "response": self.response,
                "conversation_id": self.conversation_id,
                "model": self.model,
                "status": self.status,
                "usage": self.usage,
                "duration_seconds": self.duration_seconds,
                "denied": self.denied,
            },
            "isError": !succeeded,
        })
    }
}

/// The one tool, as `tools/list` describes it.
pub(crate) fn tool_definition() -> Value {
    json!({
        "name": TOOL,
        "title": "Antigravity CLI (agy)",
        "description": "Hand a task to Google's Antigravity CLI, `agy`, which runs on the user's own \
            agy sign-in in this project directory and answers with its final message. agy is a \
            full coding agent with its own tools: it reads files, runs commands and, unless \
            mode is `plan`, edits files in the workspace itself without asking. Give it a \
            self-contained task; it does not see this conversation. Use `plan` for a read-only \
            review or second opinion. Pass the conversation_id from an earlier answer to \
            continue that conversation.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The task, complete on its own: agy sees nothing of the calling conversation."
                },
                "mode": {
                    "type": "string",
                    "enum": ["plan", "accept-edits", "auto"],
                    "description": "agy's execution mode. `plan` reads and plans without editing. Omitted, agy uses its own default, which edits files."
                },
                "model": {
                    "type": "string",
                    "description": "An agy model id, as `agy models` lists them (for example gemini-3.8-flash-low). Omitted, the antigravity profile's model."
                },
                "effort": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "xhigh", "max"],
                    "description": "agy's reasoning effort. Omitted, the antigravity profile's."
                },
                "conversation_id": {
                    "type": "string",
                    "description": "Continue the agy conversation an earlier answer named."
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        },
        "annotations": {
            "title": "Antigravity CLI (agy)",
            "readOnlyHint": false,
            "destructiveHint": true,
            "openWorldHint": true
        }
    })
}

/// Writes whole JSON-RPC messages to stdout, one per line, from any thread.
#[derive(Clone)]
struct Out(Arc<Mutex<std::io::Stdout>>);

impl Out {
    fn send(&self, message: &Value) {
        let mut stdout = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        // A closed stdout means the client is gone; the read loop ends on its
        // own when stdin closes too.
        let _ = writeln!(stdout, "{message}");
        let _ = stdout.flush();
    }

    fn reply(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&self, id: &Value, code: i64, message: &str) {
        self.send(
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
        );
    }
}

/// The agy runs in flight, by request id, so a cancellation can stop one.
type Running = Arc<Mutex<HashMap<String, Arc<Mutex<Child>>>>>;

/// Serves MCP on stdin/stdout until stdin closes.
pub(crate) fn serve(store: &Store) -> Result<u8> {
    let defaults = Defaults::from_store(store);
    let binary = agy_binary();
    let out = Out(Arc::new(Mutex::new(std::io::stdout())));
    let running: Running = Arc::default();
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.context("could not read the MCP client's request")?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                out.error(&Value::Null, -32700, &format!("not JSON: {error}"));
                continue;
            }
        };
        handle(&message, &out, &running, &defaults, binary.as_ref());
    }
    for child in running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
    {
        let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
    }
    Ok(0)
}

fn agy_binary() -> Option<PathBuf> {
    crate::doctor::resolve(&crate::doctor::binary_for(Agent::Agy))
}

fn handle(
    message: &Value,
    out: &Out,
    running: &Running,
    defaults: &Defaults,
    binary: Option<&PathBuf>,
) {
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").cloned();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // A notification: nothing is answered.
        if method == Some("notifications/cancelled")
            && let Some(request) = params.get("requestId")
        {
            let key = request.to_string();
            if let Some(child) = running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key)
            {
                let _ = child.lock().unwrap_or_else(PoisonError::into_inner).kill();
            }
        }
        return;
    };
    match method {
        Some("initialize") => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|version| KNOWN_VERSIONS.contains(version))
                .unwrap_or(PROTOCOL_VERSION);
            out.reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "alc-agy", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "One tool, `agy`, hands a self-contained task to the user's \
                        Antigravity CLI and returns its final answer."
                }),
            );
        }
        Some("ping") => out.reply(&id, json!({})),
        Some("tools/list") => out.reply(&id, json!({"tools": [tool_definition()]})),
        Some("tools/call") => {
            let name = params.get("name").and_then(Value::as_str);
            if name != Some(TOOL) {
                out.error(
                    &id,
                    -32602,
                    &format!("unknown tool {name:?}; the one tool is `agy`"),
                );
                return;
            }
            let call = match Call::parse(params.get("arguments").unwrap_or(&Value::Null)) {
                Ok(call) => call,
                Err(why) => {
                    out.reply(
                        &id,
                        json!({"content": [{"type": "text", "text": why}], "isError": true}),
                    );
                    return;
                }
            };
            let Some(binary) = binary.cloned() else {
                out.reply(
                    &id,
                    json!({"content": [{"type": "text", "text": "agy is not installed or not on PATH; install the Antigravity CLI and sign in with `agy` first"}], "isError": true}),
                );
                return;
            };
            let progress = params
                .get("_meta")
                .and_then(|meta| meta.get("progressToken"))
                .cloned();
            let (out, running, args) = (out.clone(), Arc::clone(running), call.args(defaults));
            std::thread::spawn(move || {
                let result = run(&binary, &args, &id, &running, |note, step| {
                    if let Some(token) = &progress {
                        out.send(
                            &json!({"jsonrpc": "2.0", "method": "notifications/progress",
                            "params": {"progressToken": token, "progress": step, "message": note}}),
                        );
                    }
                });
                match result {
                    Ok(result) => out.reply(&id, result),
                    Err(error) => out.reply(
                        &id,
                        json!({"content": [{"type": "text", "text": format!("{error:#}")}], "isError": true}),
                    ),
                }
            });
        }
        Some(other) => out.error(&id, -32601, &format!("method {other} is not served")),
        None => out.error(&id, -32600, "a request needs a method"),
    }
}

/// Runs agy once and reads its stream to the end.
fn run(
    binary: &PathBuf,
    args: &[OsString],
    id: &Value,
    running: &Running,
    mut progress: impl FnMut(&str, u64),
) -> Result<Value> {
    let mut child = Command::new(binary)
        .args(args)
        // agy on its own login, whatever the calling agent's environment
        // pointed it at.
        .env_remove("CLOUD_CODE_URL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {}", binary.display()))?;
    let stdout = child
        .stdout
        .take()
        .context("agy's stdout was not captured")?;
    let mut stderr = child
        .stderr
        .take()
        .context("agy's stderr was not captured")?;
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let child = Arc::new(Mutex::new(child));
    let key = id.to_string();
    running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key.clone(), Arc::clone(&child));

    let mut outcome = Outcome::default();
    let mut steps = 0_u64;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if let Some(note) = outcome.read_line(&line) {
            steps += 1;
            progress(&note, steps);
        }
    }
    let status = child
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .wait()
        .context("could not wait for agy")?;
    running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&key);
    let stderr = errors.join().unwrap_or_default();
    Ok(outcome.to_result(status.code(), &stderr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: Value) -> Result<Call, String> {
        Call::parse(&arguments)
    }

    #[test]
    fn a_call_needs_a_prompt_and_refuses_what_agy_would_not_take() {
        assert!(
            call(json!({}))
                .unwrap_err()
                .contains("`prompt` is required")
        );
        assert!(call(json!({"prompt": "  "})).is_err());
        assert!(call(json!({"prompt": 3})).is_err());
        assert!(call(json!({"prompt": "x", "effort": "ultra"})).is_err());
        assert!(call(json!({"prompt": "x", "mode": "yolo"})).is_err());
        assert!(call(json!({"prompt": "x", "conversation_id": "a; rm -rf /"})).is_err());
        let ok = call(json!({"prompt": "fix it", "mode": "plan", "effort": "high",
            "conversation_id": "95421994-d01a-4ada-8ec8-ddeec063c5c3"}))
        .unwrap();
        assert_eq!(ok.mode.as_deref(), Some("plan"));
        assert_eq!(ok.effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn a_call_runs_agy_print_on_the_profile_defaults_unless_it_names_its_own() {
        let defaults = Defaults {
            model: Some("claude-opus-5-5".to_owned()),
            effort: Some(ReasoningEffort::Ultra),
        };
        let args: Vec<String> = call(json!({"prompt": "--help me"}))
            .unwrap()
            .args(&defaults)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "--print",
                "--help me",
                "--output-format",
                "stream-json",
                "--disable-slash-commands",
                "--model",
                "claude-opus-5-5-high",
                "--effort",
                "max"
            ]
        );
        let args: Vec<String> = call(json!({"prompt": "p", "model": "gemini-3.8-flash-low",
            "effort": "low", "mode": "plan", "conversation_id": "c-1"}))
        .unwrap()
        .args(&defaults)
        .into_iter()
        .map(|arg| arg.into_string().unwrap())
        .collect();
        assert!(
            !args.iter().any(|arg| arg == "--disable-slash-commands"),
            "agy ignores --mode while slash commands are off"
        );
        assert_eq!(
            args[4..],
            [
                "--model",
                "gemini-3.8-flash-low",
                "--effort",
                "low",
                "--mode",
                "plan",
                "--conversation",
                "c-1"
            ]
        );
        let bare: Vec<OsString> = call(json!({"prompt": "p"}))
            .unwrap()
            .args(&Defaults::default());
        assert_eq!(bare.len(), 5, "no profile: agy chooses for itself");
    }

    /// Captured from agy 1.5.18 (`agy -p ... --output-format stream-json`).
    const STREAM: &str = r#"{"event":"init","conversation_id":"c08e","init":{"model":"gemini-3.8-flash-low","cwd":"/w","tools":["view_file"],"permission_mode":"request-review"}}
{"event":"step_update","step_update":{"conversation_id":"c08e","step_index":0,"state":"DONE","step_type":"user_input"}}
{"event":"step_update","step_update":{"conversation_id":"c08e","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"write_file"}}
{"event":"step_update","step_update":{"conversation_id":"c08e","step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"pong"}}
{"event":"result","result":{"conversation_id":"c08e","status":"SUCCESS","response":"pong\n","duration_seconds":1.9,"num_turns":1,"usage":{"input_tokens":11391,"output_tokens":1}}}"#;

    #[test]
    fn the_stream_folds_into_the_answer_and_the_id_that_continues_it() {
        let mut outcome = Outcome::default();
        let notes: Vec<String> = STREAM
            .lines()
            .filter_map(|line| outcome.read_line(line))
            .collect();
        assert_eq!(notes, ["agy: write_file", "agy: writing"]);
        assert_eq!(outcome.conversation_id.as_deref(), Some("c08e"));
        assert_eq!(outcome.model.as_deref(), Some("gemini-3.8-flash-low"));
        let result = outcome.to_result(Some(0), "");
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.starts_with("pong\n\n[agy conversation_id: c08e"),
            "{text}"
        );
        assert_eq!(result["structuredContent"]["usage"]["input_tokens"], 11391);
    }

    #[test]
    fn what_headless_agy_was_denied_is_said_rather_than_an_empty_answer() {
        let mut outcome = Outcome::default();
        outcome.read_line(r#"{"event":"result","result":{"conversation_id":"c3","status":"SUCCESS","response":"","denied_actions":[{"action":"command","display_name":"Bash"}]}}"#);
        let result = outcome.to_result(Some(0), "");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("agy produced no answer."), "{text}");
        assert!(text.contains("agy was denied: Bash"), "{text}");
        assert_eq!(result["structuredContent"]["denied"], json!(["Bash"]));
    }

    #[test]
    fn a_failed_run_says_why_with_what_agy_printed() {
        let mut outcome = Outcome::default();
        outcome.read_line("Error: not signed in; run agy");
        let result = outcome.to_result(Some(1), "token expired\n");
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("exit code 1"), "{text}");
        assert!(
            text.contains("not signed in") && text.contains("token expired"),
            "{text}"
        );
    }

    #[test]
    fn the_tool_tells_the_caller_agy_edits_files_and_how_to_keep_it_read_only() {
        let tool = tool_definition();
        let description = tool["description"].as_str().unwrap();
        assert!(description.contains("edits files"), "{description}");
        assert!(description.contains("`plan`"), "{description}");
        assert_eq!(tool["inputSchema"]["required"], json!(["prompt"]));
    }

    /// One call against a stand-in agy that prints the captured stream: the
    /// arguments it was given, the answer, and nothing left running.
    #[cfg(unix)]
    #[test]
    fn a_call_runs_the_binary_and_forgets_it_once_finished() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let stream = temp.path().join("stream.jsonl");
        std::fs::write(&stream, STREAM).unwrap();
        let fake = temp.path().join("agy");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat '{}'\n",
                temp.path().join("args.txt").display(),
                stream.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let running: Running = Arc::default();
        let args = call(json!({"prompt": "ping"}))
            .unwrap()
            .args(&Defaults::default());
        let result = run(&fake, &args, &json!(7), &running, |_, _| {}).unwrap();
        assert_eq!(result["isError"], false, "{result}");
        let seen = std::fs::read_to_string(temp.path().join("args.txt")).unwrap();
        assert!(
            seen.starts_with("--print\nping\n--output-format\nstream-json"),
            "{seen}"
        );
        assert!(
            running.lock().unwrap().is_empty(),
            "a finished run is forgotten"
        );
    }
}
