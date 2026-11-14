//! The file a detached proxy daemon's stderr goes to.
//!
//! It is where the daemon's structured events land for as long as it runs,
//! and what the start action reads back to say why a daemon died on arrival.
//! The system temp directory is shared with every other user of the host, so
//! the file lives in a directory only its owner can enter, is always created
//! anew (never opened through a name somebody else may have planted), and is
//! removed again unless the daemon it belongs to came up.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use tumult_otel::logging::{err, LogOptions};

/// Most bytes the daemon writes to its log before it stops logging to it.
/// Nothing rotates the file, and a chaos proxy can run for days.
pub const DAEMON_LOG_LIMIT_BYTES: u64 = 1024 * 1024;

/// Most bytes read back from the log of a daemon that died on arrival. What
/// is read ends up in an error message; the cause is in the first lines.
const READ_LIMIT_BYTES: u64 = 8 * 1024;

const FILE_NAME: &str = "stderr.log";

/// The directory holding the log of the daemon whose pidfile is `pidfile`.
pub(crate) fn dir_for(pidfile: &Path) -> PathBuf {
    pidfile.with_extension("log.d")
}

/// A freshly created log, removed when dropped unless [`keep`](Self::keep)
/// was called: a start that fails anywhere leaves nothing behind.
#[derive(Debug)]
pub(crate) struct StartupLog {
    dir: PathBuf,
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
        private_dir(dir)?;
        let path = dir.join(FILE_NAME);
        let file = create_new(&path)?;
        if let Err(error) = owned_like(dir, &file) {
            remove_now(dir);
            return Err(error);
        }
        let log = Self {
            dir: dir.to_path_buf(),
            keep: false,
        };
        Ok((log, file))
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// The start of what the daemon wrote, at most [`READ_LIMIT_BYTES`] of
    /// it. A log that cannot be read reads as empty, and that is logged.
    pub(crate) async fn read(&self) -> String {
        match read_start(&self.path()).await {
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
        remove_now(&self.dir);
    }
}

/// Removes the log directory `dir` and the log in it. Neither existing is
/// fine; any other failure is logged, since nothing else can be done about
/// it during a rollback.
pub(crate) async fn remove(dir: &Path) {
    let file = tokio::fs::remove_file(dir.join(FILE_NAME)).await;
    let directory = tokio::fs::remove_dir(dir).await;
    log_remove_failure(dir, file.and(directory));
}

fn remove_now(dir: &Path) {
    let file = std::fs::remove_file(dir.join(FILE_NAME));
    let directory = std::fs::remove_dir(dir);
    log_remove_failure(dir, file.and(directory));
}

fn log_remove_failure(dir: &Path, result: io::Result<()>) {
    let Err(error) = result else {
        return;
    };
    if error.kind() == io::ErrorKind::NotFound {
        return;
    }
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

async fn read_start(path: &Path) -> io::Result<String> {
    use tokio::io::AsyncReadExt as _;

    let file = tokio::fs::File::open(path).await?;
    let mut bytes = Vec::new();
    file.take(READ_LIMIT_BYTES).read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Creates `dir` for its owner alone, or checks that an existing one is
/// that private.
fn private_dir(dir: &Path) -> io::Result<()> {
    match create_private_dir(dir) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => verify_private(dir),
        other => other,
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir(dir)
}

/// Refuses a `dir` that is a link, not a directory, or open to anyone but
/// its owner. The name is predictable, so somebody else may have made it.
fn verify_private(dir: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(dir)?;
    if metadata.file_type().is_dir() && owner_only(&metadata) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} is not a private directory", dir.display()),
    ))
}

/// The permission bits of group and others.
#[cfg(unix)]
const NOT_OWNER: u32 = 0o077;

#[cfg(unix)]
fn owner_only(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    (metadata.permissions().mode() & NOT_OWNER) == 0
}

#[cfg(not(unix))]
fn owner_only(_metadata: &std::fs::Metadata) -> bool {
    true
}

/// Creates the log at `path`, which must not exist afterwards as anything
/// but the file made here. A log left by a daemon that was never stopped is
/// unlinked first: it is replaced, never written through.
fn create_new(path: &Path) -> io::Result<File> {
    match open_new(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            open_new(path)
        }
        other => other,
    }
}

#[cfg(unix)]
fn open_new(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_new(path: &Path) -> io::Result<File> {
    File::options().write(true).create_new(true).open(path)
}

/// Checks that `dir` belongs to whoever just created `file` in it. Only
/// root can create a file in another user's private directory, and root
/// must not log into a directory somebody else controls.
#[cfg(unix)]
fn owned_like(dir: &Path, file: &File) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    if std::fs::symlink_metadata(dir)?.uid() == file.metadata()?.uid() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} belongs to another user", dir.display()),
    ))
}

#[cfg(not(unix))]
fn owned_like(_dir: &Path, _file: &File) -> io::Result<()> {
    Ok(())
}

/// How the daemon logs: as `service_name`, and no more than
/// [`DAEMON_LOG_LIMIT_BYTES`] to the stderr the start action gave it.
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
        assert_eq!(
            dir_for(Path::new("/tmp/tumult-net-127-0-0-1-8080.pid")),
            Path::new("/tmp/tumult-net-127-0-0-1-8080.log.d")
        );
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
