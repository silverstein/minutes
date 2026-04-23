import contextlib
import importlib.util
import io
import os
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).with_name("video_review.py")
SPEC = importlib.util.spec_from_file_location("minutes_video_review", MODULE_PATH)
assert SPEC and SPEC.loader
video_review = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(video_review)


class VideoReviewTests(unittest.TestCase):
    def test_source_minutes_config_path_prefers_xdg_config_home(self) -> None:
        with mock.patch.dict(
            os.environ,
            {"XDG_CONFIG_HOME": "/tmp/xdg-home", "HOME": "/tmp/fallback-home"},
            clear=False,
        ):
            self.assertEqual(
                video_review.source_minutes_config_path(),
                Path("/tmp/xdg-home/minutes/config.toml"),
            )

    def test_write_minutes_config_preserves_existing_model_path(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            config_path = temp_path / "minutes" / "config.toml"
            output_dir = temp_path / "out"
            source_config_data = {
                "transcription": {
                    "engine": "parakeet",
                    "model_path": "/custom/models",
                }
            }

            video_review.write_minutes_config(
                config_path=config_path,
                output_dir=output_dir,
                source_config_data=source_config_data,
                source_engine="parakeet",
                language="en",
                forced_engine=None,
            )

            config_data = tomllib.loads(config_path.read_text(encoding="utf-8"))
            self.assertEqual(config_data["transcription"]["engine"], "parakeet")
            self.assertEqual(config_data["transcription"]["model_path"], "/custom/models")
            self.assertEqual(config_data["transcription"]["language"], "en")

    def test_load_source_minutes_config_invalid_toml_returns_empty_dict(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            config_path = Path(temp_dir) / "config.toml"
            config_path.write_text("not = [valid", encoding="utf-8")

            self.assertEqual(video_review.load_source_minutes_config(config_path), {})

    def test_transcribe_with_openai_skips_when_cli_missing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            audio_path = Path(temp_dir) / "sample.wav"
            audio_path.write_bytes(b"fake")
            stderr = io.StringIO()

            with (
                mock.patch.dict(os.environ, {"OPENAI_API_KEY": "test-key"}, clear=False),
                mock.patch.object(video_review.shutil, "which", return_value=None),
                contextlib.redirect_stderr(stderr),
            ):
                result = video_review.transcribe_with_openai(audio_path)

            self.assertIsNone(result)
            self.assertIn("openai CLI is not installed", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
