# Tumult Chaos Lab

Learn fault injection in your browser. Run four guided experiments with the real
Tumult engine, compare live measurements, and explore the results with an optional
AI tutor.

**You need Docker with Compose running.** Install Docker Desktop on macOS/Windows,
or Docker Engine with the Compose plugin on Linux. The first build needs internet
access. Python, Node, Rust, and an AI key are not required to use the lab.

## 1. Start the lab

Download and extract the ZIP or tar.gz bundle from the
[Chaos Lab 1.0.0 release](https://github.com/mwigge/tumult/releases/tag/chaos-lab-v1.0.0).
Open a terminal in the extracted `tumult-chaos-lab-1.0.0` folder.

Alternatively, clone Tumult:

```sh
git clone https://github.com/mwigge/tumult.git
cd tumult
```

From the **extracted bundle folder or Tumult repository root**, run:

```sh
cd chaos-lab
docker compose up -d --build --wait
```

Open **[http://localhost:8089](http://localhost:8089)**. Wait until the page says
**Tumult ready** and **Sandbox responding**. The version number also appears in
the readiness message. The first build takes a few minutes; subsequent starts
reuse the downloaded images.

Keep your terminal in `chaos-lab/` for the commands below. Linux/macOS users can
also start with `./start.sh`; Windows users can double-click `start.cmd`.

## 2. Run your first experiment

1. Select **Slow responses** and read its hypothesis.
2. Tick **I understand this changes only the bundled sandbox**.
3. Click **Run experiment**. Allow roughly 15–20 seconds.
4. Compare **Baseline**, **Fault active**, and **Recovery**. The fault adds about
   400 ms of delay; recovery should return near the baseline.
5. Read the verdict. **Deviated** is expected here: the fault broke the latency
   objective, even though HTTP requests still succeeded.

Use **Stop experiment** to cancel and wait for cleanup. The sandbox also expires
faults automatically. Try the other lessons: **HTTP failures**, **Unavailable
cache**, and **Database timeout**.

Open **Learning guide** in the page header for a deeper explanation. Each run
provides **Export evidence**, **Experiment definition**, and **Tumult journal**
downloads.

These lessons change a bundled teaching API. Cache and database failures are
simulated; HTTP responses and timings are measured live. The
[learning guide](docs/learning-guide.md) explains what the results do and do not
prove.

## 3. Add an AI tutor — optional

1. Click **AI settings**.
2. Choose **OpenAI** or **Claude** and paste your API key. A default model is
   already selected; change it only if your account requires another model.
3. Click **Save connection**, type a question, and click **Ask tutor**.

Provider charges depend on your account. Keys stay in server memory for your
browser session; use **Disconnect** in AI settings to remove a connection. The
tutor explains evidence and cannot start faults.

**Ollama:** local inference normally needs no key, but you must already have a
reachable Ollama server and an installed model. Follow the
[Ollama and provider setup guide](docs/providers.md) for local or cloud access.

## Stop or resume

From `chaos-lab/`:

```sh
docker compose down          # Stop; keep your saved runs.
docker compose up -d --wait  # Resume; enter your AI key again if you want tutoring.
```

## If something does not work

| Problem                        | Next step                                                                                                                 |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------- |
| Docker command fails           | Start Docker Desktop, or check that Docker Engine is running. Confirm `docker compose version` works.                     |
| Page does not open             | Run `docker compose ps`, then `docker compose logs --tail=100 lab target`. Wait for both services to be healthy.          |
| Port 8089 is already in use    | Copy `.env.example` to `.env`, set `LAB_PORT=8090`, run `docker compose up -d --wait`, then open `http://localhost:8090`. |
| Tutor rejects the key or model | Check your provider account, billing, and model access. The guided experiments still work without AI.                     |
| Ollama cannot connect          | Follow the [provider setup guide](docs/providers.md#local-ollama), including the container-to-host networking step.       |

## More information

- [Learning guide](docs/learning-guide.md) — mechanisms, measurements, and follow-up questions.
- [Provider setup](docs/providers.md) — OpenAI, Claude, local Ollama, and Ollama cloud.
- [Lab behavior and data](docs/reference.md) — saved evidence, privacy, settings, and deliberate data reset.
- [Development checks](docs/development.md) — change the code and run tests.
- [Acceptance and validation](docs/acceptance.md) — verified behavior and testing limits.

This is a shared local workstation lab. Run history is shared between browsers;
there are no customer accounts or tenant boundaries. See the [data reference](docs/reference.md#data-privacy-and-the-local-boundary).

## License

Apache-2.0, as part of Tumult. See [LICENSE](../LICENSE).
