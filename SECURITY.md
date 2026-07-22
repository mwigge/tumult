# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 2.16.x  | :white_check_mark: |
| < 2.16  | :x:                |

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

- Workspace crates forbid unsafe code unless a narrowly scoped module documents
  and reviews the required invariant.
- **cargo-audit** and **cargo-deny** run in CI.
- **Clippy pedantic** enforced with `-D warnings`
- Production fallible paths return typed errors; panics are reserved for
  invariant violations and tests.
- **Null-byte validation** on all script plugin arguments
- **No hardcoded credentials** — secrets resolved from environment at runtime
- Full security assessment: [docs/security-assessment.md](docs/security-assessment.md)

## Dependency Management

Tumult tracks the [RustSec Advisory Database](https://rustsec.org/) via
`cargo-audit`. CI fails on known vulnerabilities. Unmaintained crate warnings
are reviewed and documented in the security assessment.
