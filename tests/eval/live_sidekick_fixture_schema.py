#!/usr/bin/env python3
"""Versioned schema validator for public live-sidekick behavior fixtures.

This module intentionally has no third-party dependencies.  CI uses it before
the executable runners so malformed or silently reclassified contracts fail
with a stable JSON path instead of a runner-specific exception.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Sequence


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_FIXTURE_DIR = REPO_ROOT / "crates/core/tests/fixtures/live_sidekick_eval/v1"
SCHEMA_VERSION = 1

TOP_LEVEL_KEYS = {
    "schema_version",
    "id",
    "description",
    "content_origin",
    "privacy",
    "matrix",
    "initial_state",
    "events",
    "expectations",
    "execution",
}
PRIVACY_KEYS = {
    "generation_method",
    "source_material",
    "approved_role_tokens",
}
MATRIX_KEYS = {"surfaces", "capture_modes"}
INITIAL_STATE_KEYS = {"user_role", "posture"}
EVENT_KEYS = {"at_ms", "kind", "session_id", "payload"}
EXPECTATION_KEYS = {
    "ordered_actions",
    "forbidden_actions",
    "state_equals",
    "required_source_kinds",
    "required_source_event_ids",
    "provenance_required",
    "max_unsolicited_messages",
    "parity_group",
}
EXECUTION_KEYS = {
    "status",
    "target",
    "reason",
    "deferred_capabilities",
    "deferred_assertions",
    "replays",
    "cases",
}
REPLAY_KEYS = {
    "name",
    "session_id",
    "event_indexes",
    "expected_reductions",
    "expected_state",
}
ROUTING_CASE_KEYS = {"request_id", "outcome", "skill_id"}

SURFACES = {"terminal", "gui", "coach"}
CAPTURE_MODES = {"live", "recording"}
USER_ROLES = {
    "presenter",
    "participant",
    "observer",
    "decision_maker",
    "technical_responder",
}
POSTURES = {"on_demand", "strategist", "silent_watch", "decision_tracker"}
SOURCE_KINDS = {
    "transcript_final",
    "screen_image",
    "desktop_metadata",
    "meeting_artifact",
    "coach_nudge",
    "repository_result",
    "user_statement",
}
EVENT_KINDS = {
    "background_started",
    "capture_started",
    "capture_stopped",
    "coach_nudge",
    "focus_changed",
    "foreground_completed",
    "foreground_started",
    "meeting_finalized",
    "posture_changed",
    "processing_started",
    "provider_capability_changed",
    "role_changed",
    "screen_disclosed",
    "screen_inspected",
    "screen_requested",
    "screen_state_changed",
    "source_policy_invalidated",
    "speaker_corrected",
    "surface_request",
    "transcript_final",
    "user_message",
}
EXECUTION_STATUSES = {"executable", "executable_projection", "contract_only"}
EXECUTION_TARGETS = {"core_reducer", "skill_routing", "future_orchestration"}
CORE_REDUCER_EVENT_KINDS = {
    "background_started",
    "capture_started",
    "capture_stopped",
    "coach_nudge",
    "meeting_finalized",
    "posture_changed",
    "processing_started",
    "role_changed",
    "screen_disclosed",
    "source_policy_invalidated",
    "speaker_corrected",
    "transcript_final",
    "user_message",
}

# Exact payload shape is part of schema v1. Optional fields are allowed only
# where the product contract explicitly models their absence.
EVENT_PAYLOAD_KEYS: dict[str, tuple[set[str], set[str]]] = {
    "background_started": ({"run_id"}, {"source_policy_generation"}),
    "capture_started": ({"capture_mode", "capture_session_id"}, set()),
    "capture_stopped": ({"capture_session_id"}, {"final_live_event_id"}),
    "coach_nudge": ({"capture_session_id", "event_id", "text"}, set()),
    "focus_changed": ({"focus_generation"}, set()),
    "foreground_completed": ({"turn_id"}, set()),
    "foreground_started": ({"inference_call", "turn_id"}, set()),
    "meeting_finalized": ({"capture_session_id", "meeting_ref"}, set()),
    "posture_changed": ({"posture", "source"}, set()),
    "processing_started": ({"capture_session_id", "stage"}, set()),
    "provider_capability_changed": (
        {
            "ambient_filesystem_denied",
            "arbitrary_writes_denied",
            "cancellation",
            "provider_id",
            "unapproved_tools_denied",
        },
        set(),
    ),
    "role_changed": ({"role", "source", "source_event_id"}, set()),
    "screen_disclosed": ({"capture_session_id", "event_id", "opaque_ref"}, set()),
    "screen_inspected": ({"event_id", "turn_id"}, set()),
    "screen_requested": ({"source", "source_event_id", "text", "turn_id"}, set()),
    "screen_state_changed": ({"state"}, {"opaque_ref"}),
    "source_policy_invalidated": (
        {"reason", "source_policy_generation"},
        set(),
    ),
    "speaker_corrected": (
        {"from_speaker", "source", "source_event_id", "to_speaker"},
        set(),
    ),
    "surface_request": ({"request_id", "text"}, set()),
    "transcript_final": (
        {"capture_session_id", "event_id", "speaker", "text"},
        {"speaker_confidence"},
    ),
    "user_message": ({"source_event_id", "text", "turn_id"}, set()),
}
PAYLOAD_STRING_FIELDS = {
    "capture_session_id",
    "corrected_speaker",
    "event_id",
    "final_live_event_id",
    "from_speaker",
    "meeting_ref",
    "opaque_ref",
    "provider_id",
    "reason",
    "request_id",
    "run_id",
    "source_event_id",
    "speaker",
    "text",
    "to_speaker",
    "turn_id",
}
PAYLOAD_INTEGER_FIELDS = {"focus_generation", "source_policy_generation"}
PAYLOAD_BOOLEAN_FIELDS = {
    "ambient_filesystem_denied",
    "arbitrary_writes_denied",
    "cancellation",
    "unapproved_tools_denied",
}
PAYLOAD_ENUMS = {
    "capture_mode": CAPTURE_MODES,
    "inference_call": {"fresh"},
    "posture": POSTURES,
    "role": USER_ROLES,
    "source": {"typed_user"},
    "speaker_confidence": {"inferred", "corrected_mapping"},
    "stage": {"transcribing"},
    "state": {"available", "cleaned", "denied", "disabled", "stopped", "waiting"},
}


@dataclass(frozen=True)
class Finding:
    fixture: str
    path: str
    rule: str


def _is_nonempty_string(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def _is_string_list(value: Any, *, nonempty: bool = False) -> bool:
    return (
        isinstance(value, list)
        and (not nonempty or bool(value))
        and all(_is_nonempty_string(item) for item in value)
    )


def _expect_keys(
    findings: list[Finding], fixture: str, path: str, value: Any, expected: set[str]
) -> bool:
    if not isinstance(value, dict):
        findings.append(Finding(fixture, path, "object_required"))
        return False
    actual = set(value)
    for key in sorted(expected - actual):
        findings.append(Finding(fixture, f"{path}.{key}", "required_key_missing"))
    for key in sorted(actual - expected):
        findings.append(Finding(fixture, f"{path}.{key}", "unknown_key"))
    return actual == expected


def _enum_list(
    findings: list[Finding], fixture: str, path: str, value: Any, allowed: set[str]
) -> None:
    if not _is_string_list(value, nonempty=True):
        findings.append(Finding(fixture, path, "nonempty_string_array_required"))
        return
    if len(value) != len(set(value)):
        findings.append(Finding(fixture, path, "duplicate_value"))
    for index, item in enumerate(value):
        if item not in allowed:
            findings.append(Finding(fixture, f"{path}[{index}]", "unsupported_value"))


def _validate_execution(
    data: dict[str, Any], fixture: str, findings: list[Finding]
) -> None:
    execution = data.get("execution")
    if not isinstance(execution, dict):
        findings.append(Finding(fixture, "$.execution", "object_required"))
        return
    for key in sorted(set(execution) - EXECUTION_KEYS):
        findings.append(Finding(fixture, f"$.execution.{key}", "unknown_key"))

    status = execution.get("status")
    target = execution.get("target")
    if status not in EXECUTION_STATUSES:
        findings.append(Finding(fixture, "$.execution.status", "unsupported_value"))
        return
    if target not in EXECUTION_TARGETS:
        findings.append(Finding(fixture, "$.execution.target", "unsupported_value"))
        return

    replays = execution.get("replays")
    cases = execution.get("cases")
    deferred_capabilities = execution.get("deferred_capabilities")
    deferred_assertions = execution.get("deferred_assertions")

    if status == "contract_only":
        if target != "future_orchestration":
            findings.append(
                Finding(fixture, "$.execution.target", "contract_only_target_must_be_future")
            )
        if not _is_nonempty_string(execution.get("reason")):
            findings.append(
                Finding(fixture, "$.execution.reason", "nonempty_string_required")
            )
        if not _is_string_list(deferred_capabilities, nonempty=True):
            findings.append(
                Finding(
                    fixture,
                    "$.execution.deferred_capabilities",
                    "nonempty_string_array_required",
                )
            )
        if replays is not None or cases is not None or deferred_assertions is not None:
            findings.append(
                Finding(fixture, "$.execution", "contract_only_cannot_claim_execution")
            )
        return

    if execution.get("reason") is not None or deferred_capabilities is not None:
        findings.append(Finding(fixture, "$.execution", "executable_has_deferred_fields"))

    if status == "executable_projection":
        if not _is_string_list(deferred_assertions, nonempty=True):
            findings.append(
                Finding(
                    fixture,
                    "$.execution.deferred_assertions",
                    "projection_must_name_deferred_assertions",
                )
            )
    elif deferred_assertions is not None:
        findings.append(
            Finding(fixture, "$.execution.deferred_assertions", "full_execution_cannot_defer")
        )

    if target == "core_reducer":
        if not isinstance(replays, list) or not replays:
            findings.append(
                Finding(fixture, "$.execution.replays", "nonempty_array_required")
            )
            return
        if cases is not None:
            findings.append(Finding(fixture, "$.execution.cases", "wrong_target_field"))
        event_count = len(data.get("events", [])) if isinstance(data.get("events"), list) else 0
        replay_names: set[str] = set()
        for replay_index, replay in enumerate(replays):
            path = f"$.execution.replays[{replay_index}]"
            _expect_keys(findings, fixture, path, replay, REPLAY_KEYS)
            if not isinstance(replay, dict):
                continue
            name = replay.get("name")
            if not _is_nonempty_string(name):
                findings.append(Finding(fixture, f"{path}.name", "nonempty_string_required"))
            elif name in replay_names:
                findings.append(Finding(fixture, f"{path}.name", "duplicate_replay_name"))
            else:
                replay_names.add(name)
            if not _is_nonempty_string(replay.get("session_id")):
                findings.append(
                    Finding(fixture, f"{path}.session_id", "nonempty_string_required")
                )
            indexes = replay.get("event_indexes")
            if (
                not isinstance(indexes, list)
                or not indexes
                or not all(isinstance(item, int) and not isinstance(item, bool) for item in indexes)
            ):
                findings.append(
                    Finding(fixture, f"{path}.event_indexes", "nonempty_integer_array_required")
                )
            else:
                if indexes != sorted(set(indexes)):
                    findings.append(
                        Finding(fixture, f"{path}.event_indexes", "indexes_must_be_unique_sorted")
                    )
                for item_index, event_index in enumerate(indexes):
                    if event_index < 0 or event_index >= event_count:
                        findings.append(
                            Finding(
                                fixture,
                                f"{path}.event_indexes[{item_index}]",
                                "event_index_out_of_range",
                            )
                        )
                    else:
                        event_kind = data["events"][event_index].get("kind")
                        if event_kind not in CORE_REDUCER_EVENT_KINDS:
                            findings.append(
                                Finding(
                                    fixture,
                                    f"{path}.event_indexes[{item_index}]",
                                    "event_not_supported_by_core_reducer_runner",
                                )
                            )
            reductions = replay.get("expected_reductions")
            if not isinstance(reductions, list):
                findings.append(
                    Finding(fixture, f"{path}.expected_reductions", "array_required")
                )
            elif isinstance(indexes, list) and len(reductions) != len(indexes):
                findings.append(
                    Finding(fixture, f"{path}.expected_reductions", "one_per_event_required")
                )
            if not isinstance(replay.get("expected_state"), dict):
                findings.append(Finding(fixture, f"{path}.expected_state", "object_required"))
    elif target == "skill_routing":
        if replays is not None:
            findings.append(Finding(fixture, "$.execution.replays", "wrong_target_field"))
        if not isinstance(cases, list) or not cases:
            findings.append(Finding(fixture, "$.execution.cases", "nonempty_array_required"))
            return
        surface_requests = {
            event.get("payload", {}).get("request_id")
            for event in data.get("events", [])
            if isinstance(event, dict) and event.get("kind") == "surface_request"
        }
        for case_index, case in enumerate(cases):
            path = f"$.execution.cases[{case_index}]"
            _expect_keys(findings, fixture, path, case, ROUTING_CASE_KEYS)
            if not isinstance(case, dict):
                continue
            if case.get("request_id") not in surface_requests:
                findings.append(Finding(fixture, f"{path}.request_id", "unknown_request_id"))
            if case.get("outcome") not in {"skill", "clarify"}:
                findings.append(Finding(fixture, f"{path}.outcome", "unsupported_value"))
            skill_id = case.get("skill_id")
            if case.get("outcome") == "skill" and not _is_nonempty_string(skill_id):
                findings.append(Finding(fixture, f"{path}.skill_id", "skill_id_required"))
            if case.get("outcome") == "clarify" and skill_id is not None:
                findings.append(Finding(fixture, f"{path}.skill_id", "clarify_skill_must_be_null"))


def _validate_payload_values(
    fixture: str, path_prefix: str, payload: dict[str, Any], findings: list[Finding]
) -> None:
    for key, value in payload.items():
        path = f"{path_prefix}.payload.{key}"
        if key in PAYLOAD_STRING_FIELDS:
            if not _is_nonempty_string(value):
                findings.append(Finding(fixture, path, "nonempty_string_required"))
        elif key in PAYLOAD_INTEGER_FIELDS:
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                findings.append(Finding(fixture, path, "nonnegative_integer_required"))
        elif key in PAYLOAD_BOOLEAN_FIELDS:
            if not isinstance(value, bool):
                findings.append(Finding(fixture, path, "boolean_required"))
        elif key in PAYLOAD_ENUMS:
            if value not in PAYLOAD_ENUMS[key]:
                findings.append(Finding(fixture, path, "unsupported_value"))


def validate_fixture(path: Path) -> tuple[dict[str, Any] | None, list[Finding]]:
    fixture = path.name
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None, [Finding(fixture, "$", "valid_utf8_json_required")]
    if not isinstance(data, dict):
        return None, [Finding(fixture, "$", "object_required")]

    findings: list[Finding] = []
    _expect_keys(findings, fixture, "$", data, TOP_LEVEL_KEYS)
    if data.get("schema_version") != SCHEMA_VERSION:
        findings.append(Finding(fixture, "$.schema_version", "unsupported_schema_version"))
    fixture_id = data.get("id")
    if not _is_nonempty_string(fixture_id):
        findings.append(Finding(fixture, "$.id", "nonempty_string_required"))
    elif fixture_id != path.stem.replace("_", "-"):
        findings.append(Finding(fixture, "$.id", "id_must_match_filename"))
    if not _is_nonempty_string(data.get("description")):
        findings.append(Finding(fixture, "$.description", "nonempty_string_required"))
    if data.get("content_origin") != "synthetic":
        findings.append(Finding(fixture, "$.content_origin", "synthetic_origin_required"))

    _expect_keys(findings, fixture, "$.privacy", data.get("privacy"), PRIVACY_KEYS)
    matrix = data.get("matrix")
    _expect_keys(findings, fixture, "$.matrix", matrix, MATRIX_KEYS)
    if isinstance(matrix, dict):
        _enum_list(findings, fixture, "$.matrix.surfaces", matrix.get("surfaces"), SURFACES)
        _enum_list(
            findings,
            fixture,
            "$.matrix.capture_modes",
            matrix.get("capture_modes"),
            CAPTURE_MODES,
        )

    initial = data.get("initial_state")
    _expect_keys(findings, fixture, "$.initial_state", initial, INITIAL_STATE_KEYS)
    if isinstance(initial, dict):
        if initial.get("user_role") not in USER_ROLES:
            findings.append(Finding(fixture, "$.initial_state.user_role", "unsupported_value"))
        if initial.get("posture") not in POSTURES:
            findings.append(Finding(fixture, "$.initial_state.posture", "unsupported_value"))

    events = data.get("events")
    if not isinstance(events, list) or not events:
        findings.append(Finding(fixture, "$.events", "nonempty_array_required"))
    else:
        previous_at = -1
        for index, event in enumerate(events):
            path_prefix = f"$.events[{index}]"
            if not isinstance(event, dict):
                findings.append(Finding(fixture, path_prefix, "object_required"))
                continue
            required_event_keys = EVENT_KEYS - ({"session_id"} if event.get("kind") == "surface_request" else set())
            _expect_keys(findings, fixture, path_prefix, event, required_event_keys)
            at_ms = event.get("at_ms")
            if not isinstance(at_ms, int) or isinstance(at_ms, bool) or at_ms < 0:
                findings.append(Finding(fixture, f"{path_prefix}.at_ms", "nonnegative_integer_required"))
            elif at_ms < previous_at:
                findings.append(Finding(fixture, f"{path_prefix}.at_ms", "events_must_be_time_ordered"))
            else:
                previous_at = at_ms
            event_kind = event.get("kind")
            if event_kind not in EVENT_KINDS:
                findings.append(Finding(fixture, f"{path_prefix}.kind", "unsupported_event_kind"))
            if event_kind != "surface_request" and not _is_nonempty_string(event.get("session_id")):
                findings.append(Finding(fixture, f"{path_prefix}.session_id", "nonempty_string_required"))
            payload = event.get("payload")
            if not isinstance(payload, dict):
                findings.append(Finding(fixture, f"{path_prefix}.payload", "object_required"))
            elif event_kind in EVENT_PAYLOAD_KEYS:
                required_payload, optional_payload = EVENT_PAYLOAD_KEYS[event_kind]
                actual_payload = set(payload)
                for key in sorted(required_payload - actual_payload):
                    findings.append(
                        Finding(
                            fixture,
                            f"{path_prefix}.payload.{key}",
                            "required_key_missing",
                        )
                    )
                for key in sorted(actual_payload - required_payload - optional_payload):
                    findings.append(
                        Finding(
                            fixture,
                            f"{path_prefix}.payload.{key}",
                            "unknown_key",
                        )
                    )
                _validate_payload_values(fixture, path_prefix, payload, findings)

    expectations = data.get("expectations")
    _expect_keys(findings, fixture, "$.expectations", expectations, EXPECTATION_KEYS)
    if isinstance(expectations, dict):
        for key in (
            "ordered_actions",
            "forbidden_actions",
            "required_source_kinds",
            "required_source_event_ids",
        ):
            if not _is_string_list(expectations.get(key)):
                findings.append(Finding(fixture, f"$.expectations.{key}", "string_array_required"))
        for index, source_kind in enumerate(expectations.get("required_source_kinds", [])):
            if source_kind not in SOURCE_KINDS:
                findings.append(
                    Finding(
                        fixture,
                        f"$.expectations.required_source_kinds[{index}]",
                        "unsupported_value",
                    )
                )
        if not isinstance(expectations.get("state_equals"), dict):
            findings.append(Finding(fixture, "$.expectations.state_equals", "object_required"))
        if not isinstance(expectations.get("provenance_required"), bool):
            findings.append(Finding(fixture, "$.expectations.provenance_required", "boolean_required"))
        maximum = expectations.get("max_unsolicited_messages")
        if not isinstance(maximum, int) or isinstance(maximum, bool) or maximum < 0:
            findings.append(
                Finding(
                    fixture,
                    "$.expectations.max_unsolicited_messages",
                    "nonnegative_integer_required",
                )
            )
        if not _is_nonempty_string(expectations.get("parity_group")):
            findings.append(Finding(fixture, "$.expectations.parity_group", "nonempty_string_required"))

    _validate_execution(data, fixture, findings)
    return data, findings


def validate_fixture_dir(fixture_dir: Path) -> tuple[list[dict[str, Any]], list[Finding]]:
    if not fixture_dir.is_dir():
        return [], [Finding(fixture_dir.name or "fixtures", "$", "fixture_directory_missing")]
    paths = sorted(fixture_dir.glob("*.json"))
    if not paths:
        return [], [Finding(fixture_dir.name, "$", "fixture_json_required")]
    documents: list[dict[str, Any]] = []
    findings: list[Finding] = []
    fixture_ids: set[str] = set()
    for path in paths:
        document, path_findings = validate_fixture(path)
        findings.extend(path_findings)
        if document is None:
            continue
        documents.append(document)
        fixture_id = document.get("id")
        if isinstance(fixture_id, str):
            if fixture_id in fixture_ids:
                findings.append(Finding(path.name, "$.id", "duplicate_fixture_id"))
            fixture_ids.add(fixture_id)
    return documents, findings


def _print_summary(documents: Iterable[dict[str, Any]], findings: Sequence[Finding]) -> None:
    documents = list(documents)
    counts = {status: 0 for status in sorted(EXECUTION_STATUSES)}
    for document in documents:
        status = document.get("execution", {}).get("status")
        if status in counts:
            counts[status] += 1
    outcome = "pass" if not findings else "fail"
    rendered_counts = " ".join(f"{key}={value}" for key, value in counts.items())
    print(f"schema={outcome} version={SCHEMA_VERSION} fixtures={len(documents)} {rendered_counts}")
    for finding in findings:
        print(f"finding fixture={finding.fixture} path={finding.path} rule={finding.rule}")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Validate versioned live-sidekick fixtures.")
    parser.add_argument("fixture_dir", nargs="?", type=Path, default=DEFAULT_FIXTURE_DIR)
    args = parser.parse_args(argv)
    documents, findings = validate_fixture_dir(args.fixture_dir)
    _print_summary(documents, findings)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
