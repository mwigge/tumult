# GameDays

A worked GameDay campaign against PostgreSQL, runnable end to end.

- `pg-*.toon` — the four fault experiments: connection kill, container
  pause, CPU stress, memory stress.
- `q2-postgres-resilience.gameday.toon` — the campaign definition: runs all
  four under shared k6 load and maps results to DORA/NIS2 requirements.
- `q2-postgres-resilience.gameday.journal.toon` — a recorded journal from a
  real campaign run, kept as a reference output.
- `expectations.json` says what the journals of a proof run must hold, for
  each experiment and for the campaign.

Run it with `tumult gameday run gamedays/q2-postgres-resilience.gameday.toon --force`
(CLI; `--force` replaces the recorded journal next to it) or from the
**GameDays** page of the web UI. The scripted demo in
`scripts/gameday-demo.sh` stands up the targets and runs the whole campaign.
