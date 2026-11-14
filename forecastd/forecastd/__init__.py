"""tumult-forecastd: the optional Timestral forecasting sidecar.

One endpoint, ``POST /forecast``, turns regular-step context arrays (``NaN``
for gaps) into quantile paths; ``GET /healthz`` reports which model is
loaded, its size and the hash of its weights. The model is loaded once at
start-up. Timestral (and PyTorch) are imported only when the Timestral model
is selected, so the service and its tests run without them.
"""

__version__ = "0.1.0"
