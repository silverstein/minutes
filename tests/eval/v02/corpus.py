"""Public synthetic conformance corpus: 3 domains, 24 decision pairs, 120 cases.

This is deliberately reproducible smoke coverage, not independent human-authored
holdout evidence. External suites use the same JSON schema without this generator.
"""
from __future__ import annotations

import json
from pathlib import Path


DOMAINS = {
    "software": ["billing cadence", "release channel", "incident paging", "backup region",
                 "onboarding trial", "support hours", "archive retention", "api versioning"],
    "museum": ["gallery lighting", "exhibit rotation", "ticket allocation", "artifact transport",
               "visitor routing", "school sessions", "storage climate", "restoration schedule"],
    "manufacturing": ["supplier inspection", "shift handover", "spare inventory", "machine calibration",
                      "packing material", "dispatch route", "maintenance window", "quality sampling"],
}


def meeting(title: str, date: str, body: str, *, people: list[str] | None = None,
            decisions: list[dict] | None = None, restricted: bool = False) -> str:
    # JSON scalar/list values are valid YAML, avoiding a YAML dependency.
    fields = {"title": title, "type": "meeting", "date": date, "duration": "10m",
              "attendees": people or ["Ann Carter", "Joanna Flores"],
              "people": people or ["Ann Carter", "Joanna Flores"],
              "decisions": decisions or [],
              "speaker_map": [{"speaker_label": "SPEAKER_0", "name": (people or ["Ann Carter"])[0],
                               "confidence": "high", "source": "manual"}]}
    if restricted:
        fields["sensitivity"] = "restricted"
    header = "\n".join(f"{key}: {json.dumps(value, ensure_ascii=False)}" for key, value in fields.items())
    return f"---\n{header}\n---\n\n## Transcript\n\n{body}\n"


def synthetic_suite() -> dict:
    documents, cases = [], []
    for domain, topics in DOMAINS.items():
        for index, topic in enumerate(topics):
            stem = f"{domain}-{index + 1:02}"
            old_path, new_path = f"{stem}-initial.md", f"{stem}-updated.md"
            initial_id, current_id = f"{stem}-v1", f"{stem}-v2"
            initial_text = f"For {domain} {topic}, approve the pilot option through review."
            current_text = f"For {domain} {topic}, replace the pilot option with the permanent option."
            for current, path, text, decision_id in [(False, old_path, initial_text, initial_id),
                                                      (True, new_path, current_text, current_id)]:
                date = f"2026-{'02' if current else '01'}-{index + 1:02}T10:00:00Z"
                decision = {"id": decision_id, "topic": f"{domain} {topic}", "text": text,
                            "authority": "high", "effective_date": date[:10]}
                if current:
                    decision["supersedes"] = initial_id
                body = f"[SPEAKER_0 0:05] {text}\n\n[SPEAKER_1 0:20] The planning owner is Joanna Flores."
                if current:
                    body += f"\n\n[SPEAKER_0 0:38] This explicitly supersedes decision {initial_id}."
                documents.append({"path": path, "content": meeting(f"{domain.title()} {topic} review",
                                                                    date, body, decisions=[decision])})
            first, second = topic.split()
            queries = [f"{domain} {topic}", f"{second} {domain} {first}",
                       f"{domain.upper()} {topic.upper()}", f"{domain} {first} {second[:3]}"]
            for variant, query in enumerate(queries):
                cases.append({"id": f"{stem}-retrieval-{variant}", "domain": domain,
                              "category": ["literal", "token_order", "casefold", "prefix"][variant],
                              "kind": "retrieval", "query": query,
                              "relevance": {old_path: 1, new_path: 1}})
            cases.append({"id": f"{stem}-current", "domain": domain, "kind": "currentness",
                          "topic": f"{domain} {topic}", "current_id": current_id,
                          "superseded_id": initial_id, "current_path": new_path})
    for name in ("unicorn acquisition", "board approved dividend", "secret lunar budget"):
        cases.append({"id": name.replace(" ", "-"), "domain": "adversarial", "kind": "abstention",
                      "query": name, "relevance": {}})
    documents.extend([
        {"path": "identity-ann.md", "content": meeting("Identity Ann", "2026-03-01T10:00:00Z",
          "[SPEAKER_0 0:01] Ann Carter owns the ambernotebook project.", people=["Ann Carter"])},
        {"path": "identity-joanna.md", "content": meeting("Identity Joanna", "2026-03-02T10:00:00Z",
          "[SPEAKER_0 0:01] Joanna Flores is planning the ambernotebook project.", people=["Joanna Flores"])},
        {"path": "identity-ann-other.md", "content": meeting("Identity second Ann", "2026-03-03T10:00:00Z",
          "[SPEAKER_0 0:01] Ann Hughes owns a different ambernotebook project.", people=["Ann Hughes"])},
        {"path": "restricted.md", "content": meeting("Restricted source", "2026-03-04T10:00:00Z",
          "[SPEAKER_0 0:01] The restrictedmarker budget is 91723.", restricted=True)},
    ])
    return {"schema_version": 1, "provenance": {"kind": "public-synthetic", "blinded": False,
            "description": "Generated lexical, temporal, identity and privacy conformance smoke; not model quality evidence."},
            "documents": documents, "cases": cases}


def validate_suite(suite: dict) -> None:
    if suite.get("schema_version") != 1 or not isinstance(suite.get("provenance"), dict):
        raise ValueError("suite needs schema_version=1 and provenance")
    paths = []
    for doc in suite["documents"]:
        path = Path(doc["path"])
        if path.is_absolute() or ".." in path.parts or path.suffix != ".md":
            raise ValueError(f"unsafe document path: {path}")
        if not isinstance(doc["content"], str):
            raise ValueError("document content must be text")
        paths.append(doc["path"])
    ids = [case["id"] for case in suite["cases"]]
    if len(set(paths)) != len(paths) or len(set(ids)) != len(ids) or not ids:
        raise ValueError("document paths and nonempty case IDs must be unique")
    for case in suite["cases"]:
        if case["kind"] not in ("retrieval", "abstention", "currentness"):
            raise ValueError(f"unsupported case kind: {case['kind']}")
        if case["kind"] == "currentness":
            if case["current_path"] not in paths:
                raise ValueError("currentness source absent from corpus")
        else:
            if not set(case["relevance"]).issubset(paths):
                raise ValueError("relevance refers to absent document")
            if any(isinstance(grade, bool) or not isinstance(grade, int) or grade < 1
                   for grade in case["relevance"].values()):
                raise ValueError("relevance grades must be positive integers")
            if (case["kind"] == "abstention") != (not case["relevance"]):
                raise ValueError("only abstention cases may have empty relevance")


def write_suite(suite: dict, root: Path, size: int) -> int:
    validate_suite(suite)
    for document in suite["documents"]:
        path = root / document["path"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(document["content"], encoding="utf-8")
    for index in range(max(0, size - len(suite["documents"]))):
        path = root / f"distractor-{index:06}.md"
        path.write_text(meeting(f"Unrelated log {index}", "2025-01-01T10:00:00Z",
                               f"[SPEAKER_0 0:00] Zebrapebble{index} routine log only."), encoding="utf-8")
    return max(size, len(suite["documents"]))
