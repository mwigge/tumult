"""The request handling, exercised with the deterministic stub model."""

from __future__ import annotations

import math
import unittest

import numpy as np

from forecastd import model as model_module
from forecastd.__main__ import main
from forecastd.model import (
    CHECKPOINT_FILES,
    DEFAULT_MODEL_REVISION,
    DEFAULT_WEIGHTS_SHA256,
    TIMESTRAL_LEVELS,
    StubModel,
    WeightsMismatch,
    default_threads,
    hash_weights,
    load_model_from_env,
    snapshot_revision,
    verify_weights,
    weights_pin,
)
from forecastd.service import (
    MAX_BODY,
    MAX_CONTEXT,
    MAX_COVARIATES,
    MAX_HORIZON,
    MAX_LEVELS,
    MAX_VARIATES,
    BadRequest,
    Gate,
    gate_from_env,
    handle_forecast,
    handle_healthz,
    parse_body,
)


class RefusingModel(StubModel):
    """A model that turns the request down the way Timestral does."""

    def forecast(self, context, horizon, past_covariates, known_covariates):
        raise ValueError("prediction_length exceeds what this checkpoint supports")


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

    def test_only_numbers_and_null_are_values(self) -> None:
        nan, inf = float("nan"), float("inf")
        bad_values = ["1.5", True, False, nan, inf, -inf, {}, [1.0], 10**400]
        for value in bad_values:
            for field in ("context", "past_covariates", "known_covariates"):
                payload = {
                    "context": [1.0, 2.0, 3.0],
                    "horizon": 1,
                    "past_covariates": [[0.0, 0.0, 0.0]],
                    "known_covariates": [[0.0, 0.0, 0.0, 0.0]],
                }
                rows = [1.0, value, 3.0, 4.0][: 4 if field == "known_covariates" else 3]
                payload[field] = rows if field == "context" else [rows]
                with self.subTest(value=value, field=field), self.assertRaises(BadRequest):
                    handle_forecast(payload, self.model)
        # A gap is still a gap, and whole numbers are numbers.
        body = handle_forecast({"context": [1, None, 3.0, 4], "horizon": 1}, self.model)
        self.assertEqual(body["context_points"], 3)

    def test_quantile_levels_must_be_plain_numbers(self) -> None:
        for levels in (["0.5"], [True], [float("nan")], [0.5] * (MAX_LEVELS + 1), [None]):
            with self.subTest(levels=levels), self.assertRaises(BadRequest):
                handle_forecast(
                    {"context": [1.0, 2.0], "horizon": 1, "quantile_levels": levels}, self.model
                )

    def test_the_number_of_series_is_bounded(self) -> None:
        row = [1.0, 2.0, 3.0]
        ok = handle_forecast({"context": [row] * MAX_VARIATES, "horizon": 1}, self.model)
        self.assertEqual(len(ok["quantiles"]), MAX_VARIATES)
        too_many = [
            {"context": [row] * (MAX_VARIATES + 1), "horizon": 1},
            {"context": row, "horizon": 1, "past_covariates": [row] * (MAX_COVARIATES + 1)},
            {
                "context": row,
                "horizon": 1,
                "known_covariates": [[0.0] * 4] * (MAX_COVARIATES + 1),
            },
        ]
        for payload in too_many:
            with self.subTest(payload=len(str(payload))), self.assertRaises(BadRequest):
                handle_forecast(payload, self.model)

    def test_rows_must_be_arrays_of_one_length(self) -> None:
        for context in ([[1.0, 2.0], 3.0], [[1.0, 2.0], "ab"], [[1.0], []], [[]]):
            with self.subTest(context=context), self.assertRaises(BadRequest):
                handle_forecast({"context": context, "horizon": 1}, self.model)

    def test_the_limits_are_what_the_daemon_mirrors(self) -> None:
        # tumult-api/src/forecast/client.rs reads these two from this file.
        self.assertEqual(MAX_CONTEXT, 32_768)
        self.assertEqual(MAX_HORIZON, 4_096)
        self.assertLessEqual(MAX_BODY, 8 << 20)
        with self.assertRaises(BadRequest):
            handle_forecast({"context": [1.0, 2.0], "horizon": MAX_HORIZON + 1}, self.model)

    def test_a_request_the_model_refuses_is_a_bad_request(self) -> None:
        # Not a server fault: the daemon must not open its circuit for it.
        with self.assertRaises(BadRequest) as raised:
            handle_forecast({"context": [1.0, 2.0, 3.0], "horizon": 2}, RefusingModel())
        # What the model said is logged, not sent back.
        self.assertEqual(str(raised.exception), "the model refused the request")

    def test_healthz_reports_the_model(self) -> None:
        body = handle_healthz(self.model)
        self.assertEqual(body["status"], "ok")
        self.assertEqual(body["model"], "stub")
        self.assertEqual(body["size"], "0")
        self.assertIn("weights_hash", body)
        self.assertEqual(body["max_context"], 32_768)


class GateTests(unittest.TestCase):
    def test_a_gate_admits_its_limit_and_no_more(self) -> None:
        gate = Gate(2)
        self.assertEqual(gate.limit, 2)
        self.assertTrue(gate.enter())
        self.assertTrue(gate.enter())
        self.assertFalse(gate.enter(), "full: the caller is not made to wait")
        gate.leave()
        self.assertTrue(gate.enter())
        gate.leave()
        gate.leave()
        with self.assertRaises(ValueError):
            gate.leave()  # leaving more often than entering is a bug

    def test_the_limit_comes_from_the_environment(self) -> None:
        self.assertEqual(gate_from_env({}).limit, 4)
        self.assertEqual(gate_from_env({"FORECASTD_MAX_CONCURRENT": " "}).limit, 4)
        self.assertEqual(gate_from_env({"FORECASTD_MAX_CONCURRENT": "9"}).limit, 9)
        for bad in ("0", "-1", "many", "1.5"):
            with self.subTest(bad=bad), self.assertRaises(ValueError) as raised:
                gate_from_env({"FORECASTD_MAX_CONCURRENT": bad})
            self.assertIn("FORECASTD_MAX_CONCURRENT", str(raised.exception))
        with self.assertRaises(ValueError):
            Gate(0)


class ParseBodyTests(unittest.TestCase):
    def test_a_json_body_is_parsed(self) -> None:
        self.assertEqual(parse_body(b'{"horizon": 1}'), {"horizon": 1})

    def test_what_is_not_json_is_a_bad_request(self) -> None:
        for raw in (b"", b"not json", b"\xff\xfe", b'{"a": '):
            with self.subTest(raw=raw), self.assertRaises(BadRequest):
                parse_body(raw)

    def test_nesting_past_the_interpreter_limit_is_a_bad_request(self) -> None:
        for raw in (b"[" * 200_000, b'{"a":' * 100_000, b"[" * 200_000 + b"]" * 200_000):
            with self.subTest(start=raw[:8]), self.assertRaises(BadRequest) as raised:
                parse_body(raw)
            self.assertIn("nested", str(raised.exception))


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


    def test_the_stub_says_it_is_a_stub(self) -> None:
        info = load_model_from_env({"FORECASTD_MODEL": "stub"}).info.as_dict()
        self.assertEqual(info["model"], "stub")
        self.assertEqual(info["revision"], "none")
        self.assertEqual(info["weights_hash"], "none")

    def test_revision_is_the_hub_snapshot_commit(self) -> None:
        from pathlib import Path

        hub = Path("/cache/hub/models--mistralai--Timestral-5M/snapshots/0123abc")
        self.assertEqual(snapshot_revision(hub), "0123abc")
        self.assertEqual(snapshot_revision(Path("/weights/Timestral-5M")), "local")

    def test_thread_settings_are_validated_before_the_model_loads(self) -> None:
        self.assertTrue(1 <= default_threads() <= 4)
        for bad in ("many", "0", "-2"):
            with self.assertRaises(ValueError):
                load_model_from_env({"FORECASTD_MODEL": "timestral", "FORECASTD_THREADS": bad})
        with self.assertRaises(ValueError):
            load_model_from_env({"FORECASTD_MODEL": "prophet"})



class WeightsPinTests(unittest.TestCase):
    """Which weights may be loaded, decided before anything is loaded."""

    def test_the_default_model_is_pinned_to_the_tested_weights(self) -> None:
        self.assertEqual(DEFAULT_MODEL_REVISION, "bdd20cfc15a3fea0e4cb5ccef37f8d3ca7aa335f")
        self.assertEqual(
            DEFAULT_WEIGHTS_SHA256,
            "577236a804ccb3c2b02638c94b6b433920acf8edd248c9f7a6a32eb50104d63e",
        )
        self.assertEqual(weights_pin({}), (DEFAULT_MODEL_REVISION, DEFAULT_WEIGHTS_SHA256))
        explicit = {"FORECASTD_MODEL_SIZE": "5M", "FORECASTD_MODEL_ID": "mistralai/Timestral-5M"}
        self.assertEqual(weights_pin(explicit), (DEFAULT_MODEL_REVISION, DEFAULT_WEIGHTS_SHA256))

    def test_the_environment_overrides_the_pin(self) -> None:
        sha = "ab" * 32
        env = {"FORECASTD_MODEL_REVISION": "cafe", "FORECASTD_WEIGHTS_SHA256": sha.upper()}
        self.assertEqual(weights_pin(env), ("cafe", sha))
        # A new revision without its hash keeps the tested hash: it will not match.
        only_revision = weights_pin({"FORECASTD_MODEL_REVISION": "cafe"})
        self.assertEqual(only_revision, ("cafe", DEFAULT_WEIGHTS_SHA256))

    def test_another_model_must_bring_its_own_pin(self) -> None:
        sha = "cd" * 32
        for env in (
            {"FORECASTD_MODEL_SIZE": "28M"},
            {"FORECASTD_MODEL_SIZE": "28M", "FORECASTD_MODEL_REVISION": "cafe"},
            {"FORECASTD_MODEL_SIZE": "28M", "FORECASTD_WEIGHTS_SHA256": sha},
            {"FORECASTD_MODEL_ID": "someone/else", "FORECASTD_MODEL_REVISION": "cafe"},
        ):
            with self.subTest(env=env), self.assertRaises(ValueError) as raised:
                weights_pin(env)
            self.assertIn("FORECASTD_WEIGHTS_SHA256", str(raised.exception))
        both = {
            "FORECASTD_MODEL_SIZE": "28M",
            "FORECASTD_MODEL_REVISION": "cafe",
            "FORECASTD_WEIGHTS_SHA256": sha,
        }
        self.assertEqual(weights_pin(both), ("cafe", sha))
        # A mounted directory has no hub revision to pin; its hash is enough.
        mounted = {
            "FORECASTD_MODEL_SIZE": "28M",
            "FORECASTD_WEIGHTS_PATH": "/weights",
            "FORECASTD_WEIGHTS_SHA256": sha,
        }
        self.assertEqual(weights_pin(mounted), (None, sha))

    def test_a_malformed_hash_is_refused(self) -> None:
        for bad in ("abc", "g" * 64, "ab" * 33):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                weights_pin({"FORECASTD_WEIGHTS_SHA256": bad})

    def test_weights_that_do_not_match_are_refused(self) -> None:
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "model.safetensors").write_bytes(b"weights")
            actual = hash_weights(root)
            self.assertEqual(verify_weights(root, actual), actual)
            with self.assertRaises(WeightsMismatch) as raised:
                verify_weights(root, "00" * 32)
            message = str(raised.exception)
            self.assertIn(actual, message)
            self.assertIn("00" * 32, message)
            (root / "model.safetensors").write_bytes(b"tampered")
            with self.assertRaises(WeightsMismatch):
                verify_weights(root, actual)

    def test_only_the_files_the_model_reads_are_hashed(self) -> None:
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as tmp:
            bare = Path(tmp) / "bare"
            bare.mkdir()
            (bare / "config.json").write_bytes(b"{}")
            (bare / "model.safetensors").write_bytes(b"weights")
            pinned = hash_weights(bare)
            # The same two files as a full download leaves them.
            full = Path(tmp) / "full"
            (full / ".cache" / "huggingface").mkdir(parents=True)
            (full / ".cache" / "huggingface" / "state.json").write_bytes(b"x")
            (full / "README.md").write_bytes(b"read me")
            for name in CHECKPOINT_FILES:
                (full / name).write_bytes((bare / name).read_bytes())
            self.assertNotEqual(hash_weights(full), pinned)
            self.assertEqual(verify_weights(full, pinned, CHECKPOINT_FILES), pinned)
            (full / "model.safetensors").write_bytes(b"other")
            with self.assertRaises(WeightsMismatch):
                verify_weights(full, pinned, CHECKPOINT_FILES)
            (full / "config.json").unlink()
            with self.assertRaises(FileNotFoundError):
                verify_weights(full, pinned, CHECKPOINT_FILES)

    def test_the_pin_is_checked_before_the_model_is_selected(self) -> None:
        # Raised by the pin, not by a missing torch: nothing was imported.
        with self.assertRaises(ValueError) as raised:
            load_model_from_env({"FORECASTD_MODEL": "timestral", "FORECASTD_MODEL_SIZE": "28M"})
        self.assertIn("FORECASTD_WEIGHTS_SHA256", str(raised.exception))


class PackageVersionTests(unittest.TestCase):
    def test_a_missing_distribution_is_unknown(self) -> None:
        self.assertEqual(model_module._package_version("no-such-distribution-xyz"), "unknown")

    def test_any_other_failure_is_not_swallowed(self) -> None:
        from unittest import mock

        with mock.patch("importlib.metadata.version", side_effect=OSError("disk gone")):
            with self.assertRaises(OSError):
                model_module._package_version("numpy")


class MainTests(unittest.TestCase):
    def test_a_configuration_that_cannot_load_exits_non_zero(self) -> None:
        import os
        from unittest import mock

        for env in (
            {"FORECASTD_MODEL": "prophet"},
            {"FORECASTD_MODEL": "timestral", "FORECASTD_WEIGHTS_SHA256": "nope"},
            {"FORECASTD_MODEL": "stub", "FORECASTD_BIND": "8700"},
            {"FORECASTD_MODEL": "stub", "FORECASTD_MAX_CONCURRENT": "0"},
        ):
            # The process-wide log setup is left alone here: ``test_logs``
            # runs the real thing in a process of its own.
            with self.subTest(env=env), mock.patch.dict(os.environ, env, clear=True):
                with mock.patch("forecastd.logs.configure"):
                    with self.assertLogs("forecastd", level="ERROR") as seen:
                        self.assertEqual(main(), 1)
                self.assertEqual([record.event for record in seen.records], ["startup.failed"])


if __name__ == "__main__":
    unittest.main()
