# Plugin Manifest Specification

The plugin manifest declares a script plugin's identity and capabilities. It is a JSON file named `plugin.json` in the plugin's root directory.

## Fields

### Top-Level

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Plugin identifier (e.g., `tumult-kafka`). Must be unique across discovered plugins. |
| `version` | string | Yes | Semantic version (e.g., `0.1.0`) |
| `description` | string | Yes | One-line description of the plugin |
| `actions` | array of ScriptAction | Yes | Available chaos actions (may be empty) |
| `probes` | array of ScriptProbe | Yes | Available probes (may be empty) |

### ScriptAction

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Action identifier (e.g., `kill-broker`) |
| `script` | string (path) | Yes | Relative path to the script from plugin root |
| `description` | string | Yes | One-line description of what the action does |

### ScriptProbe

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Probe identifier (e.g., `consumer-lag`) |
| `script` | string (path) | Yes | Relative path to the script from plugin root |
| `description` | string | Yes | One-line description of what the probe measures |

## Environment Variable Convention

When executing a script, Tumult passes experiment arguments as environment variables:

- Prefix: `TUMULT_`
- Key transformation: argument name is uppercased
- Example: argument `broker_id` becomes `TUMULT_BROKER_ID`

All values are passed as strings. Scripts are responsible for type conversion.

## Example Manifest

```json
{
  "name": "tumult-kafka",
  "version": "0.2.0",
  "description": "Kafka chaos actions and probes",
  "actions": [
    {
      "name": "kill-broker",
      "script": "actions/kill-broker.sh",
      "description": "Kill a Kafka broker process via SSH"
    },
    {
      "name": "partition-topic",
      "script": "actions/partition-topic.sh",
      "description": "Create network partition for a topic"
    },
    {
      "name": "corrupt-message",
      "script": "actions/corrupt-message.sh",
      "description": "Inject corrupt messages into a topic"
    }
  ],
  "probes": [
    {
      "name": "consumer-lag",
      "script": "probes/consumer-lag.sh",
      "description": "Check consumer group lag via kafka-consumer-groups"
    },
    {
      "name": "broker-health",
      "script": "probes/broker-health.sh",
      "description": "Verify broker cluster health via JMX"
    }
  ]
}
```

## Naming Conventions

- Plugin name: `tumult-<technology>` (e.g., `tumult-kafka`, `tumult-redis`, `tumult-nginx`)
- Action names: imperative verb + noun (e.g., `kill-broker`, `drain-node`, `flush-cache`)
- Probe names: noun or measurement (e.g., `consumer-lag`, `connection-count`, `broker-health`)
- Script paths: relative to plugin root, organized in `actions/` and `probes/` subdirectories
