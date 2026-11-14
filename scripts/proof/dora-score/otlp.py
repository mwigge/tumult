#!/usr/bin/env python3
"""Write one OTLP ExportTraceServiceRequest (protobuf) to stdout: spans of
one service, the way the service's own SDK would export them. The daemon's
OTLP/HTTP endpoint takes protobuf, so the message is encoded here by hand
(standard library only) and posted with curl.

    otlp.py --service payments-api --days-ago 3 \
        --resource ict.entity.id=cif-0001 --attr http.route=/pay | \
        curl --data-binary @- -H 'content-type: application/x-protobuf' $URL/v1/traces

`--days-ago` backdates the span; `--num` adds a numeric span attribute;
`--count` repeats the span on that many consecutive earlier days.
"""

import argparse
import os
import struct
import sys
import time

DAY_NS = 86_400 * 1_000_000_000


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


def string_value(key, value):
    any_value = field_bytes(1, value.encode())  # AnyValue.string_value
    return field_bytes(1, key.encode()) + field_bytes(2, any_value)


def double_value(key, value):
    any_value = varint((4 << 3) | 1) + struct.pack("<d", value)  # AnyValue.double_value
    return field_bytes(1, key.encode()) + field_bytes(2, any_value)


def pairs(items):
    out = []
    for item in items:
        key, sep, value = item.partition("=")
        if not sep:
            sys.exit(f"otlp.py: expected key=value, got {item!r}")
        out.append((key, value))
    return out


def span(name, start_ns, attributes):
    return (
        field_bytes(1, os.urandom(16))  # trace_id
        + field_bytes(2, os.urandom(8))  # span_id
        + field_bytes(5, name.encode())  # name
        + field_varint(6, 2)  # kind = SERVER
        + field_fixed64(7, start_ns)  # start_time_unix_nano
        + field_fixed64(8, start_ns + 2_000_000)  # end_time_unix_nano
        + b"".join(field_bytes(9, kv) for kv in attributes)  # attributes
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--service", required=True)
    parser.add_argument("--name", default="GET /")
    parser.add_argument("--days-ago", type=float, default=0.0)
    parser.add_argument("--count", type=int, default=1)
    parser.add_argument("--resource", action="append", default=[])
    parser.add_argument("--attr", action="append", default=[])
    parser.add_argument("--num", action="append", default=[])
    args = parser.parse_args()

    resource = b"".join(
        field_bytes(1, string_value(k, v))  # Resource.attributes
        for k, v in [("service.name", args.service)] + pairs(args.resource)
    )
    attributes = [string_value(k, v) for k, v in pairs(args.attr)]
    attributes += [double_value(k, float(v)) for k, v in pairs(args.num)]
    first_ns = time.time_ns() - int(args.days_ago * DAY_NS)
    spans = b"".join(
        field_bytes(
            2, span(args.name, first_ns - day * DAY_NS, attributes)
        )  # ScopeSpans.spans
        for day in range(args.count)
    )
    resource_spans = field_bytes(1, resource) + field_bytes(2, spans)
    sys.stdout.buffer.write(field_bytes(1, resource_spans))


main()
