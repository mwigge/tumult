# Claude Agent Instructions — Tumult

**Version**: 1.0.0 | **Last Updated**: 2026-03-29

---

## Project Context

Tumult is a Rust-native chaos engineering platform. Fast, portable, observable.

| Crate | Purpose |
|-------|---------|
| `tumult-cli` | CLI binary — the `tumult` command |
| `tumult-core` | Engine: experiment runner, hypothesis eval, controls |
| `tumult-plugin` | Plugin trait, registry, manifest loader |
| `tumult-otel` | OpenTelemetry setup, span creation, OTLP export |
| `tumult-ssh` | SSH remote execution (russh) |
| `tumult-stress` | CPU/memory/IO stress (stress-ng) |
| `tumult-containers` | Docker/Podman chaos |
| `tumult-process` | Process kill/restart |
| `tumult-kubernetes` | K8s chaos (kube-rs) |
| `tumult-db` | Database chaos (PostgreSQL, MySQL, Redis) |
| `tumult-kafka` | Kafka broker chaos + JMX probes |
| `tumult-mcp` | MCP server adapter for AQE integration |
| `docs/` | Architecture, ADRs, guides, plugin docs |

---

## SDLC Way of Working

### Product Mindset

- Every feature answers **As a [role], I can [action] so that [value]** — if the value is not obvious, do not build it
- Scope is negotiated and bounded before coding starts
- Acceptance criteria are written before implementation, not after
- Prefer small, releasable increments over large batches

### Architecture First

- For any change touching more than two files or introducing a new abstraction: **design before code**
- Significant decisions get an ADR in `docs/adr/` — what was decided and why
- Follow 12-factor principles: config via env vars, stateless processes, explicit dependencies
- Security is designed in, not bolted on — apply `/security-review` before any auth, secrets, or input handling work
- Prefer explicit over implicit; simple over clever; composition over inheritance

### Development

**Before starting any feature — mandatory repo hygiene:**
```bash
git checkout main
git fetch origin
git pull --ff-only           # fast-forward only; if it fails, investigate before proceeding
git branch                   # confirm clean main with no leftover branches
git status                   # confirm working tree is clean
git checkout -b {type}/{description}   # create the feature branch
```

- **Branch per feature**: `{type}/{description}` — one feature, one branch, no exceptions
- **Branch scope**: keep it small and focused; if a branch grows beyond ~10 commits or ~400 lines changed, consider splitting
- **TDD**: write the failing test first, then the implementation — use `/tdd-workflow`
- **Commit messages**: Conventional Commits (see rules below) — describe *what is being built*, never the process
- **Before every commit**: run quality gates (see below)
- **Rust style**: `cargo fmt`, `cargo clippy`, all public functions documented
- Keep functions small and single-purpose; if you need a comment to explain what a block does, extract it

### Testing (Shift Left)

- Tests are written **before** implementation (TDD) or **alongside** it (never after)
- Coverage threshold: **≥ 90%** for all changed files
- Test pyramid: unit (fast, isolated) → integration (external calls mocked) → E2E (full pipeline)
- Mock at the boundary: patch real I/O, never internal implementation details
- Use `/verification-loop` after completing a feature, before opening a PR

### Code Review

- PRs are small and focused — a single feature, ideally < 400 lines changed
- Self-review (`git diff`) before requesting review
- Review turnaround target: < 24 hours

### Observability First

- Every new action or probe must emit an OTel span with `tumult.*` attributes
- Structured logging only — no `println!()` in library code, no credentials in log output
- Metrics follow the `tumult_<component>_<metric>_<unit>` naming convention

---

## Conventional Commits (MANDATORY)

All commits must follow these rules:

1. Prefix with type: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, etc. followed by `:` and a space
2. `feat` for new features; `fix` for bug fixes
3. Optional scope in parentheses: `fix(parser):`
4. Description immediately after prefix — short imperative summary
5. Body (optional) starts one blank line after description
6. Footers (optional) one blank line after body — `Token: value` format, use `-` not spaces in token
7. Breaking changes: `!` before `:` in prefix, OR `BREAKING CHANGE:` footer (both optional if `!` used)
8. `BREAKING CHANGE` token must be uppercase
9. `BREAKING-CHANGE` is synonymous with `BREAKING CHANGE` in footers

**Commit message describes the feature being built — NEVER mention "TDD", "red phase", "failing tests", or "add tests for".**

---

## Quality Gates (MANDATORY — all must pass before merge)

### Pre-Commit Checks

```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo doc --no-deps
```

### Gate Thresholds

| Check | Tool | Threshold | Blocks merge |
|-------|------|-----------|-------------|
| Formatting | cargo fmt | 0 issues | Yes |
| Linting | cargo clippy | 0 warnings (-D warnings) | Yes |
| Test pass rate | cargo test | 100% | Yes |
| Test coverage | cargo-tarpaulin | ≥ 90% | Yes |
| Security audit | cargo audit | 0 HIGH/CRITICAL | Yes |
| Documentation | cargo doc | 0 errors | Warning |

### Quick Fixes

```bash
cargo fmt                    # auto-format
cargo clippy --fix           # auto-fix lint warnings
cargo test                   # run all tests
cargo audit                  # check for CVEs
cargo tarpaulin --out Html   # coverage report
```

---

## Agent Roles

### CODER — Senior Rust Engineer
Implements features. Owns code quality and test coverage.
- Apply `/rust-patterns` for idioms; `/rust-agentic` for specialist guidance
- Run `/precommit-linter` before every commit
- All public functions have doc comments
- Ensure `cargo test` passes at 100% before pushing

### ARCHITECT — Product Owner + Technical Lead
Defines strategy, creates branches, approves merges.
- Only role authorised to approve PR merges
- Creates branches: `{type}/{description}`
- Defines acceptance criteria before CODER starts
- Reviews design for scalability, security, and alignment with roadmap
- Apply `/system-patterns` for architecture decisions

### TESTER — Senior Tester
Owns test strategy and coverage validation.
- Write failing tests first (TDD red phase), then confirm green after implementation
- Apply `/tdd-workflow` for Red-Green-Refactor discipline
- Verify ≥ 90% coverage on changed files before declaring done

### CODE REVIEWER — Rust Review Specialist
Owns code review quality and approval standards.
- Apply `/code-reviewer` for structured Rust review (ownership, lifetimes, unsafe, async)
- Severity labels: blocking, important, nit, suggestion, learning, praise
- Security checklist on every review
- Chaos-specific checks: rollback safety, blast radius, probe idempotency

### SECURITY ENGINEER — Application Security
Owns threat modelling, vulnerability analysis, and security gates.
- Apply `/security-engineer` for threat models, unsafe audits, dependency scanning
- Run `cargo audit` + `cargo deny` before every PR
- Review all SSH, credential, and input handling code
- OWASP mapping for CLI tools and remote execution

### DATABASE SPECIALIST — Data Layer Chaos
Owns database chaos actions, probes, and recovery validation.
- Apply `/database-postgresql`, `/database-redis`, `/database-kafka` per target system
- JMX probe design for Kafka
- Connection management and failover testing patterns
- Recovery validation and data integrity checks

### NETWORK ENGINEER — Network Chaos
Owns network fault injection, partition simulation, and connectivity probes.
- Apply `/network-engineering` for tc, iptables, DNS, TCP patterns
- SSH remote execution of network faults
- Guaranteed rollback of all network modifications
- Asymmetric partition and split-brain scenarios

### INDEXER — Documentation Specialist
Organises `docs/`, maintains ADRs, keeps cross-references valid.

### ORCHESTRATOR — Principal Engineer
Breaks complex tasks into sub-tasks, coordinates agents, enforces gate completion.

---

## Workflows

### New Feature
1. **ARCHITECT** — define AC, create branch, document scope
2. **CODER** — implement with tests; run quality checks
3. **TESTER** — write tests first (TDD); verify coverage
4. **INDEXER** — update docs, CHANGELOG.md, ADRs
5. **ARCHITECT** — verify all gates pass; approve merge

### Bug Fix
1. **TESTER** — write failing test reproducing the bug
2. **CODER** — fix with minimal changes; run quality checks
3. **TESTER** — confirm test now passes
4. **ARCHITECT** — approve merge

### Documentation Only
1. **INDEXER** — make changes
2. **ARCHITECT** — approve merge

---

## Security Rules (Non-Negotiable)

- **No hardcoded credentials** — require env vars, fail-fast if absent, never log connection strings
- **No secrets in git** — `.env` files are gitignored; use `.env.example` with placeholder values
- Apply `/security-review` for any auth, input handling, secrets, or external API work
- `cargo audit` HIGH = 0 is a hard gate

### CVE and Dependency Security (Non-Negotiable)

- **Run `cargo audit` before every PR** — zero HIGH or CRITICAL CVEs permitted
- **Pin minimum versions on security-sensitive crates** — when a CVE fix is released, update `Cargo.toml` immediately
- **Check CVE advisories before starting any feature that touches dependencies**
- **Resolve transitive CVEs** — never accept a CRITICAL as "transitive, not our problem"

---

## Skills Reference

Invoke these at the start of the relevant task:

### Rust Development
| Skill | Invoke | When |
|-------|--------|------|
| Rust idioms + 179 rules | `/rust-patterns` | Starting any Rust implementation |
| Rust specialist agents | `/rust-agentic` | Core impl, debugging, security, lint, style |
| System design patterns | `/system-patterns` | Builder, newtype, typestate, async, plugin architecture |

### Quality & Testing
| Skill | Invoke | When |
|-------|--------|------|
| TDD Red-Green-Refactor | `/tdd-workflow` | Any new feature or bug fix |
| Code review | `/code-reviewer` | Reviewing PRs — structured Rust review with severity labels |
| Pre-commit & linting | `/precommit-linter` | Before committing, after CI failures, formatting issues |

### Security
| Skill | Invoke | When |
|-------|--------|------|
| Security engineer | `/security-engineer` | Threat modelling, unsafe audit, dependency scanning, OWASP |

### Observability
| Skill | Invoke | When |
|-------|--------|------|
| Rust OpenTelemetry | `/rust-otel` | OTel SDK setup, span design, metrics, structured logging |
| OTel Collector config | `/otel-collector` | Collector pipelines, sampling, RED metrics |
| OTel semantic conventions | `/otel-semconv` | Attribute naming, span kind, resource vs span placement |

### Chaos Targets — Databases
| Skill | Invoke | When |
|-------|--------|------|
| PostgreSQL chaos | `/database-postgresql` | Connection kill, lock injection, replication, failover |
| Redis chaos | `/database-redis` | CLIENT KILL, memory pressure, sentinel, cluster chaos |
| Kafka chaos | `/database-kafka` | Broker kill, partition, JMX probes, consumer disruption |

### Chaos Targets — Infrastructure
| Skill | Invoke | When |
|-------|--------|------|
| Network engineering | `/network-engineering` | tc, iptables, DNS, TCP, partition simulation |

### OpenSpec
| Skill | Invoke | When |
|-------|--------|------|
| Propose a change | `/opsx:propose` | Starting a new feature or initiative |
| Explore ideas | `/opsx:explore` | Thinking through problems before implementing |
| Apply a change | `/opsx:apply` | Implementing tasks from a change |
| Archive a change | `/opsx:archive` | Finalising completed changes |

---

## Documentation Rules

- **README.md** must be updated with every feature that changes user-facing behaviour
- **docs/adr/** — new ADR for every significant architectural decision (numbered: ADR-NNN)
- **docs/guides/** — user-facing guides updated with each new capability
- **docs/plugins/** — per-plugin documentation updated when a plugin ships
- **docs/architecture/** — drawio diagrams updated when architecture changes
- **CHANGELOG.md** — updated with every releasable change (Keep a Changelog format)
- All documentation updates happen as part of the feature branch, not after

---

## Agent Rules Summary

1. **ALWAYS** run `cargo fmt && cargo clippy -- -D warnings && cargo test` before committing
2. **NEVER** commit with clippy warnings or failing tests
3. **NEVER** hardcode credentials — require env vars, fail-fast
4. **ALWAYS** write the failing test before the implementation (TDD)
5. **ALWAYS** maintain ≥ 90% coverage on changed files
6. **ALWAYS** add doc comments to all public functions and types
7. **NEVER** bypass quality gates
8. **ALWAYS** fetch main and confirm a clean working tree before creating a feature branch
9. **ALWAYS** create a clean branch per feature — `{type}/{description}`
10. **KEEP** branches small: one feature, one branch, ideally < 400 lines changed and < 10 commits
11. **ALWAYS** update CHANGELOG.md for every releasable change
12. **ALWAYS** update relevant docs (README.md, ADRs, guides) alongside code changes
13. **NEVER** use `println!()` in library crates — use `tracing` macros only
14. **ALWAYS** emit OTel spans for every action and probe execution
15. **ALWAYS** use `thiserror` for library errors, `anyhow` only in the CLI binary
16. **NEVER** use `unwrap()` or `expect()` in library code — propagate errors with `?`
17. **ALWAYS** derive `serde::Serialize` + `serde::Deserialize` on all data model types
18. **ALWAYS** test TOON round-trip serialization for new data types

---

## Reference

| Document | Path |
|----------|------|
| Architecture diagrams | `docs/architecture/tumult-architecture.drawio` |
| ADR index | `docs/adr/` |
| Plugin authoring guide | `docs/plugins/authoring-guide.md` |
| CLI reference | `docs/guides/cli-reference.md` |
| Experiment format | `docs/guides/experiment-format.md` |
| OpenSpec changes | `openspec/changes/` |

**License**: Apache 2.0 | **Acknowledgements**: See `NOTICE` file
