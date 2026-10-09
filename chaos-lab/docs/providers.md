# Connect an AI tutor

Open **AI settings**, select a provider, and save its connection. The model field is prefilled and can be changed to a model available to your account.

| Provider           | Default model                  | What you provide                                                  |
| ------------------ | ------------------------------ | ----------------------------------------------------------------- |
| OpenAI             | `gpt-4.1-mini`                 | An OpenAI API key                                                 |
| Anthropic / Claude | `claude-sonnet-4-6`            | An Anthropic API key                                              |
| Ollama local       | `llama3.2:3b`                  | A running Ollama server with that model installed; usually no key |
| Ollama cloud       | Choose a supported cloud model | An Ollama API key and the endpoint override below                 |

Provider account setup is covered by the [OpenAI API quickstart](https://developers.openai.com/api/docs/quickstart) and [Claude API overview](https://platform.claude.com/docs/en/api/overview). Model availability and billing depend on your provider account. The defaults correspond to documented [GPT-4.1 mini](https://developers.openai.com/api/docs/models/gpt-4.1-mini) and [Claude Sonnet 4.6](https://platform.claude.com/docs/en/models/sonnet-4-6/overview) model identifiers.

The tutor receives your question and selected run evidence. It explains results and suggests follow-up questions. **It cannot execute commands or start experiments.** Only the explicit experiment controls change the sandbox. Treat generated explanations as suggestions to check against evidence.

## Local Ollama

Install and start Ollama separately, then download the default model:

```sh
ollama pull llama3.2:3b
```

The lab connects to `http://host.docker.internal:11434`. Compose maps this name to the host gateway. Ollama must listen on an address reachable from the container; a host service bound only to loopback may be unreachable, particularly on Linux. Configure an appropriate host interface and restrict access to the Docker bridge or trusted local machine. See the official [Ollama server configuration and networking FAQ](https://docs.ollama.com/faq).

Choose **Ollama · local model** in AI settings, leave the key empty, and save. Local Ollama requires its own model download and sufficient memory; those are additional to the lab containers. The lab does not install or start Ollama for you. [Ollama authentication documentation](https://docs.ollama.com/api/authentication) distinguishes local inference from direct cloud access.

## Ollama cloud or an endpoint override

From the repository’s `chaos-lab/` directory, copy `.env.example` to `.env` and set the server endpoint. Do not put API keys in this file.

```dotenv
OLLAMA_BASE_URL=https://ollama.com
```

Recreate the lab service so the environment changes take effect:

```sh
docker compose up -d --wait
```

Then choose Ollama in AI settings, replace the local default with a cloud-supported model, and enter your Ollama API key. Direct Ollama cloud requests require a key; the endpoint is an operator setting, not a URL supplied by the tutor. See [Ollama API authentication](https://docs.ollama.com/api/authentication).

[Back to the lab setup](../README.md).
