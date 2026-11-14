"""How weights reach the model: hashed, checked against the pin, and only
then handed to the loader — as a private copy of the bytes that were
hashed, never as the path that was checked.

These tests need neither torch nor the weights: the loader is a fake, so
they run wherever the stub tests run (CI included). ``test_timestral``
repeats the mismatch case against the real loader."""

from __future__ import annotations

import hashlib
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from forecastd.model import (
    CHECKPOINT_FILES,
    Backend,
    TimestralModel,
    WeightsMismatch,
    hash_weights,
    load_model_from_env,
    stage_weights,
)

CONFIG = b'{"quantile_levels": [0.1, 0.5, 0.9]}'
WEIGHTS = b"\x00weights\xff" * 1000


def framed(*parts: tuple[str, bytes]) -> str:
    """The digest ``hash_weights`` is documented to produce: per file, the
    length of its name (8 bytes, big-endian), the name, the length of its
    content (8 bytes) and the content."""
    digest = hashlib.sha256()
    for name, content in parts:
        digest.update(len(name.encode()).to_bytes(8, "big"))
        digest.update(name.encode())
        digest.update(len(content).to_bytes(8, "big"))
        digest.update(content)
    return digest.hexdigest()


class Forecaster:
    """What the fake loader returns: the bytes it found at the path it was
    given."""

    quantile_levels = (0.1, 0.5, 0.9)

    def __init__(self, directory: Path) -> None:
        self.directory = Path(directory)
        self.files = {name: (self.directory / name).read_bytes() for name in CHECKPOINT_FILES}
        self.modes = {
            name: stat.S_IMODE((self.directory / name).stat().st_mode) for name in CHECKPOINT_FILES
        }
        self.directory_mode = stat.S_IMODE(self.directory.stat().st_mode)
        self.others = sorted(p.name for p in self.directory.iterdir() if p.name not in self.files)


class Recorder:
    """A backend that records the order of what the service asks of it."""

    def __init__(self, checkpoint: Path, before_load=None) -> None:
        self.checkpoint = checkpoint
        self.before_load = before_load
        self.calls: list[str] = []
        self.loaded: list[Forecaster] = []

    def resolve(self, source: str, revision: str | None = None) -> Path:
        self.calls.append(f"resolve {source} @ {revision}")
        return self.checkpoint

    def load(self, directory: Path) -> Forecaster:
        self.calls.append("load")
        if self.before_load is not None:
            self.before_load()
        self.loaded.append(Forecaster(directory))
        return self.loaded[-1]

    def set_threads(self, threads: int) -> None:
        self.calls.append(f"threads {threads}")

    def backend(self) -> Backend:
        return Backend(resolve=self.resolve, load=self.load, set_threads=self.set_threads)


class LoadingTests(unittest.TestCase):
    def setUp(self) -> None:
        holder = tempfile.TemporaryDirectory()
        self.addCleanup(holder.cleanup)
        self.tmp = Path(holder.name)
        self.checkpoint = self.tmp / "hub" / "snapshots" / "0123abc"
        self.checkpoint.mkdir(parents=True)
        (self.checkpoint / "config.json").write_bytes(CONFIG)
        (self.checkpoint / "model.safetensors").write_bytes(WEIGHTS)
        (self.checkpoint / "README.md").write_bytes(b"not part of the model")
        self.pin = hash_weights(self.checkpoint, CHECKPOINT_FILES)
        # Every private copy is made under this directory.
        self.staging = self.tmp / "staging"
        self.staging.mkdir()
        patched = mock.patch.object(tempfile, "tempdir", str(self.staging))
        patched.start()
        self.addCleanup(patched.stop)

    def staged(self) -> list[str]:
        return sorted(p.name for p in self.staging.iterdir())

    def test_the_loader_is_never_called_on_weights_that_do_not_match(self) -> None:
        recorder = Recorder(self.checkpoint)
        with self.assertRaises(WeightsMismatch) as raised:
            TimestralModel("00" * 32, backend=recorder.backend())
        self.assertEqual(recorder.calls, ["resolve mistralai/Timestral-5M @ None"])
        self.assertIn(self.pin, str(raised.exception))
        self.assertIn("00" * 32, str(raised.exception))
        self.assertEqual(self.staged(), [], "the refused copy is removed")

    def test_weights_are_resolved_then_checked_then_loaded(self) -> None:
        recorder = Recorder(self.checkpoint)
        model = TimestralModel(self.pin, revision="0123abc", threads=3, backend=recorder.backend())
        self.assertEqual(
            recorder.calls, ["threads 3", "resolve mistralai/Timestral-5M @ 0123abc", "load"]
        )
        self.assertEqual(model.info.weights_hash, self.pin)
        self.assertEqual(model.info.revision, "0123abc")
        self.assertEqual(model.levels, (0.1, 0.5, 0.9))

    def test_the_loader_is_given_a_private_read_only_copy(self) -> None:
        recorder = Recorder(self.checkpoint)
        TimestralModel(self.pin, backend=recorder.backend())
        (loaded,) = recorder.loaded
        self.assertNotEqual(loaded.directory, self.checkpoint)
        self.assertEqual(loaded.directory.parent, self.staging)
        self.assertEqual(loaded.files, {"config.json": CONFIG, "model.safetensors": WEIGHTS})
        self.assertEqual(loaded.others, [], "only the files that were hashed are there")
        self.assertEqual(loaded.directory_mode, 0o500)
        self.assertEqual(set(loaded.modes.values()), {0o400})
        self.assertEqual(self.staged(), [], "the copy is removed once the model is loaded")

    def test_files_swapped_after_the_check_are_not_what_is_loaded(self) -> None:
        def swap() -> None:
            (self.checkpoint / "model.safetensors").write_bytes(b"swapped in after the check")
            (self.checkpoint / "config.json").write_bytes(b'{"evil": true}')

        recorder = Recorder(self.checkpoint, before_load=swap)
        model = TimestralModel(self.pin, backend=recorder.backend())
        (loaded,) = recorder.loaded
        self.assertEqual(loaded.files, {"config.json": CONFIG, "model.safetensors": WEIGHTS})
        self.assertEqual(model.info.weights_hash, self.pin)
        self.assertNotEqual(hash_weights(self.checkpoint, CHECKPOINT_FILES), self.pin)

    def test_a_loader_that_fails_leaves_no_copy_behind(self) -> None:
        def broken() -> None:
            raise OSError("cannot map the file")

        recorder = Recorder(self.checkpoint, before_load=broken)
        with self.assertRaises(OSError):
            TimestralModel(self.pin, backend=recorder.backend())
        self.assertEqual(self.staged(), [])

    def test_a_missing_file_is_an_error_before_the_loader(self) -> None:
        (self.checkpoint / "config.json").unlink()
        recorder = Recorder(self.checkpoint)
        with self.assertRaises(FileNotFoundError):
            TimestralModel(self.pin, backend=recorder.backend())
        self.assertNotIn("load", recorder.calls)
        self.assertEqual(self.staged(), [])

    def test_the_environment_reaches_the_same_path(self) -> None:
        recorder = Recorder(self.checkpoint)
        env = {
            "FORECASTD_MODEL": "timestral",
            "FORECASTD_WEIGHTS_PATH": str(self.checkpoint),
            "FORECASTD_WEIGHTS_SHA256": "00" * 32,
        }
        with self.assertRaises(WeightsMismatch):
            load_model_from_env(env, backend=recorder.backend())
        self.assertNotIn("load", recorder.calls)
        env["FORECASTD_WEIGHTS_SHA256"] = self.pin
        model = load_model_from_env(env, backend=recorder.backend())
        self.assertEqual(recorder.calls[-1], "load")
        self.assertEqual(model.info.weights_hash, self.pin)

    def test_the_staged_copy_hashes_like_the_directory(self) -> None:
        staged, digest = stage_weights(self.checkpoint, self.pin, CHECKPOINT_FILES)
        self.assertEqual(digest, self.pin)
        self.assertEqual(hash_weights(staged, CHECKPOINT_FILES), self.pin)
        self.assertEqual(stat.S_IMODE(staged.stat().st_mode), 0o500)


class FramingTests(unittest.TestCase):
    """The digest covers where one file ends and the next begins."""

    def directory(self, **files: bytes) -> Path:
        holder = tempfile.TemporaryDirectory()
        self.addCleanup(holder.cleanup)
        root = Path(holder.name)
        for name, content in files.items():
            (root / name).write_bytes(content)
        return root

    def test_the_digest_is_the_documented_framing(self) -> None:
        root = self.directory(**{"config.json": CONFIG, "model.safetensors": WEIGHTS})
        expected = framed(("config.json", CONFIG), ("model.safetensors", WEIGHTS))
        self.assertEqual(hash_weights(root, CHECKPOINT_FILES), expected)
        self.assertEqual(hash_weights(root), expected)
        single = root / "model.safetensors"
        self.assertEqual(hash_weights(single), framed(("model.safetensors", WEIGHTS)))

    def test_moving_a_byte_between_name_and_content_changes_the_digest(self) -> None:
        # Unframed, both are the byte stream "abc".
        one = self.directory(a=b"bc")
        other = self.directory(ab=b"c")
        self.assertNotEqual(hash_weights(one), hash_weights(other))

    def test_moving_bytes_between_two_files_changes_the_digest(self) -> None:
        # Unframed, both are "a" "1b2" "b" "3" = "a1" "b" "2b3" as a stream.
        one = self.directory(a=b"1b2", b=b"3")
        other = self.directory(a=b"1", b=b"2b3")
        self.assertNotEqual(hash_weights(one), hash_weights(other))

    def test_files_in_subdirectories_are_named_by_their_path(self) -> None:
        one = self.directory()
        other = self.directory()
        for root, folder in ((one, "x"), (other, "y")):
            (root / folder).mkdir()
            (root / folder / "w.bin").write_bytes(b"same")
        self.assertNotEqual(hash_weights(one), hash_weights(other))
        self.assertEqual(hash_weights(one), framed(("x/w.bin", b"same")))


if __name__ == "__main__":
    unittest.main()
