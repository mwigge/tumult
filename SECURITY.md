# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 2.20.x  | :white_check_mark: |
| 2.19.x  | :white_check_mark: |
| < 2.19  | :x:                |

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
- **`cargo deny check advisories`** runs in CI (see below).
- **Clippy pedantic** enforced with `-D warnings`
- Production fallible paths return typed errors; panics are reserved for
  invariant violations and tests.
- **Null-byte validation** on all script plugin arguments
- **No hardcoded credentials** — secrets resolved from environment at runtime
- Full security assessment: [docs/security-assessment.md](docs/security-assessment.md)

## Dependency Management

Tumult tracks the [RustSec Advisory Database](https://rustsec.org/) via
`cargo deny check advisories`, which runs in CI. Exceptions to specific
advisories live in `deny.toml`, each with a written justification (typically:
the vulnerable crate is reachable only through a transitive dependency, no
upstream fix has been released, and the vulnerable code path is never
exercised with untrusted input). Unmaintained-crate warnings are reviewed and
documented in the security assessment.

## Deployment Security

- **TLS.** The Krönika daemon (`tumultd`) can serve TLS directly when
  `KRONIKA_TLS_CERT` / `KRONIKA_TLS_KEY` are set (support being added);
  otherwise deploy it — and the MCP server — behind a TLS-terminating reverse
  proxy (nginx/Caddy/Ingress). Never expose the plain-HTTP listeners to an
  untrusted network.
- **Non-loopback listeners require auth tokens.** Both the MCP server and
  `tumultd` fail closed: they refuse to bind a non-loopback address without
  configured authentication. Issue a distinct token per principal and rotate
  on a schedule.
- See [docs/guides/production-deployment.md](docs/guides/production-deployment.md)
  for the full hardening checklist.
