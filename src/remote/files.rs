//! The page's file panel: a read-only look at the session's working directory.
//!
//! Two questions only - what is in this folder, and what is in this file - and
//! both are answered relative to the directory the agent was started in. The
//! panel is how somebody following an agent from a phone checks the file it
//! just said it edited, without asking the agent to print it into the very
//! screen they are trying to read.
//!
//! What keeps it a window onto that one directory:
//!
//! - A requested path is relative, and every component is a plain name: no
//!   root, no prefix, no `..`, no `.`. That is refused before the filesystem
//!   is asked anything.
//! - The joined path is then canonicalised and must still sit under the
//!   canonical root, so a symlink inside the project that points out of it
//!   is refused rather than followed.
//! - Directory listings skip the entries that would be noise at best and a
//!   leak at worst (`.git`'s object store) and are capped, so a folder of a
//!   hundred thousand build artefacts cannot be shipped to a phone in one go.
//! - File reads are capped, binary files are reported as binary rather than
//!   dumped, and the text goes through the same secret masking as the
//!   terminal stream, so a `.env` holding the provider key alc injected shows
//!   it masked here exactly as it would be on screen.
//!
//! Operator links only - the route checks that. A viewer link is meant to be
//! safe to hand to somebody to watch, and a project's source tree is a larger
//! thing to hand them than the screen is.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::remote::scrub::SecretScrubber;

/// The most entries one listing returns. Past this the listing says it was
/// cut short rather than pretending the folder ends there.
const MAX_ENTRIES: usize = 1_000;

/// The most bytes of one file that are sent. Large enough for any source file
/// somebody would read on a phone; past it the panel shows the start and says
/// the rest was left out.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// How much of a file is inspected to decide whether it is text.
const SNIFF_BYTES: usize = 8 * 1024;

/// Names never listed. `.git` is the one that matters: its object store is
/// thousands of opaque files, and its config can carry a remote URL with a
/// token in it.
const SKIPPED: [&str; 2] = [".git", ".DS_Store"];

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Entry {
    pub name: String,
    /// `dir`, `file` or `link` (a symlink whose target is outside the root or
    /// gone, so it can be named but not opened).
    pub kind: &'static str,
    pub size: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Listing {
    /// The folder listed, relative to the root, with `/` separators; empty for
    /// the root itself.
    pub path: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct FileView {
    pub path: String,
    pub size: u64,
    /// Absent for a binary file.
    pub text: Option<String>,
    pub binary: bool,
    pub truncated: bool,
}

/// Lists one folder under `root`.
pub(crate) fn list(root: &Path, relative: &str) -> Result<Listing> {
    let (root, target) = resolve(root, relative)?;
    let metadata = fs::metadata(&target).context("no such folder")?;
    if !metadata.is_dir() {
        bail!("not a folder");
    }

    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in fs::read_dir(&target).context("the folder could not be read")? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if SKIPPED.contains(&name) {
            continue;
        }
        if entries.len() == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let (kind, size) = if file_type.is_symlink() {
            // Followed only when it stays inside the root, which is the same
            // test opening it would apply.
            match fs::canonicalize(entry.path()) {
                Ok(real) if real.starts_with(&root) => match fs::metadata(&real) {
                    Ok(meta) if meta.is_dir() => ("dir", None),
                    Ok(meta) => ("file", Some(meta.len())),
                    Err(_) => ("link", None),
                },
                _ => ("link", None),
            }
        } else if file_type.is_dir() {
            ("dir", None)
        } else {
            ("file", entry.metadata().ok().map(|meta| meta.len()))
        };
        entries.push(Entry {
            name: name.to_owned(),
            kind,
            size,
        });
    }

    // Folders first, then names as a person sorts them: case-insensitive.
    entries.sort_by(|left, right| {
        (left.kind != "dir")
            .cmp(&(right.kind != "dir"))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(Listing {
        path: normalised(relative),
        entries,
        truncated,
    })
}

/// Reads one file under `root`, masked with `secrets`.
pub(crate) fn read(root: &Path, relative: &str, secrets: &[String]) -> Result<FileView> {
    let (_, target) = resolve(root, relative)?;
    let metadata = fs::metadata(&target).context("no such file")?;
    if !metadata.is_file() {
        bail!("not a file");
    }
    let size = metadata.len();

    let file = fs::File::open(&target).context("the file could not be opened")?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES)
        .read_to_end(&mut bytes)
        .context("the file could not be read")?;
    let truncated = size > bytes.len() as u64;

    let sniff = &bytes[..bytes.len().min(SNIFF_BYTES)];
    if sniff.contains(&0) {
        return Ok(FileView {
            path: normalised(relative),
            size,
            text: None,
            binary: true,
            truncated: false,
        });
    }

    let mut scrubber = SecretScrubber::new(secrets);
    let mut masked = scrubber.push(&bytes);
    masked.extend(scrubber.flush());
    // A cut at the byte cap can land inside a multi-byte character; lossy
    // decoding turns that one character into a replacement mark instead of
    // refusing the whole file.
    let text = String::from_utf8_lossy(&masked).into_owned();

    Ok(FileView {
        path: normalised(relative),
        size,
        text: Some(text),
        binary: false,
        truncated,
    })
}

/// The canonical root, and the canonical target under it.
fn resolve(root: &Path, relative: &str) -> Result<(PathBuf, PathBuf)> {
    let root = fs::canonicalize(root).context("the session's folder is gone")?;
    let mut joined = root.clone();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(name) => joined.push(name),
            Component::CurDir => {}
            // `..`, a root or a drive prefix: never reachable from a path that
            // means "somewhere under the session's folder".
            _ => bail!("only paths inside the session's folder can be opened"),
        }
    }
    let target = fs::canonicalize(&joined).context("no such file or folder")?;
    if !target.starts_with(&root) {
        bail!("only paths inside the session's folder can be opened");
    }
    Ok((root, target))
}

/// `relative` as the page will display it: `/`-separated, no leading or
/// trailing separator, no `.` components.
fn normalised(relative: &str) -> String {
    Path::new(relative)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("README.md"), "# hi\n").unwrap();
        fs::write(dir.path().join("b.txt"), "b").unwrap();
        fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        dir
    }

    #[test]
    fn a_listing_puts_folders_first_and_skips_git() {
        let dir = fixture();
        let listing = list(dir.path(), "").unwrap();
        let names: Vec<_> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "b.txt", "blob.bin", "README.md"]);
        assert_eq!(listing.entries[0].kind, "dir");
        assert!(!listing.truncated);
    }

    #[test]
    fn a_nested_folder_lists_with_a_normalised_path() {
        let dir = fixture();
        let listing = list(dir.path(), "./src/").unwrap();
        assert_eq!(listing.path, "src");
        assert_eq!(listing.entries[0].name, "main.rs");
    }

    #[test]
    fn a_text_file_reads_and_a_binary_one_does_not() {
        let dir = fixture();
        let view = read(dir.path(), "src/main.rs", &[]).unwrap();
        assert_eq!(view.text.as_deref(), Some("fn main() {}\n"));
        assert!(!view.binary);

        let blob = read(dir.path(), "blob.bin", &[]).unwrap();
        assert!(blob.binary);
        assert!(blob.text.is_none());
    }

    #[test]
    fn a_secret_in_a_file_is_masked() {
        let dir = fixture();
        fs::write(dir.path().join(".env"), "KEY=sk-test-0123456789abcdef\n").unwrap();
        let view = read(dir.path(), ".env", &["sk-test-0123456789abcdef".to_owned()]).unwrap();
        let text = view.text.unwrap();
        assert!(!text.contains("sk-test-0123456789abcdef"), "{text}");
        assert!(text.starts_with("KEY="));
    }

    #[test]
    fn a_large_file_is_cut_and_says_so() {
        let dir = fixture();
        let big = "x".repeat(MAX_FILE_BYTES as usize + 10);
        fs::write(dir.path().join("big.txt"), &big).unwrap();
        let view = read(dir.path(), "big.txt", &[]).unwrap();
        assert!(view.truncated);
        assert_eq!(view.text.unwrap().len(), MAX_FILE_BYTES as usize);
    }

    #[test]
    fn a_path_out_of_the_folder_is_refused() {
        let dir = fixture();
        for path in ["../", "src/../../etc", "/etc/passwd"] {
            assert!(list(dir.path(), path).is_err(), "{path} listed");
            assert!(read(dir.path(), path, &[]).is_err(), "{path} read");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_folder_is_named_but_not_followed() {
        let dir = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "nope").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();

        let listing = list(dir.path(), "").unwrap();
        let escape = listing.entries.iter().find(|e| e.name == "escape").unwrap();
        assert_eq!(escape.kind, "link");
        assert!(list(dir.path(), "escape").is_err());
        assert!(read(dir.path(), "escape/secret.txt", &[]).is_err());
    }
}
