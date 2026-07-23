#!/usr/bin/env python3
"""Release gate for the public Sidekick SOTA corpus.

Unlike the public structural check, this entry point requires a local private
corpus. It never prints the corpus path, matching text, or corpus hashes.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path
from typing import Sequence

from check_live_sidekick_fixture_privacy import main as check_privacy


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FIXTURE_DIR = REPO_ROOT / "tests/fixtures/sidekick_sota/v1"
PRIVATE_CORPUS_ENV = "MINUTES_PRIVATE_EVAL_CORPUS_DIR"


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Require structural and local private-overlap privacy gates."
    )
    parser.add_argument(
        "--fixture-dir",
        type=Path,
        default=DEFAULT_FIXTURE_DIR,
    )
    parser.add_argument("--private-corpus-dir", type=Path)
    args = parser.parse_args(argv)
    private_corpus_dir = args.private_corpus_dir
    if private_corpus_dir is None:
        configured = os.environ.get(PRIVATE_CORPUS_ENV, "").strip()
        private_corpus_dir = Path(configured) if configured else None
    if private_corpus_dir is None:
        print(
            "release_privacy=blocked "
            "rule=private_corpus_required "
            f"configuration={PRIVATE_CORPUS_ENV}"
        )
        return 2
    return check_privacy(
        [
            str(args.fixture_dir),
            "--private-corpus-dir",
            str(private_corpus_dir),
        ]
    )


if __name__ == "__main__":
    sys.exit(main())
