# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 1.0.x   | :white_check_mark: |
| < 1.0.0 | :x:                |

## Reporting a Vulnerability

If you discover a security vulnerability in Tumult, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

Please use [GitHub's private vulnerability reporting](https://github.com/mwigge/tumult/security/advisories/new) to report security issues.

### What to include

- Description of the vulnerability
- Steps to reproduce
- Affected component (crate name, plugin, Docker config)
- Potential impact assessment
- Suggested fix (if any)

### Response timeline

- **Acknowledgement:** within 48 hours
- **Initial assessment:** within 7 days
- **Fix or mitigation:** depends on severity (critical: 72 hours, high: 14 days, medium: 30 days)

## Security Practices

- **Zero unsafe code** in all Tumult crates
- **cargo-audit** runs on every commit via pre-commit hook and CI
- **Clippy pedantic** enforced with `-D warnings`
- **No `.unwrap()` in production code** — enforced by code review
- **Null-byte validation** on all script plugin arguments
- **No hardcoded credentials** — secrets resolved from environment at runtime
- Full security assessment: [docs/security-assessment.md](docs/security-assessment.md)

## Dependency Management

Tumult tracks the [RustSec Advisory Database](https://rustsec.org/) via `cargo-audit`. The CI pipeline fails on any HIGH or CRITICAL advisory. Unmaintained crate warnings are tracked and documented in the security assessment.
