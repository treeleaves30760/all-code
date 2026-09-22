//! Where the background bridge keeps its state, all in `<config>/run/`:
//! `bridge.port` (the port, chosen once), `bridge.token` (0600, the token every
//! model request must carry), `bridge.lock` (held while one is starting) and
//! `bridge/routes/<id>.json` (one Codex login a Claude Code session runs on).

use std::fs;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bridge::tiers::ModelTiers;
use crate::remote::{Secrets, generate_token, restricted_dir};

/// The ports a bridge picks from: below the ranges Linux (32768 and up) and
/// Windows and macOS (49152 and up) hand out for outgoing connections, so none
/// of those can be sitting on it while the bridge is down.
pub(crate) const PORT_RANGE: Range<u16> = 20_000..30_000;

pub(crate) fn run_dir(config_dir: &Path) -> PathBuf {
    Secrets::run_dir(config_dir)
}

pub(crate) fn port_path(config_dir: &Path) -> PathBuf {
    run_dir(config_dir).join("bridge.port")
}

pub(crate) fn token_path(config_dir: &Path) -> PathBuf {
    run_dir(config_dir).join("bridge.token")
}

pub(crate) fn lock_path(config_dir: &Path) -> PathBuf {
    run_dir(config_dir).join("bridge.lock")
}

pub(crate) fn routes_dir(config_dir: &Path) -> PathBuf {
    run_dir(config_dir).join("bridge").join("routes")
}

pub(crate) fn read_token(config_dir: &Path) -> Option<String> {
    fs::read_to_string(token_path(config_dir))
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
}

/// How long a caller that lost the race to create the token file waits for
/// the winner to write the token into it.
const TOKEN_WAIT: Duration = Duration::from_secs(2);

/// How often that caller looks while it waits.
const TOKEN_POLL: Duration = Duration::from_millis(20);

/// The bridge's token, minted by whichever alc asks first.
///
/// Minting is exclusive. A launch and a session's `apiKeyHelper` can both find
/// no token on a fresh configuration, and if each wrote its own, the last write
/// would win while the other caller went on holding a token that is on no
/// disk. A bridge started with that one refuses every request carrying the
/// token everyone else reads. So the file is created with `create_new`:
/// exactly one caller makes it, and every other adopts what that one writes.
pub(crate) fn load_or_create_token(config_dir: &Path) -> Result<String> {
    if let Some(token) = read_token(config_dir) {
        return Ok(token);
    }
    // Minted before the file exists, so nothing that can fail stands between
    // creating the file and writing the token into it.
    let token = generate_token()?;
    restricted_dir(&run_dir(config_dir))?;
    let path = token_path(config_dir);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return adopt_token(config_dir);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to create {}", path.display()));
        }
    };
    if let Err(error) = file
        .write_all(token.as_bytes())
        .and_then(|()| file.sync_all())
    {
        // Left behind, an empty file would keep every later caller waiting for
        // a token that is never coming.
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(error).with_context(|| format!("failed to write {}", path.display()));
    }
    Ok(token)
}

/// Adopts the token another alc is writing. Its file exists from the moment
/// that alc creates it and reads empty until the token is in it, so an empty
/// read here means "not yet" - until it has meant that for `TOKEN_WAIT`.
fn adopt_token(config_dir: &Path) -> Result<String> {
    let deadline = Instant::now() + TOKEN_WAIT;
    loop {
        if let Some(token) = read_token(config_dir) {
            return Ok(token);
        }
        if Instant::now() >= deadline {
            bail!(
                "{} still holds no token {} seconds after another alc created it; \
                 delete it and try again",
                token_path(config_dir).display(),
                TOKEN_WAIT.as_secs()
            );
        }
        thread::sleep(TOKEN_POLL);
    }
}

/// Mints a fresh token, replacing the one on disk. Every session still holding
/// the old one is answered 401 on its next request, and Claude Code runs its
/// helper again on a 401 - so rotating heals on its own. Only a running bridge
/// rotates, when it has to move ports; everything else goes through
/// `load_or_create_token`, which never replaces a token.
pub(crate) fn rotate_token(config_dir: &Path) -> Result<String> {
    restricted_dir(&run_dir(config_dir))?;
    let token = generate_token()?;
    crate::config::atomic_write(&token_path(config_dir), token.as_bytes(), true)?;
    Ok(token)
}

pub(crate) fn remembered_port(config_dir: &Path) -> Option<u16> {
    fs::read_to_string(port_path(config_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|port| PORT_RANGE.contains(port))
}

pub(crate) fn remember_port(config_dir: &Path, port: u16) -> Result<()> {
    restricted_dir(&run_dir(config_dir))?;
    crate::config::atomic_write(&port_path(config_dir), port.to_string().as_bytes(), false)
}

pub(crate) fn choose_port() -> Result<u16> {
    let mut bytes = [0_u8; 2];
    getrandom::fill(&mut bytes)
        .context("failed to read operating-system randomness for the bridge's port")?;
    let span = PORT_RANGE.end - PORT_RANGE.start;
    Ok(PORT_RANGE.start + u16::from_le_bytes(bytes) % span)
}

/// One Codex login a Claude Code session runs on, written by the launch that
/// resolved it in the user's own shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RouteRecord {
    pub id: String,
    /// The alc profile, which the usage ledger credits.
    pub profile: String,
    /// The Codex `auth.json` this route's requests sign with.
    pub auth_file: PathBuf,
    /// Where Claude's own model ids land; see `bridge::tiers`.
    pub tiers: ModelTiers,
}

impl RouteRecord {
    pub(crate) fn new(profile: &str, auth_file: PathBuf, tiers: ModelTiers) -> Self {
        Self {
            id: route_id(profile, &auth_file),
            profile: profile.to_owned(),
            auth_file,
            tiers,
        }
    }
}

/// `codex-` and twelve hex digits of the profile and the login it names, so two
/// Codex homes are two routes and a session keeps the account it started on.
pub(crate) fn route_id(profile: &str, auth_file: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(profile.as_bytes());
    hasher.update([0]);
    hasher.update(auth_file.to_string_lossy().as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("codex-{hex}")
}

pub(crate) fn valid_route_id(id: &str) -> bool {
    id.strip_prefix("codex-").is_some_and(|hex| {
        hex.len() == 12
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Writes `route` under its own name, and only a route alc named.
///
/// The hub writes records that arrive over its control socket, so a record is
/// checked here rather than trusted for having come from `RouteRecord::new`.
/// A name that is really a path would put a file wherever it points, and a
/// well-formed name over another profile or login would move every session on
/// that route to an account it did not start on. Both are refused before any
/// path is built from the name.
pub(crate) fn write_route(config_dir: &Path, route: &RouteRecord) -> Result<()> {
    if !valid_route_id(&route.id) {
        bail!(
            "refusing to write bridge route {:?}: alc names a route `codex-` and twelve hex digits",
            route.id
        );
    }
    let own = route_id(&route.profile, &route.auth_file);
    if route.id != own {
        bail!(
            "refusing to write bridge route {}: the profile and Codex login it holds make route {own}",
            route.id
        );
    }
    restricted_dir(&run_dir(config_dir))?;
    let dir = routes_dir(config_dir);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let bytes = serde_json::to_vec_pretty(route).context("failed to encode the bridge route")?;
    crate::config::atomic_write(&dir.join(format!("{}.json", route.id)), &bytes, false)
}

/// The route named `id`, or `None` when there is none - including when `id` is
/// not a route name at all, so a request path can never reach another file.
pub(crate) fn read_route(config_dir: &Path, id: &str) -> Result<Option<RouteRecord>> {
    if !valid_route_id(id) {
        return Ok(None);
    }
    let path = routes_dir(config_dir).join(format!("{id}.json"));
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("{} is not a route alc can read", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub(crate) fn route_count(config_dir: &Path) -> usize {
    fs::read_dir(routes_dir(config_dir)).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .count()
    })
}

/// Points alc's own settings files at a bridge that had to move, answering how
/// many it rewrote. Only `settings-*.json` files, and only the origin.
pub(crate) fn move_settings_origin(config_dir: &Path, from: u16, to: u16) -> Result<usize> {
    let old = format!("http://127.0.0.1:{from}/");
    let new = format!("http://127.0.0.1:{to}/");
    let Ok(entries) = fs::read_dir(crate::agents::claude_settings::settings_dir(config_dir)) else {
        return Ok(0);
    };
    let mut moved = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let ours = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("settings-") && name.ends_with(".json"));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if !ours || !text.contains(&old) {
            continue;
        }
        crate::config::atomic_write(&path, text.replace(&old, &new).as_bytes(), true)?;
        moved += 1;
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers() -> ModelTiers {
        ModelTiers {
            strongest: "gpt-6-astra".to_owned(),
            default: "gpt-5.6-terra".to_owned(),
            cheapest: "gpt-5.6-luna".to_owned(),
        }
    }

    /// Every file under `root`, however deep, so a test can say that nothing
    /// else was written anywhere.
    fn files_under(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(current) = pending.pop() {
            for entry in fs::read_dir(&current).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files
    }

    #[test]
    fn the_token_is_minted_once_and_kept_until_rotated() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(read_token(temp.path()), None);
        let first = load_or_create_token(temp.path()).unwrap();
        assert!(first.len() >= 40, "{first}");
        assert_eq!(load_or_create_token(temp.path()).unwrap(), first);
        let rotated = rotate_token(temp.path()).unwrap();
        assert_ne!(rotated, first);
        assert_eq!(read_token(temp.path()), Some(rotated));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(token_path(temp.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    /// Two starters on a fresh configuration - a launch and a session's
    /// helper, say - must come away holding the same token, and it must be
    /// the one on disk: a bridge started with any other refuses every request
    /// that carries the one on disk.
    #[test]
    fn two_starters_racing_for_the_first_token_agree_on_one() {
        for _ in 0..20 {
            let temp = tempfile::tempdir().unwrap();
            let start = std::sync::Barrier::new(2);
            let starter = || {
                start.wait();
                load_or_create_token(temp.path()).unwrap()
            };
            let (first, second) = std::thread::scope(|scope| {
                let one = scope.spawn(starter);
                let two = scope.spawn(starter);
                (one.join().unwrap(), two.join().unwrap())
            });
            assert_eq!(first, second);
            assert_eq!(read_token(temp.path()), Some(first));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(token_path(temp.path()))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode & 0o077, 0, "{mode:o}");
            }
        }
    }

    /// The loser of that race can find the file before the winner has written
    /// into it. Empty means "not yet": it waits for the token, and gives up -
    /// leaving the file to whoever created it - only when none arrives.
    #[test]
    fn a_token_file_still_being_written_is_waited_for_and_never_replaced() {
        let temp = tempfile::tempdir().unwrap();
        restricted_dir(&run_dir(temp.path())).unwrap();
        let path = token_path(temp.path());

        fs::write(&path, "").unwrap();
        let adopted = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                fs::write(&path, "the-winners-token").unwrap();
            });
            load_or_create_token(temp.path()).unwrap()
        });
        assert_eq!(adopted, "the-winners-token");

        fs::write(&path, "").unwrap();
        let started = Instant::now();
        let error = load_or_create_token(temp.path()).unwrap_err().to_string();
        assert!(started.elapsed() >= TOKEN_WAIT, "{:?}", started.elapsed());
        assert!(error.contains("bridge.token"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn the_port_is_chosen_from_the_range_and_remembered() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(remembered_port(temp.path()), None);
        for _ in 0..200 {
            assert!(PORT_RANGE.contains(&choose_port().unwrap()));
        }
        remember_port(temp.path(), 24_817).unwrap();
        assert_eq!(remembered_port(temp.path()), Some(24_817));
        // A value outside the range is not one alc wrote.
        fs::write(port_path(temp.path()), "8080").unwrap();
        assert_eq!(remembered_port(temp.path()), None);
    }

    #[test]
    fn a_route_is_named_by_its_profile_and_login_and_round_trips() {
        let a = route_id("codex", Path::new("/home/ada/.codex/auth.json"));
        assert!(valid_route_id(&a), "{a}");
        assert_eq!(
            a,
            route_id("codex", Path::new("/home/ada/.codex/auth.json"))
        );
        assert_ne!(
            a,
            route_id("codex", Path::new("/home/ada/.codex-work/auth.json"))
        );
        assert_ne!(a, route_id("work", Path::new("/home/ada/.codex/auth.json")));
        for bad in [
            "",
            "codex-",
            "codex-0123456789AB",
            "codex-../../etc",
            "profile:x",
            "codex-0123456789abc",
        ] {
            assert!(!valid_route_id(bad), "{bad}");
        }

        let temp = tempfile::tempdir().unwrap();
        let route = RouteRecord::new(
            "codex",
            PathBuf::from("/home/ada/.codex/auth.json"),
            tiers(),
        );
        assert_eq!(route_count(temp.path()), 0);
        write_route(temp.path(), &route).unwrap();
        assert_eq!(
            read_route(temp.path(), &route.id).unwrap(),
            Some(route.clone())
        );
        assert_eq!(route_count(temp.path()), 1);
        assert_eq!(read_route(temp.path(), "codex-000000000000").unwrap(), None);
        assert_eq!(
            read_route(temp.path(), "../../secrets").unwrap(),
            None,
            "never a path"
        );
    }

    #[test]
    fn a_route_whose_name_is_a_path_is_refused_and_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("alc");
        let escaping = RouteRecord {
            id: "../../x".to_owned(),
            ..RouteRecord::new(
                "codex",
                PathBuf::from("/home/ada/.codex/auth.json"),
                tiers(),
            )
        };
        let refused = write_route(&config_dir, &escaping).unwrap_err().to_string();
        assert!(refused.contains("../../x"), "{refused}");
        assert_eq!(files_under(temp.path()), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_route_whose_name_is_not_its_own_is_refused_and_writes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("alc");
        let honest = RouteRecord::new(
            "codex",
            PathBuf::from("/home/ada/.codex/auth.json"),
            tiers(),
        );
        // Well-formed names, each over contents it was not made from: one made
        // up, and this route's own name over another account's login.
        let made_up = RouteRecord {
            id: "codex-0123456789ab".to_owned(),
            ..honest.clone()
        };
        let swapped = RouteRecord {
            auth_file: PathBuf::from("/home/eve/.codex/auth.json"),
            ..honest.clone()
        };
        for forged in [&made_up, &swapped] {
            let refused = write_route(&config_dir, forged).unwrap_err().to_string();
            assert!(refused.contains(&forged.id), "{refused}");
        }
        assert_eq!(files_under(temp.path()), Vec::<PathBuf>::new());

        // The record alc made is still written, and only where routes live.
        write_route(&config_dir, &honest).unwrap();
        assert_eq!(
            files_under(temp.path()),
            vec![routes_dir(&config_dir).join(format!("{}.json", honest.id))]
        );
    }

    #[test]
    fn a_move_rewrites_only_alcs_settings_files_that_name_the_old_port() {
        let temp = tempfile::tempdir().unwrap();
        let dir = crate::agents::claude_settings::settings_dir(temp.path());
        fs::create_dir_all(&dir).unwrap();
        let named = dir.join("settings-00000000000000aa.json");
        let other = dir.join("settings-00000000000000bb.json");
        let foreign = dir.join("notes.json");
        fs::write(
            &named,
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:24817/r/codex-0123456789ab"}}"#,
        )
        .unwrap();
        fs::write(
            &other,
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://openrouter.ai/api"}}"#,
        )
        .unwrap();
        fs::write(&foreign, "http://127.0.0.1:24817/").unwrap();

        assert_eq!(
            move_settings_origin(temp.path(), 24_817, 25_001).unwrap(),
            1
        );
        assert!(
            fs::read_to_string(&named)
                .unwrap()
                .contains("http://127.0.0.1:25001/r/codex-0123456789ab")
        );
        assert!(fs::read_to_string(&other).unwrap().contains("openrouter"));
        assert_eq!(
            fs::read_to_string(&foreign).unwrap(),
            "http://127.0.0.1:24817/"
        );
    }
}
