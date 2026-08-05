---
title: Experiment Scheduling
parent: Guides
nav_order: 19
---

# Experiment Scheduling

The `tumultd` daemon runs registered experiment definitions on a fixed
interval. Schedules are managed from the web UI (**Schedules** page,
operator role) or the JSON API, and the daemon fires them in the
background.

## How it works

- A schedule points at a definition in the registry (`registry_id`) and
  fires every `interval_s` seconds. The first fire is one interval after
  creation.
- The interval must be between 60 seconds and 30 days — below 60s the
  scheduler's tick cannot keep up meaningfully.
- The daemon's schedule scheduler ticks every `TUMULTD_SCHEDULE_TICK_S`
  seconds (default 30, minimum 1; invalid values fall back to the default)
  and fires every due, enabled schedule.
- Scheduled runs go through the **normal run path** — the same tier
  classification and approval gating as `POST /api/runs` — so a scheduled
  production-tier run still parks for approval.
- Fired runs are recorded with actor `schedule:<name>` and appear in the
  audit trail like any other run.
- Missed fires collapse: after daemon downtime a schedule fires exactly once
  on the first tick back, never a piled-up backlog. A full run queue retries
  the fire next tick; a broken definition advances the schedule anyway so it
  does not error every tick.

## API

- `GET /api/schedules` — list every schedule, with its definition name.
- `POST /api/schedules` — create an enabled schedule:

  ```json
  {
    "name": "hourly-latency",
    "registry_id": "<registry id>",
    "interval_s": 3600,
    "vars": {"duration_s": "30"},
    "env": "dev",
    "target": "optional-target"
  }
  ```

  `vars`, `env` (default `dev`) and `target` are optional. Creation
  validates the interval bounds and that the definition resolves with the
  supplied variables — a bad schedule fails fast with 400 instead of
  erroring every tick.
- `POST /api/schedules/{id}/enable` with `{"enabled": false}` — disable (or
  re-enable) a schedule.
- `POST /api/schedules/{id}/delete` — remove a schedule. Runs it already
  fired are untouched.

## Web UI

The **Schedules** page lists every schedule with its interval, next and
last run, environment and status, and offers create, enable/disable and
delete.

## CLI-only alternative

For a CLI-only setup without the daemon, cron or a systemd timer invoking
`tumult run` still works:

```cron
# Run a chaos experiment every weekday at 10:00 UTC
0 10 * * 1-5 /usr/local/bin/tumult run /etc/tumult/experiments/api-latency.toon
```

```ini
# /etc/systemd/system/tumult-experiment.timer
[Unit]
Description=Run Tumult experiment on schedule

[Timer]
OnCalendar=Mon..Fri 10:00 UTC
Persistent=true

[Install]
WantedBy=timers.target
```

```ini
# /etc/systemd/system/tumult-experiment.service
[Unit]
Description=Tumult chaos experiment

[Service]
Type=oneshot
ExecStart=/usr/local/bin/tumult run /etc/tumult/experiments/api-latency.toon
```
