from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = REPO_ROOT / "scripts" / "archive_format_census.py"
SPEC = importlib.util.spec_from_file_location("archive_format_census", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
CENSUS = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CENSUS
SPEC.loader.exec_module(CENSUS)


class ArchiveFormatCensusTests(unittest.TestCase):
    def test_scan_reports_aggregates_without_names_paths_or_content(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "synthetic-archive"
            root.mkdir()
            secret_filename = "Privileged Client Alpha Settlement.pdf"
            secret_content = "SYNTHETIC_SECRET_CANARY"
            (root / secret_filename).write_text(secret_content, encoding="utf-8")
            (root / "legacy.WPD").write_bytes(b"wpd")
            (root / "letter.DOCX").write_bytes(b"docx")
            (root / ".hidden-client.rtf").write_bytes(b"")
            (root / "README").write_bytes(b"x")
            (root / "placeholder.pdf.icloud").write_bytes(b"")

            package = root / "Synthetic Matter.pages"
            package.mkdir()
            (package / "index.xml").write_text(
                "PACKAGE_SECRET_CANARY", encoding="utf-8"
            )

            symlink_created = False
            try:
                os.symlink(root / secret_filename, root / "alias-to-secret")
                symlink_created = True
            except (NotImplementedError, OSError):
                pass

            report = CENSUS.scan(root).as_dict()
            serialized = json.dumps(report, sort_keys=True)

            self.assertEqual(report["schema"], CENSUS.SCHEMA_VERSION)
            self.assertEqual(report["status"], "complete")
            self.assertEqual(report["summary"]["regular_files"], 6)
            self.assertEqual(report["summary"]["packages"], 1)
            self.assertEqual(report["signals"]["icloud_placeholders"], 1)
            self.assertEqual(report["signals"]["hidden_artifacts"], 1)
            self.assertEqual(
                report["signals"]["symlinks_skipped"], 1 if symlink_created else 0
            )
            self.assertNotIn("index.xml", serialized)
            self.assertNotIn(secret_filename, serialized)
            self.assertNotIn(secret_content, serialized)
            self.assertNotIn("PACKAGE_SECRET_CANARY", serialized)
            self.assertNotIn(temporary_directory, serialized)

            formats = {
                item["extension"]: item for item in report["formats"]
            }
            self.assertEqual(formats[".pdf"]["files"], 1)
            self.assertEqual(formats[".wpd"]["files"], 1)
            self.assertEqual(formats[".docx"]["files"], 1)
            self.assertEqual(formats[".pages"]["packages"], 1)
            self.assertNotIn(".xml", formats)

    def test_scan_is_bounded_when_max_files_is_reached(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "bounded"
            root.mkdir()
            for index in range(4):
                (root / f"synthetic-{index}.txt").write_text(
                    "synthetic", encoding="utf-8"
                )

            report = CENSUS.scan(root, max_files=2).as_dict()

            self.assertEqual(report["status"], "bounded")
            self.assertEqual(report["summary"]["artifacts"], 2)

    def test_scan_never_opens_regular_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory) / "no-content-read"
            root.mkdir()
            (root / "synthetic.pdf").write_bytes(b"must-not-be-opened")

            with mock.patch(
                "builtins.open",
                side_effect=AssertionError("regular file content was opened"),
            ):
                report = CENSUS.scan(root).as_dict()

            self.assertEqual(report["summary"]["regular_files"], 1)
            self.assertFalse(report["privacy"]["document_content_read"])

    def test_home_and_filesystem_roots_are_refused(self) -> None:
        with self.assertRaisesRegex(ValueError, "refusing"):
            CENSUS.validate_root(Path.home())
        with self.assertRaisesRegex(ValueError, "refusing"):
            CENSUS.validate_root(Path(Path.home().anchor))

    def test_unusual_extensions_are_collapsed(self) -> None:
        self.assertEqual(CENSUS._extension_for_name("document"), "<none>")
        self.assertEqual(
            CENSUS._extension_for_name("document.client-secret-extension"),
            "<other>",
        )
        self.assertEqual(CENSUS._extension_for_name("ARCHIVE.TAR.GZ"), ".tar.gz")


if __name__ == "__main__":
    unittest.main()
