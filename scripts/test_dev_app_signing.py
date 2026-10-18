#!/usr/bin/env python3
"""Native packaging regression using inert Mach-O fixtures, never an installed app."""

import os
from pathlib import Path
import plistlib
import shlex
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parent.parent


def run(*args, **kwargs):
    return subprocess.run(args, check=True, cwd=ROOT, **kwargs)


def main():
    if sys.platform != "darwin":
        raise SystemExit("This regression requires native macOS codesign and clang")
    source = (ROOT / "scripts/install-dev-app.sh").read_text()
    start = source.index('SIDECAR_BIN="$BUILD_APP/Contents/MacOS/minutes"')
    end = source.index('if [[ "$INSTALL_AFTER_BUILD" == "1" ]]; then', start)
    signing = source[start:end]
    verification = signing[signing.index('echo "=== Verifying bundle seal (strict) ==="'):]
    with tempfile.TemporaryDirectory(prefix="minutes-signing-test-") as directory:
        root = Path(directory)
        app = root / "Signing Fixture.app"
        binaries = app / "Contents/MacOS"
        resources = app / "Contents/Resources"
        binaries.mkdir(parents=True)
        resources.mkdir()
        (resources / "canary.txt").write_text("original resource\n")
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps({
            "CFBundleIdentifier": "com.useminutes.tests.signing-fixture",
            "CFBundleExecutable": "minutes-app",
            "CFBundlePackageType": "APPL",
            "CFBundleVersion": "1.0.0",
            "CFBundleShortVersionString": "1.0.0",
        }))
        c_file = root / "fixture.c"
        c_file.write_text(
            'const char graph[] = "MINUTES_GRAPH_WORKER_CDHASH_V1=' + '0' * 40 + '";\n'
            'const char speech[] = "MINUTES_APPLE_SPEECH_WORKER_CDHASH_V1=' + '0' * 40 + '";\n'
            'int main(void) { return graph[0] == speech[0] ? 0 : 1; }\n'
        )
        for name in ("minutes-app", "minutes", "minutes-graph-worker", "minutes-apple-speech-worker"):
            binary = binaries / name
            run("xcrun", "clang", "-o", str(binary), str(c_file))
            # ARM linkers may add a signature automatically. Start with the
            # unsigned state found on Intel, independently of runner architecture.
            run("codesign", "--remove-signature", str(binary))
        env = dict(os.environ, BUILD_APP=str(app), DEV_PRODUCT_NAME="Signing Fixture",
                   SIGN_MODE="adhoc", SIGNING_IDENTITY="")
        # find's traversal order is unspecified. Exercise a valid adversarial
        # order with the main executable first, using the real signing block.
        find_order = '''
find() {
  printf '%s\\n' "$BUILD_APP/Contents/MacOS/minutes-app"
  /usr/bin/find "$@" ! -name minutes-app
}
'''
        # First reproduce the old order against native unsigned binaries.
        legacy = signing.replace('    if [[ "$nested_executable" == "$MAIN_BIN" ]]; then\n      continue\n    fi\n', '')
        failed = subprocess.run(["bash", "-c", "set -euo pipefail\n" + find_order + legacy],
                                cwd=ROOT, env=env, capture_output=True, text=True)
        assert failed.returncode != 0, "main-first signing unexpectedly accepted unsigned nested code"
        assert "code object is not signed at all" in failed.stderr, failed.stderr
        print("PASS: native main-first signing reproduces unsigned nested-code refusal", flush=True)

        run("bash", "-c", "set -euo pipefail\n" + find_order + signing, env=env)
        entitlement_report = run("codesign", "-d", "--entitlements", ":-", str(binaries / "minutes"),
                                 capture_output=True)
        assert plistlib.loads(entitlement_report.stdout)["com.apple.security.device.audio-input"] is True
        for worker in ("graph", "apple-speech"):
            cdhash = (resources / f"minutes-{worker}-worker.cdhash").read_text().strip()
            run("python3", f"scripts/seal_{worker.replace('-', '_')}_worker_hash.py",
                "--verify", str(binaries / "minutes-app"), cdhash)
        print("PASS: nested signatures, CLI entitlement, XPC bindings and strict outer seal", flush=True)

        (resources / "canary.txt").write_text("tampered resource\n")
        marker = root / "continued-after-failed-seal"
        failed = subprocess.run(["bash", "-c", "set -euo pipefail\n" + verification
                                 + "\ntouch " + shlex.quote(str(marker))],
                                cwd=ROOT, env=env, capture_output=True, text=True)
        assert failed.returncode != 0 and not marker.exists(), "failed strict verification did not stop installation"
        print("PASS: a damaged outer seal stops before installation", flush=True)


if __name__ == "__main__":
    main()
