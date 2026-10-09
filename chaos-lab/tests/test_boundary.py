"""Actual streaming limits work without trusting HTTP headers."""

import asyncio

import pytest

from agent.boundary import BodyLimit


@pytest.mark.asyncio
async def test_chunked_body_is_capped_before_downstream():
    messages = iter(
        [
            {"type": "http.request", "body": b"1234", "more_body": True},
            {"type": "http.request", "body": b"5678", "more_body": False},
        ]
    )
    sent = []
    reached = False

    async def receive():
        return next(messages)

    async def send(message):
        sent.append(message)

    async def downstream(*_args):
        nonlocal reached
        reached = True

    await BodyLimit(downstream, limit=7)(
        {"type": "http", "method": "POST"}, receive, send
    )
    assert not reached
    assert sent[0]["status"] == 413


@pytest.mark.asyncio
async def test_disconnected_body_never_calls_downstream():
    async def receive():
        return {"type": "http.disconnect"}

    async def unexpected(*_args):
        pytest.fail("Disconnected request forwarded or responded to")

    await BodyLimit(unexpected)({"type": "http", "method": "POST"}, receive, unexpected)


@pytest.mark.asyncio
async def test_empty_and_multi_chunk_requests_replayed_once():
    messages = iter(
        [
            {"type": "http.request", "body": b"12", "more_body": True},
            {"type": "http.request", "body": b"34", "more_body": False},
            {"type": "http.disconnect"},
        ]
    )

    async def receive():
        return next(messages)

    async def downstream(scope, buffered, send):
        assert await buffered() == {
            "type": "http.request",
            "body": b"1234",
            "more_body": False,
        }
        assert await buffered() == {"type": "http.disconnect"}

    async def send(_message):
        pass

    await BodyLimit(downstream)({"type": "http", "method": "POST"}, receive, send)


@pytest.mark.asyncio
async def test_slow_request_body_deadline(monkeypatch):
    timeout = asyncio.timeout
    monkeypatch.setattr("agent.boundary.asyncio.timeout", lambda _: timeout(0.001))
    sent = []

    async def receive():
        await asyncio.Event().wait()

    async def send(message):
        sent.append(message)

    async def downstream(*_args):
        pytest.fail("Incomplete request forwarded")

    await BodyLimit(downstream)({"type": "http", "method": "POST"}, receive, send)
    assert sent[0]["status"] == 408
