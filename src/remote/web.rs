//! The page's customisation folder: settings, themes and mods.
//!
//! Everything a person (or the agent they ask) can change about the remote
//! page is a plain file under `<config dir>/web/`:
//!
//! ```text
//! web/
//! ├── AGENTS.md        the reference an agent reads before it edits anything
//! ├── settings.json    theme, accent, terminal font, which mods are off
//! ├── themes/<id>.json colour tokens for the page and the terminal
//! ├── mods/<id>/       mod.json, mod.js, mod.css
//! └── examples/        mods and a theme to copy from; never loaded
//! ```
//!
//! The page picks the folder up live: it polls `revision`, swaps settings,
//! themes and stylesheets in place, and offers a reload when a script
//! changed. Nothing here is sandboxed. A mod is JavaScript that runs in the
//! page with that page's token, which is the same trust a person extends to
//! a shell script they put on their own PATH - so the folder belongs to the
//! account running the hub, and AGENTS.md says to read a mod before
//! installing it.
//!
//! What keeps the folder from being a way out of itself:
//!
//! - A mod or theme id is a short slug, checked before any path is built.
//! - Only the three fixed file names inside a mod's folder are ever read,
//!   and a symlink is refused rather than followed, so a mod folder cannot
//!   be made to hand out a file from elsewhere on the machine.
//! - Every read is capped, and a settings write is parsed into a typed
//!   struct and clamped before it is saved.
//! - Scripts run only under a content-security-policy hash the server
//!   computes from the very bytes it serves, so the page cannot be talked
//!   into running anything that is not a mod file on this disk.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::config::atomic_write;

/// The reference written into the folder. An agent asked to "make the page
/// do X" is pointed here first, the way `herdr-app` points its own at one.
pub(crate) const AGENTS_MD: &str = include_str!("../../web/custom/AGENTS.md");

/// Examples copied into `examples/` so there is something to start from.
const EXAMPLES: [(&str, &str); 8] = [
    (
        "examples/mods/quick-prompts/mod.json",
        include_str!("../../web/custom/examples/mods/quick-prompts/mod.json"),
    ),
    (
        "examples/mods/quick-prompts/mod.js",
        include_str!("../../web/custom/examples/mods/quick-prompts/mod.js"),
    ),
    (
        "examples/mods/scratchpad/mod.json",
        include_str!("../../web/custom/examples/mods/scratchpad/mod.json"),
    ),
    (
        "examples/mods/scratchpad/mod.js",
        include_str!("../../web/custom/examples/mods/scratchpad/mod.js"),
    ),
    (
        "examples/mods/scratchpad/mod.css",
        include_str!("../../web/custom/examples/mods/scratchpad/mod.css"),
    ),
    (
        "examples/mods/session-age/mod.json",
        include_str!("../../web/custom/examples/mods/session-age/mod.json"),
    ),
    (
        "examples/mods/session-age/mod.js",
        include_str!("../../web/custom/examples/mods/session-age/mod.js"),
    ),
    (
        "examples/themes/tidepool.json",
        include_str!("../../web/custom/examples/themes/tidepool.json"),
    ),
];

/// The largest mod file served. A mod is a page feature, not a framework.
const MAX_MOD_FILE: u64 = 256 * 1024;
/// The largest settings, manifest or theme file read.
const MAX_JSON: u64 = 64 * 1024;
/// How many mods and themes are listed, so a folder full of junk cannot be
/// turned into a page that takes a minute to load.
const MAX_ENTRIES: usize = 64;
/// A font family the terminal is given; anything longer is not a font list.
const MAX_FONT_FAMILY: usize = 200;

/// The page's settings, as `settings.json` holds them.
///
/// Typed rather than passed through: what the browser sends is parsed into
/// this, clamped, and only then written, so a hand-edit and a page save end
/// up in the same shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct WebSettings {
    /// `system`, `light`, `dark`, a built-in theme id, or a file in
    /// `themes/`.
    pub theme: String,
    /// `#rrggbb`, or none for the theme's own.
    pub accent: Option<String>,
    pub density: Density,
    pub terminal: TerminalSettings,
    /// Mods present on disk but not loaded.
    pub disabled_mods: Vec<String>,
    /// Each mod's own saved state, keyed by mod id. Opaque to alc.
    pub mod_settings: Map<String, Value>,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_owned(),
            accent: None,
            density: Density::Comfortable,
            terminal: TerminalSettings::default(),
            disabled_mods: Vec::new(),
            mod_settings: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Density {
    Comfortable,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct TerminalSettings {
    /// None lets the page pick for the screen it is on.
    pub font_size: Option<u16>,
    pub font_family: Option<String>,
    pub line_height: f32,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub scrollback: u32,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_size: None,
            font_family: None,
            line_height: 1.0,
            cursor_style: CursorStyle::Block,
            cursor_blink: true,
            scrollback: 5_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CursorStyle {
    Block,
    Bar,
    Underline,
}

impl WebSettings {
    /// Brings every field into the range the page can draw.
    fn clamp(mut self) -> Self {
        if !is_theme_name(&self.theme) {
            self.theme = "system".to_owned();
        }
        if let Some(accent) = &self.accent
            && !is_hex_colour(accent)
        {
            self.accent = None;
        }
        let terminal = &mut self.terminal;
        terminal.font_size = terminal.font_size.map(|size| size.clamp(8, 28));
        terminal.font_family = terminal
            .font_family
            .take()
            .map(|family| family.trim().to_owned())
            .filter(|family| !family.is_empty() && is_font_family(family));
        terminal.line_height = if terminal.line_height.is_finite() {
            terminal.line_height.clamp(1.0, 2.0)
        } else {
            1.0
        };
        terminal.scrollback = terminal.scrollback.clamp(500, 100_000);
        self.disabled_mods.retain(|id| is_slug(id));
        self.disabled_mods.sort();
        self.disabled_mods.dedup();
        self.mod_settings.retain(|id, _| is_slug(id));
        self
    }
}

/// One mod as the page lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub enabled: bool,
    pub has_js: bool,
    pub has_css: bool,
    /// The CSP hash of `mod.js`, so the page can tell which scripts changed
    /// on disk since it loaded them.
    pub script_hash: Option<String>,
    /// What is wrong with it, in a sentence; a broken mod is listed, not
    /// hidden, so the settings page can say why it did not load.
    pub error: Option<String>,
}

/// One theme from `themes/`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThemeInfo {
    pub id: String,
    pub name: String,
    /// `light` or `dark`: which built-in palette the tokens are laid over.
    pub mode: String,
    pub tokens: BTreeMap<String, String>,
    pub terminal: BTreeMap<String, String>,
    pub error: Option<String>,
}

/// What `GET /api/web` answers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub settings: WebSettings,
    pub mods: Vec<ModInfo>,
    pub themes: Vec<ThemeInfo>,
    /// Changes whenever any file in the folder does; the page polls it.
    pub revision: String,
    /// Where the folder is, for an operator. Empty for a viewer.
    pub dir: String,
    /// The settings file did not parse; the defaults are in effect.
    pub settings_error: Option<String>,
}

/// Which of a mod's two loadable files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModFile {
    Script,
    Style,
}

impl ModFile {
    fn name(self) -> &'static str {
        match self {
            Self::Script => "mod.js",
            Self::Style => "mod.css",
        }
    }
}

/// The customisation folder.
#[derive(Debug, Clone)]
pub(crate) struct WebDir {
    root: PathBuf,
}

impl WebDir {
    pub(crate) fn new(config_dir: &Path) -> Self {
        Self {
            root: config_dir.join("web"),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Creates the folder and refreshes the files alc owns in it.
    ///
    /// `AGENTS.md` and `examples/` are alc's, rewritten when they differ, so
    /// an agent always reads the reference for the alc that is running.
    /// Nothing a person wrote - settings, themes, mods - is touched.
    pub(crate) fn scaffold(&self) -> Result<()> {
        for dir in ["mods", "themes"] {
            fs::create_dir_all(self.root.join(dir))
                .with_context(|| format!("failed to create {}", self.root.join(dir).display()))?;
        }
        write_if_changed(&self.root.join("AGENTS.md"), AGENTS_MD)?;
        for (relative, contents) in EXAMPLES {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            write_if_changed(&path, contents)?;
        }
        Ok(())
    }

    pub(crate) fn load_settings(&self) -> (WebSettings, Option<String>) {
        let path = self.root.join("settings.json");
        match read_capped(&path, MAX_JSON) {
            Ok(None) => (WebSettings::default(), None),
            Ok(Some(text)) => match serde_json::from_str::<WebSettings>(&text) {
                Ok(settings) => (settings.clamp(), None),
                Err(error) => (
                    WebSettings::default(),
                    Some(format!("settings.json: {error}")),
                ),
            },
            Err(error) => (WebSettings::default(), Some(error.to_string())),
        }
    }

    /// Parses, clamps and writes what the page sent; answers with what was
    /// actually saved.
    pub(crate) fn save_settings(&self, body: &[u8]) -> Result<WebSettings> {
        if body.len() as u64 > MAX_JSON {
            bail!("settings are larger than 64 KiB");
        }
        // A file that does not parse is somebody's hand edit, and the page
        // is holding defaults in its place: saving them would erase it.
        if let (_, Some(error)) = self.load_settings() {
            bail!("{error}; fix it (alc remote web check) before saving from the page");
        }
        let settings: WebSettings =
            serde_json::from_slice(body).context("settings are not valid JSON")?;
        let settings = settings.clamp();
        // Measured after indentation (see write_settings): a body under the
        // cap can pretty-print past it.
        self.write_settings(&settings)?;
        Ok(settings)
    }

    /// Turns a mod on or off in settings.json.
    pub(crate) fn set_enabled(&self, id: &str, on: bool) -> Result<()> {
        if !is_slug(id) {
            bail!("`{id}` is not a mod id");
        }
        let (mut settings, error) = self.load_settings();
        if let Some(error) = error {
            bail!("{error}; fix settings.json first (alc remote web check)");
        }
        settings.disabled_mods.retain(|off| off != id);
        if !on {
            settings.disabled_mods.push(id.to_owned());
        }
        self.write_settings(&settings.clamp())
    }

    /// Deletes a mod's folder and forgets its switch and its saved state.
    pub(crate) fn remove_mod(&self, id: &str) -> Result<()> {
        if !is_slug(id) {
            bail!("`{id}` is not a mod id");
        }
        let dir = self.root.join("mods").join(id);
        let is_real_dir = fs::symlink_metadata(&dir)
            .map(|meta| meta.file_type().is_dir())
            .unwrap_or(false);
        if !is_real_dir {
            bail!("there is no mods/{id}");
        }
        fs::remove_dir_all(&dir).with_context(|| format!("could not remove mods/{id}"))?;
        let (mut settings, error) = self.load_settings();
        if error.is_none() {
            settings.disabled_mods.retain(|off| off != id);
            settings.mod_settings.remove(id);
            self.write_settings(&settings)?;
        }
        Ok(())
    }

    fn write_settings(&self, settings: &WebSettings) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let mut text = serde_json::to_string_pretty(settings)?;
        text.push('\n');
        if text.len() as u64 > MAX_JSON {
            bail!("settings are larger than 64 KiB");
        }
        atomic_write(&self.root.join("settings.json"), text.as_bytes(), false)
    }

    pub(crate) fn snapshot(&self, operator: bool) -> Snapshot {
        // The revision first: a write landing mid-snapshot then yields an
        // old revision with new contents, which the next poll corrects,
        // rather than new revision with old contents, which it never would.
        let revision = self.revision();
        let (settings, settings_error) = self.load_settings();
        let mut mods = self.mods(&settings);
        if settings_error.is_some() {
            for info in &mut mods {
                info.enabled = false;
            }
        }
        Snapshot {
            mods,
            themes: self.themes(),
            revision,
            dir: if operator {
                self.root.display().to_string()
            } else {
                String::new()
            },
            settings,
            settings_error,
        }
    }

    /// Every mod folder, enabled or not, sorted by id.
    pub(crate) fn mods(&self, settings: &WebSettings) -> Vec<ModInfo> {
        let mut mods = Vec::new();
        for id in slugs(&self.root.join("mods"), true) {
            let dir = self.root.join("mods").join(&id);
            let mut info = ModInfo {
                name: id.clone(),
                enabled: !settings.disabled_mods.contains(&id),
                id,
                description: String::new(),
                version: None,
                author: None,
                has_js: is_plain_file(&dir.join("mod.js")),
                has_css: is_plain_file(&dir.join("mod.css")),
                script_hash: None,
                error: None,
            };
            match read_capped(&dir.join("mod.json"), MAX_JSON) {
                Ok(Some(text)) => match serde_json::from_str::<Manifest>(&text) {
                    Ok(manifest) => {
                        if let Some(name) = manifest.name.filter(|name| !name.trim().is_empty()) {
                            info.name = truncate(name.trim(), 60);
                        }
                        info.description =
                            truncate(manifest.description.unwrap_or_default().trim(), 280);
                        info.version = manifest.version.map(|value| truncate(&value, 24));
                        info.author = manifest.author.map(|value| truncate(&value, 60));
                    }
                    Err(error) => info.error = Some(format!("mod.json: {error}")),
                },
                Ok(None) => {}
                Err(error) => info.error = Some(error.to_string()),
            }
            if !info.has_js && !info.has_css && info.error.is_none() {
                info.error = Some("has neither mod.js nor mod.css".to_owned());
            }
            for (present, file) in [
                (info.has_js, ModFile::Script),
                (info.has_css, ModFile::Style),
            ] {
                if !present || info.error.is_some() {
                    continue;
                }
                match self.mod_file(&info.id, file) {
                    Ok(text) if file == ModFile::Script => info.script_hash = Some(csp_hash(&text)),
                    Ok(_) => {}
                    Err(error) => info.error = Some(error.to_string()),
                }
            }
            mods.push(info);
        }
        mods
    }

    /// One mod file's text. Refuses anything but the fixed names, a symlink,
    /// an oversized file, or bytes that are not UTF-8 - the page hashes the
    /// text it is given, so it has to be exactly the text on disk.
    pub(crate) fn mod_file(&self, id: &str, file: ModFile) -> Result<String> {
        if !is_slug(id) {
            bail!("not a mod id");
        }
        let dir = self.root.join("mods").join(id);
        let is_real_dir = fs::symlink_metadata(&dir)
            .map(|meta| meta.file_type().is_dir())
            .unwrap_or(false);
        if !is_real_dir {
            bail!("no such mod");
        }
        let path = dir.join(file.name());
        match read_capped(&path, MAX_MOD_FILE)? {
            // A browser's UTF-8 decoder drops a leading byte-order mark, so
            // the page would hash different text than this one did.
            Some(text) => Ok(text
                .strip_prefix('\u{feff}')
                .map(str::to_owned)
                .unwrap_or(text)),
            None => bail!("{} is missing", file.name()),
        }
    }

    /// The CSP source for every enabled mod's script, so the page may run
    /// exactly those bytes and nothing else inline.
    pub(crate) fn script_hashes(&self) -> Vec<String> {
        // Fail closed: with settings.json unreadable, which mods are off is
        // unknown, and running one the operator turned off is the worse
        // mistake.
        let (settings, error) = self.load_settings();
        if error.is_some() {
            return Vec::new();
        }
        self.mods(&settings)
            .into_iter()
            .filter(|info| info.enabled && info.error.is_none())
            .filter_map(|info| info.script_hash)
            .collect()
    }

    pub(crate) fn themes(&self) -> Vec<ThemeInfo> {
        let dir = self.root.join("themes");
        let mut themes = Vec::new();
        let Ok(entries) = fs::read_dir(&dir) else {
            return themes;
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| name.strip_suffix(".json").map(str::to_owned))
            .filter(|id| is_slug(id))
            .collect();
        names.sort();
        names.truncate(MAX_ENTRIES);
        for id in names {
            let mut theme = ThemeInfo {
                name: id.clone(),
                id: id.clone(),
                mode: "dark".to_owned(),
                tokens: BTreeMap::new(),
                terminal: BTreeMap::new(),
                error: None,
            };
            match read_capped(&dir.join(format!("{id}.json")), MAX_JSON) {
                Ok(Some(text)) => match serde_json::from_str::<ThemeFile>(&text) {
                    Ok(file) => {
                        if let Some(name) = file.name.filter(|name| !name.trim().is_empty()) {
                            theme.name = truncate(name.trim(), 60);
                        }
                        if file.mode.as_deref() == Some("light") {
                            theme.mode = "light".to_owned();
                        }
                        theme.tokens = file
                            .tokens
                            .into_iter()
                            .filter(|(key, value)| is_token_name(key) && is_css_value(value))
                            .collect();
                        theme.terminal = file
                            .terminal
                            .into_iter()
                            .filter(|(key, value)| is_slug(key) && is_css_value(value))
                            .collect();
                    }
                    Err(error) => theme.error = Some(format!("{id}.json: {error}")),
                },
                Ok(None) => continue,
                Err(error) => theme.error = Some(error.to_string()),
            }
            themes.push(theme);
        }
        themes
    }

    /// A short fingerprint of every file the page reads from the folder.
    pub(crate) fn revision(&self) -> String {
        let mut hasher = Sha256::new();
        let mut visit = |path: &Path| {
            if let Ok(meta) = fs::symlink_metadata(path) {
                hasher.update(path.to_string_lossy().as_bytes());
                hasher.update(meta.len().to_le_bytes());
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|time| time.as_nanos())
                    .unwrap_or_default();
                hasher.update(modified.to_le_bytes());
            }
        };
        visit(&self.root.join("settings.json"));
        for id in slugs(&self.root.join("mods"), true) {
            for file in ["mod.json", "mod.js", "mod.css"] {
                visit(&self.root.join("mods").join(&id).join(file));
            }
        }
        if let Ok(entries) = fs::read_dir(self.root.join("themes")) {
            let mut paths: Vec<PathBuf> =
                entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            paths.sort();
            for path in paths.iter().take(MAX_ENTRIES) {
                visit(path);
            }
        }
        let digest = hasher.finalize();
        digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Everything `alc remote web check` reports: one line per problem.
    pub(crate) fn check(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let (settings, settings_error) = self.load_settings();
        problems.extend(settings_error);
        let mods = self.mods(&settings);
        for info in &mods {
            if let Some(error) = &info.error {
                problems.push(format!("mods/{}: {error}", info.id));
            }
        }
        for id in &settings.disabled_mods {
            if !mods.iter().any(|info| &info.id == id) {
                problems.push(format!(
                    "settings.json: disabledMods names `{id}`, which is not in mods/"
                ));
            }
        }
        for theme in self.themes() {
            if let Some(error) = theme.error {
                problems.push(format!("themes/{error}"));
            }
        }
        let theme = settings.theme.as_str();
        let built_in = [
            "system", "light", "dark", "midnight", "daylight", "fjord", "ember", "paper",
        ];
        if !built_in.contains(&theme)
            && !self
                .root
                .join("themes")
                .join(format!("{theme}.json"))
                .is_file()
        {
            problems.push(format!(
                "settings.json: theme `{theme}` is neither built in nor in themes/"
            ));
        }
        problems
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Manifest {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    author: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ThemeFile {
    name: Option<String>,
    mode: Option<String>,
    tokens: BTreeMap<String, String>,
    terminal: BTreeMap<String, String>,
}

/// `'sha256-…'`, the CSP source expression for an inline script.
pub(crate) fn csp_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    format!(
        "'sha256-{}'",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

/// The page's content-security policy, with the given extra script sources.
pub(crate) fn content_security_policy(script_hashes: &[String]) -> String {
    let mut script = String::from("'self'");
    if !script_hashes.is_empty() {
        // The first characters of a refused inline script reach the page's
        // violation event, which is how it says which mod was blocked.
        script.push_str(" 'report-sample'");
    }
    for hash in script_hashes {
        script.push(' ');
        script.push_str(hash);
    }
    format!(
        "default-src 'self'; script-src {script}; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; \
         base-uri 'none'; form-action 'none'"
    )
}

/// The child names of `dir` that are slugs, sorted and capped. With
/// `dirs_only`, plain directories only - never a symlink to one.
fn slugs(dir: &Path, dirs_only: bool) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            !dirs_only
                || entry
                    .file_type()
                    .map(|kind| kind.is_dir() && !kind.is_symlink())
                    .unwrap_or(false)
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| is_slug(name))
        .collect();
    names.sort();
    names.truncate(MAX_ENTRIES);
    names
}

fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_file())
        .unwrap_or(false)
}

/// Reads a file of at most `cap` bytes as UTF-8. `Ok(None)` when it does not
/// exist; an error for a symlink, a directory, an oversized or non-UTF-8 file.
fn read_capped(path: &Path, cap: u64) -> Result<Option<String>> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("{name} could not be read"),
    };
    if meta.file_type().is_symlink() {
        bail!("{name} is a symlink; mods and themes must be plain files");
    }
    if !meta.is_file() {
        bail!("{name} is not a file");
    }
    if meta.len() > cap {
        bail!("{name} is larger than {} KiB", cap / 1024);
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    fs::File::open(path)
        .and_then(|file| file.take(cap + 1).read_to_end(&mut bytes))
        .map_err(|_| anyhow::anyhow!("{name} could not be read"))?;
    let text = String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{name} is not UTF-8"))?;
    Ok(Some(text))
}

fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if fs::read(path).ok().as_deref() == Some(contents.as_bytes()) {
        return Ok(());
    }
    atomic_write(path, contents.as_bytes(), false)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// A mod, theme or terminal-colour id: lowercase-led, `[a-z0-9_-]`, short.
pub(crate) fn is_slug(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= 48
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_theme_name(value: &str) -> bool {
    is_slug(value)
}

fn is_hex_colour(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// A CSS custom property name: `--` and a slug.
fn is_token_name(value: &str) -> bool {
    value.strip_prefix("--").is_some_and(is_slug)
}

/// A colour or length a token may hold. No `;`, braces, quotes, backslashes
/// or `url(`, so a value cannot close the declaration it is set in or fetch
/// anything.
fn is_css_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && !value.to_ascii_lowercase().contains("url(")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " #%.,()-/+".contains(c))
}

fn is_font_family(value: &str) -> bool {
    value.len() <= MAX_FONT_FAMILY
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || " ,-_\"'".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> (tempfile::TempDir, WebDir) {
        let temp = tempfile::tempdir().unwrap();
        let web = WebDir::new(temp.path());
        web.scaffold().unwrap();
        (temp, web)
    }

    fn write(web: &WebDir, relative: &str, text: &str) {
        let path = web.root().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn scaffold_writes_the_reference_and_examples_but_no_mods() {
        let (_temp, web) = dir();
        assert!(web.root().join("AGENTS.md").is_file());
        assert!(web.root().join("examples/mods/scratchpad/mod.js").is_file());
        let (settings, error) = web.load_settings();
        assert!(error.is_none());
        assert!(web.mods(&settings).is_empty(), "examples must not load");
    }

    #[test]
    fn settings_default_when_absent_and_clamp_on_save() {
        let (_temp, web) = dir();
        assert_eq!(web.load_settings().0, WebSettings::default());
        let saved = web
            .save_settings(
                br#"{"theme":"../etc","accent":"red","terminal":{"fontSize":99,"scrollback":1,
                    "fontFamily":"x;}body{","lineHeight":9},"disabledMods":["ok","../no","ok"],
                    "modSettings":{"good":{"a":1},"../bad":1}}"#,
            )
            .unwrap();
        assert_eq!(saved.theme, "system");
        assert_eq!(saved.accent, None);
        assert_eq!(saved.terminal.font_size, Some(28));
        assert_eq!(saved.terminal.scrollback, 500);
        assert_eq!(saved.terminal.font_family, None);
        assert_eq!(saved.terminal.line_height, 2.0);
        assert_eq!(saved.disabled_mods, vec!["ok".to_owned()]);
        assert_eq!(saved.mod_settings.len(), 1);
        assert_eq!(web.load_settings().0, saved);
        // Past u8, which a hand edit could write, still parses and clamps.
        let big = web
            .save_settings(br#"{"terminal":{"fontSize":300}}"#)
            .unwrap();
        assert_eq!(big.terminal.font_size, Some(28));
    }

    #[test]
    fn a_broken_settings_file_is_reported_and_defaulted() {
        let (_temp, web) = dir();
        write(&web, "settings.json", "{ nope");
        let (settings, error) = web.load_settings();
        assert_eq!(settings, WebSettings::default());
        assert!(error.unwrap().contains("settings.json"));
        // Mods fail closed, and the page cannot overwrite the hand edit.
        write(&web, "mods/alpha/mod.js", "x();");
        assert!(web.script_hashes().is_empty());
        assert!(web.snapshot(true).mods.iter().all(|info| !info.enabled));
        assert!(web.save_settings(b"{}").is_err());
        assert_eq!(
            fs::read_to_string(web.root().join("settings.json")).unwrap(),
            "{ nope"
        );
    }

    #[test]
    fn mods_are_listed_with_manifest_and_state() {
        let (_temp, web) = dir();
        write(
            &web,
            "mods/alpha/mod.json",
            r#"{"name":"Alpha","description":"first"}"#,
        );
        write(&web, "mods/alpha/mod.js", "alc.toast('hi');");
        write(&web, "mods/beta/mod.css", "body{}");
        write(&web, "mods/empty/README", "x");
        write(&web, "settings.json", r#"{"disabledMods":["beta"]}"#);
        let (settings, _) = web.load_settings();
        let mods = web.mods(&settings);
        assert_eq!(mods.len(), 3);
        assert_eq!(mods[0].name, "Alpha");
        assert!(mods[0].enabled && mods[0].has_js && !mods[0].has_css);
        assert!(!mods[1].enabled && mods[1].has_css);
        assert!(mods[2].error.as_deref().unwrap().contains("neither"));
        assert_eq!(web.script_hashes(), vec![csp_hash("alc.toast('hi');")]);
    }

    #[test]
    fn mod_files_refuse_bad_ids_and_symlinks() {
        let (temp, web) = dir();
        assert!(web.mod_file("../x", ModFile::Script).is_err());
        assert!(web.mod_file("nope", ModFile::Script).is_err());
        #[cfg(unix)]
        {
            let secret = temp.path().join("secret.txt");
            fs::write(&secret, "token").unwrap();
            fs::create_dir_all(web.root().join("mods/sneaky")).unwrap();
            std::os::unix::fs::symlink(&secret, web.root().join("mods/sneaky/mod.js")).unwrap();
            assert!(web.mod_file("sneaky", ModFile::Script).is_err());
            assert!(web.script_hashes().is_empty());
        }
        let _ = temp;
    }

    #[test]
    fn themes_keep_only_safe_tokens() {
        let (_temp, web) = dir();
        write(
            &web,
            "themes/sea.json",
            r##"{"name":"Sea","mode":"light","tokens":{"--bg":"#fff","--x":"red;}*{","bg":"#000",
                "--img":"url(http://x)"},"terminal":{"background":"#001122","bad key":"#fff"}}"##,
        );
        let themes = web.themes();
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].mode, "light");
        assert_eq!(themes[0].tokens.len(), 1);
        assert_eq!(themes[0].terminal.len(), 1);
    }

    #[test]
    fn revision_moves_when_a_file_changes() {
        let (_temp, web) = dir();
        let before = web.revision();
        write(&web, "mods/alpha/mod.css", "body{}");
        assert_ne!(before, web.revision());
    }

    #[test]
    fn the_policy_lists_each_hash() {
        let policy = content_security_policy(&[csp_hash("a")]);
        assert!(policy.contains("script-src 'self' 'report-sample' 'sha256-"));
        assert!(!content_security_policy(&[]).contains("report-sample"));
        assert!(policy.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn check_reports_problems() {
        let (_temp, web) = dir();
        assert!(web.check().is_empty());
        write(
            &web,
            "settings.json",
            r#"{"theme":"nowhere","disabledMods":["ghost"]}"#,
        );
        let problems = web.check();
        assert_eq!(problems.len(), 2, "{problems:?}");
    }
}
