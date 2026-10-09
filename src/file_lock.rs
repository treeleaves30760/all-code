//! Small, process-safe advisory locks. The lock file is deliberately never
//! removed: unlinking it would let a new caller lock a different inode while
//! another caller still owns the old one.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub struct FileLock {
    file: File,
}

impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_timeout(path, DEFAULT_TIMEOUT)
    }

    pub fn acquire_timeout(path: &Path, timeout: Duration) -> Result<Self> {
        let parent = path.parent().context("lock path has no parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create lock directory {}", parent.display()))?;
        // Resolve the directory, not the leaf: a planted lock-file symlink is
        // rejected rather than followed to an unrelated lock.
        let parent = fs::canonicalize(parent)?;
        let name = path.file_name().context("lock path has no file name")?;
        let path = parent.join(name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options
            .open(&path)
            .with_context(|| format!("could not open lock {}", path.display()))?;
        ensure!(
            file.metadata()?.is_file(),
            "lock is not a regular file: {}",
            path.display()
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            ensure!(
                file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                "lock is a reparse point: {}",
                path.display()
            );
        }
        let started = Instant::now();
        loop {
            match try_lock(&file) {
                Ok(true) => return Ok(Self { file }),
                Ok(false) if started.elapsed() < timeout => {
                    thread::sleep(RETRY_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
                }
                Ok(false) => bail!("timed out waiting for lock {}", path.display()),
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("could not lock {}", path.display()));
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Closing the file also releases the OS lock, even after a crash.
        let _ = unlock(&self.file);
    }
}

/// Resolve aliases before choosing a sidecar lock. Works without creating a
/// missing leaf or parent, so it is also safe in read-only planning paths.
pub fn canonical_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    canonical_missing(&absolute)
}

fn canonical_missing(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Do not silently treat a dangling symlink as a missing file.
            if fs::symlink_metadata(path).is_ok() {
                bail!("cannot resolve {}", path.display());
            }
            let parent = path.parent().context("path has no existing ancestor")?;
            let parent = canonical_missing(parent)?;
            match path.components().next_back() {
                Some(Component::Normal(name)) => Ok(parent.join(name)),
                Some(Component::ParentDir) => Ok(parent.parent().unwrap_or(&parent).to_path_buf()),
                Some(Component::CurDir) => Ok(parent),
                _ => bail!("invalid path {}", path.display()),
            }
        }
        Err(error) => Err(error).with_context(|| format!("could not resolve {}", path.display())),
    }
}

/// Lock the canonical file's sidecar, not the data file itself. Atomic data
/// replacement therefore does not replace the inode on which callers lock.
pub fn lock_for(path: &Path) -> Result<FileLock> {
    let canonical = canonical_path(path)?;
    let name = canonical
        .file_name()
        .context("data path has no file name")?;
    let mut lock_name = name.to_os_string();
    lock_name.push(".lock");
    FileLock::acquire(&canonical.with_file_name(lock_name))
}

#[cfg(unix)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: the descriptor belongs to `file` and remains open for the call.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    ) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn unlock(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: the descriptor belongs to `file` and remains open for the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::LockFile;
    // LockFile is nonblocking and needs no OVERLAPPED/extra Windows features.
    // SAFETY: the handle belongs to `file`; locking a single byte is valid even
    // for an empty file and does not change its contents.
    if unsafe { LockFile(file.as_raw_handle(), 0, 0, 1, 0) } != 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn unlock(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFile;
    // SAFETY: this is the same live handle/range used by try_lock.
    if unsafe { UnlockFile(file.as_raw_handle(), 0, 0, 1, 0) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_contention_is_bounded_and_drop_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transaction.lock");
        let first = FileLock::acquire(&path).unwrap();
        let error = FileLock::acquire_timeout(&path, Duration::from_millis(30)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        drop(first);
        FileLock::acquire_timeout(&path, Duration::ZERO).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn canonical_planning_does_not_create_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-created/credentials.json");
        let resolved = canonical_path(&path).unwrap();
        assert!(resolved.is_absolute());
        assert!(!path.parent().unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn data_aliases_share_one_sidecar_lock() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("auth.json");
        let alias = dir.path().join("alias.json");
        fs::write(&data, b"{}").unwrap();
        symlink(&data, &alias).unwrap();
        assert_eq!(
            canonical_path(&data).unwrap(),
            canonical_path(&alias).unwrap()
        );
        let first = lock_for(&alias).unwrap();
        assert!(
            FileLock::acquire_timeout(&data.with_file_name("auth.json.lock"), Duration::ZERO)
                .is_err()
        );
        drop(first);
    }

    #[cfg(unix)]
    #[test]
    fn planted_lock_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, b"unchanged").unwrap();
        let lock = dir.path().join("lock");
        symlink(&victim, &lock).unwrap();
        assert!(FileLock::acquire(&lock).is_err());
        assert_eq!(fs::read(victim).unwrap(), b"unchanged");
    }
}
