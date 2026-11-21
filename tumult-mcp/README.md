# tumult-mcp

The MCP server of Tumult. It gives an AI agent Tumult as tools: run an
experiment, read a journal, query the lake and more. The
[MCP guide](../docs/guides/mcp-guide.md) lists the tools and how to
connect a client.

## Start it

```bash
tumult-mcp                                   # stdio, for a local client
tumult-mcp --transport http --port 3100      # Streamable HTTP
```

`tumult mcp serve` in the CLI does the same. In a client config:

```json
{
  "mcpServers": {
    "tumult": { "command": "tumult-mcp" }
  }
}
```

## Key types

- `TumultHandler`: the MCP server
- `tumult_run_experiment`: the tool that runs an experiment
- `tumult_read_journal`: the tool that reads a journal
