---
title: Quality and release checks
parent: Guides
---

# Quality and release checks

Tumult uses language-specific checks. Rust uses rustfmt and Clippy (including pedantic warnings); the Svelte/TypeScript UI uses Prettier, ESLint, Svelte check and Playwright; Python scripts use Black and Ruff; the PostgreSQL initialization fixture uses SQLFluff's PostgreSQL dialect. SQL embedded in Rust is verified through database tests and security review, not passed through a text formatter that cannot interpret Rust expressions.

The imported analytics crates retain their existing crate-level exception for Clippy's pedantic group; ordinary Clippy warnings still fail CI. This release adds no blanket Rust lint exceptions.

Install Python checks in an isolated environment with `python3 -m venv .quality-venv` and `.quality-venv/bin/pip install -r requirements-quality.txt`. Web development requires Node 22.17 or later; install pinned dependencies with `npm --prefix web ci`.

Run these gates from the repository root:

```sh
# Build the UI first: tumultd embeds web/build during Rust compilation.
npm --prefix web run format:check
npm --prefix web run lint
npm --prefix web run check
npm --prefix web run build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings -W clippy::pedantic
cargo test --workspace --all-features --locked
cargo doc --workspace --no-deps --all-features --locked
cargo machete --with-metadata
cargo deny check
npm --prefix web audit --audit-level=moderate
npm --prefix web exec -- playwright install chromium
npm --prefix web run test:e2e
.quality-venv/bin/black --check scripts demo/proof
.quality-venv/bin/ruff check scripts demo/proof
.quality-venv/bin/sqlfluff lint docker/init-postgres.sql
python3 scripts/check-docs.py
python3 scripts/test-distribution.py
bash -n scripts/smoke-daemon-image.sh
python3 scripts/test-smoke-daemon-image.py
python3 scripts/test-smoke-daemon-binary.py
```

On machines with many logical CPUs, set `CARGO_BUILD_JOBS=4` and pass `-- --test-threads=2` to Cargo tests to bound linker and DuckDB concurrency. Browser tests use one worker and the production static build. CI also enforces workspace coverage on main; its 90% floor is unchanged. A passing unit suite is not a coverage measurement or proof of remote-provider support.

Review changes for duplicated business rules before extracting utilities. Shared provider dispatch, scope predicates, execution preparation, checksum verification and journal publication should have one implementation. Domain constants (protocol limits, bounded default timeouts) belong near their owning logic; credentials and deployment destinations belong in operator configuration. Test fixtures may use explicit synthetic identities and harmless marker actions.

Security checks combine dependency advisories with manual authorization, injection, credential, filesystem and fault-cleanup review. `deny.toml` records narrowly scoped pre-existing transitive advisory exceptions. Raw `cargo audit` output can therefore differ from the policy gate; release evidence must disclose that difference and recheck upstream availability rather than calling an exception-free scan successful.

DuckDB JSON and Parquet are compiled into the binaries. Offline regressions disable extension installation/loading and use an empty extension directory so a developer cache cannot hide missing dependencies. Host-compatible release jobs start the actual daemon binary with isolated credentials and storage, then verify authenticated readiness and persistence before packaging. Cross-target jobs still require matching-hardware runtime acceptance.

Published releases are built from the merged version tag. Release notes must document execution-binding, archive and retention migrations. The release workflow builds platform archives (including the native TCP proxy helper), checksums and the CLI/MCP/daemon containers. Bounded checks invoke `--version`, the CLI image's `mcp serve --help`, and the MCP image's own entrypoint with deployment arguments and `--help`. These validate executable packaging without starting listeners or deploying Kubernetes. The daemon image additionally runs authenticated readiness checks with a read-only root, writable temporary storage and a persistent database volume; a fresh container must reuse its stored identity without bootstrap provisioning. These checks do not execute faults or certify a Kubernetes deployment. A tag or successful source test does not alone establish that release assets were published or that deployed experiments work.

Record workflow results and artifact identities after publication, then test
the exact image and deployment configuration against a disposable target.
The [2.22.0 hardening review](hardening-review.md) maps the original findings to
regression coverage and lists behavior that remains outside this release.
