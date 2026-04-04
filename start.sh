#!/bin/sh
# Tumult — composable stack launcher
#
# Start one or more bundles:
#   ./start.sh                  # infra + observe (default)
#   ./start.sh infra            # chaos targets only
#   ./start.sh infra observe    # targets + observability
#   ./start.sh tumult           # MCP server (needs infra)
#   ./start.sh all              # everything
#   ./start.sh down             # stop all
#
# Bundles:
#   infra   — PostgreSQL, Redis, Kafka, SSH (chaos targets)
#   observe — SigNoz standalone + OTel Collector
#   tumult  — Tumult MCP server (containerized)
#   aqe     — Agentic QE Fleet (requires ../agentic-qe clone)

set -eu

COMPOSE_DIR="$(cd "$(dirname "$0")/docker" && pwd)"
PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Detect compose command (v2 plugin or standalone)
if docker compose version >/dev/null 2>&1; then
    COMPOSE="docker compose"
else
    COMPOSE="docker-compose"
fi

INFRA="${COMPOSE} -f ${COMPOSE_DIR}/docker-compose.yml"
OBSERVE="${COMPOSE} -f ${COMPOSE_DIR}/docker-compose.observability.yml"
AQE="${COMPOSE} -f ${COMPOSE_DIR}/docker-compose.aqe.yml"
TUMULT="${COMPOSE} -f ${COMPOSE_DIR}/docker-compose.tumult.yml"

# Ensure network exists
docker network create tumult-e2e 2>/dev/null || true

start_bundle() {
    case "$1" in
        infra)
            echo "Starting chaos targets (PostgreSQL, Redis, Kafka, SSH)..."
            ${INFRA} up -d
            ;;
        observe)
            echo "Starting observability (SigNoz + OTel Collector)..."
            ${OBSERVE} up -d
            ;;
        tumult)
            echo "Starting Tumult MCP server..."
            ${TUMULT} up -d
            ;;
        aqe)
            if [ ! -d "${PROJECT_DIR}/../agentic-qe" ]; then
                echo "Agentic QE not found. Clone it first:"
                echo "  git clone https://github.com/proffesor-for-testing/agentic-qe.git ../agentic-qe"
                return 1 2>/dev/null || exit 1
            fi
            echo "Starting Agentic QE Fleet..."
            ${AQE} up -d
            ;;
        *)
            echo "Unknown bundle: $1"
            echo "Available: infra, observe, tumult, aqe, all, down"
            exit 1
            ;;
    esac
}

if [ $# -eq 0 ]; then
    # Default: infra + observe
    start_bundle infra
    start_bundle observe
    echo ""
    echo "Stack ready:"
    echo "  Chaos targets:  PG :15432  Redis :16379  Kafka :19092  SSH :12222"
    echo "  SigNoz UI:      http://localhost:3301"
    echo "  OTLP endpoint:  http://localhost:14317"
elif [ "$1" = "all" ]; then
    start_bundle infra
    start_bundle observe
    start_bundle tumult
    start_bundle aqe
    echo ""
    echo "Full stack ready:"
    echo "  Chaos targets:  PG :15432  Redis :16379  Kafka :19092  SSH :12222"
    echo "  SigNoz UI:      http://localhost:3301"
    echo "  OTLP endpoint:  http://localhost:14317"
    echo "  Tumult MCP:     http://localhost:3100"
elif [ "$1" = "down" ]; then
    echo "Stopping all bundles..."
    ${TUMULT} down 2>/dev/null || true
    ${OBSERVE} down 2>/dev/null || true
    ${INFRA} down 2>/dev/null || true
    docker network rm tumult-e2e 2>/dev/null || true
    echo "All stopped."
elif [ "$1" = "status" ]; then
    docker ps --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null | grep -E "tumult|signoz|postgres|redis|kafka|sshd|otel|aqe" || echo "No containers running."
else
    for bundle in "$@"; do
        start_bundle "$bundle"
    done
fi
