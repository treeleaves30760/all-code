//! Installing page mods and themes from a GitHub repository or a folder.
//!
//! A source is laid out one of two ways, and both can be mixed in one repo:
//!
//! ```text
//! single mod                 collection
//! ├── mod.json               ├── mods/<id>/mod.json, mod.js, mod.css
//! ├── mod.js                 └── themes/<id>.json
//! └── mod.css
//! ```
//!
//! Only those names are taken (plus each mod's README.md), every id must be a
//! slug, nothing but regular files is read from an archive, and every file is
//! capped - so a repository cannot write anywhere but `web/mods/<id>/` and
//! `web/themes/`. What arrives is installed **off**: a mod is code that runs
//! with the page's link, and turning it on is a decision made after reading
//! it, by `alc remote web enable <id>` or the switch in Settings.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};

use crate::remote::web::{WebDir, is_slug};

/// The largest archive downloaded.
const MAX_ARCHIVE: u64 = 8 * 1024 * 1024;
/// The largest single file taken from a source; the page's own caps are
/// enforced again when it reads them.
const MAX_FILE: u64 = 256 * 1024;
/// Entries looked at in an archive before giving up on it.
const MAX_ENTRIES: usize = 5_000;

/// The files a mod folder may hold.
const MOD_FILES: [&str; 4] = ["mod.json", "mod.js", "mod.css", "README.md"];

/// Where to get mods from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    GitHub {
        owner: String,
        repo: String,
        reference: Option<String>,
        /// A folder inside the repository to install from.
        subdir: Option<String>,
    },
    Folder(PathBuf),
}

impl Source {
    /// `owner/repo`, `owner/repo@ref`, `owner/repo/sub/dir@ref`,
    /// `https://github.com/owner/repo[/tree/<ref>[/sub/dir]]`, or a folder
    /// that exists.
    pub(crate) fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        let path = Path::new(input);
        if path.is_dir() {
            return Ok(Self::Folder(path.to_path_buf()));
        }
        if let Some(rest) = input
            .strip_prefix("https://github.com/")
            .or_else(|| input.strip_prefix("github.com/"))
        {
            let rest = rest.trim_end_matches('/').trim_end_matches(".git");
            let parts: Vec<&str> = rest.split('/').collect();
            ensure!(
                parts.len() >= 2,
                "expected https://github.com/<owner>/<repo>"
            );
            let (reference, subdir) = match parts.get(2) {
                Some(&"tree") => {
                    let reference = parts.get(3).context("a /tree/ URL needs a branch or tag")?;
                    let subdir = (parts.len() > 4).then(|| parts[4..].join("/"));
                    (Some((*reference).to_owned()), subdir)
                }
                Some(_) => {
                    bail!("expected https://github.com/<owner>/<repo>[/tree/<ref>/<folder>]")
                }
                None => (None, None),
            };
            return Self::github(parts[0], parts[1], reference, subdir);
        }
        let (spec, reference) = match input.split_once('@') {
            Some((spec, reference)) => (spec, Some(reference.to_owned())),
            None => (input, None),
        };
        let parts: Vec<&str> = spec.split('/').filter(|part| !part.is_empty()).collect();
        ensure!(
            parts.len() >= 2,
            "`{input}` is neither a folder nor <owner>/<repo>[@ref] on GitHub"
        );
        let subdir = (parts.len() > 2).then(|| parts[2..].join("/"));
        Self::github(parts[0], parts[1], reference, subdir)
    }

    fn github(
        owner: &str,
        repo: &str,
        reference: Option<String>,
        subdir: Option<String>,
    ) -> Result<Self> {
        let name_ok = |value: &str| {
            !value.is_empty()
                && value.len() <= 100
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                && !value.starts_with('.')
        };
        ensure!(
            name_ok(owner) && name_ok(repo),
            "not a GitHub owner/repo: {owner}/{repo}"
        );
        if let Some(reference) = &reference {
            ensure!(
                !reference.is_empty()
                    && reference.len() <= 200
                    && reference
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
                    && !reference.contains(".."),
                "not a branch, tag or commit: {reference}"
            );
        }
        let subdir = subdir.map(|dir| dir.trim_matches('/').to_owned());
        if let Some(dir) = &subdir {
            ensure!(
                Path::new(dir)
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
                "not a folder inside the repository: {dir}"
            );
        }
        Ok(Self::GitHub {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            reference,
            subdir,
        })
    }

    /// How the source is shown and recorded.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::GitHub {
                owner,
                repo,
                reference,
                subdir,
            } => {
                let mut label = format!("{owner}/{repo}");
                if let Some(dir) = subdir {
                    label.push('/');
                    label.push_str(dir);
                }
                if let Some(reference) = reference {
                    label.push('@');
                    label.push_str(reference);
                }
                label
            }
            Self::Folder(path) => path.display().to_string(),
        }
    }

    /// The name a single mod at the source's root is installed under.
    fn default_id(&self) -> String {
        let raw = match self {
            Self::GitHub { repo, subdir, .. } => subdir
                .as_deref()
                .and_then(|dir| dir.rsplit('/').next())
                .unwrap_or(repo)
                .to_owned(),
            Self::Folder(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "mod".to_owned()),
        };
        let lowered = raw.to_ascii_lowercase();
        let trimmed = ["alc-mod-", "alc-page-mod-", "alc-"]
            .iter()
            .find_map(|prefix| lowered.strip_prefix(prefix))
            .unwrap_or(&lowered);
        let slug: String = trimmed
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let slug = slug.trim_matches('-').chars().take(48).collect::<String>();
        if is_slug(&slug) {
            slug
        } else {
            "mod".to_owned()
        }
    }
}

/// What one source holds, by the path it will have under `web/`.
#[derive(Debug, Default)]
pub(crate) struct Bundle {
    /// `mods/<id>/<file>` and `themes/<id>.json`.
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Bundle {
    pub(crate) fn mods(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .files
            .keys()
            .filter_map(|path| path.strip_prefix("mods/"))
            .filter_map(|rest| rest.split_once('/').map(|(id, _)| id.to_owned()))
            .collect();
        ids.dedup();
        ids
    }

    pub(crate) fn packages(&self) -> Vec<String> {
        self.files
            .keys()
            .filter_map(|path| path.strip_prefix("packages/"))
            .filter_map(|name| name.strip_suffix(".json"))
            .map(str::to_owned)
            .collect()
    }

    pub(crate) fn themes(&self) -> Vec<String> {
        self.files
            .keys()
            .filter_map(|path| path.strip_prefix("themes/"))
            .filter_map(|name| name.strip_suffix(".json"))
            .map(str::to_owned)
            .collect()
    }
}

/// Sorts the files of a source into a bundle. `files` are paths relative to
/// the source's root (or its chosen folder), with `/` separators.
pub(crate) fn bundle(files: BTreeMap<String, Vec<u8>>, single_id: &str) -> Result<Bundle> {
    let mut bundle = Bundle::default();
    let root_mod = files.contains_key("mod.json") || files.contains_key("mod.js");
    for (path, bytes) in files {
        let parts: Vec<&str> = path.split('/').collect();
        let target = match parts.as_slice() {
            [file] if root_mod && MOD_FILES.contains(file) => {
                Some(format!("mods/{single_id}/{file}"))
            }
            ["mods", id, file] if is_slug(id) && MOD_FILES.contains(file) => {
                Some(format!("mods/{id}/{file}"))
            }
            ["themes", name] => name
                .strip_suffix(".json")
                .filter(|id| is_slug(id))
                .map(|id| format!("themes/{id}.json")),
            ["packages", name] => name
                .strip_suffix(".json")
                .filter(|id| is_slug(id))
                .map(|id| format!("packages/{id}.json")),
            _ => None,
        };
        if let Some(target) = target {
            ensure!(
                bytes.len() as u64 <= MAX_FILE,
                "{path} is larger than {} KiB",
                MAX_FILE / 1024
            );
            ensure!(std::str::from_utf8(&bytes).is_ok(), "{path} is not UTF-8");
            bundle.files.insert(target, bytes);
        }
    }
    for id in bundle.mods() {
        let has_code = ["mod.js", "mod.css"]
            .iter()
            .any(|file| bundle.files.contains_key(&format!("mods/{id}/{file}")));
        ensure!(has_code, "mods/{id} has neither mod.js nor mod.css");
    }
    ensure!(
        !bundle.files.is_empty(),
        "found no mod (mod.json + mod.js/mod.css at the root, or mods/<id>/), no themes/<id>.json and no packages/<id>.json"
    );
    Ok(bundle)
}

/// Reads the files of a GitHub tarball under `subdir`, dropping the
/// `<repo>-<sha>/` folder GitHub wraps everything in.
pub(crate) fn files_from_tarball(
    gzipped: &[u8],
    subdir: Option<&str>,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(gzipped));
    let prefix = subdir.map(|dir| format!("{}/", dir.trim_matches('/')));
    let mut files = BTreeMap::new();
    for (count, entry) in archive
        .entries()
        .context("not a tar.gz archive")?
        .enumerate()
    {
        ensure!(count < MAX_ENTRIES, "the archive has too many entries");
        let mut entry = entry.context("the archive is damaged")?;
        // Regular files only: a link in an archive is how one writes outside
        // where it is unpacked, and nothing here needs one.
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry
            .path()
            .context("an archive path is not valid")?
            .into_owned();
        let mut parts = path.components();
        if !matches!(parts.next(), Some(Component::Normal(_))) {
            continue;
        }
        if !parts
            .clone()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            continue;
        }
        let relative = parts
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let relative = match &prefix {
            Some(prefix) => match relative.strip_prefix(prefix.as_str()) {
                Some(rest) => rest.to_owned(),
                None => continue,
            },
            None => relative,
        };
        if !wanted(&relative) {
            continue;
        }
        ensure!(
            entry.size() <= MAX_FILE,
            "{relative} is larger than {} KiB",
            MAX_FILE / 1024
        );
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("could not read {relative}"))?;
        files.insert(relative, bytes);
    }
    Ok(files)
}

/// Reads the same names out of a folder on disk. Symlinks are skipped.
pub(crate) fn files_from_folder(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut take = |relative: String, path: &Path| -> Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_file() && wanted(&relative) {
            ensure!(
                meta.len() <= MAX_FILE,
                "{relative} is larger than {} KiB",
                MAX_FILE / 1024
            );
            files.insert(relative, fs::read(path)?);
        }
        Ok(())
    };
    for file in MOD_FILES {
        take(file.to_owned(), &root.join(file)).ok();
    }
    if let Ok(entries) = fs::read_dir(root.join("mods")) {
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            for file in MOD_FILES {
                take(format!("mods/{id}/{file}"), &entry.path().join(file)).ok();
            }
        }
    }
    if let Ok(entries) = fs::read_dir(root.join("themes")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            take(format!("themes/{name}"), &entry.path()).ok();
        }
    }
    Ok(files)
}

fn wanted(relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').collect();
    match parts.as_slice() {
        [file] => MOD_FILES.contains(file),
        ["mods", _, file] => MOD_FILES.contains(file),
        ["themes", name] | ["packages", name] => name.ends_with(".json"),
        _ => false,
    }
}

fn download(url: &str) -> Result<Vec<u8>> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent
        .get(url)
        .header("User-Agent", concat!("alc/", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("could not download {url}"))?;
    response
        .body_mut()
        .with_config()
        .limit(MAX_ARCHIVE)
        .read_to_vec()
        .with_context(|| format!("could not read {url}"))
}

/// The tarball URL for a GitHub source. `HEAD` is the default branch.
fn tarball_url(owner: &str, repo: &str, reference: Option<&str>) -> String {
    #[cfg(debug_assertions)]
    if let Ok(base) = std::env::var("ALC_WEB_ARCHIVE_BASE") {
        return format!("{base}/{owner}/{repo}/{}", reference.unwrap_or("HEAD"));
    }
    format!(
        "https://codeload.github.com/{owner}/{repo}/tar.gz/{}",
        reference.unwrap_or("HEAD")
    )
}

/// Fetches a source into a bundle.
pub(crate) fn fetch(source: &Source, name: Option<&str>) -> Result<Bundle> {
    let single = match name {
        Some(name) => {
            ensure!(
                is_slug(name),
                "`{name}` is not a mod id ([a-z0-9_-], up to 48)"
            );
            name.to_owned()
        }
        None => source.default_id(),
    };
    let files = match source {
        Source::GitHub {
            owner,
            repo,
            reference,
            subdir,
        } => {
            let bytes = download(&tarball_url(owner, repo, reference.as_deref()))?;
            files_from_tarball(&bytes, subdir.as_deref())?
        }
        Source::Folder(path) => files_from_folder(path)?,
    };
    bundle(files, &single)
}

/// What an install did.
#[derive(Debug, Default)]
pub(crate) struct Installed {
    pub mods: Vec<String>,
    pub themes: Vec<String>,
    pub packages: Vec<String>,
}

/// Writes a bundle into the folder. Mods arrive off unless `enable`; an
/// existing mod or theme is replaced only with `force`.
pub(crate) fn install(
    web: &WebDir,
    bundle: &Bundle,
    source_label: &str,
    enable: bool,
    force: bool,
) -> Result<Installed> {
    web.scaffold()?;
    let (_, error) = web.load_settings();
    if let Some(error) = error {
        bail!("{error}; fix settings.json first (alc remote web check)");
    }
    let mods = bundle.mods();
    let themes = bundle.themes();
    let packages = bundle.packages();
    if !force {
        for id in &packages {
            ensure!(
                !web.root()
                    .join("packages")
                    .join(format!("{id}.json"))
                    .exists(),
                "packages/{id}.json already exists; pass --force to replace it"
            );
        }
        for id in &mods {
            ensure!(
                !web.root().join("mods").join(id).exists(),
                "mods/{id} already exists; pass --force to replace it"
            );
        }
        for id in &themes {
            ensure!(
                !web.root()
                    .join("themes")
                    .join(format!("{id}.json"))
                    .exists(),
                "themes/{id}.json already exists; pass --force to replace it"
            );
        }
    }
    for id in &mods {
        let dir = web.root().join("mods").join(id);
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("could not replace mods/{id}"))?;
        }
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("SOURCE"), format!("{source_label}\n"))?;
    }
    for (path, bytes) in &bundle.files {
        let target = web.root().join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, bytes).with_context(|| format!("could not write {path}"))?;
    }
    for id in &mods {
        web.set_enabled(id, enable)?;
    }
    Ok(Installed {
        mods,
        themes,
        packages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tarball(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, text) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, text.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn sources_parse_from_short_forms_and_urls() {
        let github = |owner: &str, repo: &str, reference: Option<&str>, subdir: Option<&str>| {
            Source::GitHub {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
                reference: reference.map(str::to_owned),
                subdir: subdir.map(str::to_owned),
            }
        };
        assert_eq!(
            Source::parse("me/alc-mod-timer").unwrap(),
            github("me", "alc-mod-timer", None, None)
        );
        assert_eq!(
            Source::parse("me/mods@v1.2").unwrap(),
            github("me", "mods", Some("v1.2"), None)
        );
        assert_eq!(
            Source::parse("me/mods/packs/focus@main").unwrap(),
            github("me", "mods", Some("main"), Some("packs/focus"))
        );
        assert_eq!(
            Source::parse("https://github.com/me/mods.git").unwrap(),
            github("me", "mods", None, None)
        );
        assert_eq!(
            Source::parse("https://github.com/me/mods/tree/dev/packs/focus").unwrap(),
            github("me", "mods", Some("dev"), Some("packs/focus"))
        );
        for bad in [
            "nope",
            "me/../x",
            "https://github.com/me",
            "me/mods@a..b",
            "me/mods/../x",
        ] {
            assert!(Source::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_single_mod_repo_is_named_after_the_repo() {
        let source = Source::parse("me/alc-mod-Focus.Timer").unwrap();
        assert_eq!(source.default_id(), "focus-timer");
        let files = files_from_tarball(
            &tarball(&[
                ("alc-mod-focus-abc123/mod.json", "{}"),
                (
                    "alc-mod-focus-abc123/mod.js",
                    "alc.define('focus-timer', () => {});",
                ),
                ("alc-mod-focus-abc123/README.md", "# hi"),
                ("alc-mod-focus-abc123/preview.png", "png"),
                ("alc-mod-focus-abc123/src/other.js", "x"),
            ]),
            None,
        )
        .unwrap();
        let bundle = bundle(files, &source.default_id()).unwrap();
        assert_eq!(bundle.mods(), vec!["focus-timer".to_owned()]);
        assert!(bundle.files.contains_key("mods/focus-timer/README.md"));
        assert_eq!(bundle.files.len(), 3);
    }

    #[test]
    fn a_collection_keeps_its_ids_and_themes_and_honours_a_subfolder() {
        let archive = tarball(&[
            ("r-1/packs/a/mods/one/mod.js", "1"),
            ("r-1/packs/a/mods/two/mod.css", "b{}"),
            ("r-1/packs/a/mods/Bad Id/mod.js", "x"),
            ("r-1/packs/a/themes/sea.json", "{}"),
            ("r-1/packs/b/mods/three/mod.js", "3"),
        ]);
        let bundle = bundle(files_from_tarball(&archive, Some("packs/a")).unwrap(), "x").unwrap();
        assert_eq!(bundle.mods(), vec!["one".to_owned(), "two".to_owned()]);
        assert_eq!(bundle.themes(), vec!["sea".to_owned()]);
    }

    #[test]
    fn nothing_escapes_and_empty_sources_are_refused() {
        // `tar` itself refuses to build `..` paths, so the escape is checked
        // on what bundle() accepts: only fixed names under mods/ and themes/.
        let mut files = BTreeMap::new();
        files.insert("../../etc/passwd".to_owned(), b"x".to_vec());
        files.insert("mods/../x/mod.js".to_owned(), b"x".to_vec());
        files.insert("mods/a/../../mod.js".to_owned(), b"x".to_vec());
        assert!(bundle(files, "x").is_err());

        let mut code_less = BTreeMap::new();
        code_less.insert("mods/a/mod.json".to_owned(), b"{}".to_vec());
        assert!(bundle(code_less, "x").is_err());
    }

    #[test]
    fn install_writes_mods_off_and_refuses_to_clobber() {
        let temp = tempfile::tempdir().unwrap();
        let web = WebDir::new(temp.path());
        let mut files = BTreeMap::new();
        files.insert(
            "mod.js".to_owned(),
            b"alc.define('hello', () => {});".to_vec(),
        );
        files.insert(
            "themes/sea.json".to_owned(),
            br#"{"mode":"light"}"#.to_vec(),
        );
        let bundle = bundle(files, "hello").unwrap();

        let done = install(&web, &bundle, "me/hello", false, false).unwrap();
        assert_eq!(done.mods, vec!["hello".to_owned()]);
        assert_eq!(done.themes, vec!["sea".to_owned()]);
        let (settings, _) = web.load_settings();
        assert_eq!(settings.disabled_mods, vec!["hello".to_owned()]);
        assert!(
            web.script_hashes().is_empty(),
            "an installed mod starts off"
        );
        assert_eq!(
            fs::read_to_string(web.root().join("mods/hello/SOURCE")).unwrap(),
            "me/hello\n"
        );

        assert!(install(&web, &bundle, "me/hello", false, false).is_err());
        install(&web, &bundle, "me/hello", true, true).unwrap();
        assert_eq!(web.script_hashes().len(), 1);
    }

    #[test]
    fn a_folder_source_reads_the_same_layout() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my-mods");
        fs::create_dir_all(root.join("mods/one")).unwrap();
        fs::write(root.join("mods/one/mod.js"), "1").unwrap();
        fs::write(root.join("mods/one/notes.txt"), "skip").unwrap();
        let bundle = bundle(files_from_folder(&root).unwrap(), "x").unwrap();
        assert_eq!(
            bundle.files.keys().collect::<Vec<_>>(),
            vec!["mods/one/mod.js"]
        );
    }
}
