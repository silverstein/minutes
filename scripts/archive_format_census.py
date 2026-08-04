#!/usr/bin/env python3
"""Produce a privacy-preserving, metadata-only archive format census.

The census intentionally does not open regular files, hash contents, follow
symlinks, or emit source names and paths. It is a discovery tool, not an
ingestion or indexing path.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import stat
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCHEMA_VERSION = "minutes.archive-census.v1"
SAFE_EXTENSION = re.compile(r"^\.[a-z0-9][a-z0-9+_-]{0,11}$")
COMPOUND_EXTENSIONS = (".tar.gz", ".tar.bz2", ".tar.xz")

PACKAGE_CATEGORIES = {
    ".pages": "apple_document",
    ".key": "presentation",
    ".numbers": "spreadsheet",
    ".rtfd": "word_processing",
}

CATEGORY_EXTENSIONS = {
    "pdf": {".pdf"},
    "word_processing": {
        ".doc",
        ".docm",
        ".docx",
        ".dot",
        ".dotm",
        ".dotx",
        ".odt",
        ".rtf",
        ".rtfd",
        ".wpd",
        ".wps",
    },
    "email": {".eml", ".mbox", ".msg", ".olm", ".ost", ".pst"},
    "plain_text": {".htm", ".html", ".md", ".text", ".txt", ".xml"},
    "image_or_scan": {
        ".bmp",
        ".gif",
        ".heic",
        ".jpeg",
        ".jpg",
        ".png",
        ".tif",
        ".tiff",
    },
    "spreadsheet": {".csv", ".numbers", ".ods", ".xls", ".xlsb", ".xlsm", ".xlsx"},
    "presentation": {".key", ".odp", ".ppt", ".pptm", ".pptx"},
    "archive": {
        ".7z",
        ".bz2",
        ".gz",
        ".rar",
        ".tar",
        ".tar.bz2",
        ".tar.gz",
        ".tar.xz",
        ".xz",
        ".zip",
    },
    "database": {".accdb", ".db", ".mdb", ".sqlite", ".sqlite3"},
    "apple_document": {".pages"},
    "icloud_placeholder": {".icloud"},
}

EXTENSION_CATEGORY = {
    extension: category
    for category, extensions in CATEGORY_EXTENSIONS.items()
    for extension in extensions
}


def _extension_for_name(name: str) -> str:
    lowered = name.casefold()
    for extension in COMPOUND_EXTENSIONS:
        if lowered.endswith(extension):
            return extension
    suffix = Path(lowered).suffix
    if not suffix:
        return "<none>"
    if SAFE_EXTENSION.fullmatch(suffix):
        return suffix
    return "<other>"


def _category_for_extension(extension: str) -> str:
    return EXTENSION_CATEGORY.get(extension, "other")


def _size_bucket(size: int) -> str:
    if size == 0:
        return "zero"
    if size < 100 * 1024:
        return "under_100_kib"
    if size < 1024 * 1024:
        return "100_kib_to_1_mib"
    if size < 10 * 1024 * 1024:
        return "1_mib_to_10_mib"
    if size < 100 * 1024 * 1024:
        return "10_mib_to_100_mib"
    return "100_mib_or_more"


def _age_bucket(timestamp: float) -> str:
    try:
        year = datetime.fromtimestamp(timestamp, tz=timezone.utc).year
    except (OSError, OverflowError, ValueError):
        return "unknown"
    if year <= 1999:
        return "through_1999"
    if year <= 2009:
        return "2000_2009"
    if year <= 2019:
        return "2010_2019"
    if year <= 2024:
        return "2020_2024"
    return "2025_or_later"


@dataclass
class FormatAggregate:
    files: int = 0
    packages: int = 0
    bytes: int = 0

    def record_file(self, size: int) -> None:
        self.files += 1
        self.bytes += size

    def record_package(self) -> None:
        self.packages += 1


@dataclass
class Census:
    max_files: int | None = None
    status: str = "complete"
    regular_files: int = 0
    packages: int = 0
    regular_file_bytes: int = 0
    directories_scanned: int = 0
    symlinks_skipped: int = 0
    hidden_artifacts: int = 0
    icloud_placeholders: int = 0
    zero_byte_files: int = 0
    permission_mode_unreadable: int = 0
    metadata_errors: int = 0
    directory_errors: int = 0
    max_depth: int = 0
    formats: dict[str, FormatAggregate] = field(
        default_factory=lambda: defaultdict(FormatAggregate)
    )
    categories: Counter[str] = field(default_factory=Counter)
    category_bytes: Counter[str] = field(default_factory=Counter)
    size_buckets: Counter[str] = field(default_factory=Counter)
    age_buckets: Counter[str] = field(default_factory=Counter)

    @property
    def artifacts(self) -> int:
        return self.regular_files + self.packages

    def at_limit(self) -> bool:
        return self.max_files is not None and self.artifacts >= self.max_files

    def record_file(self, name: str, file_stat: os.stat_result) -> None:
        extension = _extension_for_name(name)
        category = _category_for_extension(extension)
        size = file_stat.st_size
        self.regular_files += 1
        self.regular_file_bytes += size
        self.formats[extension].record_file(size)
        self.categories[category] += 1
        self.category_bytes[category] += size
        self.size_buckets[_size_bucket(size)] += 1
        self.age_buckets[_age_bucket(file_stat.st_mtime)] += 1
        if name.startswith("."):
            self.hidden_artifacts += 1
        if extension == ".icloud":
            self.icloud_placeholders += 1
        if size == 0:
            self.zero_byte_files += 1
        if file_stat.st_mode & (stat.S_IRUSR | stat.S_IRGRP | stat.S_IROTH) == 0:
            self.permission_mode_unreadable += 1

    def record_package(self, name: str, extension: str) -> None:
        category = PACKAGE_CATEGORIES[extension]
        self.packages += 1
        self.formats[extension].record_package()
        self.categories[category] += 1
        if name.startswith("."):
            self.hidden_artifacts += 1

    def as_dict(self) -> dict[str, Any]:
        formats = [
            {
                "extension": extension,
                "category": _category_for_extension(extension),
                "files": aggregate.files,
                "packages": aggregate.packages,
                "regular_file_bytes": aggregate.bytes,
            }
            for extension, aggregate in sorted(self.formats.items())
        ]
        categories = [
            {
                "category": category,
                "artifacts": count,
                "regular_file_bytes": self.category_bytes.get(category, 0),
            }
            for category, count in sorted(self.categories.items())
        ]
        return {
            "schema": SCHEMA_VERSION,
            "status": self.status,
            "privacy": {
                "document_content_read": False,
                "filenames_emitted": False,
                "paths_emitted": False,
                "symlinks_followed": False,
                "hashes_computed": False,
            },
            "summary": {
                "artifacts": self.artifacts,
                "regular_files": self.regular_files,
                "packages": self.packages,
                "regular_file_bytes": self.regular_file_bytes,
                "directories_scanned": self.directories_scanned,
            },
            "formats": formats,
            "categories": categories,
            "age_buckets": dict(sorted(self.age_buckets.items())),
            "size_buckets": dict(sorted(self.size_buckets.items())),
            "signals": {
                "symlinks_skipped": self.symlinks_skipped,
                "hidden_artifacts": self.hidden_artifacts,
                "icloud_placeholders": self.icloud_placeholders,
                "zero_byte_files": self.zero_byte_files,
                "permission_mode_unreadable": self.permission_mode_unreadable,
                "metadata_errors": self.metadata_errors,
                "directory_errors": self.directory_errors,
                "max_depth": self.max_depth,
            },
        }


def validate_root(root: Path) -> Path:
    try:
        resolved = root.expanduser().resolve(strict=True)
    except (OSError, RuntimeError) as error:
        raise ValueError("scan root is unavailable") from error
    if not resolved.is_dir():
        raise ValueError("scan root must be a directory")
    if resolved == Path(resolved.anchor) or resolved == Path.home().resolve():
        raise ValueError("refusing to scan a filesystem or home-directory root")
    return resolved


def scan(root: Path, max_files: int | None = None) -> Census:
    if max_files is not None and max_files <= 0:
        raise ValueError("max-files must be greater than zero")
    census = Census(max_files=max_files)
    stack: list[tuple[Path, int]] = [(validate_root(root), 0)]

    while stack:
        current, depth = stack.pop()
        census.max_depth = max(census.max_depth, depth)
        try:
            with os.scandir(current) as iterator:
                entries = list(iterator)
        except OSError:
            census.directory_errors += 1
            census.status = "partial"
            continue
        census.directories_scanned += 1

        for entry in entries:
            if census.at_limit():
                census.status = "bounded"
                return census
            try:
                if entry.is_symlink():
                    census.symlinks_skipped += 1
                    continue
                if entry.is_dir(follow_symlinks=False):
                    extension = _extension_for_name(entry.name)
                    if extension in PACKAGE_CATEGORIES:
                        census.record_package(entry.name, extension)
                    else:
                        stack.append((Path(entry.path), depth + 1))
                    continue
                if not entry.is_file(follow_symlinks=False):
                    continue
                file_stat = entry.stat(follow_symlinks=False)
            except OSError:
                census.metadata_errors += 1
                census.status = "partial"
                continue
            census.record_file(entry.name, file_stat)

    return census


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Report aggregate archive formats without reading document content "
            "or emitting filenames and paths."
        )
    )
    parser.add_argument("root", type=Path, help="Explicit archive folder to inspect")
    parser.add_argument(
        "--max-files",
        type=int,
        default=None,
        help="Stop after this many file/package artifacts and report status=bounded",
    )
    parser.add_argument(
        "--pretty",
        action="store_true",
        help="Indent JSON output for human review",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        report = scan(args.root, max_files=args.max_files).as_dict()
    except ValueError as error:
        print(f"archive census refused: {error}", file=sys.stderr)
        return 2
    json.dump(
        report,
        sys.stdout,
        indent=2 if args.pretty else None,
        sort_keys=True,
    )
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
