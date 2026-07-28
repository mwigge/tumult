# Multi-stage build for kronikad + kronika-demo + the pinned tumult CLI.
# The bundled DuckDB dependency needs a C toolchain — the official rust image
# ships one, so no extra packages are needed in the builder.

ARG TUMULT_VERSION=2.18.0

FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin kronikad --bin kronika-demo

# Fetch the pinned tumult release binary (the demo's seed runs REAL tumult
# experiments). The tarball is verified against the release's published
# SHA256SUMS.txt at build time — a checksum mismatch fails the build.
# Bump with: docker compose build --build-arg TUMULT_VERSION=x.y.z
# aarch64 hosts work unchanged: TARGETARCH=arm64 selects the
# aarch64-unknown-linux-musl tarball (also checksum-verified).
FROM debian:bookworm-slim AS tumult-fetch
ARG TUMULT_VERSION
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN set -e; \
    case "${TARGETARCH:-amd64}" in \
        amd64) TRIPLE=x86_64-unknown-linux-musl ;; \
        arm64) TRIPLE=aarch64-unknown-linux-musl ;; \
        *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    base="https://github.com/mwigge/tumult/releases/download/v${TUMULT_VERSION}"; \
    tarball="tumult-v${TUMULT_VERSION}-${TRIPLE}.tar.gz"; \
    curl -sfLO "${base}/SHA256SUMS.txt"; \
    curl -sfLO "${base}/${tarball}"; \
    grep "  ${tarball}\$" SHA256SUMS.txt | sha256sum -c -; \
    tar xzf "${tarball}"; \
    install -m 0755 "tumult-v${TUMULT_VERSION}-${TRIPLE}/tumult" /usr/local/bin/tumult

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 kronika

COPY --from=builder /build/target/release/kronikad /usr/local/bin/kronikad
COPY --from=builder /build/target/release/kronika-demo /usr/local/bin/kronika-demo
COPY --from=tumult-fetch /usr/local/bin/tumult /usr/local/bin/tumult
# Semantic metric definitions used by `kronikad report` and the /report endpoint.
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
# command (tumult experiment suite / sh -c '… curl /report …').
CMD ["kronikad"]
