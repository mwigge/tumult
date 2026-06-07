# Agentic Examples

This directory contains small, provider-free fixtures for the `tumult-agentic`
fault-injection module. They are intended for smoke tests, examples, and replay
regression development.

Run the deterministic smoke path from the repository root:

```bash
tumult agentic smoke
```

The smoke fixture injects malformed JSON and expects the `valid_json` contract
to fail. That failure is the success signal because it proves the injected fault
and contract feedback loop are working.

Files:

- `malformed-json-recovery.fixture.json` - minimal replay-style fixture for a
  malformed structured response
- `fake-http-malformed-json.toon` - compact TOON example for the local fake HTTP
  smoke path
- `replay-missing-output-ref.json` - intentionally incomplete replay fixture for
  validation tests
