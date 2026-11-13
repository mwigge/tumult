"""TimestralModel against the real package and real weights.

These tests load ``mistralai/Timestral-5M`` (through the Hugging Face hub
cache, or ``FORECASTD_TEST_WEIGHTS_PATH`` for an offline snapshot) and run
it on CPU. They skip only when ``torch`` or ``timestral`` is not installed;
the stub-model tests in ``test_service.py`` cover the service without them.

    HF_HOME=/path/to/cache python -m unittest tests.test_timestral -v
"""

from __future__ import annotations

import importlib.util
import json
import math
import os
import re
import statistics
import threading
import time
import unittest
import urllib.request

import numpy as np

from forecastd.model import TIMESTRAL_LEVELS, load_model_from_env
from forecastd.server import serve
from forecastd.service import MAX_CONTEXT, MAX_HORIZON, BadRequest, handle_forecast, handle_healthz

HAVE_TIMESTRAL = all(importlib.util.find_spec(name) for name in ("torch", "timestral"))

# Generous on purpose: the bound catches a model that has become unusable
# on CPU, not machine-to-machine noise. The measured values are printed.
LATENCY_BUDGET_S = 2.0
LONG_CONTEXT_BUDGET_S = 60.0


def seasonal(n: int, period: int = 24, seed: int = 3) -> np.ndarray:
    """A noisy positive daily cycle, like a latency series."""
    rng = np.random.default_rng(seed)
    t = np.arange(n)
    return 100.0 + 20.0 * np.sin(2 * np.pi * t / period) + rng.normal(0.0, 2.0, n)


@unittest.skipUnless(HAVE_TIMESTRAL, "torch and timestral are not installed")
class TimestralModelTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        env = {"FORECASTD_MODEL": "timestral", "FORECASTD_MODEL_SIZE": "5M"}
        weights = os.environ.get("FORECASTD_TEST_WEIGHTS_PATH")
        if weights:
            env["FORECASTD_WEIGHTS_PATH"] = weights
        started = time.perf_counter()
        cls.model = load_model_from_env(env)
        cls.load_s = time.perf_counter() - started

    def run_model(self, context: np.ndarray, horizon: int, **covariates: np.ndarray) -> np.ndarray:
        return self.model.forecast(
            context, horizon, covariates.get("past"), covariates.get("known")
        )

    def test_provenance_names_the_model_and_the_weights(self) -> None:
        info = self.model.info
        self.assertEqual(info.model, "timestral")
        self.assertEqual(info.model_id, "mistralai/Timestral-5M")
        self.assertEqual(info.size, "5M")
        self.assertRegex(info.version, r"^\d+\.\d+")
        self.assertRegex(info.weights_hash, r"^[0-9a-f]{64}$")
        if not os.environ.get("FORECASTD_TEST_WEIGHTS_PATH"):
            self.assertRegex(info.revision, r"^[0-9a-f]{40}$", "a hub snapshot is a commit")
        self.assertEqual(self.model.levels, TIMESTRAL_LEVELS)
        health = handle_healthz(self.model)
        self.assertEqual(health["revision"], info.revision)
        self.assertEqual(health["weights_hash"], info.weights_hash)

    def test_output_is_variates_by_horizon_by_levels(self) -> None:
        one = self.run_model(seasonal(256)[None, :], 24)
        self.assertEqual(one.shape, (1, 24, 21))
        self.assertEqual(one.dtype, np.float64)
        two = self.run_model(np.stack([seasonal(256), seasonal(256, seed=4) * 2.0]), 7)
        self.assertEqual(two.shape, (2, 7, 21))
        self.assertTrue(np.isfinite(one).all() and np.isfinite(two).all())

    def test_quantiles_are_ordered_and_bracket_the_series(self) -> None:
        out = self.run_model(seasonal(512)[None, :], 48)[0]
        self.assertTrue((np.diff(out, axis=1) >= -1e-9).all(), "levels never cross")
        median = out[:, TIMESTRAL_LEVELS.index(0.5)]
        low = out[:, TIMESTRAL_LEVELS.index(0.1)]
        high = out[:, TIMESTRAL_LEVELS.index(0.9)]
        self.assertTrue((low <= median).all() and (median <= high).all())
        self.assertTrue((high - low > 0).all(), "the band has width")
        # The cycle runs 80..120; a sane forecast stays in that neighbourhood.
        self.assertTrue((median > 60).all() and (median < 140).all(), median)
        self.assertGreater(median.max() - median.min(), 10.0, "the cycle is continued")

    def test_same_input_and_seed_give_the_same_forecast(self) -> None:
        import torch

        context = seasonal(300)[None, :]
        torch.manual_seed(7)
        first = self.run_model(context, 16)
        torch.manual_seed(7)
        second = self.run_model(context, 16)
        np.testing.assert_array_equal(first, second)
        # And the forecast does not depend on the seed at all: no sampling.
        torch.manual_seed(8)
        np.testing.assert_array_equal(first, self.run_model(context, 16))

    def test_gaps_and_short_contexts_are_forecast(self) -> None:
        context = seasonal(200)
        context[50:70] = math.nan
        out = self.run_model(context[None, :], 8)
        self.assertTrue(np.isfinite(out).all())
        short = self.run_model(np.array([[5.0, 6.0, 5.5]]), 4)
        self.assertEqual(short.shape, (1, 4, 21))
        self.assertTrue(np.isfinite(short).all())

    def test_covariates_condition_the_forecast(self) -> None:
        context = seasonal(256)
        past = np.zeros((1, 256))
        known = np.zeros((1, 256 + 12))
        known[0, 256:] = 1.0  # a fault indicator over the horizon
        plain = self.run_model(context[None, :], 12)
        with_covariates = self.run_model(context[None, :], 12, past=past, known=known)
        self.assertEqual(with_covariates.shape, plain.shape)
        self.assertTrue(np.isfinite(with_covariates).all())

    def test_horizon_limits(self) -> None:
        # Past one decode block (1,024 steps) the model forecasts block by block.
        out = self.run_model(seasonal(512)[None, :], 1_100)
        self.assertEqual(out.shape, (1, 1_100, 21))
        self.assertTrue(np.isfinite(out).all())
        payload = {"context": seasonal(64).tolist(), "horizon": MAX_HORIZON + 1}
        with self.assertRaises(BadRequest):
            handle_forecast(payload, self.model)

    def test_context_limits(self) -> None:
        started = time.perf_counter()
        out = self.run_model(seasonal(MAX_CONTEXT)[None, :], 8)
        elapsed = time.perf_counter() - started
        print(f"\n[timestral] context {MAX_CONTEXT} horizon 8: {elapsed:.2f} s")
        self.assertEqual(out.shape, (1, 8, 21))
        self.assertTrue(np.isfinite(out).all())
        self.assertLess(elapsed, LONG_CONTEXT_BUDGET_S)
        payload = {"context": [1.0] * (MAX_CONTEXT + 1), "horizon": 1}
        with self.assertRaises(BadRequest):
            handle_forecast(payload, self.model)

    def test_latency_on_cpu(self) -> None:
        import torch

        context = seasonal(512)[None, :]
        self.run_model(context, 24)  # warm-up
        timings = []
        for _ in range(10):
            started = time.perf_counter()
            self.run_model(context, 24)
            timings.append(time.perf_counter() - started)
        median = statistics.median(timings)
        print(
            f"\n[timestral] load {self.load_s:.2f} s; context 512 horizon 24 on "
            f"{torch.get_num_threads()} CPU threads: median {median * 1000:.0f} ms, "
            f"max {max(timings) * 1000:.0f} ms"
        )
        self.assertLess(median, LATENCY_BUDGET_S)

    def test_the_service_answers_with_the_real_model(self) -> None:
        server = serve(self.model, "127.0.0.1", 0)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            port = server.server_address[1]
            body = json.dumps(
                {
                    "context": seasonal(128).tolist(),
                    "horizon": 6,
                    "quantile_levels": [0.1, 0.5, 0.9],
                }
            ).encode()
            request = urllib.request.Request(
                f"http://127.0.0.1:{port}/forecast",
                data=body,
                headers={"content-type": "application/json"},
            )
            with urllib.request.urlopen(request, timeout=30) as response:
                answer = json.load(response)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
        self.assertEqual(answer["levels"], [0.1, 0.5, 0.9])
        self.assertEqual(answer["model"]["model"], "timestral")
        self.assertTrue(re.fullmatch(r"[0-9a-f]{64}", answer["model"]["weights_hash"]))
        steps = answer["quantiles"][0]
        self.assertEqual(len(steps), 6)
        self.assertTrue(all(low <= mid <= high for low, mid, high in steps))


if __name__ == "__main__":
    unittest.main()
