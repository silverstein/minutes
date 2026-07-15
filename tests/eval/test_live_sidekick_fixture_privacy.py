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
import sys
import tempfile
import unittest
from pathlib import Path


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
        self.assertEqual(len(documents), 14)
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

    def test_private_overlap_output_never_echoes_text_paths_or_hashes(self) -> None:
        shared_text = "alpha beta gamma delta epsilon zeta"
        with tempfile.TemporaryDirectory() as fixture_temp, tempfile.TemporaryDirectory() as corpus_temp:
            fixture_dir = Path(fixture_temp)
            corpus_dir = Path(corpus_temp)
            (fixture_dir / "control.json").write_text(
                json.dumps(synthetic_fixture(shared_text)), encoding="utf-8"
            )
            (corpus_dir / "private.txt").write_text(shared_text, encoding="utf-8")

            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                result = CHECKER.main(
                    [
                        str(fixture_dir),
                        "--private-corpus-dir",
                        str(corpus_dir),
                        "--ngram-size",
                        "5",
                        "--overlap-threshold",
                        "0",
                    ]
                )

            rendered = output.getvalue()
            self.assertEqual(result, 1)
            self.assertIn("overlap=fail", rendered)
            self.assertIn("temporary-synthetic-control", rendered)
            self.assertNotIn(shared_text, rendered)
            self.assertNotIn(str(corpus_dir), rendered)
            self.assertNotIn("private.txt", rendered)
            self.assertNotRegex(rendered, r"\b[a-f0-9]{32,}\b")


if __name__ == "__main__":
    unittest.main()
