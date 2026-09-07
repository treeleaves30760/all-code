//! The control channel between an `alc` command and a running hub.
//!
//! Separate from the browser's HTTP plane on purpose. This one creates
//! processes: it takes a resolved `LaunchSpec` and spawns an agent from it.
//! A browser token must never be able to reach that, so it is a different
//! socket with a different credential, and on unix it is not a network
//! socket at all - a 0600 unix socket inside the 0700 run directory, which
//! the kernel's own permission check gates before a byte is read.
//!
//! Windows has no unix sockets in a form portable enough to rely on here, so
//! it falls back to a loopback TCP listener on an ephemeral port. There the
//! `ctl` secret is the only gate, which is why the secret is required on
//! every request on both platforms rather than only where it is load-bearing.
//!
//! Framing is one JSON object per line. The payloads are small and
//! infrequent (create, list, kill), and a line-delimited protocol stays
//! debuggable with nothing but a socket tool.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::remote::wire::SessionCard;

/// A control connection, whatever it is made of on this platform.
#[cfg(unix)]
pub(crate) type CtlStream = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
pub(crate) type CtlStream = std::net::TcpStream;

#[cfg(unix)]
pub(crate) type CtlListener = std::os::unix::net::UnixListener;
#[cfg(not(unix))]
pub(crate) type CtlListener = std::net::TcpListener;

/// What a client asks the hub to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub(crate) enum CtlRequest {
    /// Confirms this is a hub, which one, and where its page is.
    Hello,
    /// Spawns an agent from an already-resolved launch.
    Create(Box<CreateRequest>),
    List,
    Kill {
        id: String,
    },
    Rename {
        id: String,
        name: String,
    },
    /// Turns this connection into a raw byte relay for one session: the
    /// bytes the pty produces, and the bytes it should consume. Sent by a
    /// terminal attaching to a session the hub owns.
    Attach {
        id: String,
        cols: u16,
        rows: u16,
    },
    /// Out of band, because the relay itself carries only terminal bytes and
    /// a size change is not one.
    Resize {
        id: String,
        cols: u16,
        rows: u16,
    },
    /// Ends the hub. Refused while sessions are live unless `drain`.
    Shutdown {
        drain: bool,
    },
}

/// Everything the hub needs to spawn an agent on the client's behalf.
///
/// `cwd` and `environ` are the reason this type exists. The hub is a
/// long-lived process started from whichever shell happened to run the first
/// `alc --share`; spawning into ITS directory with ITS environment would
/// mean the second session, started from a different repository, quietly
/// edited the first one's. Both travel with every request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CreateRequest {
    pub spec: WireSpec,
    pub cwd: String,
    pub environ: Vec<(String, String)>,
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    pub scrollback_bytes: usize,
    /// What alc believes it launched the agent in, if anything.
    pub permission: crate::remote::permission::PermState,
}

/// A `LaunchSpec` in a form that survives a socket.
///
/// `OsString` has no portable serialisation, and a launch that cannot be
/// represented as UTF-8 is refused loudly rather than lossily converted -
/// silently mangling a path in an agent's arguments would be far worse than
/// declining to share that one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub provider_name: String,
    pub provider_kind: String,
    pub agent: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub secret_values: Vec<String>,
    pub secret_env: Vec<String>,
}

/// What the hub answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "kebab-case")]
pub(crate) enum CtlReply {
    Hello {
        alc: String,
        instance: String,
        port: u16,
        pid: u32,
    },
    Created {
        id: String,
    },
    Sessions {
        sessions: Vec<SessionCard>,
    },
    Ok,
    Error {
        message: String,
    },
}

/// Where the control socket lives. Inside the 0700 run directory, so on unix
/// the directory's own mode is the first gate.
pub(crate) fn socket_path(config_dir: &Path) -> PathBuf {
    crate::remote::settings::Secrets::run_dir(config_dir).join("ctl.sock")
}

/// Where the hub records what it is, for a client deciding whether to join.
pub(crate) fn hub_record_path(config_dir: &Path) -> PathBuf {
    crate::remote::settings::Secrets::run_dir(config_dir).join("hub.json")
}

/// What a running hub publishes about itself. Token *values* are never in
/// here: a client reads its own from the token files, which are 0600.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HubRecord {
    pub pid: u32,
    pub port: u16,
    pub instance: String,
    pub alc: String,
    /// Whether the listener is on the network rather than loopback, so a
    /// later `alc sessions` can print the address a phone actually opens.
    #[serde(default)]
    pub lan: bool,
    /// Windows has no unix socket here, so the control channel is a
    /// loopback port instead. Defaulted so a record written by another
    /// platform - or by an older alc - still parses rather than reading as
    /// "no hub".
    #[cfg(not(unix))]
    #[serde(default)]
    pub ctl_port: u16,
}

/// Sends one request and reads one reply.
pub(crate) fn request(config_dir: &Path, secret: &str, body: &CtlRequest) -> Result<CtlReply> {
    let mut stream = connect(config_dir)?;
    let envelope = serde_json::json!({ "secret": secret, "request": body });
    let mut line = serde_json::to_string(&envelope)?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .context("failed to send a request to the hub")?;
    stream.flush().ok();

    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader
        .read_line(&mut reply)
        .context("the hub closed the connection without answering")?;
    let reply: CtlReply =
        serde_json::from_str(reply.trim()).context("the hub sent an answer alc cannot read")?;
    if let CtlReply::Error { message } = &reply {
        bail!("{message}");
    }
    Ok(reply)
}

#[cfg(unix)]
pub(crate) fn connect(config_dir: &Path) -> Result<CtlStream> {
    let path = socket_path(config_dir);
    CtlStream::connect(&path).with_context(|| {
        format!(
            "no hub is listening at {}; start one with `alc hub start`",
            path.display()
        )
    })
}

#[cfg(not(unix))]
pub(crate) fn connect(config_dir: &Path) -> Result<CtlStream> {
    let record = read_hub_record(config_dir)?
        .context("no hub is running; start one with `alc hub start`")?;
    CtlStream::connect(("127.0.0.1", record.ctl_port))
        .context("no hub is listening; start one with `alc hub start`")
}

/// Binds the control socket, replacing a stale one left by a hub that died.
#[cfg(unix)]
pub(crate) fn listen(config_dir: &Path) -> Result<CtlListener> {
    let path = socket_path(config_dir);
    // A unix socket file outlives the process that made it, so a hub killed
    // with SIGKILL leaves one behind. Connecting to it is what tells the two
    // cases apart: refused means nothing is there.
    if path.exists() && CtlStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = CtlListener::bind(&path)
        .with_context(|| format!("failed to listen on {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict {}", path.display()))?;
    }
    Ok(listener)
}

#[cfg(not(unix))]
pub(crate) fn listen(_config_dir: &Path) -> Result<CtlListener> {
    CtlListener::bind("127.0.0.1:0").context("failed to open the hub's control port")
}

/// Reads one request off a connection, checking the secret before anything
/// else is looked at.
pub(crate) fn read_request<R: BufRead>(reader: &mut R, secret: &str) -> Result<CtlRequest> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read a control request")?;
    let envelope: serde_json::Value =
        serde_json::from_str(line.trim()).context("a control request was not valid JSON")?;
    let presented = envelope
        .get("secret")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    // Constant-time, for the same reason the browser plane's is: on Windows
    // this socket is reachable by any local process.
    if !crate::remote::request::constant_time_eq(presented.as_bytes(), secret.as_bytes()) {
        bail!("the control secret does not match");
    }
    serde_json::from_value(
        envelope
            .get("request")
            .cloned()
            .context("a control request had no body")?,
    )
    .context("a control request named an operation alc does not have")
}

pub(crate) fn read_hub_record(config_dir: &Path) -> Result<Option<HubRecord>> {
    let path = hub_record_path(config_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(serde_json::from_str(&text).ok()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_without_the_secret_is_refused_before_it_is_looked_at() {
        let line = r#"{"request":{"op":"list"}}"#.to_owned() + "\n";
        let error = read_request(&mut line.as_bytes(), "the-real-secret").unwrap_err();
        assert!(error.to_string().contains("secret"), "{error}");
    }

    #[test]
    fn a_request_with_the_wrong_secret_is_refused() {
        let line = r#"{"secret":"guess","request":{"op":"list"}}"#.to_owned() + "\n";
        assert!(read_request(&mut line.as_bytes(), "the-real-secret").is_err());
    }

    #[test]
    fn a_request_with_the_right_secret_parses() {
        let line = r#"{"secret":"the-real-secret","request":{"op":"list"}}"#.to_owned() + "\n";
        let request = read_request(&mut line.as_bytes(), "the-real-secret").unwrap();
        assert!(matches!(request, CtlRequest::List));
    }

    #[test]
    fn an_unknown_operation_is_an_error_rather_than_a_default() {
        let line = r#"{"secret":"s","request":{"op":"detonate"}}"#.to_owned() + "\n";
        assert!(read_request(&mut line.as_bytes(), "s").is_err());
    }

    #[test]
    fn the_socket_lives_inside_the_owner_only_run_directory() {
        let temp = tempfile::tempdir().unwrap();
        let socket = socket_path(temp.path());
        assert!(socket.starts_with(temp.path().join("run")));
        assert_eq!(socket.file_name().unwrap(), "ctl.sock");
    }
}
