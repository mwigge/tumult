//! Executable files for tests that spawn a stand-in program.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Writes `content` to `path` as an executable file that can be spawned at
/// once.
///
/// The file is written by a child process, never by the test process. A test
/// that writes it with `std::fs::write` holds a write descriptor for a moment.
/// When another test thread starts a process in that moment, the new child
/// inherits the descriptor and keeps it until its own `exec`, and the kernel
/// refuses to execute a file that is open for writing: the spawn fails with
/// "Text file busy" (`ETXTBSY`). Closing the file first does not help, because
/// the copy in the other child is still open. A descriptor that never exists
/// in the test process cannot be inherited from it.
///
/// The shell gets a `PATH` of its own: another test of the same binary may
/// point the `PATH` of the process elsewhere for a moment, and the shell would
/// then not find `chmod`.
///
/// # Panics
///
/// When `sh` cannot be started or cannot write the file.
pub fn write_executable(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    let status = Command::new("/bin/sh")
        .env("PATH", "/usr/bin:/bin")
        .args(["-c", r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .arg(content)
        .status()
        .unwrap_or_else(|err| panic!("start sh to write {}: {err}", path.display()));
    assert!(status.success(), "write {}: {status}", path.display());
}

/// Writes an executable `#!/bin/sh` script named `name` into `dir` and
/// returns its path. See [`write_executable`].
///
/// # Panics
///
/// When the script cannot be written.
pub fn fake_bin(dir: impl AsRef<Path>, name: &str, body: &str) -> PathBuf {
    let path = dir.as_ref().join(name);
    write_executable(&path, &format!("#!/bin/sh\n{body}\n"));
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn write_executable_stores_the_content_unchanged_with_mode_755() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool");
        let content = "#!/bin/sh\nprintf '%s\\n' \"$1\" 100% 'a b' `true`\n";

        write_executable(&path, content);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    /// Another test of the same binary may point `PATH` elsewhere for a
    /// moment. The shell that writes the file finds its tools all the same.
    #[test]
    fn write_executable_does_not_depend_on_the_path_of_the_test_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool");
        {
            let mut env = crate::EnvGuard::lock();
            env.set("PATH", dir.path());
            write_executable(&path, "#!/bin/sh\nexit 0\n");
        }

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn fake_bin_is_a_shell_script_that_runs() {
        let dir = tempfile::tempdir().unwrap();

        let bin = fake_bin(dir.path(), "greet", r#"echo "hello $1""#);

        assert_eq!(bin, dir.path().join("greet"));
        let output = Command::new(&bin).arg("world").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hello world\n");
    }

    #[test]
    #[should_panic(expected = "write")]
    fn write_executable_panics_when_the_directory_is_missing() {
        let dir = tempfile::tempdir().unwrap();

        write_executable(dir.path().join("missing").join("tool"), "#!/bin/sh\n");
    }

    /// The race this helper removes: several threads write a script and spawn
    /// it while the others fork. Written in-process, some spawn fails with
    /// "Text file busy" within a few hundred rounds.
    #[test]
    fn a_fresh_executable_spawns_while_other_threads_start_processes() {
        const THREADS: usize = 8;
        const ROUNDS: usize = 40;
        let dir = tempfile::tempdir().unwrap();

        let failures: Vec<String> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..THREADS)
                .map(|thread| {
                    let dir = dir.path();
                    scope.spawn(move || {
                        (0..ROUNDS)
                            .filter_map(|round| {
                                let bin = fake_bin(dir, &format!("bin-{thread}-{round}"), "exit 0");
                                Command::new(&bin).status().err().map(|err| err.to_string())
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect()
        });

        assert_eq!(failures, Vec::<String>::new());
    }
}
