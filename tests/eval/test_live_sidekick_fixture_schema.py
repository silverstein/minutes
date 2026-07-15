#!/usr/bin/env python3
"""Negative controls for the live-sidekick fixture schema gate."""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SCHEMA_PATH = REPO_ROOT / "tests/eval/live_sidekick_fixture_schema.py"
FIXTURE_DIR = REPO_ROOT / "crates/core/tests/fixtures/live_sidekick_eval/v1"
SPEC = importlib.util.spec_from_file_location("live_sidekick_fixture_schema", SCHEMA_PATH)
if SPEC is None or SPEC.loader is None:  # pragma: no cover
    raise RuntimeError("could not load live-sidekick fixture schema")
SCHEMA = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SCHEMA
SPEC.loader.exec_module(SCHEMA)


def load_control() -> dict[str, object]:
    return json.loads((FIXTURE_DIR / "typed_user_preempts_background.json").read_text())


def load_completion_control() -> dict[str, object]:
    return json.loads((FIXTURE_DIR / "foreground_invocation_aba.json").read_text())


def load_provider_control() -> dict[str, object]:
    return json.loads((FIXTURE_DIR / "provider_capability_denied.json").read_text())


class LiveSidekickFixtureSchemaTests(unittest.TestCase):
    def findings_for(self, data: dict[str, object]) -> set[str]:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "typed_user_preempts_background.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            _, findings = SCHEMA.validate_fixture(path)
            return {finding.rule for finding in findings}

    def test_repository_corpus_is_schema_valid(self) -> None:
        documents, findings = SCHEMA.validate_fixture_dir(FIXTURE_DIR)
        self.assertEqual(len(documents), 18)
        self.assertEqual(findings, [])

    def test_unknown_schema_version_fails_closed(self) -> None:
        data = load_control()
        data["schema_version"] = 2
        self.assertIn("unsupported_schema_version", self.findings_for(data))

    def test_unknown_top_level_key_fails_closed(self) -> None:
        data = load_control()
        data["unreviewed_contract"] = True
        self.assertIn("unknown_key", self.findings_for(data))

    def test_contract_only_fixture_cannot_claim_execution(self) -> None:
        data = load_control()
        data["execution"] = {
            "status": "contract_only",
            "target": "future_orchestration",
            "reason": "synthetic negative control",
            "deferred_capabilities": ["future capability"],
            "replays": [],
        }
        self.assertIn("contract_only_cannot_claim_execution", self.findings_for(data))

    def test_projection_must_name_what_is_not_executed(self) -> None:
        data = load_control()
        execution = copy.deepcopy(data["execution"])
        execution["status"] = "executable_projection"
        execution.pop("deferred_assertions", None)
        data["execution"] = execution
        self.assertIn("projection_must_name_deferred_assertions", self.findings_for(data))

    def test_replay_indexes_must_exist_and_be_sorted(self) -> None:
        data = load_control()
        data["execution"]["replays"][0]["event_indexes"] = [999, 0]
        rules = self.findings_for(data)
        self.assertIn("indexes_must_be_unique_sorted", rules)
        self.assertIn("event_index_out_of_range", rules)

    def test_core_runner_cannot_claim_future_event_support(self) -> None:
        data = load_control()
        data["events"].append(
            {
                "at_ms": 100,
                "kind": "focus_changed",
                "session_id": "SESSION_A",
                "payload": {"focus_generation": 2},
            }
        )
        replay = data["execution"]["replays"][0]
        replay["event_indexes"].append(len(data["events"]) - 1)
        replay["expected_reductions"].append({})
        self.assertIn(
            "event_not_supported_by_core_reducer_runner", self.findings_for(data)
        )

    def test_executable_payload_ids_and_types_are_explicit(self) -> None:
        missing_id = load_control()
        del missing_id["events"][0]["payload"]["capture_session_id"]
        self.assertIn("required_key_missing", self.findings_for(missing_id))

        wrong_type = load_control()
        wrong_type["events"][0]["payload"]["capture_session_id"] = 7
        self.assertIn("nonempty_string_required", self.findings_for(wrong_type))

    def test_replay_surface_is_explicit_and_declared(self) -> None:
        missing = load_control()
        del missing["execution"]["replays"][0]["surface"]
        rules = self.findings_for(missing)
        self.assertIn("required_key_missing", rules)
        self.assertIn("replay_surface_not_declared", rules)

        undeclared = load_control()
        undeclared["execution"]["replays"][0]["surface"] = "coach"
        self.assertIn("replay_surface_not_declared", self.findings_for(undeclared))

    def test_reduction_contract_requires_full_actions(self) -> None:
        summarized = load_control()
        summarized["execution"]["replays"][0]["expected_reductions"][0] = {
            "accepted": True,
            "action_types": ["live_transcript_attached"],
            "rejection": None,
        }
        rules = self.findings_for(summarized)
        self.assertIn("required_key_missing", rules)
        self.assertIn("unknown_key", rules)

        rejected_with_action = load_control()
        rejected_with_action["execution"]["replays"][0]["expected_reductions"][0] = {
            "accepted": False,
            "actions": [{"type": "meeting_ended"}],
            "rejection": "invalid_transition",
        }
        self.assertIn(
            "rejected_reduction_actions_must_be_empty",
            self.findings_for(rejected_with_action),
        )

    def test_completion_invocation_reference_must_point_backward(self) -> None:
        data = load_completion_control()
        data["events"][4]["payload"]["invocation_from_event_index"] = 5
        self.assertIn(
            "invocation_reference_must_point_backward", self.findings_for(data)
        )

        negative = load_completion_control()
        negative["events"][4]["payload"]["invocation_from_event_index"] = -1
        self.assertIn(
            "invocation_reference_must_point_backward", self.findings_for(negative)
        )

    def test_completion_invocation_reference_must_be_replayed(self) -> None:
        data = load_completion_control()
        replay = data["execution"]["replays"][0]
        del replay["event_indexes"][1]
        del replay["expected_reductions"][1]
        self.assertIn(
            "invocation_reference_must_be_in_replay", self.findings_for(data)
        )

    def test_completion_invocation_reference_kind_and_id_are_exact(self) -> None:
        wrong_kind = load_completion_control()
        wrong_kind["events"][4]["payload"]["invocation_from_event_index"] = 2
        self.assertIn(
            "invocation_reference_wrong_event_kind", self.findings_for(wrong_kind)
        )

        wrong_id = load_completion_control()
        wrong_id["events"][4]["payload"]["turn_id"] = "FOREGROUND_OTHER"
        self.assertIn("invocation_reference_id_mismatch", self.findings_for(wrong_id))

    def test_completion_cannot_supply_literal_invocation_identity(self) -> None:
        data = load_completion_control()
        data["events"][4]["payload"]["invocation"] = {
            "sequence": 1,
            "source_policy_generation": 0,
            "user_generation": 1,
        }
        self.assertIn("unknown_key", self.findings_for(data))

    def test_provider_binding_is_exact_and_complete(self) -> None:
        missing = load_provider_control()
        del missing["events"][0]["payload"]["attestation_id"]
        self.assertIn("required_key_missing", self.findings_for(missing))

        boolean_bag = load_provider_control()
        boolean_bag["events"][0]["payload"]["cancellation"] = True
        rules = self.findings_for(boolean_bag)
        self.assertIn("unknown_key", rules)

        legacy_alias = load_provider_control()
        legacy_alias["events"][0]["kind"] = "provider_capability_changed"
        rules = self.findings_for(legacy_alias)
        self.assertIn("unsupported_event_kind", rules)

        zero_generation = load_provider_control()
        zero_generation["events"][0]["payload"]["binding_generation"] = 0
        rules = self.findings_for(zero_generation)
        self.assertIn("positive_integer_required", rules)

        unsupported_enum = load_provider_control()
        unsupported_enum["events"][0]["payload"]["isolation_profile"] = "assumed"
        self.assertIn("unsupported_value", self.findings_for(unsupported_enum))


if __name__ == "__main__":
    unittest.main()
