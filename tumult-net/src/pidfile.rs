//! The pidfile of a chaos proxy: the pid of its daemon, in decimal.
//!
//! The daemon writes it once it serves its listen address. The start
//! action waits for it to name the daemon it spawned, and the rollback
//! reads it to find that daemon again.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use tumult_otel::logging::err;

/// Records `pid` in `path` in one step: the pid goes to a new file next to
/// `path`, which is then renamed onto it. A reader sees what was there
/// before or the new pid, never a part of it.
///
/// # Errors
///
/// When the new file cannot be made or written, or the rename fails. A new
/// file that was made is removed again.
pub fn write(path: &Path, pid: u32) -> io::Result<()> {
    let staged = staged_path(path, pid)?;
    // `create_new`: a file or a link planted under the staged name is
    // never written through.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let written = file
        .write_all(pid.to_string().as_bytes())
        .and_then(|()| std::fs::rename(&staged, path));
    if written.is_err() {
        remove_staged(&staged);
    }
    written
}

/// The pid `path` names. `None` when there is no pidfile, or when what it
/// holds is not a pid.
///
/// # Errors
///
/// When the pidfile exists but cannot be read.
pub(crate) async fn read(path: &Path) -> io::Result<Option<u32>> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => Ok(contents.trim().parse::<u32>().ok()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The new file a write of `pid` makes next to `path`.
fn staged_path(path: &Path, pid: u32) -> io::Result<PathBuf> {
    let Some(name) = path.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the pidfile path {} names no file", path.display()),
        ));
    };
    let mut staged = name.to_os_string();
    staged.push(format!(".{pid}.tmp"));
    Ok(path.with_file_name(staged))
}

fn remove_staged(path: &Path) {
    let Err(error) = std::fs::remove_file(path) else {
        return;
    };
    tracing::warn!(
        name: "net.proxyd.pidfile.cleanup_failed",
        {
            file.path = %path.display(),
            error.type = "io",
            error.message = %err(&error),
        },
        "a half-written pidfile could not be removed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_write_replaces_a_stale_pidfile_and_leaves_nothing_beside_it() {
        let dir = std::env::temp_dir().join(format!(
            "tumult-net-test-pidfile-write-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("proxy.pid");
        std::fs::write(&path, "999999999").unwrap();

        let written = write(&path, 4242);
        let named = read(&path).await;
        let beside: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let _ = std::fs::remove_dir_all(&dir);

        written.expect("the write");
        assert_eq!(named.expect("the read"), Some(4242));
        assert_eq!(beside, ["proxy.pid"]);
    }
}
