# Product acceptance and validation

This document defines what “working Tumult Chaos Lab” means. The target user is a developer or reliability learner on a local workstation. It is not a SaaS tenant-isolation or production fault-injection acceptance claim.

## Story 1: Run a guided experiment without AI credentials

As a developer learning chaos engineering, I want to run a contained experiment without configuring an AI provider so that I can learn from actual measurements immediately.

**Scenario: First successful lesson**

Given Docker Compose can build the lab and both application services are healthy,
when I acknowledge the sandbox scope and run a catalogue scenario,
then the lab executes the real Tumult binary against the bundled target,
and baseline, fault-window, and recovery observations appear with a verdict,
and evidence exports are available when their files exist.

**Scenario: Injection requires deliberate action**

Given a lesson is selected and the acknowledgement is unchecked,
when I inspect the experiment controls,
then Run experiment is disabled,
and an unarmed API request cannot start a run.

**Scenario: A second run is rejected**

Given an experiment is active,
when I attempt to start another experiment,
then the lab rejects it without starting a second fault,
and the original run remains observable and stoppable.

## Story 2: Understand the measured mechanism

As a reliability learner, I want separate baseline, fault, and recovery evidence so that I can explain a change rather than infer resilience from a successful process exit.

**Scenario: The fault exposes a weakness**

Given baseline and recovery meet the objective and fault-window samples violate it,
when I inspect the result,
then the lab verdict is deviated,
and the UI shows the recorded latency and error measurements for all available phases.

**Scenario: Baseline or recovery is unhealthy**

Given baseline or recovery fails the objective,
when the run is evaluated,
then the result is inconclusive,
and the explanation does not claim successful resilience or verified recovery.

**Scenario: Native execution and learning verdict differ**

Given the native experiment completes and restores steady state while the fault window violates the objective,
when I inspect both evidence records,
then the native completion and lab deviation remain separately represented,
and documentation explains the questions they answer.

## Story 3: Stop safely and keep evidence

As a developer running a local lab, I want to stop an experiment and retain its observations so that I can investigate interrupted work without leaving an indefinite fault.

**Scenario: Explicit stop**

Given an experiment is running,
when I select Stop experiment,
then cancellation and cleanup are requested,
and the final record describes the stopped or failed outcome without fabricated completed phases.

**Scenario: Restart after interruption**

Given an unfinished record exists from a previous process,
when the lab starts,
then it attempts to reset the target and marks that record interrupted,
and it preserves existing measurements,
and the interrupted run is not offered as actively stoppable in the UI.

**Scenario: Independent expiry**

Given a fault has been enabled and the lab process can no longer send cleanup,
when the target-enforced lease expires,
then the target no longer applies that fault,
and a configured lease cannot exceed the target's 15-second maximum.

**Scenario: Evidence persists across a normal shutdown**

Given completed evidence exists,
when I stop and restart with Compose without deleting volumes,
then saved runs remain available,
and API-provider connections are absent after the application process restarts.

## Story 4: Use a tutor without granting execution control

As a learner, I want to ask an AI provider about evidence using my own credentials so that I can explore unfamiliar concepts while retaining control of experiments.

**Scenario: Configured provider**

Given a valid provider connection is configured in this browser session,
when I submit a question,
then the backend sends the question and selected run context to that provider,
and the response is displayed as text,
and no experiment is started by the tutor response.

**Scenario: Missing or rejected provider credentials**

Given no provider connection exists or a provider rejects its credentials,
when I try to ask a question,
then the UI offers configuration or displays an actionable error,
and guided experiments remain available.

**Scenario: Local Ollama without a key**

Given a configured local Ollama endpoint is reachable and its chosen model is installed,
when I save an Ollama connection with an empty key,
then local inference requests can be sent without a fabricated API key,
and the tutor retains the same read-only behavior.

**Scenario: Credential separation**

Given one browser session has a saved provider connection,
when a different browser session opens the lab,
then the second session does not receive the first session's credentials,
and neither session receives keys in evidence exports or status responses.

This credential separation does not isolate run history: the sandbox and saved experiments are intentionally shared on the local workstation.

## Story 5: Access the learning flow on a small screen

As a learner using a laptop or narrow browser window, I want readable and operable controls so that I can complete the experiment flow without hidden actions or inaccessible output.

**Scenario: Responsive flow**

Given the viewport is 320, 768, 1024, or 1440 pixels wide,
when I navigate the lesson, settings, and results,
then the page does not overflow horizontally,
and labels, metrics, and action controls remain visible.

**Scenario: Untrusted tutor content**

Given a model response contains HTML or script-like text,
when the UI displays it,
then it appears as inert text,
and it cannot create HTML elements or execute browser code.

## Verification record — 2026-10-09

Validated on Linux amd64 using Docker Compose, the pinned Tumult 2.22.0 release, Python 3.12 in containers, Python 3.14 for the development suite, and headless Chromium.

| Validation                    | Result                                                                                                                                                                           |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Full Python suite             | **165 passed**, zero failed or skipped, including all four scenarios with the real Tumult binary                                                                                 |
| Agent/backend line coverage   | **99.44%** (713/717 statements); unchanged 95% gate; includes the activity helper                                                                                                |
| Browser contract              | Passed arm/run/stop, download links, key clearing, no browser credential storage, inert model HTML, interrupted history, guide navigation, and widths 320/768/1024/1440          |
| Real Compose/browser workflow | Passed provider settings without outbound model calls, actual latency run, all three downloads, active-fault stop, recovery, history and reload; no browser errors               |
| Measured latency example      | Baseline p95 **8.23 ms**, injected fault p95 **410.35 ms**, recovery p95 **8.10 ms**, eight observations per phase                                                               |
| Crash and independent expiry  | Killed only the lab application during an active latency fault; the separate target recovered without the application, **11.27 seconds** after the crash                         |
| Restart and persistence       | Completed evidence survived; unfinished run became interrupted/inconclusive without invented recovery; configured provider credentials were cleared                              |
| Build and startup             | Pinned binary download/checksum verification, automatic control-secret provisioning, repeated Compose startup and healthy non-root services passed                               |
| Quality checks                | Black, Ruff, mypy, Prettier, ESLint, shell syntax and Compose configuration checks passed                                                                                        |
| Python/npm dependency audits  | No known vulnerabilities reported for the lab's pinned Python runtime requirements or npm development lockfile                                                                   |
| Python security scan          | No unreviewed medium/high Bandit findings; scoped suppressions document the fixed HTTPS release download, shared control-directory traversal, and internal container listener    |
| Provider adapters             | OpenAI, Claude, local/cloud Ollama request/response, timeouts, invalid credentials, rate errors, redirect rejection and secret-redaction contracts tested with fixtures          |
| Live model inference          | **Not tested** with paid/customer credentials; no local Ollama server was running. A valid provider account or reachable installed local model is still required                 |
| Other host platforms          | Docker Desktop/macOS/Windows and Linux arm64 were not executed in this validation; architecture-specific release assets and the Docker setup support those Linux-container paths |

The screenshots and raw run downloads are generated under `test-results/` when live checks run. A compact copy of the measured browser/restart results is included in [validation-results.json](validation-results.json).

### Reproduce

Follow the development setup in the [development guide](development.md), then run:

```sh
./scripts/check.sh
.venv/bin/python tests/browser_live.py
.venv/bin/python tests/restart_live.py
```

Set `TUMULT_TEST_BIN` to a verified Linux Tumult binary before the full check. The browser contract uses fixtures; the two Python browser/restart scripts exercise the actual Compose service. The restart script intentionally kills only this lab's application container and preserves volumes.

The browser tools accept `PLAYWRIGHT_CHROMIUM_EXECUTABLE` for an existing Chromium binary. The fixture test additionally accepts `PLAYWRIGHT_MODULE` and optional `UI_SCREENSHOT`.

### Security assessment and limits

The fixed catalogue is enforced by the API and the runner; prompts cannot dispatch tools or alter targets. Runtime containers have no Docker socket, privileged mode or extra Linux capabilities. The fault target is on an internal network with no published port. Target changes require an automatically generated private token, reject unknown faults and unbounded durations, and expire independently. Child activities terminate when their Tumult parent exits; final status waits for cleanup and measured recovery.

The browser boundary rejects foreign hosts and cross-origin mutation, requires an existing session and explicit arming, limits actual request bytes, and uses a restrictive content-security policy. Keys are memory-only, scoped to a browser session, absent from status/evidence, and removed from echoed model output. Tutor calls have concurrency, rate, body, token and whole-request time limits. These controls were exercised in tests; this is not a penetration-test certification.

The bundled Tumult version retains the upstream quick-xml advisory exceptions described in the [v2.22.0 release notes](https://github.com/mwigge/tumult/releases/tag/v2.22.0). The lab's dependency-audit results do not mean the bundled Rust binary or base image is vulnerability-free.

This remains a shared local workstation sandbox. It has no customer accounts or tenant isolation. Faults model application responses and simulated dependencies; the results do not establish kernel/network behavior, real database resilience, statistical service objectives, or production readiness.
