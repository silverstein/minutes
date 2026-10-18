#!/usr/bin/env python3
"""Point the Homebrew tap at a released version.

The tap holds two independent files, and they drift independently. The CLI
formula was bumped faithfully every release while the desktop cask sat at
0.18.2 through six of them, until a user reported that `brew upgrade --cask`
had been a no-op for months (#736). Nothing was broken; the step simply
depended on someone remembering, and release-time steps rot invisibly between
releases. So this does both, from the release event, with no memory involved.

The governing risk is a *wrong* hash rather than a stale one. A stale cask
installs an old version; a wrong one fails every install with a checksum
mismatch. Everything below follows from that:

- Nothing is written until every read, download and check has succeeded, and
  then both files land in one commit through the Git Data API. Writing the
  formula first meant a later failure left the tap half-updated.
- The DMG is validated three ways: GitHub must call the asset `uploaded`, the
  downloaded byte count must equal the declared size, and the computed hash
  must equal the digest GitHub recorded. A sized read on a connection that
  closes early does not necessarily raise, so hashing until the stream ends
  can silently digest a truncated file.
- A cask already at the right version is still checked against the expected
  hash, and repaired if it disagrees. Treating the version as proof of
  currency made a wrong hash permanently unfixable by this tool.
- Edits must match exactly once, so a restructured tap file makes this refuse
  rather than guess.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

TAP_REPO = "silverstein/homebrew-tap"
TAP_BRANCH = "main"
SOURCE_REPO = "silverstein/minutes"
CASK_PATH = "Casks/minutes.rb"
FORMULA_PATH = "Formula/minutes.rb"
SHERPA_ASSET = "minutes-macos-arm64-sherpa.tar.gz"
API = "https://api.github.com"


class BumpError(RuntimeError):
    """A condition that must stop the bump rather than be guessed around."""


class HostChangeStripsAuth(urllib.request.HTTPRedirectHandler):
    """Drop Authorization when a redirect crosses to another host.

    Release asset downloads answer with a 302 to object storage, and the
    stdlib redirect handler copies every header onto the redirected request,
    Authorization included. Confirmed against the installed runtime rather
    than assumed. Without this, a token that can write to the tap repository
    is handed to a storage host with no business seeing it.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        new = super().redirect_request(req, fp, code, msg, headers, newurl)
        if new is None:
            return None
        same_host = (
            urllib.parse.urlsplit(newurl).netloc
            == urllib.parse.urlsplit(req.full_url).netloc
        )
        if not same_host:
            for header in list(new.headers):
                if header.lower() == "authorization":
                    del new.headers[header]
            new.unredirected_hdrs.pop("Authorization", None)
        return new


# ---------------------------------------------------------------------------
# Pure transforms
# ---------------------------------------------------------------------------


def substitute_once(
    pattern: str, replacement, text: str, what: str, flags: int = re.MULTILINE
) -> str:
    """Replace exactly one match, or refuse.

    Zero matches means the file moved on and this script no longer understands
    it. More than one means the file is ambiguous. Either way, writing would be
    a guess.
    """
    # MULTILINE so `^` anchors per line: these directives sit indented inside a
    # cask or formula block, never at the start of the file.
    new_text, count = re.subn(pattern, replacement, text, count=2, flags=flags)
    if count != 1:
        raise BumpError(
            f"expected exactly one {what} in the tap file, found {count}. "
            "Refusing to write a guess; update this script alongside the tap."
        )
    return new_text


def bump_cask(content: str, version: str, sha256: str) -> str:
    content = substitute_once(
        r'^(\s*version\s+)"[^"]+"',
        lambda m: f'{m.group(1)}"{version}"',
        content,
        "version line",
    )
    return substitute_once(
        r'^(\s*sha256\s+)"[0-9a-f]{64}"',
        lambda m: f'{m.group(1)}"{sha256}"',
        content,
        "sha256 line",
    )


def bump_formula(content: str, version: str, sherpa_sha256: str | None) -> str:
    content = substitute_once(
        r'(tag:\s*)"v[^"]+"',
        lambda m: f'{m.group(1)}"v{version}"',
        content,
        "git tag reference",
    )
    if sherpa_sha256 is None:
        return content

    # The staged plugin is pinned by URL and hash. Moving only the git tag
    # would build a new CLI and hand it the previous release's plugin, which
    # is a version skew the ABI check would reject at load time.
    content = substitute_once(
        rf'(releases/download/)v[^/]+(/{re.escape(SHERPA_ASSET)})',
        lambda m: f"{m.group(1)}v{version}{m.group(2)}",
        content,
        "sherpa resource url",
    )
    return substitute_once(
        r'(resource "sherpa-plugin" do[^\n]*(?:\n(?!\s*end\b)[^\n]*)*?sha256\s+)"[0-9a-f]{64}"',
        lambda m: f'{m.group(1)}"{sherpa_sha256}"',
        content,
        "sherpa resource sha256",
        flags=re.MULTILINE | re.DOTALL,
    )


def formula_sherpa_sha256(content: str) -> str | None:
    match = re.search(
        r'resource "sherpa-plugin" do.*?sha256\s+"([0-9a-f]{64})"',
        content,
        re.MULTILINE | re.DOTALL,
    )
    return match.group(1) if match else None


def formula_sherpa_version(content: str) -> str | None:
    match = re.search(
        rf'releases/download/v([^/]+)/{re.escape(SHERPA_ASSET)}', content
    )
    return match.group(1) if match else None


def cask_version(content: str) -> str | None:
    match = re.search(r'^\s*version\s+"([^"]+)"', content, re.MULTILINE)
    return match.group(1) if match else None


def cask_sha256(content: str) -> str | None:
    match = re.search(r'^\s*sha256\s+"([0-9a-f]{64})"', content, re.MULTILINE)
    return match.group(1) if match else None


def formula_version(content: str) -> str | None:
    match = re.search(r'tag:\s*"v([^"]+)"', content)
    return match.group(1) if match else None


def tap_mismatches(
    formula: str,
    cask: str,
    version: str,
    expected_sha256: str | None,
    sherpa_sha256: str | None = None,
) -> list[str]:
    """Describe any tap fields that do not match a released version."""
    mismatches: list[str] = []
    if formula_version(formula) != version:
        mismatches.append(
            f"formula is {formula_version(formula) or 'unparseable'}, expected {version}"
        )
    if sherpa_sha256 is not None:
        # A stale plugin pin is invisible in the version line, and the failure
        # it causes appears at transcription time as a refusing loader.
        if formula_sherpa_version(formula) != version:
            mismatches.append(
                f"formula stages the sherpa plugin from "
                f"v{formula_sherpa_version(formula) or 'unparseable'}, expected v{version}"
            )
        if formula_sherpa_sha256(formula) != sherpa_sha256:
            mismatches.append("formula sherpa sha256 does not match the release archive")
    if expected_sha256 is not None:
        if cask_version(cask) != version:
            mismatches.append(
                f"cask is {cask_version(cask) or 'unparseable'}, expected {version}"
            )
        if cask_sha256(cask) != expected_sha256:
            mismatches.append("cask sha256 does not match the release DMG")
    return mismatches


# ---------------------------------------------------------------------------
# GitHub plumbing
# ---------------------------------------------------------------------------


class HttpFailure(BumpError):
    """A failed request, carrying its status so callers can discriminate."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(message)
        self.status = status


def request(
    url: str, token: str, method: str = "GET", payload: dict | None = None
) -> dict:
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    req.add_header("Accept", "application/vnd.github+json")
    req.add_header("X-GitHub-Api-Version", "2022-11-28")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req) as response:
            return json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")[:400]
        # Never interpolate the token into an error.
        raise HttpFailure(
            error.code, f"{method} {url} failed with {error.code}: {body}"
        ) from None


def read_tap_file(path: str, token: str) -> str:
    payload = request(f"{API}/repos/{TAP_REPO}/contents/{path}?ref={TAP_BRANCH}", token)
    return base64.b64decode(payload["content"]).decode()


def commit_tap_files(files: dict[str, str], message: str, token: str) -> str:
    """Write every changed file in one commit.

    Two Contents API calls are two commits, so a failure between them leaves
    the tap half-updated, and two concurrent runs can interleave into a formula
    and cask from different versions. A tree plus a commit plus a non-forced
    ref update is atomic and rejects a stale base outright.
    """
    ref = request(f"{API}/repos/{TAP_REPO}/git/ref/heads/{TAP_BRANCH}", token)
    base_sha = ref["object"]["sha"]
    base_commit = request(f"{API}/repos/{TAP_REPO}/git/commits/{base_sha}", token)

    tree = request(
        f"{API}/repos/{TAP_REPO}/git/trees",
        token,
        method="POST",
        payload={
            "base_tree": base_commit["tree"]["sha"],
            "tree": [
                {"path": path, "mode": "100644", "type": "blob", "content": content}
                for path, content in sorted(files.items())
            ],
        },
    )
    commit = request(
        f"{API}/repos/{TAP_REPO}/git/commits",
        token,
        method="POST",
        payload={"message": message, "tree": tree["sha"], "parents": [base_sha]},
    )
    # force defaults to false: a concurrent run that moved the branch makes
    # this fail rather than silently discard its commit.
    request(
        f"{API}/repos/{TAP_REPO}/git/refs/heads/{TAP_BRANCH}",
        token,
        method="PATCH",
        payload={"sha": commit["sha"], "force": False},
    )
    return commit["sha"][:8]


def check_write_access(token: str) -> int:
    """Prove the token may write to the tap, without writing anything.

    Reading the tap needs no more than public access, so a successful bump
    dry-run says nothing about whether the token can actually commit. A
    permission set to read-only therefore stays invisible until a release, and
    a release is the worst moment to discover it.

    Updating a ref to the commit it already points at is the probe: it creates
    no commit and moves nothing, but the API still refuses it without write
    access.
    """
    ref = request(f"{API}/repos/{TAP_REPO}/git/ref/heads/{TAP_BRANCH}", token)
    sha = ref["object"]["sha"]
    try:
        request(
            f"{API}/repos/{TAP_REPO}/git/refs/heads/{TAP_BRANCH}",
            token,
            method="PATCH",
            payload={"sha": sha, "force": False},
        )
    except HttpFailure as failure:
        if failure.status in (401, 403, 404):
            # 404 as well as 403: GitHub hides repositories a token cannot
            # reach rather than admitting they exist.
            raise BumpError(
                "the token cannot write to "
                f"{TAP_REPO} (HTTP {failure.status}). Check that the "
                "fine-grained token grants Contents: Read and write on that "
                "repository, and that the repository is in its scope."
            ) from None
        if failure.status == 422:
            # Some deployments decline a no-op ref update outright. That is
            # not evidence either way, and claiming otherwise would be the
            # kind of check that reassures without verifying.
            print(
                "::warning::write access could not be determined: the no-op "
                f"ref update returned 422. This is inconclusive, not a failure."
            )
            return 0
        raise
    print(f"token can write to {TAP_REPO} (no commit was created)")
    return 0


def dmg_digest(version: str, token: str) -> str | None:
    """SHA-256 of the released DMG, or None when the release has no DMG."""
    return asset_digest(f"Minutes_{version}_aarch64.dmg", version, token)


def sherpa_digest(version: str, token: str) -> str | None:
    """SHA-256 of the macOS arm64 sherpa archive the formula stages.

    The formula compiles the CLI with `engine-sherpa` and installs the signed
    plugin from this archive beside the binary, because the plugin crate
    downloads a prebuilt sherpa-onnx bundle during its own build and a formula
    sandbox does not get that network. The archive is pinned by URL and hash,
    so it has to move with every release or a new CLI loads an old plugin.
    """
    return asset_digest(SHERPA_ASSET, version, token)


def asset_digest(asset_name: str, version: str, token: str) -> str | None:
    """SHA-256 of one release asset, or None when the release lacks it."""
    release = request(f"{API}/repos/{SOURCE_REPO}/releases/tags/v{version}", token)
    asset = next((a for a in release.get("assets", []) if a["name"] == asset_name), None)
    if asset is None:
        return None

    if asset.get("state") != "uploaded":
        raise BumpError(
            f"{asset_name} is in state {asset.get('state')!r}, not 'uploaded'; "
            "refusing to hash an asset GitHub has not finished storing"
        )

    opener = urllib.request.build_opener(HostChangeStripsAuth)
    req = urllib.request.Request(asset["url"])
    req.add_header("Authorization", f"Bearer {token}")
    req.add_header("Accept", "application/octet-stream")
    digest = hashlib.sha256()
    read = 0
    with opener.open(req) as response:
        for chunk in iter(lambda: response.read(1024 * 1024), b""):
            digest.update(chunk)
            read += len(chunk)
    computed = digest.hexdigest()

    declared = asset.get("size")
    if declared is not None and read != declared:
        raise BumpError(
            f"downloaded {read} bytes of {asset_name} but GitHub declares "
            f"{declared}; refusing to write the hash of a partial file"
        )

    recorded = asset.get("digest") or ""
    if recorded.startswith("sha256:") and recorded.split(":", 1)[1] != computed:
        raise BumpError(
            f"{asset_name} hashed to {computed} but GitHub records {recorded}; "
            "refusing to write a hash the two disagree on"
        )

    return computed


def wait_until_current(
    version: str, token: str, attempts: int = 8, delay_seconds: float = 2
) -> int:
    """Wait for GitHub's branch-content reads to converge after the tap write.

    The Git Data API ref update is atomic, but the Contents API can briefly
    return files from different branch generations. The v0.25.0 release saw
    the new cask and old formula in the same verification pass. Re-derive the
    expected DMG hash once, then retry only the cheap tap reads for a bounded
    window. A real mismatch still fails the workflow.
    """
    version = version.lstrip("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise BumpError(f"refusing to verify non-release version {version!r}")
    if attempts < 1:
        raise BumpError("verification attempts must be at least 1")

    expected = dmg_digest(version, token)
    if expected is None:
        raise BumpError(
            f"release v{version} has no Minutes_{version}_aarch64.dmg; "
            "cannot verify the cask"
        )
    sherpa = sherpa_digest(version, token)

    mismatches: list[str] = []
    for attempt in range(1, attempts + 1):
        formula = read_tap_file(FORMULA_PATH, token)
        cask = read_tap_file(CASK_PATH, token)
        mismatches = tap_mismatches(formula, cask, version, expected, sherpa)
        if not mismatches:
            print(f"tap verified at {version} with matching DMG hash")
            return 0
        if attempt < attempts:
            print(
                f"::notice::tap verification attempt {attempt}/{attempts} "
                f"has not converged yet: {'; '.join(mismatches)}"
            )
            time.sleep(delay_seconds)

    raise BumpError(
        f"tap does not match release {version} after {attempts} attempts: "
        + "; ".join(mismatches)
    )


# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------


def run(version: str, token: str, dry_run: bool) -> int:
    version = version.lstrip("v")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise BumpError(
            f"refusing to bump to a non-release version {version!r}; "
            "prereleases and suffixed tags do not belong in the tap"
        )

    # Everything is read, downloaded and checked before anything is written.
    formula = read_tap_file(FORMULA_PATH, token)
    cask = read_tap_file(CASK_PATH, token)
    pending: dict[str, str] = {}

    sherpa = sherpa_digest(version, token)
    if sherpa is None:
        # Visible, not fatal: a release without the archive leaves the formula
        # pinned to the last one that had it, which is a skew worth saying out
        # loud rather than a reason to fail the whole bump.
        print(
            f"::warning::release v{version} has no {SHERPA_ASSET}; "
            f"formula still stages the plugin from "
            f"v{formula_sherpa_version(formula)}"
        )

    formula_current = formula_version(formula) == version and (
        sherpa is None
        or (
            formula_sherpa_version(formula) == version
            and formula_sherpa_sha256(formula) == sherpa
        )
    )
    if formula_current:
        print(f"formula already at {version}")
    else:
        pending[FORMULA_PATH] = bump_formula(formula, version, sherpa)
        print(f"formula {formula_version(formula)} -> {version}")
        if sherpa is not None:
            print(
                f"formula sherpa plugin "
                f"v{formula_sherpa_version(formula)} -> v{version} "
                f"(sha256 {sherpa[:12]}...)"
            )

    expected = dmg_digest(version, token)
    if expected is None:
        # Not a failure, but it must be visible: silence is how the cask froze.
        print(
            f"::warning::release v{version} has no Minutes_{version}_aarch64.dmg; "
            "cask left as is"
        )
    elif cask_version(cask) == version and cask_sha256(cask) == expected:
        print(f"cask already at {version} with a matching hash")
    else:
        if cask_version(cask) == version:
            # The version alone is not proof of currency. An earlier draft
            # returned here, which made a wrong hash unfixable by this tool.
            print(
                f"::warning::cask says {version} but its hash is "
                f"{cask_sha256(cask)}, expected {expected}; repairing"
            )
        pending[CASK_PATH] = bump_cask(cask, version, expected)
        print(f"cask {cask_version(cask)} -> {version} (sha256 {expected[:12]}...)")

    if not pending:
        print("tap already current; nothing to do")
        return 0
    if dry_run:
        print(f"dry run: would commit {', '.join(sorted(pending))}")
        return 0

    commit = commit_tap_files(pending, f"minutes {version}", token)
    print(f"committed {commit}: {', '.join(sorted(pending))}")
    return 0


FIXTURE_CASK = """cask "minutes" do
  version "0.18.2"
  sha256 "71b267b411ce77c15e19b97a3cd38ad3c8d7c82e080f1ff1702ce7e39c06fbf9"

  url "https://github.com/silverstein/minutes/releases/download/v#{version}/Minutes_#{version}_aarch64.dmg"
  app "Minutes.app"
end
"""

FIXTURE_FORMULA = """class Minutes < Formula
  url "https://github.com/silverstein/minutes.git", tag: "v0.24.0"
  license "MIT"

  on_macos do
    on_arm do
      resource "sherpa-plugin" do
        url "https://github.com/silverstein/minutes/releases/download/v0.24.0/minutes-macos-arm64-sherpa.tar.gz"
        sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      end
    end
  end
end
"""


def self_test() -> int:
    failures = 0

    def check(condition: bool, message: str) -> None:
        nonlocal failures
        if condition:
            print(f"self-test ok: {message}")
        else:
            print(f"self-test FAILED: {message}", file=sys.stderr)
            failures += 1

    new_sha = "d" * 64
    updated = bump_cask(FIXTURE_CASK, "0.24.0", new_sha)
    check(
        cask_version(updated) == "0.24.0" and cask_sha256(updated) == new_sha,
        "cask bump rewrites both version and hash",
    )
    # The url line interpolates #{version} and must survive, or the cask would
    # point at a literal path and every install would 404.
    check(
        "#{version}" in updated and updated.count("sha256") == 1,
        "cask bump leaves surrounding content intact",
    )
    check(
        formula_version(bump_formula(FIXTURE_FORMULA, "0.25.0", None)) == "0.25.0",
        "formula bump rewrites the tag",
    )
    # The pin that goes stale silently: only the tag line moves on its own, and
    # a new CLI handed the previous release's plugin fails at load, not here.
    sherpa_sha = "b" * 64
    bumped = bump_formula(FIXTURE_FORMULA, "0.25.0", sherpa_sha)
    check(
        formula_sherpa_version(bumped) == "0.25.0"
        and formula_sherpa_sha256(bumped) == sherpa_sha,
        "formula bump moves the staged sherpa plugin url and hash",
    )
    check(
        formula_sherpa_version(bump_formula(FIXTURE_FORMULA, "0.25.0", None)) == "0.24.0",
        "a release without the sherpa archive leaves the pin untouched",
    )
    check(
        tap_mismatches(bumped, FIXTURE_CASK, "0.25.0", None, sherpa_sha) == [],
        "verifier accepts a fully bumped formula",
    )
    check(
        len(tap_mismatches(FIXTURE_FORMULA, FIXTURE_CASK, "0.25.0", None, sherpa_sha)) == 3,
        "verifier catches a stale tag, stale plugin url and stale plugin hash",
    )
    check(
        len(tap_mismatches(
            bump_formula(FIXTURE_FORMULA, "0.25.0", None),
            FIXTURE_CASK, "0.25.0", None, sherpa_sha,
        )) == 2,
        "verifier catches a bumped tag that left the plugin pin behind",
    )

    current_formula = bump_formula(FIXTURE_FORMULA, "0.25.0", None)
    current_cask = bump_cask(FIXTURE_CASK, "0.25.0", new_sha)
    check(
        tap_mismatches(current_formula, current_cask, "0.25.0", new_sha) == [],
        "tap verification accepts matching formula, cask, and DMG hash",
    )
    check(
        len(tap_mismatches(FIXTURE_FORMULA, current_cask, "0.25.0", new_sha)) == 1,
        "tap verification rejects a stale formula even when the cask is current",
    )
    check(
        bump_cask(updated, "0.24.0", new_sha) == updated,
        "cask bump is idempotent",
    )

    for label, content, fn in [
        ("a cask with no version line", 'cask "minutes" do\nend\n',
         lambda c: bump_cask(c, "1.0.0", new_sha)),
        ("a cask with two version lines",
         'version "1.0.0"\nversion "2.0.0"\nsha256 "%s"\n' % ("a" * 64),
         lambda c: bump_cask(c, "1.0.0", new_sha)),
        ("a formula with no tag", "class Minutes < Formula\nend\n",
         lambda c: bump_formula(c, "1.0.0", None)),
    ]:
        try:
            fn(content)
        except BumpError:
            check(True, f"refused {label}")
        else:
            check(False, f"accepted {label}")

    # Version parsing must reject anything that is not a plain release, since
    # the tap has no way to express a prerelease.
    for bad in ["1.2.3-rc1", "1.2", "latest", "", "1.2.3.4"]:
        try:
            run(bad, "unused-token", dry_run=True)
        except BumpError:
            check(True, f"refused version {bad!r}")
        except Exception as error:  # network attempted means the guard passed it
            check(False, f"refused version {bad!r} (got {type(error).__name__})")
        else:
            check(False, f"refused version {bad!r}")

    # A token that can read the tap but not write to it is the realistic
    # misconfiguration: picking Read-only for Contents. It cannot be produced
    # here, so the branch is driven directly rather than left unexercised.
    import types

    real_request = globals()["request"]
    for status in (403, 404):
        def refuse(url, token, method="GET", payload=None, _status=status):
            if method == "GET":
                return {"object": {"sha": "0" * 40}}
            raise HttpFailure(_status, "refused")

        globals()["request"] = refuse
        try:
            check_write_access("token")
        except BumpError as error:
            check(
                "Contents: Read and write" in str(error),
                f"a {status} on the ref update reports the permission to fix",
            )
        else:
            check(False, f"a {status} on the ref update was treated as success")
        finally:
            globals()["request"] = real_request

    def inconclusive(url, token, method="GET", payload=None):
        if method == "GET":
            return {"object": {"sha": "0" * 40}}
        raise HttpFailure(422, "no-op declined")

    globals()["request"] = inconclusive
    try:
        check(
            check_write_access("token") == 0,
            "a 422 is reported as inconclusive rather than as proof either way",
        )
    finally:
        globals()["request"] = real_request

    # The redirect handler is the only thing standing between the tap token and
    # a storage host, so it is exercised rather than trusted.
    handler = HostChangeStripsAuth()
    original = urllib.request.Request("https://api.github.com/x")
    original.add_header("Authorization", "Bearer secret")
    same = handler.redirect_request(
        original, None, 302, "Found", {}, "https://api.github.com/y"
    )
    cross = handler.redirect_request(
        original, None, 302, "Found", {}, "https://objects.example.com/y"
    )
    check(
        any(k.lower() == "authorization" for k in (same.headers if same else {})),
        "redirect within the same host keeps Authorization",
    )
    check(
        not any(k.lower() == "authorization" for k in (cross.headers if cross else {})),
        "redirect to another host drops Authorization",
    )

    if failures == 0:
        print("self-test ok: all checks passed")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", help="release version, with or without a leading v")
    parser.add_argument("--dry-run", action="store_true", help="report without writing")
    parser.add_argument(
        "--wait-current",
        action="store_true",
        help="wait boundedly until the tap matches the released version and DMG hash",
    )
    parser.add_argument("--self-test", action="store_true", help="exercise the logic")
    parser.add_argument(
        "--check-access",
        action="store_true",
        help="prove the token can write to the tap, without writing",
    )
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if args.check_access:
        token = os.environ.get("HOMEBREW_TAP_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""
        if not token:
            raise BumpError("no token in HOMEBREW_TAP_TOKEN or GITHUB_TOKEN")
        return check_write_access(token)
    if not args.version:
        parser.error("--version is required unless --self-test is given")

    token = os.environ.get("HOMEBREW_TAP_TOKEN") or os.environ.get("GITHUB_TOKEN") or ""
    if not token:
        raise BumpError(
            "no token in HOMEBREW_TAP_TOKEN or GITHUB_TOKEN; the default "
            "workflow token cannot write to another repository"
        )
    if args.wait_current:
        if args.dry_run:
            parser.error("--wait-current and --dry-run are mutually exclusive")
        return wait_until_current(args.version, token)
    return run(args.version, token, args.dry_run)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BumpError as error:
        print(f"::error::{error}", file=sys.stderr)
        raise SystemExit(1)
