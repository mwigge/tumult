# Demo proof suite

Every headline claim Tumult makes — ChaosGraph token efficiency, the first-class
MCP surface, agentic trajectory fault modelling — is checked here **against the
live demo**, with no mocks and no marketing numbers. Thresholds are set from the
*measured* behaviour, so the suite proves the property, not a specific figure.

## Run it

```bash
make demo            # bring the stack up first (once)
make demo-proof      # then validate every claim against it
```

Or directly (standard-library Python only):

```bash
python3 demo/proof/validate.py
# override the target if needed:
MCP_URL=http://host:3100 TUMULT_MCP_TOKEN=... python3 demo/proof/validate.py
```

It exits non-zero if any check fails.

## What it proves

**ChaosGraph token efficiency (5 checks).** The honest claim is *boundedness*, not
a flat multiplier:

- a targeted `chaosgraph_neighbors` answer is small (~110 tokens) **and**
- it stays that size no matter how many times the experiment runs, while reading
  journals grows by a full journal (~480 tokens) every run,
- so the graph is ~8× more compact per run and answers store-wide questions
  (~20× less than reading all journals),
- and the compact answer is *correct* — it names the real fault and service.

**MCP first-class.** `tools/list` returns the full tool set, each with annotations
and (for structured tools) an `outputSchema`; a tool round-trips with
`structuredContent`; a bad bearer token is rejected; a failing call sets
`isError`.

**Agentic trajectories.** Each bundled trajectory pack runs and its contract fires
as designed — grounding failure cascades to an unhealthy final step, a reflection
loop is detected, a tool-failure cascade recovers via its fallback.

The numbers printed are whatever your demo actually measures — run it and see.
