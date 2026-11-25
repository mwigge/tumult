# Docs screenshots

One command takes every Krönika picture of the docs again, and records
the terminal demo that the landing page and `/watch` play:

```bash
scripts/docs-shots/shoot.sh
```

The pictures land in `docs/images/3.0/screenshots/` and the recording in
`docs/demo/tumult-demo.cast`. Give another folder and file as the two
arguments to compare first.

## What you need

- A Linux host: the tumult CLI of the run comes out of the Linux image.
- Docker with compose.
- Node, with Playwright and its Chromium in `web/`
  (`npm ci` and `npx playwright install chromium` there).
- Python 3, curl, and [oha](https://github.com/hatoo/oha) on the `PATH`.
- About half an hour: the image build, the seed and twelve soak runs.

## What it does

1. Builds the daemon image from this checkout with
   `docker/Dockerfile.tumultd`, with at most 12 GB of memory and CPUs 0 to 7
   (`DOCS_SHOTS_BUILD_MEMORY`, `DOCS_SHOTS_BUILD_CPUS`), and starts it with
   `docker/docker-compose.kronika.yml` under a compose project of its own
   (`tumult-docs-shots`). The ports are 34317 and 34318 on 127.0.0.1, so the
   demo on 14317 and 14318 can stay up.
2. Makes a new admin password and new tokens for this run. They live in a
   temp work dir and go with it. Nothing secret is in the repository.
3. Runs the demo seed: eight experiments, three manual records, the
   approver `bob` and one GameDay campaign.
4. Runs `data.py`, which adds what the pictures show:
   - soak runs of four builds of a checkout service on 127.0.0.1, with a
     stored performance verdict after each new build;
   - two message-queue experiments, one accepted risk;
   - a definition bound to env `dev`, one run that waits for approval, and
     one run that `bob` approved and that is still running;
   - two schedules, two webhooks and a third user;
   - the HTML report of the last soak run;
   - three DR runs of `config-service` against a lab of files
     (`experiments/dr-lab/lab.sh`): one restores a backup and measures its
     RTO and RPO, one does not recover, and one sees no outage;
   - DR exercises run by hand, tagged with the plan scenarios they tested,
     a failover drill of `config-db` that fulfils its plan with steps,
     actions, a file and a sign-off, and a planned backup;
   - an executive digest, a DORA evidence pack and the two DR reports, with
     the HTML of the last three for the report pictures.

   The tumult CLI comes out of the image and runs with a HOME and a lake
   in the work dir. It never reads or writes your own lake.
5. Runs `shoot.mjs`, which signs in as `admin` and takes one picture per
   entry of `shots.json`: 1440 pixels wide at scale 2, light theme, or
   dark for an entry with `"theme": "dark"`. A picture with metadata in
   it fails the run.
6. Runs `cast.py`, which records a real terminal session against the same
   daemon: a soak run, the performance verdict, a gate and a report. Each
   line of output is what the command printed; only the typing is paced.
7. Takes the stack down with its volumes. The image stays, as a build
   cache: `docker image rm tumult-docs-shots:latest` removes it.

## Add a picture

Add an entry to `shots.json`: the file name, the route, the height, the
theme when it is dark, and the steps before the picture (`click`, `fill`,
`scrollTo`, `waitFor`, `wait`, `blur`). `{key}` in a route takes an id from
the `ids.json` that `data.py` writes. An entry with `file` instead of
`route` opens a file of the work dir, such as a report that `data.py` saved.
Then show the picture on a page: `scripts/check-images.py` fails on a
picture that no page names.

## Take one picture again

```bash
DOCS_SHOTS_KEEP=1 scripts/docs-shots/shoot.sh
# The last line names the work dir. Then:
node scripts/docs-shots/shoot.mjs <work dir> docs/images/3.0/screenshots runs-new
# When you are done:
scripts/docs-shots/shoot.sh --down
```

`DOCS_SHOTS_NO_BUILD=1` skips the image build when the image is there.
`DOCS_SHOTS_PROJECT` and the `DOCS_SHOTS_*_PORT` variables move the stack
when the names or ports are taken.
