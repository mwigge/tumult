"""A plain web service with no telemetry code in it.

Everything that reaches the resilience platform comes from OpenTelemetry
auto-instrumentation (`opentelemetry-instrument`) and the resource
attributes injected through the environment (see ../docker-compose.yml and
docs/guides/otel-autoinstrumentation.md). The app itself only knows HTTP.
"""

import os
import random
import time

from flask import Flask, jsonify

app = Flask(__name__)
START = time.time()


@app.get("/healthz")
def healthz():
    return jsonify(ok=True, uptime_s=round(time.time() - START, 1))


@app.get("/checkout")
def checkout():
    # A little latency spread so the series has shape.
    time.sleep(random.uniform(0.005, 0.05))
    if random.random() < float(os.environ.get("APP_ERROR_RATE", "0.02")):
        return jsonify(error="payment gateway timeout"), 502
    return jsonify(order_id=random.randint(1000, 9999), status="confirmed")


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=int(os.environ.get("PORT", "8080")))
