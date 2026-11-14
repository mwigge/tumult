#!/usr/bin/env python3
"""Seed a day of experiment spans through the daemon's OTLP/HTTP endpoint.

One `resilience.experiment` span is what the `experiment_count` metric
counts. This writes back-dated spans, a few per minute on an hourly cycle,
so the forecast endpoint has a full context window with a season in it.
The protobuf is encoded by hand (standard library only), the way the
phase-4 proof does it. Prints the number of spans sent.

    seed_spans.py <daemon-url>
"""

import math
import os
import struct
import sys
import time
import urllib.request

MINUTE_NS = 60 * 1_000_000_000
MINUTES = 24 * 60 - 10
CHUNK_MINUTES = 240


def varint(n):
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        if n:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def field_bytes(number, payload):
    return varint((number << 3) | 2) + varint(len(payload)) + payload


def field_varint(number, value):
    return varint(number << 3) + varint(value)


def field_fixed64(number, value):
    return varint((number << 3) | 1) + struct.pack("<Q", value)


def key_value(key, value):
    any_value = field_bytes(1, value.encode())  # AnyValue.string_value
    return field_bytes(1, key.encode()) + field_bytes(2, any_value)


def runs_in_minute(minute):
    """Between 2 and 6 runs a minute, on a one-hour cycle."""
    return 4 + round(2 * math.sin(2 * math.pi * minute / 60))


def span(start_ns):
    return (
        field_bytes(1, os.urandom(16))  # trace_id
        + field_bytes(2, os.urandom(8))  # span_id
        + field_bytes(5, b"resilience.experiment")  # name
        + field_varint(6, 1)  # kind = INTERNAL
        + field_fixed64(7, start_ns)  # start_time_unix_nano
        + field_fixed64(8, start_ns + 1_000_000_000)  # end_time_unix_nano
        + field_bytes(9, key_value("resilience.experiment.title", "seeded history"))
    )


def request_body(spans):
    resource = b"".join(
        field_bytes(1, key_value(k, v))  # Resource.attributes
        for k, v in (
            ("service.name", "tumult"),
            ("resilience.target.system", "proof-service"),
            ("resilience.target.environment", "dev"),
        )
    )
    scope_spans = b"".join(field_bytes(2, s) for s in spans)  # ScopeSpans.spans
    resource_spans = field_bytes(1, resource) + field_bytes(2, scope_spans)
    return field_bytes(1, resource_spans)


def post(url, body):
    request = urllib.request.Request(
        f"{url}/v1/traces",
        data=body,
        headers={"content-type": "application/x-protobuf"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        if response.status != 200:
            raise SystemExit(f"OTLP/HTTP export answered {response.status}")


def main():
    url = sys.argv[1].rstrip("/")
    first = (time.time_ns() // MINUTE_NS - MINUTES - 3) * MINUTE_NS
    sent = 0
    for chunk in range(0, MINUTES, CHUNK_MINUTES):
        spans = []
        for minute in range(chunk, min(chunk + CHUNK_MINUTES, MINUTES)):
            count = runs_in_minute(minute)
            start = first + minute * MINUTE_NS
            spans.extend(
                span(start + i * MINUTE_NS // (count + 1)) for i in range(count)
            )
        post(url, request_body(spans))
        sent += len(spans)
    print(sent)


main()
