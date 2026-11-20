//! The file a detached proxy daemon's stderr goes to.
//!
//! It is where the daemon's structured events land for as long as it runs,
//! and what the start action reads back to say why a daemon died on arrival.
//! The directory it lives in has a predictable name, and the system temp
//! directory is shared with every other user of the host. So the directory
//! is opened once, without following a link, and checked through that open
//! handle to belong to the current user alone; the file is then created,
//! read and unlinked relative to the handle, never through the name again.
//! The file is always created anew, and removed unless the daemon it belongs
//! to came up.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use tumult_otel::logging::{err, LogOptions};

use private_dir::PrivateDir;

/// Most bytes the daemon writes to its log before it stops logging to it.
/// Nothing rotates the file, and a chaos proxy can run for days.
pub const DAEMON_LOG_LIMIT_BYTES: u64 = 1024 * 1024;

/// Most bytes read back from the log of a daemon that died on arrival. What
/// is read ends up in an error message; the cause is in the first lines.
const READ_LIMIT_BYTES: u64 = 8 * 1024;

const FILE_NAME: &str = "stderr.log";

/// Where the log directory of the daemon whose pidfile is `pidfile` may be,
/// the preferred place first: in `runtime_dir` (the user's
/// `$XDG_RUNTIME_DIR`, which no other user can enter) when that is an
/// absolute path, and next to the pidfile.
pub(crate) fn dirs_for(pidfile: &Path, runtime_dir: Option<OsString>) -> Vec<PathBuf> {
    let beside = pidfile.with_extension("log.d");
    let Some(preferred) = in_runtime_dir(&beside, runtime_dir) else {
        return vec![beside];
    };
    vec![preferred, beside]
}

fn in_runtime_dir(beside: &Path, runtime_dir: Option<OsString>) -> Option<PathBuf> {
    let runtime_dir = PathBuf::from(runtime_dir?);
    if !runtime_dir.is_absolute() {
        return None;
    }
    Some(runtime_dir.join(beside.file_name()?))
}

/// A freshly created log, removed when dropped unless [`keep`](Self::keep)
/// was called: a start that fails anywhere leaves nothing behind.
#[derive(Debug)]
pub(crate) struct StartupLog {
    path: PathBuf,
    dir: PrivateDir,
    keep: bool,
}

impl StartupLog {
    /// Creates `dir` as a private directory, and in it the empty log file.
    /// Returns the guard and the open file, which becomes the daemon's
    /// stderr.
    ///
    /// # Errors
    ///
    /// Fails when `dir` exists and is not a private directory of the current
    /// user, or when the file cannot be created in it.
    pub(crate) fn create(dir: &Path) -> io::Result<(Self, File)> {
        let opened = PrivateDir::create(dir)?;
        let file = opened.create_file(FILE_NAME)?;
        let log = Self {
            path: dir.to_path_buf(),
            dir: opened,
            keep: false,
        };
        Ok((log, file))
    }

    /// [`create`](Self::create) in the first of `dirs` that can be used. A
    /// directory that cannot is logged and the next one tried.
    ///
    /// # Errors
    ///
    /// The failure of the last directory when none can be used.
    pub(crate) fn create_in_first(dirs: &[PathBuf]) -> io::Result<(Self, File)> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no log directory to try");
        for dir in dirs {
            match Self::create(dir) {
                Ok(created) => return Ok(created),
                Err(error) => {
                    log_unusable(dir, &error);
                    last = error;
                }
            }
        }
        Err(last)
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> PathBuf {
        self.path.join(FILE_NAME)
    }

    /// The start of what the daemon wrote, at most [`READ_LIMIT_BYTES`] of
    /// it. A log that cannot be read reads as empty, and that is logged.
    pub(crate) async fn read(&self) -> String {
        match self.read_start().await {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(
                    name: "net.proxy.log.read_failed",
                    { error.type = "io", error.message = %err(&error) },
                    "the chaos proxy's startup log could not be read"
                );
                String::new()
            }
        }
    }

    async fn read_start(&self) -> io::Result<String> {
        let dir = self.dir.try_clone()?;
        tokio::task::spawn_blocking(move || read_start_of(&dir))
            .await
            .map_err(io::Error::other)?
    }

    /// Leaves the log in place for the running daemon. [`remove`] takes it
    /// away when the proxy is stopped.
    pub(crate) fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for StartupLog {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        // Two unlinks in a directory of our own: not worth a blocking task,
        // and a destructor cannot await one.
        log_remove_failure(&self.path, remove_from(&self.dir, &self.path));
    }
}

fn log_unusable(dir: &Path, error: &io::Error) {
    tracing::warn!(
        name: "net.proxy.log.dir_unusable",
        {
            file.path = %dir.display(),
            error.type = "io",
            error.message = %err(error),
        },
        "a directory for the chaos proxy's log could not be used"
    );
}

/// Removes the log directory `dir` and the log in it. Neither existing is
/// fine; any other failure is logged, since nothing else can be done about
/// it during a rollback. A `dir` that is not a private directory of the
/// current user is such a failure: nothing is removed from it.
pub(crate) async fn remove(dir: &Path) {
    let path = dir.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || remove_at(&path))
        .await
        .unwrap_or_else(|error| Err(io::Error::other(error)));
    log_remove_failure(dir, removed);
}

fn remove_at(path: &Path) -> io::Result<()> {
    match PrivateDir::open(path) {
        Ok(dir) => remove_from(&dir, path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Unlinks the log in the open `dir`, then the directory itself, which only
/// goes when it is empty. `path` is where `dir` was opened.
fn remove_from(dir: &PrivateDir, path: &Path) -> io::Result<()> {
    let file = absent_is_fine(dir.unlink(FILE_NAME));
    let directory = absent_is_fine(std::fs::remove_dir(path));
    file.and(directory)
}

fn absent_is_fine(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn log_remove_failure(dir: &Path, result: io::Result<()>) {
    let Err(error) = result else {
        return;
    };
    tracing::warn!(
        name: "net.proxy.log.remove_failed",
        {
            file.path = %dir.display(),
            error.type = "io",
            error.message = %err(&error),
        },
        "the chaos proxy's log could not be removed"
    );
}

fn read_start_of(dir: &PrivateDir) -> io::Result<String> {
    use std::io::Read as _;

    let file = dir.open_file(FILE_NAME)?;
    let mut bytes = Vec::new();
    file.take(READ_LIMIT_BYTES).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(unix)]
mod private_dir {
    //! A directory held open, and everything done in it done through that
    //! handle.

    use std::fs::File;
    use std::io;
    use std::os::fd::OwnedFd;
    use std::path::Path;

    use rustix::fs::{fstat, mkdir, open, openat, unlinkat, AtFlags, Mode, OFlags};
    use rustix::io::Errno;
    use rustix::process::geteuid;

    /// A directory that was, when it was opened, the current user's and
    /// closed to everybody else.
    #[derive(Debug)]
    pub(crate) struct PrivateDir(OwnedFd);

    impl PrivateDir {
        /// Creates `path` for its owner alone and opens it, or opens the
        /// directory that is already there.
        pub(crate) fn create(path: &Path) -> io::Result<Self> {
            match mkdir(path, Mode::RWXU) {
                Ok(()) | Err(Errno::EXIST) => Self::open(path),
                Err(errno) => Err(errno.into()),
            }
        }

        /// Opens the directory `path`, which must not be a link, and
        /// refuses it unless it is private. The check is made on the open
        /// directory, so it holds for everything done through the handle
        /// whatever becomes of the name.
        pub(crate) fn open(path: &Path) -> io::Result<Self> {
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let fd = open(path, flags, Mode::empty()).map_err(|errno| match errno {
                // What a link answers, depending on the platform.
                Errno::LOOP | Errno::NOTDIR => refused(path, NOT_PRIVATE),
                other => other.into(),
            })?;
            let stat = fstat(&fd)?;
            let mode = Mode::from_raw_mode(stat.st_mode);
            match refusal(stat.st_uid, geteuid().as_raw(), mode) {
                Some(reason) => Err(refused(path, reason)),
                None => Ok(Self(fd)),
            }
        }

        pub(crate) fn try_clone(&self) -> io::Result<Self> {
            Ok(Self(self.0.try_clone()?))
        }

        /// Creates the file `name`, which afterwards exists as nothing but
        /// the file made here. One left by a daemon that was never stopped,
        /// or a link planted under that name, is unlinked first: it is
        /// replaced, never written through.
        pub(crate) fn create_file(&self, name: &str) -> io::Result<File> {
            match self.open_new(name) {
                Err(Errno::EXIST) => {
                    unlinkat(&self.0, name, AtFlags::empty())?;
                    Ok(self.open_new(name)?)
                }
                other => Ok(other?),
            }
        }

        fn open_new(&self, name: &str) -> Result<File, Errno> {
            let flags =
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            openat(&self.0, name, flags, Mode::RUSR | Mode::WUSR).map(File::from)
        }

        /// Opens the file `name` for reading; never through a link.
        pub(crate) fn open_file(&self, name: &str) -> io::Result<File> {
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            Ok(openat(&self.0, name, flags, Mode::empty()).map(File::from)?)
        }

        pub(crate) fn unlink(&self, name: &str) -> io::Result<()> {
            Ok(unlinkat(&self.0, name, AtFlags::empty())?)
        }
    }

    const NOT_PRIVATE: &str = "is not a private directory";
    const NOT_OURS: &str = "belongs to another user";

    /// Why a directory of `owner` with `mode` is not `user`'s private one;
    /// `None` when it is. Root included: it must not log into a directory
    /// somebody else controls.
    pub(super) fn refusal(owner: u32, user: u32, mode: Mode) -> Option<&'static str> {
        if owner != user {
            return Some(NOT_OURS);
        }
        if mode.intersects(Mode::RWXG | Mode::RWXO) {
            return Some(NOT_PRIVATE);
        }
        None
    }

    fn refused(path: &Path, reason: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} {reason}", path.display()),
        )
    }
}

#[cfg(not(unix))]
mod private_dir {
    //! Without directory handles and owner-only modes, the directory is
    //! used by name.

    use std::fs::File;
    use std::io;
    use std::path::{Path, PathBuf};

    #[derive(Debug)]
    pub(crate) struct PrivateDir(PathBuf);

    impl PrivateDir {
        pub(crate) fn create(path: &Path) -> io::Result<Self> {
            match std::fs::create_dir(path) {
                Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
                _ => Self::open(path),
            }
        }

        pub(crate) fn open(path: &Path) -> io::Result<Self> {
            if std::fs::symlink_metadata(path)?.is_dir() {
                return Ok(Self(path.to_path_buf()));
            }
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not a private directory", path.display()),
            ))
        }

        pub(crate) fn try_clone(&self) -> io::Result<Self> {
            Ok(Self(self.0.clone()))
        }

        pub(crate) fn create_file(&self, name: &str) -> io::Result<File> {
            let path = self.0.join(name);
            match open_new(&path) {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    std::fs::remove_file(&path)?;
                    open_new(&path)
                }
                other => other,
            }
        }

        pub(crate) fn open_file(&self, name: &str) -> io::Result<File> {
            File::open(self.0.join(name))
        }

        pub(crate) fn unlink(&self, name: &str) -> io::Result<()> {
            std::fs::remove_file(self.0.join(name))
        }
    }

    fn open_new(path: &Path) -> io::Result<File> {
        File::options().write(true).create_new(true).open(path)
    }
}

/// How the daemon logs: as `service_name`, and no more than
/// `DAEMON_LOG_LIMIT_BYTES` to the stderr the start action gave it.
#[must_use]
pub fn daemon_log_options(service_name: &str) -> LogOptions {
    LogOptions::new(service_name).with_output_limit(DAEMON_LOG_LIMIT_BYTES)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// A directory of this test's own, removed with everything in it.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("tumult-net-test-log-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn log_dir(&self) -> PathBuf {
            self.0.join("proxy.log.d")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // Best effort: a leftover only costs temp space.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn create_private_dir(dir: &Path) -> io::Result<()> {
        use std::os::unix::fs::DirBuilderExt as _;

        std::fs::DirBuilder::new().mode(0o700).create(dir)
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn the_log_directory_is_named_after_the_pidfile() {
        let pidfile = Path::new("/tmp/tumult-net-127-0-0-1-8080.pid");
        let beside = PathBuf::from("/tmp/tumult-net-127-0-0-1-8080.log.d");

        assert_eq!(dirs_for(pidfile, None), std::slice::from_ref(&beside));
        // Not a place: an empty or relative value is ignored.
        for unusable in ["", "run/user/1000", "."] {
            assert_eq!(
                dirs_for(pidfile, Some(unusable.into())),
                std::slice::from_ref(&beside),
                "{unusable:?}"
            );
        }
    }

    #[test]
    fn the_runtime_directory_of_the_user_is_preferred_to_the_temp_directory() {
        let pidfile = Path::new("/tmp/tumult-net-127-0-0-1-8080.pid");

        assert_eq!(
            dirs_for(pidfile, Some("/run/user/1000".into())),
            [
                PathBuf::from("/run/user/1000/tumult-net-127-0-0-1-8080.log.d"),
                PathBuf::from("/tmp/tumult-net-127-0-0-1-8080.log.d"),
            ]
        );
    }

    #[test]
    fn the_log_goes_to_the_first_directory_that_can_be_used() {
        let scratch = Scratch::new("first");
        let missing = scratch.0.join("no-such-runtime-dir").join("proxy.log.d");
        let dirs = [missing.clone(), scratch.log_dir()];

        let logs = tumult_otel::logging::testing::LogCapture::start(tracing::Level::DEBUG);
        let (log, _file) = StartupLog::create_in_first(&dirs).unwrap();

        assert_eq!(log.path(), scratch.log_dir().join(FILE_NAME));
        let logged = logs.snapshot();
        let skipped = logged.named("net.proxy.log.dir_unusable");
        assert_eq!(skipped.len(), 1, "{}", logged.raw);
        assert_eq!(skipped[0]["file.path"], missing.display().to_string());

        let error = StartupLog::create_in_first(&[missing]).expect_err("nowhere to log");
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_directory_of_the_user_closed_to_all_others_is_private() {
        use rustix::fs::Mode;

        let refusal =
            |owner, user, mode| private_dir::refusal(owner, user, Mode::from_bits_truncate(mode));

        assert_eq!(refusal(1000, 1000, 0o700), None);
        assert_eq!(refusal(1000, 1000, 0o500), None);
        assert_eq!(refusal(0, 0, 0o700), None);
        for open in [0o750, 0o705, 0o701, 0o710, 0o777, 0o1777] {
            assert_eq!(
                refusal(1000, 1000, open),
                Some("is not a private directory"),
                "{open:o}"
            );
        }
        // Whoever asks, root included.
        for (owner, user) in [(1001, 1000), (1000, 0), (0, 1000)] {
            assert_eq!(
                refusal(owner, user, 0o700),
                Some("belongs to another user"),
                "{owner} {user}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_log_is_created_empty_and_for_its_owner_alone() {
        let scratch = Scratch::new("private");
        let (log, file) = StartupLog::create(&scratch.log_dir()).unwrap();

        assert_eq!(mode_of(&scratch.log_dir()), 0o700);
        assert_eq!(mode_of(&log.path()), 0o600);
        assert_eq!(file.metadata().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_dropped_log_is_removed_and_a_kept_one_stays_until_removed() {
        let scratch = Scratch::new("drop");
        let dir = scratch.log_dir();

        let (log, _file) = StartupLog::create(&dir).unwrap();
        let path = log.path();
        assert!(path.is_file());
        drop(log);
        assert!(!dir.exists(), "a failed start leaves nothing behind");

        let (log, _file) = StartupLog::create(&dir).unwrap();
        log.keep();
        assert!(path.is_file(), "a running daemon keeps its log");

        remove(&dir).await;
        assert!(!dir.exists(), "stopping the proxy removes it");
        // Removing what is not there is fine, and is not reported.
        let logs = tumult_otel::logging::testing::LogCapture::start(tracing::Level::DEBUG);
        remove(&dir).await;
        assert!(logs.snapshot().events.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_link_is_replaced_and_never_written_through() {
        let scratch = Scratch::new("link");
        let dir = scratch.log_dir();
        let victim = scratch.0.join("victim");
        std::fs::write(&victim, "precious").unwrap();
        create_private_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&victim, dir.join(FILE_NAME)).unwrap();

        let (log, mut file) = StartupLog::create(&dir).unwrap();
        file.write_all(b"daemon output").unwrap();

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
        let made = std::fs::symlink_metadata(log.path()).unwrap();
        assert!(made.file_type().is_file(), "the log is a file of its own");
        assert_eq!(
            std::fs::read_to_string(log.path()).unwrap(),
            "daemon output"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_others_can_enter_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = Scratch::new("open");
        let open = scratch.log_dir();
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        let linked = scratch.0.join("linked.log.d");
        let private = scratch.0.join("private");
        create_private_dir(&private).unwrap();
        std::os::unix::fs::symlink(&private, &linked).unwrap();

        for dir in [&open, &linked] {
            let error = StartupLog::create(dir).expect_err("must be refused");
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
            assert!(
                error.to_string().contains("is not a private directory"),
                "{error}"
            );
        }
        assert!(!open.join(FILE_NAME).exists());
        assert!(!private.join(FILE_NAME).exists());
        // The same directory, once private, is accepted.
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (log, _file) = StartupLog::create(&open).unwrap();
        assert!(log.path().is_file());
    }

    /// A private directory holding a log that is not ours to remove.
    #[cfg(unix)]
    fn victim_in(scratch: &Scratch) -> PathBuf {
        let victim = scratch.0.join("victim");
        create_private_dir(&victim).unwrap();
        std::fs::write(victim.join(FILE_NAME), "precious").unwrap();
        victim
    }

    #[cfg(unix)]
    #[test]
    fn a_dropped_log_is_removed_from_the_directory_it_was_made_in() {
        let scratch = Scratch::new("swap");
        let dir = scratch.log_dir();
        let (log, _file) = StartupLog::create(&dir).unwrap();
        // The name now leads somewhere else.
        let moved = scratch.0.join("moved.log.d");
        std::fs::rename(&dir, &moved).unwrap();
        let victim = victim_in(&scratch);
        std::os::unix::fs::symlink(&victim, &dir).unwrap();

        drop(log);

        assert_eq!(
            std::fs::read_to_string(victim.join(FILE_NAME)).unwrap(),
            "precious"
        );
        assert!(
            !moved.join(FILE_NAME).exists(),
            "the log that was made is the one removed"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_removes_nothing_through_a_link_or_from_an_open_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = Scratch::new("stop");
        let victim = victim_in(&scratch);
        let linked = scratch.log_dir();
        std::os::unix::fs::symlink(&victim, &linked).unwrap();
        let open = scratch.0.join("open.log.d");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(open.join(FILE_NAME), "somebody's").unwrap();

        let logs = tumult_otel::logging::testing::LogCapture::start(tracing::Level::DEBUG);
        remove(&linked).await;
        remove(&open).await;

        assert!(victim.join(FILE_NAME).is_file(), "reached through a link");
        assert!(open.join(FILE_NAME).is_file(), "not a private directory");
        let logged = logs.snapshot();
        assert_eq!(
            logged.named("net.proxy.log.remove_failed").len(),
            2,
            "{}",
            logged.raw
        );
    }

    #[tokio::test]
    async fn only_the_start_of_a_long_log_is_read_back() {
        let scratch = Scratch::new("read");
        let (log, mut file) = StartupLog::create(&scratch.log_dir()).unwrap();
        let limit = usize::try_from(READ_LIMIT_BYTES).unwrap();
        file.write_all(b"cause\n").unwrap();
        file.write_all(&vec![b'x'; limit * 3]).unwrap();

        let read = log.read().await;

        assert_eq!(read.len(), limit);
        assert!(read.starts_with("cause\nxxx"), "the start, not the end");
    }

    #[test]
    fn the_daemon_limits_what_it_writes_to_its_log() {
        let options = daemon_log_options("tumult-net-proxyd");
        assert_eq!(options.service_name, "tumult-net-proxyd");
        assert_eq!(options.output_limit, Some(DAEMON_LOG_LIMIT_BYTES));
    }
}
