#!/usr/bin/env python3
"""Negative controls for the feature-to-execution audit; no Rust build needed."""

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import test_coverage as coverage


class CoverageTests(unittest.TestCase):
    def fixture(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        for directory in ("tooling", "crates/core/src", ".github/workflows"):
            (root / directory).mkdir(parents=True)
        (root / "crates/core/Cargo.toml").write_text(
            '[features]\nstreaming = []\nwhisper = []\nspeech = ["streaming", "whisper"]\n'
        )
        (root / "crates/core/src/lib.rs").write_text(
            '#[cfg(all(feature = "streaming", feature = "whisper"))]\npub mod dictation;\n'
        )
        (root / "crates/core/src/dictation.rs").write_text('#[test]\nfn cancellation() {}\n')
        mapping = {
            "schema_version": 1,
            "package": "minutes-core",
            "manifest": "crates/core/Cargo.toml",
            "library": "crates/core/src/lib.rs",
            "scope": "fixture",
            "profiles": {"speech": {
                "features": ["speech"], "platforms": ["linux"], "selection": "feature-modules",
                "workflow": ".github/workflows/ci.yml", "job": "tests",
            }},
        }
        (root / coverage.MAPPING).write_text(json.dumps(mapping))
        (root / ".github/workflows/ci.yml").write_text(
            'on: [pull_request]\njobs:\n  tests:\n    steps:\n      - run: python scripts/test_coverage.py run speech\n'
        )
        return root

    def test_real_repository_profiles_execute_all_discovered_feature_modules(self):
        report = coverage.audit(coverage.ROOT)
        self.assertIn("dictation", report["modules"])
        self.assertIn("speech/linux", report["modules"]["dictation"]["profiles"])

    def test_transitive_features_cover_a_module_without_copying_module_names(self):
        report = coverage.audit(self.fixture())
        self.assertEqual(report["modules"]["dictation"]["profiles"], ["speech/linux"])

    def test_missing_whisper_feature_cannot_claim_dictation_coverage(self):
        root = self.fixture()
        mapping = json.loads((root / coverage.MAPPING).read_text())
        mapping["profiles"]["speech"]["features"] = ["streaming"]
        (root / coverage.MAPPING).write_text(json.dumps(mapping))
        with self.assertRaisesRegex(ValueError, "dictation"):
            coverage.audit(root)

    def test_new_gate_without_profile_fails(self):
        root = self.fixture()
        with (root / "crates/core/src/lib.rs").open("a") as file:
            file.write('#[cfg(feature = "new-engine")]\npub mod new_engine;\n')
        (root / "crates/core/src/new_engine.rs").write_text('#[test]\nfn works() {}\n')
        with self.assertRaisesRegex(ValueError, "new_engine"):
            coverage.audit(root)

    def test_comment_cannot_substitute_for_executing_workflow(self):
        root = self.fixture()
        (root / ".github/workflows/ci.yml").write_text(
            'on: [pull_request]\njobs:\n  tests:\n    steps:\n      # python scripts/test_coverage.py run speech\n      - run: echo skipped\n'
        )
        with self.assertRaisesRegex(ValueError, "missing executing CI"):
            coverage.audit(root)

    def test_allowing_failure_invalidates_coverage(self):
        root = self.fixture()
        workflow = root / ".github/workflows/ci.yml"
        workflow.write_text(workflow.read_text() + '        continue-on-error: true\n')
        with self.assertRaisesRegex(ValueError, "must not continue on error"):
            coverage.audit(root)

    def test_manual_only_workflow_is_not_a_required_execution_gate(self):
        root = self.fixture()
        workflow = root / ".github/workflows/ci.yml"
        workflow.write_text(workflow.read_text().replace("pull_request", "workflow_dispatch"))
        with self.assertRaisesRegex(ValueError, "pull-request workflow"):
            coverage.audit(root)

    def test_platform_gate_cannot_be_covered_on_wrong_platform(self):
        root = self.fixture()
        (root / "crates/core/src/lib.rs").write_text(
            '#[cfg(all(feature = "whisper", target_os = "windows"))]\npub mod dictation;\n'
        )
        with self.assertRaisesRegex(ValueError, "dictation"):
            coverage.audit(root)

    def test_unknown_cfg_fails_closed(self):
        tree = coverage.parse_cfg('all(feature = "absent", undocumented_target)')
        with self.assertRaisesRegex(ValueError, "unsupported cfg"):
            coverage.cfg_enabled(tree, set(), "linux")

    def test_nested_comments_do_not_create_modules(self):
        root = self.fixture()
        library = root / "crates/core/src/lib.rs"
        library.write_text(library.read_text() + '\n/* outer /* nested */ #[cfg(feature="fake")] pub mod fake; */\n')
        self.assertEqual(list(coverage.audit(root)["modules"]), ["dictation"])

    def test_string_and_nested_module_declarations_are_not_root_modules(self):
        root = self.fixture()
        library = root / "crates/core/src/lib.rs"
        library.write_text(library.read_text() + '''
const EXAMPLE: &str = r##"#[cfg(feature="fake")] pub mod fake;"##;
mod nested { #[cfg(feature="nested")] mod nested_feature; }
''')
        self.assertEqual(list(coverage.audit(root)["modules"]), ["dictation"])

    def test_cargo_listing_must_include_each_selected_module(self):
        tests = coverage.listed_tests('dictation::tests::cancel: test\nother::works: test\n2 tests, 0 benchmarks\n')
        coverage.require_module_tests(["dictation"], tests)
        with self.assertRaisesRegex(ValueError, "streaming_whisper"):
            coverage.require_module_tests(["dictation", "streaming_whisper"], tests)

    def test_unknown_feature_is_not_silently_ignored(self):
        with self.assertRaisesRegex(ValueError, "unknown Cargo feature"):
            coverage.feature_closure(["typo"], {"features": {"streaming": []}})

    def test_runner_refuses_zero_module_tests_before_execution(self):
        root = self.fixture()
        with patch.object(coverage.sys, "platform", "linux"), patch.object(
            coverage.subprocess, "run", return_value=SimpleNamespace(returncode=0, stdout="other::works: test\n")
        ) as execute:
            with self.assertRaisesRegex(ValueError, "no tests for: dictation"):
                coverage.run_profile(root, "speech")
            self.assertEqual(execute.call_count, 1)

    def test_runner_uses_discovered_filter_and_propagates_test_failure(self):
        root = self.fixture()
        with patch.object(coverage.sys, "platform", "linux"), patch.object(
            coverage.subprocess, "run", side_effect=[
                SimpleNamespace(returncode=0, stdout="dictation::tests::cancel: test\n"),
                SimpleNamespace(returncode=0, stdout=""),
                SimpleNamespace(returncode=42),
            ]
        ) as execute:
            self.assertEqual(coverage.run_profile(root, "speech"), 42)
            command = execute.call_args_list[2].args[0]
            self.assertIn("dictation::", command)
            self.assertIn("--test-threads=1", command)
            self.assertNotIn("--list", command)
        report = json.loads((root / "target/test-coverage/speech-linux.json").read_text())
        self.assertEqual(report["exit_code"], 42)
        self.assertEqual(report["listed_tests"], ["dictation::tests::cancel"])
        self.assertEqual(report["runnable_tests"], ["dictation::tests::cancel"])
        self.assertEqual(report["ignored_tests"], [])

    def test_runner_refuses_a_module_with_only_ignored_tests(self):
        root = self.fixture()
        listing = SimpleNamespace(returncode=0, stdout="dictation::tests::needs_model: test\n")
        with patch.object(coverage.sys, "platform", "linux"), patch.object(
            coverage.subprocess, "run", side_effect=[listing, listing]
        ) as execute:
            with self.assertRaisesRegex(ValueError, "no tests for: dictation"):
                coverage.run_profile(root, "speech")
            self.assertEqual(execute.call_count, 2)
            self.assertIn("--ignored", execute.call_args_list[1].args[0])


if __name__ == "__main__":
    unittest.main()
