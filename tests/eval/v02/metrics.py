"""Dependency-free, independently testable benchmark metrics.

No LLM judge and no fuzzy citation acceptance. Duplicate retrievals cannot inflate
recall. Empty-reference cases are evaluated as abstention, not perfect retrieval.
"""
from __future__ import annotations

import math
import re


def retrieval_scores(actual: list[str], relevance: dict[str, int], k: int) -> dict:
    if k < 1:
        raise ValueError("k must be positive")
    actual = actual[:k]
    relevant = {key for key, grade in relevance.items() if grade > 0}
    if not relevant:
        return {"recall_at_k": None, "ndcg_at_k": None, "abstention": not actual}
    seen = set()
    gains = []
    for key in actual:
        grade = relevance.get(key, 0) if key not in seen else 0
        seen.add(key)
        gains.append(grade)
    dcg = sum((2 ** grade - 1) / math.log2(rank + 2) for rank, grade in enumerate(gains))
    ideal = sorted(relevance.values(), reverse=True)[:k]
    idcg = sum((2 ** grade - 1) / math.log2(rank + 2) for rank, grade in enumerate(ideal))
    return {"recall_at_k": len(seen & relevant) / len(relevant),
            "ndcg_at_k": dcg / idcg if idcg else 0.0, "abstention": None}


def percentile(values: list[float], quantile: float) -> float | None:
    """Nearest-rank percentile (explicitly used in latency reports)."""
    if not values:
        return None
    if not 0 <= quantile <= 1:
        raise ValueError("quantile must be in [0, 1]")
    ordered = sorted(values)
    return ordered[max(0, math.ceil(quantile * len(ordered)) - 1)]


def words(text: str) -> list[str]:
    """Unicode casefolded words; preserve apostrophes and decimal numbers."""
    return re.findall(r"\d+(?:[.,]\d+)*|[^\W\d_]+(?:['’][^\W\d_]+)*", text.casefold())


def word_errors(reference: str, hypothesis: str) -> dict:
    """Word-level Levenshtein with S/D/I counts and deterministic tie breaks."""
    ref, hyp = words(reference), words(hypothesis)
    previous = [(j, 0, 0, j) for j in range(len(hyp) + 1)]
    for i, ref_word in enumerate(ref, 1):
        current = [(i, 0, i, 0)]
        for j, hyp_word in enumerate(hyp, 1):
            if ref_word == hyp_word:
                current.append(previous[j - 1])
                continue
            total, sub, delete, insert = previous[j - 1]
            substitution = (total + 1, sub + 1, delete, insert)
            total, sub, delete, insert = previous[j]
            deletion = (total + 1, sub, delete + 1, insert)
            total, sub, delete, insert = current[j - 1]
            insertion = (total + 1, sub, delete, insert + 1)
            current.append(min((substitution, deletion, insertion), key=lambda value: value[0]))
        previous = current
    total, sub, delete, insert = previous[-1]
    return {"reference_words": len(ref), "errors": total, "substitutions": sub,
            "deletions": delete, "insertions": insert,
            "wer": total / len(ref) if ref else None}


def critical_errors(reference: str, hypothesis: str, terms: list[dict]) -> dict:
    """Exact normalized phrase occurrence preservation, not semantic scoring.

    Both missing and spurious occurrences count, including a duplicated number
    or inserted negation. Does not silently consider 'twelve' equal to '12'.
    """
    ref, hyp = words(reference), words(hypothesis)
    details = []
    for item in terms:
        phrase = words(item["text"])
        if not phrase:
            raise ValueError("critical terms must contain words")
        count = lambda tokens: sum(tokens[i:i + len(phrase)] == phrase
                                   for i in range(len(tokens) - len(phrase) + 1))
        expected, actual = count(ref), count(hyp)
        details.append({"text": item["text"], "kind": item["kind"],
                        "reference_occurrences": expected, "hypothesis_occurrences": actual,
                        "errors": abs(expected - actual)})
    return {"terms": details, "errors": sum(item["errors"] for item in details),
            "reference_occurrences": sum(item["reference_occurrences"] for item in details)}


def _max_assignment(weights: list[list[float]]) -> float:
    """Exact bitmask assignment for <=12 speakers; no external solver."""
    n = len(weights)
    if n > 12:
        raise ValueError("DER supports at most 12 distinct speakers per recording")
    states = {0: 0.0}
    for row in weights:
        following = {}
        for mask, score in states.items():
            for col, weight in enumerate(row):
                if mask & (1 << col):
                    continue
                new_mask = mask | (1 << col)
                following[new_mask] = max(following.get(new_mask, -1), score + weight)
        states = following
    return max(states.values(), default=0.0)


def diarization_error(reference: list[dict], hypothesis: list[dict], duration: float) -> dict:
    """DER over [0,duration], zero collar, overlap included, global best mapping.

    Scores speaker-time, not wall time. Unmapped speakers count as confusion;
    speech during reference silence counts as false alarm.
    """
    if not math.isfinite(duration) or duration <= 0:
        raise ValueError("duration must be finite and positive")
    for segment in reference + hypothesis:
        if (not isinstance(segment.get("speaker"), str) or not segment["speaker"]
                or not all(math.isfinite(float(segment[key])) for key in ("start", "end"))
                or not 0 <= segment["start"] < segment["end"] <= duration):
            raise ValueError("invalid diarization segment")
    ref_names = sorted({s["speaker"] for s in reference})
    hyp_names = sorted({s["speaker"] for s in hypothesis})
    count = max(len(ref_names), len(hyp_names))
    if count > 12:
        raise ValueError("DER supports at most 12 distinct speakers per recording")
    weights = [[0.0] * count for _ in range(count)]
    boundaries = sorted({0.0, duration, *(s[key] for s in reference + hypothesis for key in ("start", "end"))})
    miss = false_alarm = reference_time = shared_capacity = 0.0
    for start, end in zip(boundaries, boundaries[1:]):
        mid, width = (start + end) / 2, end - start
        ref = {s["speaker"] for s in reference if s["start"] <= mid < s["end"]}
        hyp = {s["speaker"] for s in hypothesis if s["start"] <= mid < s["end"]}
        reference_time += len(ref) * width
        miss += max(0, len(ref) - len(hyp)) * width
        false_alarm += max(0, len(hyp) - len(ref)) * width
        shared_capacity += min(len(ref), len(hyp)) * width
        for r in ref:
            for h in hyp:
                weights[ref_names.index(r)][hyp_names.index(h)] += width
    confusion = max(0.0, shared_capacity - _max_assignment(weights))
    return {"der": (miss + false_alarm + confusion) / reference_time if reference_time else None,
            "missed_speaker_seconds": miss, "false_alarm_speaker_seconds": false_alarm,
            "confusion_speaker_seconds": confusion, "reference_speaker_seconds": reference_time,
            "collar_seconds": 0, "overlap": "included", "mapping": "global-optimal"}
