#!/usr/bin/env python3
"""Discover, audit and execute feature-gated core tests.

The profile file specifies feature combinations, not copied lists of Rust modules
or test names. New feature-gated root modules must be enabled by a CI profile.
The runner asks libtest which tests actually exist before executing them, so a
feature typo or empty filter cannot turn into a successful zero-test gate.

`check` requires PyYAML to inspect workflow structure; `run` uses the Python
standard library and Cargo only. This is intentionally scoped to out-of-line
root modules: in-function cfg branches still need focused regression tests.
"""

from __future__ import annotations

import argparse
import json
import re
import shlex
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAPPING = "tooling/test-coverage.json"
PLATFORMS = {"linux", "macos", "windows"}


def without_comments(source: str) -> str:
    # Preserve strings (including URLs) while removing line and nested block
    # comments. Module declarations are ordinary Rust tokens, not string data.
    result, index, depth = [], 0, 0
    while index < len(source):
        if depth:
            if source.startswith("/*", index):
                depth += 1
                index += 2
            elif source.startswith("*/", index):
                depth -= 1
                index += 2
            else:
                result.append("\n" if source[index] == "\n" else " ")
                index += 1
        elif source.startswith("//", index):
            end = source.find("\n", index)
            index = len(source) if end < 0 else end
        elif source.startswith("/*", index):
            depth = 1
            result.append(" ")
            index += 2
        elif raw := re.match(r'(?:b|c)?r(#+)?"', source[index:]):
            terminator = '"' + (raw.group(1) or "")
            end = source.find(terminator, index + raw.end())
            if end < 0:
                raise ValueError("unclosed Rust raw string")
            end += len(terminator)
            result.append(source[index:end])
            index = end
        elif source[index] == '"':
            end = index + 1
            while end < len(source):
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            result.append(source[index:end])
            index = end
        else:
            result.append(source[index])
            index += 1
    if depth:
        raise ValueError("unclosed Rust block comment")
    return "".join(result)


def parse_cfg(expression: str):
    tokens = re.findall(r'"(?:\\.|[^"\\])*"|[A-Za-z_][\w]*|[(),=]', expression)
    if re.sub(r'\s+', '', expression) != "".join(tokens):
        raise ValueError(f"unsupported cfg syntax: {expression}")
    cursor = 0

    def parse():
        nonlocal cursor
        if cursor >= len(tokens):
            raise ValueError(f"incomplete cfg: {expression}")
        name = tokens[cursor]
        cursor += 1
        if cursor < len(tokens) and tokens[cursor] == "(":
            cursor += 1
            children = []
            while cursor < len(tokens) and tokens[cursor] != ")":
                children.append(parse())
                if cursor < len(tokens) and tokens[cursor] == ",":
                    cursor += 1
                elif cursor >= len(tokens) or tokens[cursor] != ")":
                    raise ValueError(f"expected comma in cfg: {expression}")
            if cursor >= len(tokens) or tokens[cursor] != ")":
                raise ValueError(f"unclosed cfg: {expression}")
            cursor += 1
            if name not in {"all", "any", "not"} or (name == "not" and len(children) != 1):
                raise ValueError(f"unsupported cfg operator: {name}")
            return (name, children)
        if cursor < len(tokens) and tokens[cursor] == "=":
            cursor += 1
            if cursor >= len(tokens) or not tokens[cursor].startswith('"'):
                raise ValueError(f"expected cfg string: {expression}")
            value = json.loads(tokens[cursor])
            cursor += 1
            return (name, value)
        return (name, None)

    tree = parse()
    if cursor != len(tokens):
        raise ValueError(f"trailing cfg tokens: {expression}")
    return tree


def cfg_enabled(tree, features: set[str], platform: str) -> bool:
    name, value = tree
    if name in {"all", "any", "not"}:
        # Evaluate all children so unknown predicates never hide in short-circuits.
        values = [cfg_enabled(child, features, platform) for child in value]
        return all(values) if name == "all" else any(values) if name == "any" else not values[0]
    if name == "feature":
        return value in features
    if name == "target_os":
        return value == platform
    if name == "test" and value is None:
        return True
    if name == "unix" and value is None:
        return platform in {"linux", "macos"}
    if name == "windows" and value is None:
        return platform == "windows"
    raise ValueError(f"unsupported cfg predicate {name}={value!r}; extend the auditor explicitly")


def feature_closure(requested: list[str], manifest: dict) -> set[str]:
    definitions = manifest.get("features", {})
    enabled, pending = set(), list(requested)
    while pending:
        name = pending.pop()
        if name in enabled:
            continue
        if name not in definitions:
            raise ValueError(f"unknown Cargo feature: {name}")
        enabled.add(name)
        pending.extend(item for item in definitions[name] if "/" not in item and not item.startswith("dep:"))
    return enabled


def root_code_positions(source: str) -> list[bool]:
    """Mark root code, excluding strings (including Rust raw strings) and bodies."""
    positions = [False] * len(source)
    index, depth = 0, 0
    while index < len(source):
        raw = re.match(r'(?:b|c)?r(#+)?"', source[index:])
        if raw:
            terminator = '"' + (raw.group(1) or "")
            end = source.find(terminator, index + raw.end())
            if end < 0:
                raise ValueError("unclosed Rust raw string")
            index = end + len(terminator)
            continue
        if source[index] == '"':
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            continue
        # Character literals can contain braces; lifetimes have no closing quote.
        char = re.match(r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\])'", source[index:])
        if char:
            index += char.end()
            continue
        positions[index] = depth == 0
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
        index += 1
    return positions


def discover_modules(root: Path, mapping: dict) -> dict:
    library = root / mapping["library"]
    source = without_comments(library.read_text())
    root_positions = root_code_positions(source)
    pattern = r'((?:\s*#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;'
    modules = {}
    for match in re.finditer(pattern, source):
        if not root_positions[match.end() - 1]:
            continue
        attributes, name = match.groups()
        conditions = re.findall(r'#\[cfg\((.*?)\)\]', attributes, re.S)
        if "cfg_attr" in attributes and re.search(r'\bfeature\s*=', attributes):
            raise ValueError(f"unsupported cfg_attr for {name}; extend the auditor explicitly")
        if not any(re.search(r'\bfeature\s*=', condition) for condition in conditions):
            continue
        if "cfg_attr" in attributes or "#[path" in attributes:
            raise ValueError(f"unsupported module attribute for {name}; extend the auditor explicitly")
        candidates = [library.parent / f"{name}.rs", library.parent / name / "mod.rs"]
        paths = [path for path in candidates if path.is_file()]
        if len(paths) != 1:
            raise ValueError(f"cannot resolve one source file for feature module {name}")
        modules[name] = {"conditions": [parse_cfg(condition) for condition in conditions], "source": str(paths[0].relative_to(root))}
    if not modules:
        raise ValueError("no feature-gated modules discovered; refusing an empty coverage audit")
    return modules


def load(root: Path):
    mapping = json.loads((root / MAPPING).read_text())
    if mapping.get("schema_version") != 1:
        raise ValueError("unsupported test coverage schema")
    manifest = tomllib.loads((root / mapping["manifest"]).read_text())
    modules = discover_modules(root, mapping)
    for name, profile in mapping["profiles"].items():
        if profile.get("selection") not in {"all", "feature-modules"}:
            raise ValueError(f"invalid selection for {name}")
        if not profile.get("platforms") or not set(profile["platforms"]) <= PLATFORMS:
            raise ValueError(f"invalid platforms for {name}")
        profile["enabled"] = feature_closure(profile["features"], manifest)
    return mapping, modules


def selected_modules(modules: dict, profile: dict, platform: str) -> list[str]:
    return sorted(name for name, module in modules.items() if all(
        cfg_enabled(condition, profile["enabled"], platform) for condition in module["conditions"]
    ))


def audit(root: Path) -> dict:
    import yaml  # Only the static workflow auditor requires PyYAML.

    mapping, modules = load(root)
    for name, profile in mapping["profiles"].items():
        document = yaml.safe_load((root / profile["workflow"]).read_text())
        # YAML 1.1 parsers treat the unquoted GitHub key `on` as a boolean.
        triggers = document.get("on", document.get(True, {}))
        if "pull_request" not in triggers:
            raise ValueError(f"coverage profile {name} requires a pull-request workflow")
        job = document.get("jobs", {}).get(profile["job"], {})
        invoked = False
        for step in job.get("steps", []):
            for line in str(step.get("run", "")).splitlines():
                if "scripts/test_coverage.py" not in line:
                    continue
                words = shlex.split(line, comments=True)
                if len(words) != 4 or words[1:] != ["scripts/test_coverage.py", "run", name]:
                    continue
                if step.get("continue-on-error") or job.get("continue-on-error"):
                    raise ValueError(f"coverage profile {name} must not continue on error")
                if step.get("if") is False or job.get("if") is False:
                    raise ValueError(f"coverage profile {name} is disabled")
                invoked = True
        if not invoked:
            raise ValueError(f"profile {name} missing executing CI step in {profile['workflow']}:{profile['job']}")
    coverage = {name: [] for name in modules}
    for name, profile in mapping["profiles"].items():
        for platform in profile["platforms"]:
            for module in selected_modules(modules, profile, platform):
                coverage[module].append(f"{name}/{platform}")
    uncovered = [name for name, profiles in coverage.items() if not profiles]
    if uncovered:
        raise ValueError(f"feature modules without an executing profile: {', '.join(uncovered)}")
    return {"schema_version": 1, "scope": mapping["scope"], "modules": {
        name: {"source": modules[name]["source"], "profiles": profiles} for name, profiles in coverage.items()
    }}


def listed_tests(output: str) -> list[str]:
    return re.findall(r'^([^\s]+): test$', output, re.M)


def require_module_tests(modules: list[str], tests: list[str]) -> None:
    missing = [module for module in modules if not any(test.startswith(f"{module}::") for test in tests)]
    if missing:
        raise ValueError(f"compiled profile lists no tests for: {', '.join(missing)}")


def run_profile(root: Path, name: str) -> int:
    mapping, modules = load(root)
    profile = mapping["profiles"].get(name)
    if profile is None:
        raise ValueError(f"unknown coverage profile: {name}")
    platform = "macos" if sys.platform == "darwin" else "windows" if sys.platform == "win32" else sys.platform
    if platform not in profile["platforms"]:
        raise ValueError(f"profile {name} is not qualified for {platform}")
    selected = selected_modules(modules, profile, platform)
    command = ["cargo", "test", "-p", mapping["package"], "--no-default-features", "--features", ",".join(profile["features"]), "--lib", "--"]
    if profile["selection"] == "feature-modules":
        if not selected:
            raise ValueError(f"profile {name} has no feature modules")
        command.extend(f"{module}::" for module in selected)
    listing = subprocess.run([*command, "--list"], cwd=root, text=True, stdout=subprocess.PIPE)
    if listing.returncode:
        return listing.returncode
    tests = listed_tests(listing.stdout)
    require_module_tests(selected, tests)
    ignored_listing = subprocess.run([*command, "--ignored", "--list"], cwd=root, text=True, stdout=subprocess.PIPE)
    if ignored_listing.returncode:
        return ignored_listing.returncode
    ignored = listed_tests(ignored_listing.stdout)
    ignored_names = set(ignored)
    runnable = [test for test in tests if test not in ignored_names]
    # libtest's normal listing includes #[ignore] tests. A model-only module
    # must not masquerade as deterministic regression coverage.
    require_module_tests(selected, runnable)
    print(f"Coverage profile {name}: {len(runnable)} runnable tests, {len(ignored)} ignored; modules: {', '.join(selected)}", flush=True)
    result = subprocess.run([*command, "--test-threads=1", "--nocapture"], cwd=root)
    report = root / "target/test-coverage" / f"{name}-{platform}.json"
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(json.dumps({
        "schema_version": 1, "profile": name, "platform": platform,
        "features": sorted(profile["enabled"]), "modules": selected,
        "listed_tests": tests, "runnable_tests": runnable, "ignored_tests": ignored,
        "command": command, "exit_code": result.returncode,
        "qualification": "Each selected module has nonignored regression tests. Ignored tests are inventoried separately; this is not a hardware or model-quality qualification.",
    }, indent=2) + "\n")
    return result.returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["check", "run"])
    parser.add_argument("profile", nargs="?")
    args = parser.parse_args()
    try:
        if args.operation == "check":
            if args.profile:
                parser.error("check takes no profile")
            print(json.dumps(audit(ROOT), indent=2))
            return 0
        if not args.profile:
            parser.error("run requires a profile")
        return run_profile(ROOT, args.profile)
    except (ValueError, OSError) as error:
        print(f"test coverage: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
