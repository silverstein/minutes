#!/usr/bin/env python3
"""Tests for the public live-sidekick fixture privacy gate.

Negative cases use reserved or obviously synthetic values only.  They are
detector controls, not copied identifiers.
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[2]
CHECKER_PATH = REPO_ROOT / "scripts/check_live_sidekick_fixture_privacy.py"
FIXTURE_DIR = REPO_ROOT / "crates/core/tests/fixtures/live_sidekick_eval/v1"

SPEC = importlib.util.spec_from_file_location("live_sidekick_fixture_privacy", CHECKER_PATH)
if SPEC is None or SPEC.loader is None:  # pragma: no cover - import machinery guard
    raise RuntimeError("could not load live-sidekick privacy checker")
CHECKER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CHECKER
SPEC.loader.exec_module(CHECKER)


def synthetic_fixture(text: str = "routine synthetic discussion") -> dict[str, object]:
    return {
        "schema_version": 1,
        "id": "temporary-synthetic-control",
        "description": "A temporary detector control.",
        "content_origin": "synthetic",
        "privacy": {
            "generation_method": "behavior_first_from_scratch",
            "source_material": "none",
            "approved_role_tokens": ["USER", "FACILITATOR"],
        },
        "matrix": {"surfaces": ["terminal"], "capture_modes": ["live"]},
        "initial_state": {"user_role": "observer", "posture": "on_demand"},
        "events": [
            {
                "at_ms": 0,
                "kind": "transcript_final",
                "session_id": "SESSION_A",
                "payload": {
                    "event_id": "EVENT_A",
                    "speaker": "FACILITATOR",
                    "text": text,
                },
            }
        ],
        "expectations": {
            "ordered_actions": ["remain_quiet"],
            "forbidden_actions": ["external_mutation"],
            "state_equals": {},
            "required_source_kinds": ["transcript_final"],
            "required_source_event_ids": ["EVENT_A"],
            "provenance_required": True,
            "max_unsolicited_messages": 0,
            "parity_group": "temporary-control",
        },
    }


class LiveSidekickFixturePrivacyTests(unittest.TestCase):
    def check_data(self, data: dict[str, object]) -> list[object]:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "control.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            document, load_findings = CHECKER._load_fixture(path)
            self.assertEqual(load_findings, [])
            self.assertIsNotNone(document)
            return CHECKER.check_fixture(document)

    def test_repository_fixtures_are_synthetic_and_clean(self) -> None:
        documents, findings = CHECKER.check_fixture_dir(FIXTURE_DIR)
        self.assertEqual(len(documents), 18)
        self.assertEqual(findings, [])

    def test_required_scenarios_are_present(self) -> None:
        expected = {
            "capture_mode_parity.json",
            "typed_user_preempts_background.json",
            "transcript_is_untrusted_data.json",
            "role_correction.json",
            "speaker_correction.json",
            "screen_provenance.json",
            "screen_unavailable.json",
            "quiet_cadence.json",
            "meeting_end_handoff.json",
            "gui_turn_continuity.json",
            "routing_disambiguation.json",
            "wrong_session_evidence.json",
            "provider_capability_denied.json",
            "policy_invalidation.json",
            "foreground_invocation_aba.json",
            "background_invocation_aba.json",
            "lifecycle_completion_invalidation.json",
            "finalized_evidence_provenance.json",
        }
        self.assertEqual({path.name for path in FIXTURE_DIR.glob("*.json")}, expected)

    def test_origin_and_privacy_metadata_fail_closed(self) -> None:
        wrong_origin = synthetic_fixture()
        wrong_origin["content_origin"] = "redacted"
        self.assertIn(
            "synthetic_origin_required",
            {item.rule for item in self.check_data(wrong_origin)},
        )

        missing_privacy = synthetic_fixture()
        del missing_privacy["privacy"]
        self.assertIn(
            "privacy_metadata_required",
            {item.rule for item in self.check_data(missing_privacy)},
        )

    def test_speaker_fields_require_declared_allowlisted_role_tokens(self) -> None:
        data = synthetic_fixture()
        data["events"][0]["payload"]["speaker"] = "GUEST_A"
        self.assertIn(
            "speaker_role_token_not_approved",
            {item.rule for item in self.check_data(data)},
        )

    def test_forbidden_field_names_are_rejected_at_any_depth(self) -> None:
        data = synthetic_fixture()
        data["events"][0]["payload"]["company"] = "synthetic value"
        self.assertIn(
            "forbidden_field_name",
            {item.rule for item in self.check_data(data)},
        )

    def test_structural_identifier_patterns_are_rejected(self) -> None:
        controls = {
            "email_address": "sample.person@example.invalid",
            "phone_number": "+1 (555) 010-0000",
            "url": "https://example.invalid/private",
            "ip_address": "192.0.2.1",
            "social_handle": "@synthetic_actor",
            "currency_or_price": "$1234",
            "long_identifier": "sampleidentifier0000000000000000000",
            "secret_format": "sk_abcdefghijklmnopqrstuv",
            "absolute_home_path": "/Users/SAMPLE/private",
            "exact_date": "2040-01-02",
            "street_address": "123 Sample Street",
            "sensitive_domain_content": "the patient record is present",
        }
        for expected_rule, value in controls.items():
            with self.subTest(rule=expected_rule):
                rules = {item.rule for item in self.check_data(synthetic_fixture(value))}
                self.assertIn(expected_rule, rules)

    def test_unapproved_proper_noun_is_a_failing_warning(self) -> None:
        findings = self.check_data(synthetic_fixture("the Example token appears"))
        warning = next(item for item in findings if item.rule == "unexpected_proper_noun")
        self.assertEqual(warning.severity, "warning")

        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary)
            (path / "control.json").write_text(
                json.dumps(synthetic_fixture("the Example token appears")), encoding="utf-8"
            )
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(CHECKER.main([str(path)]), 1)

    def test_private_digest_path_does_not_materialize_raw_ngram_set(self) -> None:
        with mock.patch.object(
            CHECKER,
            "_normalized_ngrams",
            side_effect=AssertionError("raw n-gram set must not be used"),
        ):
            digests = CHECKER._digest_ngrams(
                "one two three four five six seven eight", 7, b"k" * 32
            )
        self.assertEqual(len(digests), 2)

    def test_private_reader_uses_no_follow_descriptor_when_available(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "control.md"
            path.write_text("one two three four five six seven", encoding="utf-8")
            real_open = os.open
            with mock.patch.object(CHECKER.os, "open", wraps=real_open) as opened:
                text, failure = CHECKER._read_private_text(path, 1024)
            self.assertIsNone(failure)
            self.assertEqual(text, "one two three four five six seven")
            flags = opened.call_args.args[1]
            if hasattr(os, "O_NOFOLLOW"):
                self.assertNotEqual(flags & os.O_NOFOLLOW, 0)

    def private_args(self, fixture_dir: Path, *corpus_dirs: Path) -> list[str]:
        args = [
            str(fixture_dir),
            "--private-overlap-only",
            "--aggregate-only",
            "--acknowledge-private-corpus-authorization",
            "--ngram-size",
            "7",
            "--overlap-threshold",
            "0",
        ]
        for corpus_dir in corpus_dirs:
            args.extend(["--private-corpus-dir", str(corpus_dir)])
        return args

    def test_private_overlap_output_is_aggregate_only(self) -> None:
        shared_text = "alpha beta gamma delta epsilon zeta eta theta"
        with tempfile.TemporaryDirectory() as fixture_temp, tempfile.TemporaryDirectory() as corpus_temp:
            fixture_dir = Path(fixture_temp)
            corpus_dir = Path(corpus_temp)
            (fixture_dir / "control.json").write_text(
                json.dumps(synthetic_fixture(shared_text)), encoding="utf-8"
            )
            (corpus_dir / "private.txt").write_text(shared_text, encoding="utf-8")

            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                result = CHECKER.main(self.private_args(fixture_dir, corpus_dir))

            rendered = output.getvalue()
            self.assertEqual(result, 1)
            self.assertEqual(len(rendered.strip().splitlines()), 1)
            self.assertIn("private_overlap=fail", rendered)
            self.assertIn("fixtures_with_overlap=1", rendered)
            self.assertNotIn("temporary-synthetic-control", rendered)
            self.assertNotIn(shared_text, rendered)
            self.assertNotIn(str(corpus_dir), rendered)
            self.assertNotIn("private.txt", rendered)
            self.assertNotRegex(rendered, r"\b[a-f0-9]{32,}\b")

    def test_private_overlap_supports_multiple_roots_and_text_allowlist(self) -> None:
        with (
            tempfile.TemporaryDirectory() as fixture_temp,
            tempfile.TemporaryDirectory() as first_temp,
            tempfile.TemporaryDirectory() as second_temp,
        ):
            fixture_dir = Path(fixture_temp)
            first = Path(first_temp)
            second = Path(second_temp)
            (fixture_dir / "control.json").write_text(
                json.dumps(synthetic_fixture("synthetic words remain distinct here")),
                encoding="utf-8",
            )
            (first / "one.md").write_text("private corpus one stays separate", encoding="utf-8")
            (second / "two.txt").write_text("private corpus two stays separate", encoding="utf-8")
            (second / "ignored.wav").write_bytes(b"synthetic words remain distinct here")

            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                result = CHECKER.main(self.private_args(fixture_dir, first, second))

            self.assertEqual(result, 0)
            self.assertEqual(len(output.getvalue().strip().splitlines()), 1)
            self.assertIn("private_overlap=pass", output.getvalue())
            self.assertIn("roots=2", output.getvalue())
            self.assertIn("files_scanned=2", output.getvalue())

    def test_private_overlap_rejects_symlinks_invalid_utf8_and_oversize_files(self) -> None:
        with tempfile.TemporaryDirectory() as fixture_temp, tempfile.TemporaryDirectory() as corpus_temp:
            fixture_dir = Path(fixture_temp)
            corpus_dir = Path(corpus_temp)
            (fixture_dir / "control.json").write_text(
                json.dumps(synthetic_fixture()), encoding="utf-8"
            )
            (corpus_dir / "invalid.md").write_bytes(b"\xff\xfe")
            (corpus_dir / "large.txt").write_text("0123456789", encoding="utf-8")
            (corpus_dir / "link.md").symlink_to(corpus_dir / "invalid.md")

            output = io.StringIO()
            args = self.private_args(fixture_dir, corpus_dir) + [
                "--private-max-file-bytes",
                "4",
            ]
            with contextlib.redirect_stdout(output):
                result = CHECKER.main(args)

            self.assertEqual(result, 1)
            self.assertEqual(len(output.getvalue().strip().splitlines()), 1)
            self.assertIn("private_overlap=fail", output.getvalue())
            self.assertIn("files_rejected=2", output.getvalue())
            self.assertIn("unreadable=1", output.getvalue())
            self.assertNotIn(str(corpus_dir), output.getvalue())

    def test_private_overlap_requires_authorization_and_refuses_ci(self) -> None:
        with tempfile.TemporaryDirectory() as fixture_temp, tempfile.TemporaryDirectory() as corpus_temp:
            fixture_dir = Path(fixture_temp)
            corpus_dir = Path(corpus_temp)
            (fixture_dir / "control.json").write_text(
                json.dumps(synthetic_fixture()), encoding="utf-8"
            )

            missing_authorization = self.private_args(fixture_dir, corpus_dir)
            missing_authorization.remove("--acknowledge-private-corpus-authorization")
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(CHECKER.main(missing_authorization), 2)
            self.assertEqual(output.getvalue().strip(), "private_overlap=fail configuration_failures=1")

            output = io.StringIO()
            with mock.patch.dict(os.environ, {"CI": "true"}):
                with contextlib.redirect_stdout(output):
                    self.assertEqual(CHECKER.main(self.private_args(fixture_dir, corpus_dir)), 2)
            self.assertEqual(output.getvalue().strip(), "private_overlap=fail configuration_failures=1")

            weak_ngram = self.private_args(fixture_dir, corpus_dir)
            weak_ngram[weak_ngram.index("7")] = "6"
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(CHECKER.main(weak_ngram), 2)
            self.assertEqual(
                output.getvalue().strip(),
                "private_overlap=fail configuration_failures=1",
            )


if __name__ == "__main__":
    unittest.main()
