"""Provider contracts without paid requests or external networking."""

import asyncio
import json

import httpx
import pytest

from agent.providers import DEFAULT_MODELS, ProviderConfig, ProviderError, ask_provider


def request(config, response, *, question="Explain the latency", context=None):
    seen = []

    def handle(req):
        seen.append(req)
        if isinstance(response, Exception):
            raise response
        return response

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handle)) as client:
            return await ask_provider(
                config, question, context or {"latency_ms": 400}, client=client
            )

    return asyncio.run(run()), seen


@pytest.mark.parametrize("provider", ["openai", "anthropic", "claude", "ollama"])
def test_provider_payload_and_answer(provider, monkeypatch):
    monkeypatch.delenv("OLLAMA_BASE_URL", raising=False)
    payloads = {
        "openai": {
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "Measured delay."}],
                }
            ]
        },
        "anthropic": {"content": [{"type": "text", "text": "Measured delay."}]},
        "claude": {"content": [{"type": "text", "text": "Measured delay."}]},
        "ollama": {"message": {"content": "Measured delay."}},
    }
    answer, seen = request(
        ProviderConfig(provider, "test-model", "secret-key"),
        httpx.Response(200, json=payloads[provider]),
    )
    assert answer == "Measured delay."
    req = seen[0]
    body = json.loads(req.content)
    assert body["model"] == "test-model"
    assert "tools" not in body
    assert "secret-key" not in req.content.decode()
    assert "400" in req.content.decode()
    assert "measured" in req.content.decode().lower()
    assert req.extensions["timeout"]["read"] <= 90
    if provider == "openai":
        assert str(req.url) == "https://api.openai.com/v1/responses"
        assert body["store"] is False
        assert body["max_output_tokens"] == 1800
        assert req.headers["authorization"] == "Bearer secret-key"
    elif provider in {"anthropic", "claude"}:
        assert str(req.url) == "https://api.anthropic.com/v1/messages"
        assert body["max_tokens"] == 1800
        assert req.headers["x-api-key"] == "secret-key"
        assert req.headers["anthropic-version"] == "2023-06-01"
    else:
        assert str(req.url) == "http://host.docker.internal:11434/api/chat"
        assert body["stream"] is False
        assert body["options"]["num_predict"] == 1800


def test_local_ollama_needs_no_key(monkeypatch):
    monkeypatch.setenv("OLLAMA_BASE_URL", "http://127.0.0.1:11434")
    _, seen = request(
        ProviderConfig("ollama", DEFAULT_MODELS["ollama"]),
        httpx.Response(200, json={"message": {"content": "Hi"}}),
    )
    assert "authorization" not in seen[0].headers


def test_cloud_ollama_uses_key(monkeypatch):
    monkeypatch.setenv("OLLAMA_BASE_URL", "https://ollama.com/")
    _, seen = request(
        ProviderConfig("ollama", "gpt-oss:120b", "cloud-key"),
        httpx.Response(200, json={"message": {"content": "Hi"}}),
    )
    assert str(seen[0].url) == "https://ollama.com/api/chat"
    assert seen[0].headers["authorization"] == "Bearer cloud-key"


@pytest.mark.parametrize(
    "url",
    [
        "https://user:secret@ollama.com",
        "http://ollama.com",
        "https://evil.example",
        "file:///tmp/key",
        "http://localhost:11434/api",
        "http://localhost:11434?key=secret",
        "http://localhost:11434#secret",
        "http://169.254.169.254",
        "http://127.0.0.1:bad",
    ],
)
def test_unsafe_ollama_configuration_rejected(url, monkeypatch):
    monkeypatch.setenv("OLLAMA_BASE_URL", url)
    with pytest.raises(ProviderError, match="Ollama") as error:
        request(
            ProviderConfig("ollama", "model", "secret-key"),
            httpx.Response(200, json={}),
        )
    assert url not in str(error.value)


def test_cloud_ollama_requires_key(monkeypatch):
    monkeypatch.setenv("OLLAMA_BASE_URL", "https://ollama.com")
    with pytest.raises(ProviderError, match="key"):
        request(ProviderConfig("ollama", "model"), httpx.Response(200, json={}))


@pytest.mark.parametrize(
    "config",
    [
        ProviderConfig("bad", "model"),
        ProviderConfig("openai", "model"),
        ProviderConfig("anthropic", "model"),
        ProviderConfig("openai", "bad\nmodel", "key"),
        ProviderConfig("openai", "x" * 201, "key"),
        ProviderConfig("openai", "model", "bad\nkey"),
        ProviderConfig("openai", "model", "x" * 4097),
    ],
)
def test_invalid_input_fails_before_network(config):
    with pytest.raises(ProviderError):
        request(config, httpx.Response(200, json={}))


@pytest.mark.parametrize("status", [400, 401, 403, 404, 429, 500, 503, 302])
def test_upstream_errors_are_safe(status):
    with pytest.raises(ProviderError) as error:
        request(
            ProviderConfig("openai", "model", "secret-key"),
            httpx.Response(
                status,
                text="secret-key confidential upstream details",
                headers={"location": "https://evil.example"},
            ),
        )
    assert "secret-key" not in str(error.value)
    assert "confidential" not in str(error.value)
    assert error.value.status_code is not None


@pytest.mark.parametrize(
    "failure", [httpx.ConnectError("secret-key"), httpx.ReadTimeout("secret-key")]
)
def test_network_errors_are_safe(failure):
    with pytest.raises(ProviderError) as error:
        request(ProviderConfig("openai", "model", "secret-key"), failure)
    assert "secret-key" not in str(error.value)


@pytest.mark.parametrize(
    "payload",
    [
        {},
        [],
        {"output": None},
        {"output": [None]},
        {"output": [{"content": None}]},
        {"output": [{"content": [{"text": "", "type": "output_text"}]}]},
    ],
)
def test_invalid_or_empty_provider_payload_is_safe(payload):
    with pytest.raises(ProviderError):
        request(
            ProviderConfig("openai", "model", "key"), httpx.Response(200, json=payload)
        )


def test_invalid_json_is_safe():
    with pytest.raises(ProviderError) as error:
        request(
            ProviderConfig("openai", "model", "key"),
            httpx.Response(200, text="confidential"),
        )
    assert "confidential" not in str(error.value)


@pytest.mark.parametrize("question", ["", "   ", "x" * 4001])
def test_question_is_bounded(question):
    with pytest.raises(ProviderError):
        request(
            ProviderConfig("openai", "model", "key"),
            httpx.Response(200, json={}),
            question=question,
        )


def test_context_is_bounded():
    with pytest.raises(ProviderError):
        request(
            ProviderConfig("openai", "model", "key"),
            httpx.Response(200, json={}),
            context={"data": "x" * 48001},
        )


def test_config_repr_never_contains_key():
    assert "secret-key" not in repr(ProviderConfig("openai", "model", "secret-key"))


def test_invalid_context_is_safe():
    with pytest.raises(ProviderError, match="context"):
        request(
            ProviderConfig("openai", "model", "key"),
            httpx.Response(200, json={}),
            context={"bad": float("nan")},
        )


def test_response_bytes_are_bounded():
    with pytest.raises(ProviderError, match="size limit"):
        request(
            ProviderConfig("openai", "model", "key"),
            httpx.Response(200, text="x" * 256001),
        )


def test_owned_client_is_closed_and_ignores_environment_proxy(monkeypatch):
    real_client = httpx.AsyncClient
    clients = []

    def factory(**kwargs):
        assert kwargs == {"trust_env": False}
        client = real_client(
            transport=httpx.MockTransport(
                lambda _: httpx.Response(
                    200, json={"content": [{"type": "text", "text": "Hi"}]}
                )
            )
        )
        clients.append(client)
        return client

    monkeypatch.setattr(httpx, "AsyncClient", factory)
    result = asyncio.run(
        ask_provider(ProviderConfig("anthropic", "model", "key"), "why", {})
    )
    assert result == "Hi"
    assert clients[0].is_closed


@pytest.mark.parametrize(
    "provider,payload",
    [
        (
            "anthropic",
            {"content": [{"type": "tool_use", "input": {"command": "secret"}}]},
        ),
        ("ollama", {"message": {"content": 5}}),
    ],
)
def test_non_text_responses_are_not_executed(provider, payload, monkeypatch):
    monkeypatch.delenv("OLLAMA_BASE_URL", raising=False)
    with pytest.raises(ProviderError, match="no text"):
        request(
            ProviderConfig(provider, "model", "key"), httpx.Response(200, json=payload)
        )


def test_provider_echoed_credential_is_redacted():
    answer, _ = request(
        ProviderConfig("anthropic", "model", "sk-private-test"),
        httpx.Response(
            200,
            json={
                "content": [
                    {"type": "text", "text": "Upstream echoed sk-private-test."}
                ]
            },
        ),
    )
    assert "sk-private-test" not in answer
    assert "[redacted]" in answer


def test_total_provider_deadline_bounds_stalled_transport(monkeypatch):
    monkeypatch.setattr("agent.providers.REQUEST_TIMEOUT_SECONDS", 0.02, raising=False)

    async def stalled(_request):
        await asyncio.sleep(60)
        return httpx.Response(200, json={})

    async def run():
        async with httpx.AsyncClient(transport=httpx.MockTransport(stalled)) as client:
            with pytest.raises(ProviderError, match="timed out") as error:
                await asyncio.wait_for(
                    ask_provider(
                        ProviderConfig("openai", "model", "key"),
                        "why",
                        {},
                        client=client,
                    ),
                    timeout=1,
                )
            assert error.value.status_code == 504

    asyncio.run(run())
