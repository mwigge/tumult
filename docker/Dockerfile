# Multi-stage build for kronikad + kronika-demo.
# The bundled DuckDB dependency needs a C toolchain — the official rust image
# ships one, so no extra packages are needed in the builder.

FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin kronikad --bin kronika-demo

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 kronika

COPY --from=builder /build/target/release/kronikad /usr/local/bin/kronikad
COPY --from=builder /build/target/release/kronika-demo /usr/local/bin/kronika-demo
# Semantic metric definitions used by `kronikad report`.
COPY metrics/ /etc/kronika/metrics/

ENV KRONIKA_DB=/data/kronika.duckdb \
    KRONIKA_METRICS_DIR=/etc/kronika/metrics \
    KRONIKA_OTLP_GRPC_ADDR=0.0.0.0:4317 \
    KRONIKA_OTLP_HTTP_ADDR=0.0.0.0:4318

RUN mkdir -p /data && chown kronika:kronika /data
VOLUME /data
USER kronika
EXPOSE 4317 4318

# Default: run the daemon. The demo's seed/report containers override the
# command (kronika-demo … / sh -c '… curl /report …').
CMD ["kronikad"]
