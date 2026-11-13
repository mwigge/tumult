#!/usr/bin/env python3
"""Write one OTLP ExportTraceServiceRequest (protobuf) to stdout: a single
server span from a service, the way the service's own SDK would export it.
The daemon's OTLP/HTTP endpoint takes protobuf, so the message is encoded
here by hand (standard library only) and posted with curl. target.py
imports encode() to report its own spans.

    otlp_span.py <service.name> <service.version> <environment> [<attribute>] | \
        curl --data-binary @- -H 'content-type: application/x-protobuf' $URL/v1/traces

The environment goes on the resource under <attribute> (default
resilience.target.environment; deployment.environment.name and
deployment.environment are the other two the daemon reads). An
environment of "-" leaves the resource untagged.
"""

import os
import struct
import sys
import time


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


def encode(service, version, environment, attribute="resilience.target.environment"):
    """One ExportTraceServiceRequest holding a single server span that
    ended now."""
    attributes = [
        ("service.name", service),
        ("service.version", version),
        ("resilience.target.system", service),
    ]
    if environment != "-":
        attributes.append((attribute, environment))
    resource = b"".join(
        field_bytes(1, key_value(k, v)) for k, v in attributes  # Resource.attributes
    )
    now = time.time_ns()
    span = (
        field_bytes(1, os.urandom(16))  # trace_id
        + field_bytes(2, os.urandom(8))  # span_id
        + field_bytes(5, b"GET /")  # name
        + field_varint(6, 2)  # kind = SERVER
        + field_fixed64(7, now - 2_000_000)  # start_time_unix_nano
        + field_fixed64(8, now)  # end_time_unix_nano
    )
    scope_spans = field_bytes(2, span)  # ScopeSpans.spans
    resource_spans = field_bytes(1, resource) + field_bytes(2, scope_spans)
    return field_bytes(1, resource_spans)


if __name__ == "__main__":
    sys.stdout.buffer.write(encode(*sys.argv[1:5]))
