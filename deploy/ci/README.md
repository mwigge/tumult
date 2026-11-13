# CI gate templates

The gate turns a target's resilience evidence into a verdict a pipeline can
act on. Both templates call the daemon's `GET /api/gate/{target}` through
`tumult gate` and fail the job on a `fail` verdict (`--fail-on warn` also
fails on stale evidence or a failing experiment).

Prerequisites:

- A reachable `tumultd` (`TUMULT_DAEMON_URL`) and an API token with the
  `viewer` role (`TUMULT_DAEMON_TOKEN`); a token scoped to an environment
  can only gate that environment.
- The `tumult` binary on the runner (release archive or the published
  container image).

| File | Platform |
|---|---|
| `github-gate.yml` | GitHub Actions job to drop into a workflow |
| `gitlab-gate.yml` | GitLab CI job to `include:` |

The verdict rules are documented in `docs/guides/cli-reference.md` under
`tumult gate`.
