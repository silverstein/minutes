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


class LiveSidekickFixtureSchemaTests(unittest.TestCase):
    def findings_for(self, data: dict[str, object]) -> set[str]:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "typed_user_preempts_background.json"
            path.write_text(json.dumps(data), encoding="utf-8")
            _, findings = SCHEMA.validate_fixture(path)
            return {finding.rule for finding in findings}

    def test_repository_corpus_is_schema_valid(self) -> None:
        documents, findings = SCHEMA.validate_fixture_dir(FIXTURE_DIR)
        self.assertEqual(len(documents), 14)
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


if __name__ == "__main__":
    unittest.main()
