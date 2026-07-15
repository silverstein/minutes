#!/usr/bin/env python3
"""Fail-closed privacy checks for public live-sidekick eval fixtures.

The public check is intentionally structural.  An optional local-only corpus
check compares normalized n-grams without printing matching text, corpus paths,
or corpus hashes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import secrets
import stat
import sys
import unicodedata
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Iterator, Sequence


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FIXTURE_DIR = (
    REPO_ROOT / "crates/core/tests/fixtures/live_sidekick_eval/v1"
)

ALLOWED_ROLE_TOKENS = {
    "USER",
    "FACILITATOR",
    "PARTICIPANT_A",
    "REVIEWER",
    "ENGINEER_A",
}
FORBIDDEN_FIELD_NAMES = {
    "real_name",
    "company",
    "email",
    "medical",
    "source_transcript",
    "derived_from",
}
SPEAKER_IDENTITY_FIELDS = {
    "speaker",
    "speakers",
    "from_speaker",
    "to_speaker",
    "inferred_speaker",
    "corrected_speaker",
    "attributed_speaker",
    "speaker_token",
    "speaker_id",
}
WORD_RE = re.compile(r"[a-z0-9]+")
TITLE_WORD_RE = re.compile(r"\b[A-Z][a-z]{2,}\b")
APPROVED_TITLE_WORDS = {
    "Both",
    "Evidence",
    "Explicit",
    "Meeting",
    "Minutes",
    "Native",
    "Routine",
    "Unavailable",
}
MAX_TEXT_FIELD_CHARS = 1_000
MAX_UNIQUE_WORDS_PER_FIXTURE = 220
DEFAULT_NGRAM_SIZE = 5
DEFAULT_OVERLAP_THRESHOLD = 0
DEFAULT_PRIVATE_NGRAM_SIZE = 7
DEFAULT_PRIVATE_MAX_FILE_BYTES = 4 * 1024 * 1024
DEFAULT_PRIVATE_EXTENSIONS = ("md", "txt")


PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "email_address",
        re.compile(r"\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b", re.IGNORECASE),
    ),
    ("url", re.compile(r"\b(?:https?://|www\.)\S+", re.IGNORECASE)),
    (
        "bare_domain",
        re.compile(
            r"\b(?:[a-z0-9-]+\.)+(?:com|org|net|io|dev|ai|co|gov|edu|test|invalid)\b",
            re.IGNORECASE,
        ),
    ),
    ("ip_address", re.compile(r"\b(?:\d{1,3}\.){3}\d{1,3}\b")),
    ("social_handle", re.compile(r"(?<![\w@])@[A-Za-z0-9_]{2,}\b")),
    (
        "phone_number",
        re.compile(r"(?<!\w)(?:\+?\d[\d(). -]{7,}\d)(?!\w)"),
    ),
    (
        "currency_or_price",
        re.compile(r"(?:[$€£¥]\s*\d|\b(?:USD|EUR|GBP|JPY)\s*\d)", re.IGNORECASE),
    ),
    ("exact_date", re.compile(r"\b\d{4}-\d{2}-\d{2}\b")),
    (
        "absolute_home_path",
        re.compile(r"(?:/Users/[^/\s]+|/home/[^/\s]+|[A-Za-z]:\\Users\\[^\\\s]+)"),
    ),
    (
        "street_address",
        re.compile(
            r"\b\d{1,6}\s+[A-Za-z][A-Za-z .'-]{1,40}\s+"
            r"(?:street|st|road|rd|avenue|ave|boulevard|blvd|lane|ln|drive|dr)\b",
            re.IGNORECASE,
        ),
    ),
    (
        "long_identifier",
        re.compile(r"(?=[A-Za-z0-9_-]{24,})(?=[A-Za-z0-9_-]*[A-Za-z])(?=[A-Za-z0-9_-]*\d)[A-Za-z0-9_-]{24,}"),
    ),
    (
        "secret_format",
        re.compile(
            r"(?:\bAKIA[A-Z0-9]{16}\b|\bBearer\s+[A-Za-z0-9._~+/=-]{12,}|"
            r"\b(?:sk|pk|api|token|secret)[-_][A-Za-z0-9_-]{12,}\b)",
            re.IGNORECASE,
        ),
    ),
)

SENSITIVE_DOMAIN_RE = re.compile(
    r"\b(?:patient|diagnosis|medication|prescription|dosage|disease|clinical|pharmacy)\b",
    re.IGNORECASE,
)
HIGH_ENTROPY_TOKEN_RE = re.compile(r"\b[A-Za-z0-9+/=]{32,}\b")


@dataclass(frozen=True)
class Finding:
    fixture: str
    path: str
    rule: str
    severity: str = "error"


@dataclass(frozen=True)
class FixtureDocument:
    path: Path
    fixture_id: str
    data: dict[str, Any]


def _json_path(parent: str, key: str | int) -> str:
    if isinstance(key, int):
        return f"{parent}[{key}]"
    return f"{parent}.{key}"


def _walk(value: Any, path: str = "$") -> Iterator[tuple[str, str | None, Any]]:
    if isinstance(value, dict):
        for key, child in value.items():
            child_path = _json_path(path, key)
            yield child_path, key, child
            yield from _walk(child, child_path)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            child_path = _json_path(path, index)
            yield child_path, None, child
            yield from _walk(child, child_path)


def _iter_strings(value: Any, path: str = "$") -> Iterator[tuple[str, str]]:
    if isinstance(value, str):
        yield path, value
    elif isinstance(value, dict):
        for key, child in value.items():
            yield from _iter_strings(child, _json_path(path, key))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from _iter_strings(child, _json_path(path, index))


def _entropy(token: str) -> float:
    counts = Counter(token)
    length = len(token)
    return -sum((count / length) * math.log2(count / length) for count in counts.values())


def _speaker_tokens(value: Any) -> list[str] | None:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return list(value)
    return None


def _load_fixture(path: Path) -> tuple[FixtureDocument | None, list[Finding]]:
    fixture_name = path.name
    try:
        parsed = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None, [Finding(fixture_name, "$", "valid_utf8_json")]
    if not isinstance(parsed, dict):
        return None, [Finding(fixture_name, "$", "top_level_object")]
    fixture_id = parsed.get("id")
    if not isinstance(fixture_id, str) or not fixture_id:
        fixture_id = path.stem
    return FixtureDocument(path=path, fixture_id=fixture_id, data=parsed), []


def check_fixture(document: FixtureDocument) -> list[Finding]:
    data = document.data
    fixture = document.path.name
    findings: list[Finding] = []

    if data.get("content_origin") != "synthetic":
        findings.append(Finding(fixture, "$.content_origin", "synthetic_origin_required"))

    privacy = data.get("privacy")
    approved: set[str] = set()
    if not isinstance(privacy, dict):
        findings.append(Finding(fixture, "$.privacy", "privacy_metadata_required"))
    else:
        if privacy.get("generation_method") != "behavior_first_from_scratch":
            findings.append(
                Finding(fixture, "$.privacy.generation_method", "scratch_generation_required")
            )
        if privacy.get("source_material") != "none":
            findings.append(Finding(fixture, "$.privacy.source_material", "no_source_material"))
        declared = privacy.get("approved_role_tokens")
        if not isinstance(declared, list) or not declared or not all(
            isinstance(token, str) for token in declared
        ):
            findings.append(
                Finding(fixture, "$.privacy.approved_role_tokens", "approved_role_tokens_required")
            )
        else:
            approved = set(declared)
            if len(approved) != len(declared):
                findings.append(
                    Finding(fixture, "$.privacy.approved_role_tokens", "duplicate_role_token")
                )
            for token in approved:
                if token not in ALLOWED_ROLE_TOKENS:
                    findings.append(
                        Finding(fixture, "$.privacy.approved_role_tokens", "unapproved_role_token")
                    )

    for path, key, value in _walk(data):
        if key in FORBIDDEN_FIELD_NAMES:
            findings.append(Finding(fixture, path, "forbidden_field_name"))
        if key in SPEAKER_IDENTITY_FIELDS:
            tokens = _speaker_tokens(value)
            if tokens is None:
                findings.append(Finding(fixture, path, "speaker_field_must_use_role_token"))
            else:
                for token in tokens:
                    if token not in ALLOWED_ROLE_TOKENS or token not in approved:
                        findings.append(Finding(fixture, path, "speaker_role_token_not_approved"))

    unique_words: set[str] = set()
    for path, value in _iter_strings(data):
        unique_words.update(WORD_RE.findall(value.lower()))
        if len(value) > MAX_TEXT_FIELD_CHARS:
            findings.append(Finding(fixture, path, "text_field_too_long"))

        # Structural IDs are expected to be descriptive slugs.  They are not
        # user-authored content and are excluded from long-token heuristics.
        skip_identifier_heuristics = path in {"$.id", "$.expectations.parity_group"}
        for rule, pattern in PATTERNS:
            if skip_identifier_heuristics and rule in {"long_identifier", "secret_format"}:
                continue
            if pattern.search(value):
                findings.append(Finding(fixture, path, rule))

        if SENSITIVE_DOMAIN_RE.search(value):
            findings.append(Finding(fixture, path, "sensitive_domain_content"))

        if not skip_identifier_heuristics:
            for match in HIGH_ENTROPY_TOKEN_RE.finditer(value):
                if _entropy(match.group(0)) >= 3.5:
                    findings.append(Finding(fixture, path, "high_entropy_token"))
                    break

        for match in TITLE_WORD_RE.finditer(value):
            if match.group(0) not in APPROVED_TITLE_WORDS:
                findings.append(
                    Finding(fixture, path, "unexpected_proper_noun", severity="warning")
                )

    if len(unique_words) > MAX_UNIQUE_WORDS_PER_FIXTURE:
        findings.append(Finding(fixture, "$", "fixture_vocabulary_too_broad"))

    return findings


def check_fixture_dir(fixture_dir: Path) -> tuple[list[FixtureDocument], list[Finding]]:
    if not fixture_dir.is_dir():
        return [], [Finding(fixture_dir.name or "fixtures", "$", "fixture_directory_missing")]
    paths = sorted(fixture_dir.glob("*.json"))
    if not paths:
        return [], [Finding(fixture_dir.name, "$", "fixture_json_required")]

    documents: list[FixtureDocument] = []
    findings: list[Finding] = []
    for path in paths:
        document, load_findings = _load_fixture(path)
        findings.extend(load_findings)
        if document is not None:
            documents.append(document)
            findings.extend(check_fixture(document))
    return documents, findings


def _iter_normalized_ngrams(text: str, size: int) -> Iterator[tuple[str, ...]]:
    normalized = unicodedata.normalize("NFKC", text).casefold()
    words = re.findall(r"[^\W_]+", normalized, flags=re.UNICODE)
    for index in range(max(0, len(words) - size + 1)):
        yield tuple(words[index : index + size])


def _normalized_ngrams(text: str, size: int) -> set[tuple[str, ...]]:
    return set(_iter_normalized_ngrams(text, size))


def _fixture_ngrams(document: FixtureDocument, size: int) -> set[tuple[str, ...]]:
    result: set[tuple[str, ...]] = set()
    for _, value in _iter_strings(document.data):
        result.update(_normalized_ngrams(value, size))
    return result


def _digest_ngram(ngram: tuple[str, ...], key: bytes) -> bytes:
    return hashlib.blake2b(
        "\x1f".join(ngram).encode("utf-8"), key=key, digest_size=16
    ).digest()


def _digest_ngrams(text: str, size: int, key: bytes) -> set[bytes]:
    # The private path never materializes raw n-grams as a collection. Each
    # generated tuple is keyed and digested before the iterator advances.
    return {_digest_ngram(ngram, key) for ngram in _iter_normalized_ngrams(text, size)}


def _read_private_text(path: Path, max_file_bytes: int) -> tuple[str | None, str | None]:
    """Read one regular file without following or racing a symlink.

    The status is deliberately aggregate-only (``rejected`` or ``unreadable``)
    so callers cannot accidentally expose a private path or parsing detail.
    """

    try:
        before = path.lstat()
    except OSError:
        return None, "unreadable"
    if (
        stat.S_ISLNK(before.st_mode)
        or not stat.S_ISREG(before.st_mode)
        or before.st_size > max_file_bytes
    ):
        return None, "rejected"

    flags = os.O_RDONLY
    flags |= getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError:
        return None, "unreadable"
    try:
        opened = os.fstat(descriptor)
        if (
            not stat.S_ISREG(opened.st_mode)
            or opened.st_size > max_file_bytes
            or (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino)
        ):
            return None, "rejected"
        chunks: list[bytes] = []
        remaining = max_file_bytes + 1
        while remaining:
            chunk = os.read(descriptor, min(64 * 1024, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        raw = b"".join(chunks)
        if len(raw) > max_file_bytes:
            return None, "rejected"
        try:
            return raw.decode("utf-8", errors="strict"), None
        except UnicodeError:
            return None, "unreadable"
    except OSError:
        return None, "unreadable"
    finally:
        os.close(descriptor)


@dataclass(frozen=True)
class PrivateCorpusSummary:
    digests: set[bytes]
    roots: int
    files_scanned: int
    files_rejected: int
    unreadable: int
    configuration_failures: int


def _scan_private_corpora(
    directories: Sequence[Path],
    extensions: set[str],
    ngram_size: int,
    max_file_bytes: int,
    key: bytes,
) -> PrivateCorpusSummary:
    digests: set[bytes] = set()
    files_scanned = 0
    files_rejected = 0
    unreadable = 0
    configuration_failures = 0

    for requested_root in directories:
        if not requested_root.is_absolute() or requested_root.is_symlink():
            configuration_failures += 1
            continue
        try:
            root = requested_root.resolve(strict=True)
        except OSError:
            configuration_failures += 1
            continue
        if not root.is_dir():
            configuration_failures += 1
            continue

        def record_walk_error(_: OSError) -> None:
            nonlocal unreadable
            unreadable += 1

        for current, directory_names, file_names in os.walk(
            root, followlinks=False, onerror=record_walk_error
        ):
            current_path = Path(current)
            safe_directories: list[str] = []
            for name in directory_names:
                candidate = current_path / name
                if candidate.is_symlink():
                    files_rejected += 1
                else:
                    safe_directories.append(name)
            directory_names[:] = safe_directories

            for name in file_names:
                path = current_path / name
                if path.suffix.lower().lstrip(".") not in extensions:
                    continue
                text, failure = _read_private_text(path, max_file_bytes)
                if failure == "rejected":
                    files_rejected += 1
                    continue
                if failure == "unreadable" or text is None:
                    unreadable += 1
                    continue
                files_scanned += 1
                digests.update(_digest_ngrams(text, ngram_size, key))

    return PrivateCorpusSummary(
        digests=digests,
        roots=len(directories),
        files_scanned=files_scanned,
        files_rejected=files_rejected,
        unreadable=unreadable,
        configuration_failures=configuration_failures,
    )


def check_private_overlap(
    documents: Iterable[FixtureDocument],
    private_corpus_dirs: Sequence[Path],
    ngram_size: int,
    threshold: int,
    extensions: set[str],
    max_file_bytes: int,
) -> tuple[int, PrivateCorpusSummary]:
    key = secrets.token_bytes(32)
    corpus = _scan_private_corpora(
        private_corpus_dirs, extensions, ngram_size, max_file_bytes, key
    )
    fixtures_with_overlap = 0
    for document in documents:
        fixture_digests: set[bytes] = set()
        for _, value in _iter_strings(document.data):
            fixture_digests.update(_digest_ngrams(value, ngram_size, key))
        count = len(fixture_digests & corpus.digests)
        if count > threshold:
            fixtures_with_overlap += 1
    return fixtures_with_overlap, corpus


def _print_structural_summary(documents: Sequence[FixtureDocument], findings: Sequence[Finding]) -> None:
    errors = sum(item.severity == "error" for item in findings)
    warnings = sum(item.severity == "warning" for item in findings)
    outcome = "pass" if not findings else "fail"
    print(
        f"structural={outcome} fixtures={len(documents)} errors={errors} warnings={warnings}"
    )
    for finding in findings:
        print(
            f"finding fixture={finding.fixture} path={finding.path} "
            f"severity={finding.severity} rule={finding.rule}"
        )


def _print_overlap_summary(
    documents: Sequence[FixtureDocument],
    fixtures_with_overlap: int,
    corpus: PrivateCorpusSummary,
    ngram_size: int,
    threshold: int,
    structural_failures: int,
) -> None:
    outcome = (
        "pass"
        if fixtures_with_overlap == 0
        and corpus.files_rejected == 0
        and corpus.unreadable == 0
        and corpus.configuration_failures == 0
        and structural_failures == 0
        else "fail"
    )
    print(
        f"private_overlap={outcome} roots={corpus.roots} "
        f"files_scanned={corpus.files_scanned} files_rejected={corpus.files_rejected} "
        f"unreadable={corpus.unreadable} configuration_failures={corpus.configuration_failures} "
        f"fixtures_scanned={len(documents)} fixtures_with_overlap={fixtures_with_overlap} "
        f"structural_failures={structural_failures} ngram_size={ngram_size} "
        f"threshold={threshold}"
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Check synthetic live-sidekick fixtures for public-repo privacy hygiene."
    )
    parser.add_argument(
        "fixture_dir",
        nargs="?",
        type=Path,
        default=DEFAULT_FIXTURE_DIR,
        help="directory containing versioned JSON fixtures",
    )
    parser.add_argument(
        "--private-corpus-dir",
        type=Path,
        action="append",
        default=[],
        help="authorized local-only corpus root; repeat for multiple roots",
    )
    parser.add_argument("--ngram-size", type=int)
    parser.add_argument(
        "--overlap-threshold", type=int, default=DEFAULT_OVERLAP_THRESHOLD
    )
    parser.add_argument(
        "--private-overlap-only",
        action="store_true",
        help="emit only one aggregate private-overlap result",
    )
    parser.add_argument(
        "--aggregate-only",
        action="store_true",
        help="required with --private-overlap-only; never emit per-fixture findings",
    )
    parser.add_argument(
        "--acknowledge-private-corpus-authorization",
        action="store_true",
        help="confirm authorization to read the explicitly supplied local roots",
    )
    parser.add_argument(
        "--include-extension",
        action="append",
        default=[],
        help="allowlisted text extension without a dot; defaults to md and txt",
    )
    parser.add_argument(
        "--private-max-file-bytes",
        type=int,
        default=DEFAULT_PRIVATE_MAX_FILE_BYTES,
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.ngram_size is None:
        args.ngram_size = (
            DEFAULT_PRIVATE_NGRAM_SIZE
            if args.private_overlap_only
            else DEFAULT_NGRAM_SIZE
        )

    if args.private_overlap_only:
        if (
            not args.aggregate_only
            or not args.acknowledge_private_corpus_authorization
            or not args.private_corpus_dir
            or os.environ.get("CI")
            or args.private_max_file_bytes <= 0
            or args.ngram_size < DEFAULT_PRIVATE_NGRAM_SIZE
            or args.overlap_threshold < 0
        ):
            print("private_overlap=fail configuration_failures=1")
            return 2
        extensions = {
            value.lower().lstrip(".")
            for value in (args.include_extension or DEFAULT_PRIVATE_EXTENSIONS)
            if value and value.strip(".")
        }
        if not extensions:
            print("private_overlap=fail configuration_failures=1")
            return 2
        documents, findings = check_fixture_dir(args.fixture_dir)
        fixtures_with_overlap, corpus = check_private_overlap(
            documents,
            args.private_corpus_dir,
            args.ngram_size,
            args.overlap_threshold,
            extensions,
            args.private_max_file_bytes,
        )
        _print_overlap_summary(
            documents,
            fixtures_with_overlap,
            corpus,
            args.ngram_size,
            args.overlap_threshold,
            len(findings),
        )
        failed = (
            bool(findings)
            or fixtures_with_overlap > 0
            or corpus.files_rejected > 0
            or corpus.unreadable > 0
            or corpus.configuration_failures > 0
        )
        return 1 if failed else 0

    if args.ngram_size < 3:
        print("configuration=fail rule=ngram_size_minimum")
        return 2
    if args.overlap_threshold < 0:
        print("configuration=fail rule=overlap_threshold_nonnegative")
        return 2

    documents, findings = check_fixture_dir(args.fixture_dir)
    _print_structural_summary(documents, findings)
    failed = bool(findings)

    if args.private_corpus_dir:
        print("private_overlap=fail configuration_failures=1")
        return 2

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
