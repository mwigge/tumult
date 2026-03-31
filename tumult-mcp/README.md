# tumult-mcp

MCP (Model Context Protocol) server adapter exposing Tumult chaos engineering capabilities as tools for LLM agents.

## Key Types

- `TumultMcpServer` -- MCP server implementation
- `tumult_run_experiment` -- tool to execute experiments
- `tumult_read_journal` -- tool to read and query journals

## Usage

```bash
# Start the MCP server
tumult-mcp

# Or configure in your MCP client settings
{
  "mcpServers": {
    "tumult": { "command": "tumult-mcp" }
  }
}
```

## More Information

See the [main README](../README.md) for project overview and setup.
