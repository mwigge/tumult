"""The request handling, exercised with the deterministic stub model."""

from __future__ import annotations

import math
import unittest

import numpy as np

from forecastd.model import TIMESTRAL_LEVELS, StubModel, hash_weights, load_model_from_env
from forecastd.service import BadRequest, handle_forecast, handle_healthz


class StubModelTests(unittest.TestCase):
    def test_trend_is_continued_with_ordered_quantiles(self) -> None:
        model = StubModel()
        context = np.array([[10.0 + 0.5 * i for i in range(40)]])
        out = model.forecast(context, 3, None, None)
        self.assertEqual(out.shape, (1, 3, 21))
        median = out[0, :, TIMESTRAL_LEVELS.index(0.5)]
        for h, value in enumerate(median):
            self.assertAlmostEqual(value, 10.0 + 0.5 * (40 + h), places=6)
        for step in out[0]:
            self.assertTrue(np.all(np.diff(step) >= 0), "quantiles are monotone")

    def test_gaps_are_interpolated_and_the_band_widens(self) -> None:
        model = StubModel()
        context = np.array([[1.0, math.nan, 3.0, 4.0, math.nan, 6.5, 7.0, 7.8]])
        out = model.forecast(context, 4, None, None)
        self.assertTrue(np.all(np.isfinite(out)))
        width = out[0, :, -1] - out[0, :, 0]
        self.assertGreaterEqual(width[3], width[0])

    def test_same_input_same_output(self) -> None:
        model = StubModel()
        context = np.array([[math.sin(i * 0.3) for i in range(30)]])
        a = model.forecast(context, 5, None, None)
        b = model.forecast(context, 5, None, None)
        self.assertTrue(np.array_equal(a, b))

    def test_single_point_context_is_flat(self) -> None:
        out = StubModel().forecast(np.array([[4.0]]), 2, None, None)
        self.assertTrue(np.allclose(out, 4.0))


class HandleForecastTests(unittest.TestCase):
    def setUp(self) -> None:
        self.model = StubModel()

    def test_single_variate_request_returns_requested_levels(self) -> None:
        body = handle_forecast(
            {
                "context": [1.0, 2.0, None, 4.0, 5.0, 6.0],
                "horizon": 2,
                "quantile_levels": [0.1, 0.5, 0.9],
            },
            self.model,
        )
        self.assertEqual(body["levels"], [0.1, 0.5, 0.9])
        self.assertEqual(body["horizon"], 2)
        self.assertEqual(body["context_points"], 5)
        self.assertEqual(len(body["quantiles"]), 1)
        self.assertEqual(len(body["quantiles"][0]), 2)
        self.assertEqual(len(body["quantiles"][0][0]), 3)
        self.assertEqual(body["model"]["model"], "stub")
        self.assertEqual(body["model"]["weights_hash"], "none")
        step = body["quantiles"][0][0]
        self.assertLessEqual(step[0], step[1])
        self.assertLessEqual(step[1], step[2])

    def test_default_levels_are_all_twenty_one(self) -> None:
        body = handle_forecast({"context": [[1.0, 2.0, 3.0]], "horizon": 1}, self.model)
        self.assertEqual(len(body["levels"]), 21)
        self.assertEqual(body["levels"], list(TIMESTRAL_LEVELS))

    def test_requested_level_snaps_to_the_nearest_available(self) -> None:
        body = handle_forecast(
            {"context": [1.0, 2.0, 3.0], "horizon": 1, "quantile_levels": [0.12, 0.5]},
            self.model,
        )
        self.assertEqual(body["levels"], [0.1, 0.5])
        self.assertEqual(body["requested_levels"], [0.12, 0.5])

    def test_covariates_must_match_the_shape(self) -> None:
        ok = handle_forecast(
            {
                "context": [[1.0, 2.0, 3.0]],
                "horizon": 2,
                "past_covariates": [[0.0, 0.0, 0.0]],
                "known_covariates": [[0.0, 0.0, 0.0, 0.0, 0.0]],
            },
            self.model,
        )
        self.assertEqual(ok["horizon"], 2)
        with self.assertRaises(BadRequest):
            handle_forecast(
                {"context": [1.0, 2.0, 3.0], "horizon": 2, "known_covariates": [[0.0, 0.0]]},
                self.model,
            )

    def test_bad_payloads_are_rejected(self) -> None:
        bad = [
            [],
            {"horizon": 1},
            {"context": [], "horizon": 1},
            {"context": [None, None], "horizon": 1},
            {"context": ["x"], "horizon": 1},
            {"context": [1.0, 2.0], "horizon": 0},
            {"context": [1.0, 2.0], "horizon": True},
            {"context": [1.0, 2.0], "horizon": 1, "quantile_levels": [1.5]},
            {"context": [1.0, 2.0], "horizon": 1, "quantile_levels": []},
            {"context": [[1.0, 2.0], [1.0]], "horizon": 1},
        ]
        for payload in bad:
            with self.subTest(payload=payload), self.assertRaises(BadRequest):
                handle_forecast(payload, self.model)

    def test_healthz_reports_the_model(self) -> None:
        body = handle_healthz(self.model)
        self.assertEqual(body["status"], "ok")
        self.assertEqual(body["model"], "stub")
        self.assertEqual(body["size"], "0")
        self.assertIn("weights_hash", body)
        self.assertEqual(body["max_context"], 32_768)


class EnvironmentTests(unittest.TestCase):
    def test_stub_is_selected_from_the_environment(self) -> None:
        model = load_model_from_env({"FORECASTD_MODEL": "stub"})
        self.assertEqual(model.info.model, "stub")
        with self.assertRaises(ValueError):
            load_model_from_env({"FORECASTD_MODEL": "nope"})

    def test_weights_hash_is_stable_over_a_directory(self) -> None:
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "b.safetensors").write_bytes(b"bbb")
            (root / "a.json").write_bytes(b"{}")
            first = hash_weights(root)
            self.assertEqual(first, hash_weights(root))
            (root / "a.json").write_bytes(b"{ }")
            self.assertNotEqual(first, hash_weights(root))
        self.assertEqual(hash_weights(None), "unverified")


if __name__ == "__main__":
    unittest.main()
