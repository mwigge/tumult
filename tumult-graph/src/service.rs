//! Service/target extraction from provider arguments.
//!
//! Two provider shapes are supported:
//!
//! * **Native providers** carry structured arguments; the service is the first
//!   present of a small set of well-known keys (`upstream`, `host`, …),
//!   normalised to a bare host name.
//! * **Process providers** carry a shell command line. We extract a service
//!   *only* when it can be read with confidence — a `docker`/`podman`
//!   container operand (`docker exec <c>`, `docker pause <c>`, …), a host in an
//!   `http(s)://` URL (e.g. a `curl` target), or an `ssh` host. Anything else
//!   (including `docker run`, which spins up a new container rather than acting
//!   on a named one) yields no service — we never guess.

use std::collections::HashMap;

/// Argument keys, in priority order, that name the service/target a native
/// fault acts on. The first present string value wins.
const SERVICE_ARG_KEYS: &[&str] = &[
    "upstream", "target", "host", "service", "endpoint", "address", "url", "pod",
];

/// Extract a short service name from a native provider's arguments, stripping a
/// URL scheme, path, and `:port` suffix so `http://demo-app:8080/health`,
/// `demo-app:8080`, and `demo-app` all collapse to `demo-app`.
pub(crate) fn service_from_arguments(
    arguments: &HashMap<String, serde_json::Value>,
) -> Option<String> {
    let raw = SERVICE_ARG_KEYS
        .iter()
        .find_map(|key| arguments.get(*key).and_then(serde_json::Value::as_str))?;
    let host = normalize_service(raw);
    (!host.is_empty()).then_some(host)
}

/// Reduce a raw target string to a bare host name.
pub(crate) fn normalize_service(raw: &str) -> String {
    // Drop a scheme (`http://`, `tcp://`, …).
    let no_scheme = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    // Drop any path/query.
    let authority = no_scheme.split(['/', '?']).next().unwrap_or(no_scheme);
    // Drop a `:port` suffix.
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
                host
            } else {
                authority
            }
        });
    host.trim().to_string()
}

/// `docker`/`podman` subcommands that act on an existing, named container. The
/// container is the first non-flag operand after the subcommand. `run`,
/// `create`, `compose`, etc. are deliberately excluded — they do not name an
/// existing container to target.
const CONTAINER_OPS: &[&str] = &[
    "exec", "pause", "unpause", "restart", "kill", "stop", "start", "wait", "top", "logs",
    "attach", "inspect", "port",
];

/// Flags (within the container subcommands above) that take a following value,
/// which must be skipped when scanning for the container operand.
const VALUE_FLAGS: &[&str] = &[
    "-s",
    "--signal",
    "-u",
    "--user",
    "-e",
    "--env",
    "--env-file",
    "-w",
    "--workdir",
    "-l",
    "--label",
];

/// Shell interpreters whose `-c <script>` argument carries the real command.
const SHELLS: &[&str] = &["sh", "bash", "dash", "zsh", "ash", "ksh"];

/// Extract a service from a process provider's command line, conservatively.
/// Returns `None` when no target can be read with confidence.
pub(crate) fn service_from_process(path: &str, arguments: &[String]) -> Option<String> {
    let tokens = command_tokens(path, arguments);
    docker_container(&tokens)
        .or_else(|| url_host(&tokens))
        .or_else(|| ssh_host(&tokens))
}

/// Resolve the effective command tokens: for `sh -c "<script>"` (and friends)
/// the script is tokenised on whitespace; otherwise the executable plus its
/// arguments are the tokens.
fn command_tokens<'a>(path: &'a str, arguments: &'a [String]) -> Vec<&'a str> {
    if SHELLS.contains(&basename(path)) {
        if let Some(pos) = arguments.iter().position(|a| a == "-c") {
            if let Some(script) = arguments.get(pos + 1) {
                return script.split_whitespace().collect();
            }
        }
    }
    let mut tokens = Vec::with_capacity(arguments.len() + 1);
    tokens.push(path);
    tokens.extend(arguments.iter().map(String::as_str));
    tokens
}

/// The last path component of `s` (`/usr/bin/docker` → `docker`).
fn basename(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

/// The container operand of a `docker`/`podman` container subcommand, if any.
fn docker_container(tokens: &[&str]) -> Option<String> {
    for (i, tok) in tokens.iter().enumerate() {
        let base = basename(tok);
        if base != "docker" && base != "podman" {
            continue;
        }
        let Some(sub) = tokens.get(i + 1) else {
            continue;
        };
        if !CONTAINER_OPS.contains(sub) {
            continue;
        }
        let mut j = i + 2;
        while let Some(tok) = tokens.get(j) {
            if tok.starts_with('-') {
                if VALUE_FLAGS.contains(tok) {
                    j += 1; // skip the flag's value too
                }
                j += 1;
                continue;
            }
            let name = strip_quotes(tok);
            return (!name.is_empty()).then(|| name.to_string());
        }
    }
    None
}

/// The host of the first `http(s)://` (or other `scheme://`) URL token.
fn url_host(tokens: &[&str]) -> Option<String> {
    tokens.iter().find_map(|tok| {
        let cleaned = strip_quotes(tok);
        if cleaned.contains("://") {
            let host = normalize_service(cleaned);
            (!host.is_empty()).then_some(host)
        } else {
            None
        }
    })
}

/// The host operand of an `ssh` invocation, stripped of any `user@` prefix.
fn ssh_host(tokens: &[&str]) -> Option<String> {
    // ssh flags that take a following value.
    const SSH_VALUE_FLAGS: &[&str] = &["-p", "-i", "-l", "-o", "-F", "-c", "-b", "-J"];
    let start = tokens.iter().position(|t| basename(t) == "ssh")? + 1;
    let mut j = start;
    while let Some(tok) = tokens.get(j) {
        if tok.starts_with('-') {
            if SSH_VALUE_FLAGS.contains(tok) {
                j += 1;
            }
            j += 1;
            continue;
        }
        let operand = strip_quotes(tok);
        let host = operand.rsplit_once('@').map_or(operand, |(_, host)| host);
        let host = normalize_service(host);
        return (!host.is_empty()).then_some(host);
    }
    None
}

/// Strip a single pair of surrounding single or double quotes.
fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn native_extraction_strips_scheme_path_and_port() {
        assert_eq!(normalize_service("http://demo-app:8080/health"), "demo-app");
        assert_eq!(normalize_service("demo-app:8080"), "demo-app");
        assert_eq!(normalize_service("demo-app"), "demo-app");
        assert_eq!(normalize_service("10.0.0.1:5432"), "10.0.0.1");
        // A non-numeric suffix is kept (not a port).
        assert_eq!(normalize_service("cache:main"), "cache:main");
    }

    #[test]
    fn docker_exec_yields_container() {
        let a = args(&[
            "-c",
            "docker exec demo-postgres psql -U demo -d orders -c 'SELECT 1'",
        ]);
        assert_eq!(
            service_from_process("sh", &a),
            Some("demo-postgres".to_string())
        );
    }

    #[test]
    fn docker_pause_and_unpause_yield_container() {
        assert_eq!(
            service_from_process(
                "sh",
                &args(&["-c", "docker pause demo-postgres && echo paused"])
            ),
            Some("demo-postgres".to_string())
        );
        assert_eq!(
            service_from_process("sh", &args(&["-c", "docker unpause demo-postgres"])),
            Some("demo-postgres".to_string())
        );
    }

    #[test]
    fn docker_kill_with_signal_flag_skips_flag_value() {
        // `docker kill -s SIGSTOP demo-app` — the `-s SIGSTOP` value must be
        // skipped so the container operand (demo-app) is found.
        assert_eq!(
            service_from_process(
                "sh",
                &args(&["-c", "docker kill -s SIGSTOP demo-app && echo suspended"])
            ),
            Some("demo-app".to_string())
        );
    }

    #[test]
    fn curl_url_yields_host() {
        assert_eq!(
            service_from_process(
                "sh",
                &args(&["-c", "curl -sf -m 5 http://demo-app:8080/health"])
            ),
            Some("demo-app".to_string())
        );
    }

    #[test]
    fn ssh_host_is_stripped_of_user() {
        assert_eq!(
            service_from_process(
                "ssh",
                &args(&["-p", "22", "tumult@demo-sshd", "uname", "-a"])
            ),
            Some("demo-sshd".to_string())
        );
    }

    #[test]
    fn docker_run_is_not_guessed() {
        // `docker run` creates a new container rather than acting on a named
        // one; even when a name appears (pumba's last operand) we do not guess.
        let a = args(&[
            "-c",
            "docker run --rm -v /var/run/docker.sock:/var/run/docker.sock ghcr.io/alexei-led/pumba:latest stress demo-app",
        ]);
        assert_eq!(service_from_process("sh", &a), None);
    }

    #[test]
    fn plain_command_yields_nothing() {
        assert_eq!(
            service_from_process("sh", &args(&["-c", "echo hello"])),
            None
        );
        assert_eq!(service_from_process("true", &[]), None);
    }

    #[test]
    fn direct_docker_binary_without_shell_wrapper() {
        assert_eq!(
            service_from_process("docker", &args(&["exec", "demo-postgres", "pg_isready"])),
            Some("demo-postgres".to_string())
        );
    }
}
