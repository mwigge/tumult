# Tumult OTel Collector — Multi-stage Docker build
#
# Builds the custom collector distribution using ocb inside the container,
# then packages it into a minimal runtime image.
#
# Build:
#   cd docker/tumult-collector && docker build -t tumult-otel-collector .
#
# Run:
#   docker run -v ./config.yaml:/etc/otel/config.yaml tumult-otel-collector

FROM golang:1.26-alpine AS builder

RUN apk add --no-cache git make

# Install the OpenTelemetry Collector Builder
RUN go install go.opentelemetry.io/collector/cmd/builder@v0.149.0

WORKDIR /build
COPY builder-config.yaml .

RUN builder --config builder-config.yaml

# ── Runtime image ──────────────────────────────────────────────

FROM alpine:3.20

RUN apk add --no-cache ca-certificates tzdata

COPY --from=builder /build/build/tumult-otel-collector /usr/local/bin/tumult-otel-collector
COPY config.yaml /etc/otel/config.yaml

EXPOSE 4317 4318 4319 8888 8889 13133 55679

ENTRYPOINT ["tumult-otel-collector"]
CMD ["--config", "/etc/otel/config.yaml"]
