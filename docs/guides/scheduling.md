---
title: Experiment Scheduling
parent: Guides
nav_order: 10
---

# Experiment Scheduling

Experiment scheduling is planned for a future release. Current approach: use cron/systemd timers to invoke `tumult run` on a schedule.

## Example: cron

```cron
# Run a chaos experiment every weekday at 10:00 UTC
0 10 * * 1-5 /usr/local/bin/tumult run /etc/tumult/experiments/api-latency.toon
```

## Example: systemd timer

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
