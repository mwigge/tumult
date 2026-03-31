# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | Yes                |

## Reporting a Vulnerability

If you discover a security vulnerability in Tumult, please report it
responsibly.

1. **Do not open a public GitHub issue.**
2. Send a detailed report to the repository maintainers via the
   [GitHub Security Advisories](https://github.com/mwigge/tumult/security/advisories)
   feature (preferred) or via email to the address listed in the repository.
3. Include steps to reproduce, affected versions, and potential impact.

We aim to acknowledge reports within 48 hours and to provide a fix or
mitigation within 14 days for confirmed vulnerabilities.

## Scope

The following are in scope for security reports:

- Command injection or argument injection in plugin execution
- Secret leakage (credentials, tokens) in logs, telemetry, or journal output
- Path traversal in experiment file loading
- Unsafe deserialization of experiment definitions
- Dependency vulnerabilities (tracked via `cargo audit` in CI)

## Security Practices

- All dependencies are audited in CI via `rustsec/audit-check`.
- Automatic dependency updates are managed via Dependabot.
- DuckDB analytics stores use `0o700` directory permissions on Unix.
- SSH passphrases are redacted from `Debug` output.
- Script plugin arguments are validated for null-byte injection.
