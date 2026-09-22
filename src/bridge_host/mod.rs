//! alc's background bridge: the Codex adapter as a process of its own.
//!
//! Claude Code runs background sessions - agent view, `claude --bg`, `←` on an
//! empty prompt - under a supervisor that outlives the terminal and the `alc`
//! that started them. An adapter living inside that `alc` died with it, on a
//! port the next launch could not know. So for Claude Code the adapter is one
//! detached `alc bridge serve` per configuration directory: loopback only, a
//! port chosen once and kept, a token on every model request, started on
//! demand by a launch or by the `apiKeyHelper` inside any session, and gone
//! after an hour with nothing to do. The other seven agents have no background
//! mode and keep their in-process adapter.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};

use crate::config::{Provider, Store};

pub(crate) mod files;
mod serve;

pub(crate) use serve::run as serve;

const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(10);
const STALE_LOCK: Duration = Duration::from_secs(30);

/// What a running bridge says about itself.
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct Hello {
    /// Tells one bridge process from another on a port that was reused.
    /// Nothing in alc compares two hellos yet, so it is carried, not read.
    #[allow(
        dead_code,
        reason = "part of the hello a bridge publishes; no caller compares instances yet"
    )]
    pub instance: String,
    pub alc: String,
    pub pid: u32,
    pub port: u16,
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(HELLO_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(config)
}

/// Asks whatever listens on `port` whether it is this configuration's bridge.
/// Only a listener that knows the token can answer, so some other program on
/// the port is never mistaken for it.
pub(crate) fn hello(port: u16, token: &str) -> Result<Hello> {
    let mut response = agent()
        .get(&format!("http://127.0.0.1:{port}/alc/hello"))
        .header("authorization", &format!("Bearer {token}"))
        .call()
        .context("no alc bridge answered")?;
    let text = response
        .body_mut()
        .read_to_string()
        .context("the bridge's answer could not be read")?;
    serde_json::from_str(&text).context("the bridge's answer did not parse")
}

/// A bridge that answered.
#[derive(Debug, Clone)]
pub(crate) struct Running {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub alc: String,
}

impl Running {
    /// What Claude Code's `ANTHROPIC_BASE_URL` starts with.
    pub(crate) fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// The bridge for `config_dir`, if one is up and answering to its token.
pub(crate) fn probe(config_dir: &Path) -> Option<Running> {
    let token = files::read_token(config_dir)?;
    let port = files::remembered_port(config_dir)?;
    let hello = hello(port, &token).ok()?;
    (hello.port == port).then_some(Running {
        port,
        token,
        pid: hello.pid,
        alc: hello.alc,
    })
}

/// The bridge for `config_dir`, starting one if none is up.
///
/// The probe and the spawn are separated by a lock file: two sessions asking
/// at once must not both start a bridge, because the one that loses the race
/// for the port goes on to rotate the token and rewrite the settings out from
/// under the one that won. Claude Code's supervisor can revive several
/// background sessions together and each runs the helper, so this is ordinary
/// traffic, not a corner. One starter holds the lock from before the spawn
/// until the bridge answers; a lock older than half a minute belongs to a
/// starter that died, and [`take_lock`] says how it is taken over.
pub(crate) fn ensure(config_dir: &Path) -> Result<Running> {
    ensure_within(config_dir, START_TIMEOUT)
}

/// `ensure`, with the deadline its wait uses. Nothing but the tests passes
/// anything other than [`START_TIMEOUT`]: a bridge that has not answered by
/// the deadline is the case the lock has to survive, and driving it through
/// ten real seconds would cost the suite ten seconds on every run.
fn ensure_within(config_dir: &Path, start_timeout: Duration) -> Result<Running> {
    if let Some(running) = probe(config_dir) {
        return Ok(running);
    }
    files::load_or_create_token(config_dir)?;
    let Some(lock) = take_lock(&files::lock_path(config_dir))? else {
        return wait_for(config_dir, start_timeout).context(
            "timed out waiting for another alc to start the background bridge; run `alc bridge status`",
        );
    };
    // Nothing is coming up behind a spawn that failed, so the lock goes at
    // once and the next starter tries again instead of waiting it out.
    if let Err(error) = start_detached(config_dir) {
        lock.release();
        return Err(error);
    }
    let running = wait_for(config_dir, start_timeout);
    // Held until the bridge answers, and deliberately left behind when it has
    // not: a slow start - a cold Windows box scanning a freshly installed
    // binary - used to free the lock while the bridge was still coming up,
    // and the next `ensure` then saw no bridge and no lock and spawned a
    // second one onto the same port. Left in place, it goes stale on its own
    // half a minute later, so a start that really died is not waited on for
    // ever either.
    if running.is_some() {
        lock.release();
    }
    running.context(
        "alc's background bridge did not start; run `alc bridge serve` in a terminal to see why",
    )
}

/// The lock one starter holds while it brings a bridge up.
struct Lock {
    path: PathBuf,
    mark: String,
}

impl Lock {
    /// Gives the lock up, and only if it is still this starter's. A lock is
    /// held for the length of a start, well inside the half minute before
    /// anyone may take it over - but should a start ever outlast that, the
    /// lock is another starter's by then, and removing it would free the name
    /// under a bridge that is coming up.
    fn release(self) {
        if mark_of(&self.path).as_deref() == Some(self.mark.as_str()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Takes the starting lock, or `None` when another alc holds it.
///
/// `create_new` settles it outright when nothing is there. A lock that a dead
/// starter left behind has to be taken over, and several starters can judge
/// the same one dead at once - Claude Code's supervisor reviving a handful of
/// background sessions is enough. Every way of taking it over in one step
/// lost that race: freeing the name and creating it again let each taker
/// remove another's fresh lock, and writing a mark over it and reading it
/// back a moment later lost to a taker that stalled for longer than the
/// moment, which a loaded Windows runner did. So a takeover happens under a
/// lock of its own, and whoever holds that one asks again whether the
/// starting lock is still dead before writing over it. Every other taker
/// finds the takeover lock held or the starting lock alive again, and waits
/// for the bridge as it would have for any live lock.
fn take_lock(lock: &Path) -> Result<Option<Lock>> {
    let mark = owner_mark()?;
    match create_new(lock) {
        Ok(mut file) => {
            file.write_all(mark.as_bytes())
                .with_context(|| format!("failed to write {}", lock.display()))?;
            return Ok(Some(Lock {
                path: lock.to_owned(),
                mark,
            }));
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        // A run directory that is read-only or full is not another alc
        // holding the lock. Saying so sent the reader looking for a second
        // alc that was never there, after a ten-second wait for it.
        Err(error) => {
            return Err(error).with_context(|| format!("failed to create {}", lock.display()));
        }
    }
    if !lock_is_stale(lock) {
        return Ok(None);
    }
    take_over(lock, mark)
}

/// Writes over a dead starter's lock, one taker at a time.
fn take_over(lock: &Path, mark: String) -> Result<Option<Lock>> {
    let takeover = takeover_path(lock);
    // Held only for the few steps below, so one that has gone stale belongs
    // to a taker that died between them; without this it would stop every
    // takeover after it.
    if lock_is_stale(&takeover) {
        let _ = fs::remove_file(&takeover);
    }
    match create_new(&takeover) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to create {}", takeover.display()));
        }
    }
    let taken = (|| {
        // Asked again now that nobody else can be taking it over: the taker
        // before this one may have done it already, and then the lock is alive.
        if !lock_is_stale(lock) {
            return Ok(None);
        }
        // Written as a secret, though a mark is none, for the temp file that
        // comes with it: unguessable and created exclusively.
        crate::config::atomic_write(lock, mark.as_bytes(), true)?;
        Ok(Some(Lock {
            path: lock.to_owned(),
            mark,
        }))
    })();
    let _ = fs::remove_file(&takeover);
    taken
}

/// The lock a takeover of `lock` is made under: its name, and `.takeover`.
fn takeover_path(lock: &Path) -> PathBuf {
    let mut name = lock.file_name().unwrap_or_default().to_owned();
    name.push(".takeover");
    lock.with_file_name(name)
}

/// `create_new`, asked again for a moment when Windows refuses it as access
/// denied. It does that while a file of the same name is being deleted and
/// another process still has it open - a lock being given up, which clears as
/// soon as that handle closes. A refusal that outlasts the retries is a real
/// one, and is reported as such.
fn create_new(path: &Path) -> std::io::Result<fs::File> {
    let mut tries = 0;
    loop {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied && tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(25));
            }
            result => return result,
        }
    }
}

/// What a starter writes into the lock so it can know it again: this process,
/// and a nonce, since an operating system reuses a dead starter's pid.
fn owner_mark() -> Result<String> {
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)
        .context("failed to read operating-system randomness for the bridge's lock")?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{} {nonce}", std::process::id()))
}

/// Whose lock this is, as it says itself.
fn mark_of(lock: &Path) -> Option<String> {
    fs::read_to_string(lock)
        .ok()
        .map(|mark| mark.trim().to_owned())
}

fn wait_for(config_dir: &Path, start_timeout: Duration) -> Option<Running> {
    let deadline = Instant::now() + start_timeout;
    while Instant::now() < deadline {
        if let Some(running) = probe(config_dir) {
            return Some(running);
        }
        thread::sleep(Duration::from_millis(100));
    }
    None
}

fn lock_is_stale(lock: &Path) -> bool {
    fs::metadata(lock)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > STALE_LOCK)
}

/// Starts a bridge that outlives this process - and, when this process is the
/// `apiKeyHelper`, the Claude Code session that ran it. Its standard streams
/// are closed and nothing is inherited: Claude Code reads the helper's output
/// until it ends, and a bridge still holding it would hang every session.
fn start_detached(config_dir: &Path) -> Result<()> {
    let alc = std::env::current_exe().context("failed to find alc's own path")?;
    let mut command = Command::new(alc);
    command
        .arg("--config-dir")
        .arg(config_dir)
        .args(["bridge", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so the Ctrl-C of the terminal that happened
        // to start it, and the shell's SIGHUP on exit, do not reach it.
        command.process_group(0);
    }
    // Not a plain `spawn` on Windows: `win::spawn_detached` also keeps the
    // child from inheriting this process's handles, which is what the helper's
    // stdout being read to end of file depends on.
    #[cfg(windows)]
    crate::remote::win::spawn_detached(&mut command).context("failed to start the bridge")?;
    #[cfg(not(windows))]
    command.spawn().context("failed to start the bridge")?;
    Ok(())
}

/// Stops the bridge, answering the pid of the one it stopped.
pub(crate) fn stop(config_dir: &Path) -> Result<Option<u32>> {
    let Some(running) = probe(config_dir) else {
        return Ok(None);
    };
    agent()
        .post(&format!("{}/alc/stop", running.origin()))
        .header("authorization", &format!("Bearer {}", running.token))
        .send_empty()
        .context("the bridge did not take the stop request")?;
    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if probe(config_dir).is_none() {
            return Ok(Some(running.pid));
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "the bridge (pid {}) did not stop within ten seconds",
        running.pid
    )
}

/// `label  value` rows for `alc bridge status` and `alc doctor`.
pub(crate) fn status_rows(config_dir: &Path) -> Vec<(&'static str, String)> {
    let ours = env!("CARGO_PKG_VERSION");
    let mut rows = Vec::new();
    let probed = probe(config_dir);
    let running = probed.is_some();
    match probed {
        Some(bridge) => {
            rows.push((
                "bridge",
                format!(
                    "running · pid {} · 127.0.0.1:{} · alc {}",
                    bridge.pid, bridge.port, bridge.alc
                ),
            ));
            if bridge.alc != ours {
                rows.push((
                    "version",
                    format!(
                        "started by alc {}; `alc bridge stop` swaps it for alc {ours}, and \
                         sessions reconnect within a minute",
                        bridge.alc
                    ),
                ));
            }
        }
        None => rows.push((
            "bridge",
            "not running · a Claude Code session on a Codex profile starts it when it needs it"
                .to_owned(),
        )),
    }
    // The count is wanted either way; only its wording changes when nothing
    // is up, since a route file left on disk while the bridge is stopped is
    // not being served by anything.
    let routes = files::route_count(config_dir);
    rows.push((
        "routes",
        if running {
            routes.to_string()
        } else {
            format!("{routes} · on disk; nothing is being served")
        },
    ));
    rows
}

/// What `alc claude-credential <route>` prints: the bridge's token for a Codex
/// route, after making sure a bridge is up, or a profile's API key.
///
/// One line, however it was resolved. Claude Code reads the helper's whole
/// stdout as the credential, and `Credentials::key_for` hands back an
/// environment variable exactly as the shell exported it - so a key exported
/// with a trailing newline is trimmed to the one line every background
/// session depends on. Trimming the ends is not enough on its own: a key with
/// a line break inside it - two pasted together, or one a terminal wrapped -
/// still prints two lines, and either half would reach Claude Code as a
/// secret that is not the one on disk. Such a key is refused, with the place
/// to fix it named and the value itself left unprinted.
pub(crate) fn credential(store: &Store, route: &str) -> Result<String> {
    let resolved = resolve_credential(store, route)?;
    let value = resolved.value.trim();
    if value.contains(['\n', '\r']) {
        bail!(
            "the credential alc read from {} has a line break inside it, and Claude Code reads \
             all of this helper's output as one credential; save it again without the break",
            resolved.from
        );
    }
    Ok(value.to_owned())
}

/// A credential and where alc read it, so a refusal can name the place to put
/// right without printing the value.
struct Resolved {
    value: String,
    from: String,
}

fn resolve_credential(store: &Store, route: &str) -> Result<Resolved> {
    if let Some(profile) = route.strip_prefix("profile:") {
        let provider = store.config.providers.get(profile).with_context(|| {
            format!(
                "alc has no provider profile named '{profile}' any more; start the session again \
                 with one that exists"
            )
        })?;
        let value = store
            .credentials
            .key_for(profile, provider)
            .with_context(|| {
                format!(
                    "profile '{profile}' has no API key this session can read; save one with \
                 `alc config key {profile}`"
                )
            })?;
        return Ok(Resolved {
            from: key_source(store, profile, provider),
            value,
        });
    }
    let record = files::read_route(&store.dir, route)?.with_context(|| {
        format!(
            "alc's bridge has no route '{route}'; start `alc claude` once with that Codex profile \
             to recreate it"
        )
    })?;
    if !record.auth_file.is_file() {
        bail!(
            "Codex credentials were not found at {}; run `codex login` and retry",
            record.auth_file.display()
        );
    }
    Ok(Resolved {
        value: ensure(&store.dir)?.token,
        from: format!(
            "the background bridge's token for profile '{}'",
            record.profile
        ),
    })
}

/// Which of the two places `Credentials::key_for` reads a profile's key from
/// it came from, in the words of a message that has to send the user to the
/// right one. Asked in the same order `key_for` looks.
fn key_source(store: &Store, profile: &str, provider: &Provider) -> String {
    if let Some(name) = provider.api_key_env.as_deref()
        && std::env::var(name).is_ok_and(|value| !value.is_empty())
    {
        return format!("the {name} environment variable, for profile '{profile}'");
    }
    format!(
        "profile '{profile}' in {}",
        store.credentials_path().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock left behind by a starter that died: old enough that every
    /// starter looking at it judges it stale at the same moment, which is the
    /// race itself.
    fn stale_lock(config_dir: &Path) -> PathBuf {
        crate::remote::restricted_dir(&files::run_dir(config_dir)).unwrap();
        let path = files::lock_path(config_dir);
        let file = fs::File::create(&path).unwrap();
        file.set_modified(SystemTime::now() - STALE_LOCK - Duration::from_secs(5))
            .unwrap();
        path
    }

    /// Two alcs asking for a bridge at once must not both start one. That is
    /// ordinary traffic: Claude Code's supervisor can revive several
    /// background sessions together, and each of them runs the helper.
    /// Freeing a stale lock and creating it again is three steps, and two
    /// starters that both judged one dead used to take turns removing each
    /// other's fresh lock and both come away holding it. The loser is the one
    /// that does the damage: it binds nothing, then rotates the token and
    /// rewrites the settings out from under the bridge that did start.
    #[test]
    fn only_one_of_several_starters_takes_over_a_stale_lock() {
        for _ in 0..10 {
            let temp = tempfile::tempdir().unwrap();
            let lock = stale_lock(temp.path());
            let start = std::sync::Barrier::new(8);
            let taker = || {
                start.wait();
                take_lock(&lock).unwrap()
            };
            let held: Vec<Lock> = thread::scope(|scope| {
                let takers: Vec<_> = (0..8).map(|_| scope.spawn(taker)).collect();
                takers
                    .into_iter()
                    .filter_map(|handle| handle.join().unwrap())
                    .collect()
            });
            assert_eq!(
                held.len(),
                1,
                "{} starters came away holding one lock",
                held.len()
            );
            assert_eq!(mark_of(&lock).as_deref(), Some(held[0].mark.as_str()));
        }
    }

    /// The race with the timing taken out of it. A taker that judged the
    /// starting lock dead at the same moment as another finds the takeover
    /// already in hand, and stands down rather than writing over it - so the
    /// dead lock is taken over once, by whoever holds the takeover.
    #[test]
    fn a_takeover_already_in_hand_is_not_joined() {
        let temp = tempfile::tempdir().unwrap();
        let lock = stale_lock(temp.path());
        let theirs = "4242 0123456789abcdef";
        fs::write(&lock, theirs).unwrap();
        let file = fs::File::options().write(true).open(&lock).unwrap();
        file.set_modified(SystemTime::now() - STALE_LOCK - Duration::from_secs(5))
            .unwrap();
        drop(file);
        fs::write(takeover_path(&lock), "").unwrap();

        assert!(
            take_lock(&lock).unwrap().is_none(),
            "another taker holds the takeover"
        );
        assert_eq!(
            mark_of(&lock).as_deref(),
            Some(theirs),
            "and standing down leaves the lock as it found it"
        );
    }

    /// The takeover lock is held for a few steps and removed. One that is
    /// still there half a minute later belongs to a taker that died between
    /// them, and must not stop every takeover after it.
    #[test]
    fn a_takeover_left_by_a_taker_that_died_is_taken_over_too() {
        let temp = tempfile::tempdir().unwrap();
        let lock = stale_lock(temp.path());
        let takeover = takeover_path(&lock);
        let file = fs::File::create(&takeover).unwrap();
        file.set_modified(SystemTime::now() - STALE_LOCK - Duration::from_secs(5))
            .unwrap();
        drop(file);

        let held = take_lock(&lock)
            .unwrap()
            .expect("both locks are dead, so this taker gets the starting lock");
        assert_eq!(mark_of(&lock).as_deref(), Some(held.mark.as_str()));
        assert!(!takeover.exists(), "and the takeover lock is gone again");
    }

    /// The lock a live starter holds is not up for grabs, however many ask -
    /// and a lock that was stale a moment ago is not stale any more once
    /// somebody has taken it over. Whether it is stale is asked after the
    /// create has failed rather than before it is attempted, which is what
    /// makes the second half of this true: the taker's mark is already there
    /// by then.
    #[test]
    fn a_lock_that_is_not_stale_is_nobodys_to_take() {
        for start_from_a_dead_lock in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let lock = if start_from_a_dead_lock {
                stale_lock(temp.path())
            } else {
                crate::remote::restricted_dir(&files::run_dir(temp.path())).unwrap();
                files::lock_path(temp.path())
            };
            let held = take_lock(&lock).unwrap().expect("nothing alive held it");
            for _ in 0..3 {
                assert!(
                    take_lock(&lock).unwrap().is_none(),
                    "{start_from_a_dead_lock}"
                );
            }
            assert_eq!(mark_of(&lock).as_deref(), Some(held.mark.as_str()));
            held.release();
            assert!(!lock.exists(), "a released lock is the next starter's");
        }
    }

    /// A bridge can take longer to answer than its starter waits. Freeing the
    /// lock at the deadline anyway left a moment with no bridge and no lock
    /// in it, and the next `ensure` spawned a second bridge into that moment.
    ///
    /// `start_detached` runs this very binary, which under the test harness
    /// does not know what `bridge serve` means and exits at once - a start
    /// that never answers, which is what the deadline is for.
    #[test]
    fn a_bridge_that_has_not_answered_yet_keeps_its_starters_lock() {
        let temp = tempfile::tempdir().unwrap();
        let lock = files::lock_path(temp.path());
        let failed = ensure_within(temp.path(), Duration::from_millis(200)).unwrap_err();
        assert!(
            format!("{failed:#}").contains("did not start"),
            "{failed:#}"
        );
        assert!(
            lock.is_file(),
            "the lock is kept while a bridge may still be coming up"
        );
        assert!(
            take_lock(&lock).unwrap().is_none(),
            "so the next starter waits behind it instead of spawning its own"
        );
    }
}
