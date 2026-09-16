#!/usr/bin/env python3
"""Compile the real portable modules without audio/GUI SDK dependencies.

This is a supplementary check, not a replacement for the full core/app CI.
Formatting happens only in a temporary copy; any differences are emitted as
work-continuity-format.patch for the author to apply and review.
"""
from __future__ import annotations

import difflib
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCES = [
    "crates/core/src/live_sidekick/work.rs",
    "crates/core/src/live_sidekick/work_tests.rs",
    "crates/core/src/live_sidekick/live_model.rs",
    "crates/core/examples/work_session.rs",
]


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="minutes-work-check-") as temporary:
        root = Path(temporary)
        source_dir = root / "src" / "live_sidekick"
        source_dir.mkdir(parents=True)
        (root / "examples").mkdir()
        (root / "Cargo.toml").write_text('''[package]
name = "minutes-work-continuity-check"
version = "0.0.0"
edition = "2021"
publish = false

[lib]
name = "minutes_core"

[workspace]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
''', encoding="utf-8")
        (root / "src" / "lib.rs").write_text(
            "pub mod live_sidekick { pub mod work; pub mod live_model; }\n",
            encoding="utf-8",
        )
        copies = []
        for relative in SOURCES:
            original = ROOT / relative
            target = (root / "examples" if "/examples/" in relative else source_dir) / original.name
            shutil.copyfile(original, target)
            copies.append((relative, original, target))
        manifest = str(root / "Cargo.toml")
        run("cargo", "test", "--manifest-path", manifest, "--all-targets")
        run("cargo", "clippy", "--manifest-path", manifest, "--all-targets", "--", "-D", "warnings")
        run("cargo", "fmt", "--manifest-path", manifest)
        patch = []
        for relative, original, target in copies:
            patch.extend(difflib.unified_diff(
                original.read_text(encoding="utf-8").splitlines(keepends=True),
                target.read_text(encoding="utf-8").splitlines(keepends=True),
                fromfile="a/" + relative,
                tofile="b/" + relative,
            ))
        (ROOT / "work-continuity-format.patch").write_text("".join(patch), encoding="utf-8")
        run("cargo", "run", "--quiet", "--manifest-path", manifest, "--example", "work_session")
        print("Portable module tests, Clippy, and synthetic checkpoint example passed.")
        if patch:
            print("Formatting changes are available in work-continuity-format.patch.")


if __name__ == "__main__":
    main()
