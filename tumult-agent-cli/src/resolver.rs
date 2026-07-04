//! Binary resolution: explicit env-var override, then a `PATH` search.
//!
//! A non-runnable env override is ignored with a warning rather than
//! treated as fatal, so a stale `*_BIN` setting degrades gracefully to the
//! `PATH` lookup.

use std::env;
use std::path::{Path, PathBuf};

/// Resolve an executable for `binary_name`.
///
/// Resolution order:
/// 1. `env_key` (e.g. `CLAUDE_CODE_BIN`) when set to a non-empty path — used
///    only if the path is an executable file, otherwise a warning is logged
///    and resolution falls through.
/// 2. Each directory in `PATH`, first executable match wins.
///
/// Returns `None` when no runnable binary is found.
#[must_use]
pub fn resolve_binary(env_key: &str, binary_name: &str) -> Option<PathBuf> {
    if let Some(explicit) = env::var_os(env_key) {
        let explicit = PathBuf::from(explicit);
        if !explicit.as_os_str().is_empty() {
            if is_executable(&explicit) {
                return Some(explicit);
            }
            tracing::warn!(
                env_key,
                path = %explicit.display(),
                "env override is not an executable file; falling back to PATH lookup"
            );
        }
    }
    search_path(binary_name)
}

/// Return true when `key` is set to a non-empty (non-whitespace) value.
pub(crate) fn env_nonempty(key: &str) -> bool {
    env::var(key).is_ok_and(|v| !v.trim().is_empty())
}

fn search_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(binary_name))
        .find(|candidate| is_executable(candidate))
}

/// Return true when `path` is a regular file the current user may execute.
///
/// On non-Unix platforms the permission-bit check is skipped and any regular
/// file is accepted.
#[must_use]
pub fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
