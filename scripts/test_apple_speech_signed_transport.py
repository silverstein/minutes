#!/usr/bin/env python3
"""Run the signed Apple Speech byte path under a same-UID open-holder watcher."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import plistlib
import re
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
import wave


MAX_HELD_FILE_BYTES = 64 * 1024 * 1024
EXPECTED_TEAM_ID = "63TMLKT8HN"
EXPECTED_APP_IDENTIFIERS = {
    "com.useminutes.desktop",
    "com.useminutes.desktop.dev",
}
EXPECTED_WORKER_IDENTIFIER = "com.useminutes.apple-speech-worker"

# The app spends up to two full worker wall clocks (2 x 180s) inside the two
# authenticated probes, before app launch and two strict nested code-signature
# validations of the whole bundle. The previous 420s budget left roughly 60s
# for all of that, so a slow runner would have surfaced as an evidence-free
# timeout rather than a real result.
ACCEPTANCE_TIMEOUT_SECONDS = 900
DIAGNOSTIC_STREAM_LIMIT = 32 * 1024
DIAGNOSTIC_LOG_LINES = 400
CRASH_REPORT_DIRECTORIES = (
    pathlib.Path.home() / "Library/Logs/DiagnosticReports",
    pathlib.Path("/Library/Logs/DiagnosticReports"),
)
CRASH_REPORT_PREFIXES = ("minutes-apple-speech-worker", "minutes-graph-worker", "Minutes")
CRASH_REPORT_WAIT_SECONDS = 30
CRASH_REPORT_POLL_SECONDS = 0.5


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--app", required=True, type=pathlib.Path)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument(
        "--runtime",
        action="store_true",
        help="run the fixed public-fixture analyzer path instead of the transport-only canary",
    )
    return parser.parse_args()


def codesign_details(path: pathlib.Path) -> str:
    result = subprocess.run(
        ["codesign", "-dvvv", str(path)],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return result.stdout


def detail_value(details: str, key: str) -> str:
    match = re.search(rf"^{re.escape(key)}=(.+)$", details, re.MULTILINE)
    if match is None:
        raise RuntimeError(f"missing signed identity field: {key}")
    return match.group(1).strip()


def signed_paths(app: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    with (app / "Contents" / "Info.plist").open("rb") as source:
        executable_name = plistlib.load(source)["CFBundleExecutable"]
    executable = app / "Contents" / "MacOS" / executable_name
    worker = (
        app
        / "Contents"
        / "XPCServices"
        / "com.useminutes.apple-speech-worker.xpc"
        / "Contents"
        / "MacOS"
        / "minutes-apple-speech-worker"
    )
    if not executable.is_file() or not worker.is_file():
        raise RuntimeError("signed app or Apple Speech worker executable is missing")
    return executable, worker


def verify_signed_identity(
    app: pathlib.Path,
    executable: pathlib.Path,
    worker: pathlib.Path,
) -> str:
    subprocess.run(
        ["codesign", "--verify", "--strict", "--verbose=4", str(app)],
        check=True,
    )
    app_details = codesign_details(executable)
    worker_details = codesign_details(worker)
    app_identifier = detail_value(app_details, "Identifier")
    if app_identifier not in EXPECTED_APP_IDENTIFIERS:
        raise RuntimeError("signed runtime app identifier is not allowlisted")
    if detail_value(app_details, "TeamIdentifier") != EXPECTED_TEAM_ID:
        raise RuntimeError("signed runtime app Team ID is not allowlisted")
    if detail_value(worker_details, "Identifier") != EXPECTED_WORKER_IDENTIFIER:
        raise RuntimeError("signed runtime Apple Speech worker identifier is incorrect")
    if detail_value(worker_details, "TeamIdentifier") != EXPECTED_TEAM_ID:
        raise RuntimeError("signed runtime Apple Speech worker Team ID is incorrect")
    worker_cdhash = detail_value(worker_details, "CDHash").lower()
    if re.fullmatch(r"[0-9a-f]{40}", worker_cdhash) is None:
        raise RuntimeError("signed runtime Apple Speech worker CDHash is invalid")
    recorded = (
        app / "Contents" / "Resources" / "minutes-apple-speech-worker.cdhash"
    ).read_text(encoding="ascii").strip()
    if recorded != worker_cdhash:
        raise RuntimeError("signed runtime Apple Speech worker CDHash receipt is stale")
    marker = b"MINUTES_APPLE_SPEECH_WORKER_CDHASH_V1=" + worker_cdhash.encode(
        "ascii"
    )
    if executable.read_bytes().count(marker) != 1:
        raise RuntimeError("signed runtime parent is not bound to the exact worker")
    return worker_cdhash


def user_temp_root() -> pathlib.Path:
    result = subprocess.run(
        ["getconf", "DARWIN_USER_TEMP_DIR"],
        check=True,
        capture_output=True,
        text=True,
    )
    root = pathlib.Path(result.stdout.strip()).resolve()
    if not root.is_dir():
        raise RuntimeError("Darwin user temporary directory is unavailable")
    return root


def regular_file_keys(root: pathlib.Path) -> set[tuple[int, int]]:
    keys: set[tuple[int, int]] = set()
    for directory, _, names in os.walk(root, followlinks=False):
        for name in names:
            path = pathlib.Path(directory) / name
            try:
                info = path.lstat()
            except OSError:
                continue
            if stat.S_ISREG(info.st_mode):
                keys.add((info.st_dev, info.st_ino))
    return keys


class SameUidOpenHolder:
    def __init__(self, roots: list[pathlib.Path]) -> None:
        self.roots = roots
        self.baseline = set().union(*(regular_file_keys(root) for root in roots))
        self.seen = set(self.baseline)
        self.held: list[int] = []
        self.stop = threading.Event()
        self.ready = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def start(self) -> None:
        self.thread.start()
        if not self.ready.wait(timeout=5):
            raise RuntimeError("same-UID open-holder watcher did not start")

    def close(self) -> list[bytes]:
        self.stop.set()
        self.thread.join(timeout=5)
        if self.thread.is_alive():
            raise RuntimeError("same-UID open-holder watcher did not stop")
        contents: list[bytes] = []
        for descriptor in self.held:
            try:
                os.lseek(descriptor, 0, os.SEEK_SET)
                chunks = []
                remaining = MAX_HELD_FILE_BYTES
                while remaining > 0:
                    chunk = os.read(descriptor, min(1024 * 1024, remaining))
                    if not chunk:
                        break
                    chunks.append(chunk)
                    remaining -= len(chunk)
                contents.append(b"".join(chunks))
            finally:
                os.close(descriptor)
        return contents

    def _run(self) -> None:
        self.ready.set()
        while not self.stop.is_set():
            for root in self.roots:
                for directory, _, names in os.walk(root, followlinks=False):
                    for name in names:
                        path = pathlib.Path(directory) / name
                        try:
                            info = path.lstat()
                        except OSError:
                            continue
                        key = (info.st_dev, info.st_ino)
                        if key in self.seen or not stat.S_ISREG(info.st_mode):
                            continue
                        self.seen.add(key)
                        try:
                            descriptor = os.open(
                                path,
                                os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW,
                            )
                        except OSError:
                            continue
                        self.held.append(descriptor)
            self.stop.wait(0.002)


def canary_patterns() -> tuple[bytes, bytes]:
    bits = [0x3E000000 + (index % 4096) for index in range(4096)]
    raw_f32 = b"".join(struct.pack("<I", value) for value in bits)
    pcm_i16 = b"".join(
        struct.pack(
            "<h",
            int(struct.unpack("<f", struct.pack("<I", value))[0] * 32767.0),
        )
        for value in bits
    )
    return raw_f32, pcm_i16


def runtime_fixture_patterns() -> tuple[bytes, bytes]:
    fixture = pathlib.Path(__file__).resolve().parent.parent / "crates/assets/demo.wav"
    with wave.open(str(fixture)) as source:
        if (
            source.getnchannels() != 1
            or source.getsampwidth() != 2
            or source.getframerate() != 16000
        ):
            raise RuntimeError("fixed Apple Speech runtime fixture format changed")
        pcm = source.readframes(source.getnframes())
    values = struct.unpack(f"<{len(pcm) // 2}h", pcm)
    raw_f32 = b"".join(struct.pack("<f", value / 32768.0) for value in values)
    # A bounded interior slice detects a named WAV/PCM spool without requiring
    # the watcher to retain or compare an entire file.
    return raw_f32[16_384:32_768], pcm[8_192:24_576]


def decode_stream(stream) -> str:
    """Normalize a captured stream that may be bytes, str, or absent."""
    if stream is None:
        return ""
    if isinstance(stream, bytes):
        return stream.decode("utf-8", "replace")
    return stream


def bounded_stream(stream: str) -> str:
    """Bound a captured stream so one runaway helper cannot flood the log."""
    text = decode_stream(stream)
    if not text:
        return "<empty>"
    if len(text) <= DIAGNOSTIC_STREAM_LIMIT:
        return text
    return f"<truncated to final {DIAGNOSTIC_STREAM_LIMIT} characters>\n" + text[
        -DIAGNOSTIC_STREAM_LIMIT:
    ]


def bounded_head(text: str) -> str:
    """Bound a document whose meaning lives at the top rather than the end.

    A macOS ``.ips`` crash report puts the exception, termination reason and
    faulting-thread backtrace near the head, and hundreds of loaded binary
    images at the end. Keeping the tail would discard the crash and print the
    dylib list, so these are bounded head-first.
    """
    if not text:
        return "<empty>"
    if len(text) <= DIAGNOSTIC_STREAM_LIMIT:
        return text
    return (
        text[:DIAGNOSTIC_STREAM_LIMIT]
        + f"\n<truncated to first {DIAGNOSTIC_STREAM_LIMIT} characters>"
    )


def bounded_lines(lines: list[str]) -> str:
    """Keep both ends of a line sequence when it exceeds the budget.

    A launch failure is explained by the first lines and a crash by the last,
    so neither end can be dropped. `com.apple.xpc.launchd` is chatty enough at
    launch to exhaust the budget before the interesting entries appear.
    """
    if len(lines) <= DIAGNOSTIC_LOG_LINES:
        return "\n".join(lines)
    half = DIAGNOSTIC_LOG_LINES // 2
    omitted = len(lines) - (half * 2)
    return "\n".join(
        lines[:half] + [f"<{omitted} lines omitted>"] + lines[-half:]
    )


def signal_label(returncode) -> str | None:
    """Render a negative exit status as its terminating signal name."""
    if returncode is None or returncode >= 0:
        return None
    try:
        return signal.Signals(-returncode).name
    except ValueError:
        return f"signal {-returncode}"


def unified_log_excerpt(started_at: float) -> str:
    """Collect helper-scoped unified log entries emitted during the run."""
    start = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(started_at))
    # The observed failure was reported parent-side, and the parent is the
    # signed app itself, so matching only the two helpers would have missed it
    # entirely. Client-side libxpc invalidation logs under "com.apple.xpc"
    # rather than "com.apple.xpc.launchd", and code-signature rejections come
    # from amfid. --private-data is deliberately NOT passed, so os_log private
    # fields stay redacted.
    predicate = (
        'process == "minutes-apple-speech-worker"'
        ' OR process == "minutes-graph-worker"'
        ' OR process == "amfid"'
        ' OR process == "kernel"'
        ' OR processImagePath CONTAINS "Minutes"'
        ' OR subsystem BEGINSWITH "com.apple.xpc"'
    )
    try:
        completed = subprocess.run(
            [
                "log",
                "show",
                "--start",
                start,
                "--info",
                "--style",
                "compact",
                "--predicate",
                predicate,
            ],
            capture_output=True,
            text=True,
            errors="replace",
            timeout=60,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return f"<unified log unavailable: {error}>"
    lines = completed.stdout.splitlines()
    if not lines:
        return f"<no matching entries; log exited {completed.returncode}>"
    return bounded_lines(lines)


def scan_crash_reports(started_at: float) -> list[tuple[pathlib.Path, str]]:
    """Collect crash reports for our own processes written during this run."""
    reports = []
    for directory in CRASH_REPORT_DIRECTORIES:
        try:
            entries = sorted(directory.iterdir())
        except OSError:
            continue
        for entry in entries:
            if entry.suffix not in {".ips", ".crash"}:
                continue
            if not entry.name.startswith(CRASH_REPORT_PREFIXES):
                continue
            try:
                if entry.stat().st_mtime < started_at:
                    continue
                reports.append((entry, entry.read_text("utf-8", "replace")))
            except OSError as error:
                reports.append((entry, f"<unreadable: {error}>"))
    return reports


def crash_report_excerpts(started_at: float) -> list[tuple[pathlib.Path, str]]:
    """Wait briefly for ReportCrash to finish writing, then collect reports.

    ReportCrash writes the .ips asynchronously after the process dies. Run
    30635749868 proved the cost of not waiting: ReportCrash was demonstrably
    running, yet the immediate scan reported no crash reports and the faulting
    symbol was lost. Poll until a report appears or the deadline expires.
    """
    deadline = time.monotonic() + CRASH_REPORT_WAIT_SECONDS
    while True:
        reports = scan_crash_reports(started_at)
        if reports or time.monotonic() >= deadline:
            return reports
        time.sleep(CRASH_REPORT_POLL_SECONDS)


def environment_excerpt(worker: pathlib.Path | None) -> str:
    """Report the runtime OS and the worker's linked platform versions.

    A binary built against an SDK newer than the running OS can leave a
    macOS-26 Speech symbol weakly bound to null, which surfaces as a Swift
    symbolic-reference failure rather than a link error. Recording the runtime
    build alongside the worker's LC_BUILD_VERSION makes that skew visible
    instead of leaving it to inference.
    """
    lines = []
    for label, argv in (
        ("sw_vers", ["sw_vers"]),
        ("worker platform", ["otool", "-l", str(worker)] if worker else None),
    ):
        if argv is None:
            continue
        try:
            completed = subprocess.run(
                argv, capture_output=True, text=True, errors="replace", timeout=60
            )
        except (OSError, subprocess.SubprocessError) as error:
            lines.append(f"{label}: <unavailable: {error}>")
            continue
        if label == "worker platform":
            keep, emit = [], False
            for line in completed.stdout.splitlines():
                token = line.strip()
                if token.startswith("cmd LC_BUILD_VERSION"):
                    emit = True
                elif emit and token.startswith("cmd "):
                    emit = False
                if emit:
                    keep.append(token)
            lines.append(f"{label}:\n" + ("\n".join(keep) or "<no LC_BUILD_VERSION>"))
        else:
            lines.append(f"{label}:\n{completed.stdout.strip()}")
    return "\n".join(lines) or "<unavailable>"


def emit_failure_diagnostics(
    started_at: float, returncode, stdout, stderr, worker: pathlib.Path | None = None
) -> None:
    """Print helper failure evidence to stderr.

    Diagnostics must never reach stdout: the workflow tees stdout into
    ``signed-runtime-provenance.json``, so anything printed there would corrupt
    the receipt. This is safe to emit in full because the runtime job holds no
    secrets and the only audio in the process is either the synthetic canary or
    the checked-in public fixture, so no signing material and no private
    utterance can appear here.
    """
    def write(line: str) -> None:
        print(line, file=sys.stderr)

    write("=== signed Apple Speech acceptance diagnostics ===")
    write(f"exit status: {returncode if returncode is not None else 'timed out'}")
    terminator = signal_label(returncode)
    if terminator:
        write(f"terminated by: {terminator}")
    write("--- environment ---")
    write(environment_excerpt(worker))
    write("--- child stdout ---")
    write(bounded_stream(stdout))
    write("--- child stderr ---")
    write(bounded_stream(stderr))
    # Crash reports come before the unified log because they are the densest
    # evidence and `log show` can burn a minute first. sys.stderr is line
    # buffered, so anything already written survives a job cancellation.
    reports = crash_report_excerpts(started_at)
    if not reports:
        write("--- crash reports: none written during this run ---")
    for path, body in reports:
        write(f"--- crash report {path} ---")
        write(bounded_head(body))
    write("--- unified log ---")
    write(bounded_stream(unified_log_excerpt(started_at)))
    write("=== end diagnostics ===")
    sys.stderr.flush()


def main() -> int:
    args = parse_args()
    if re.fullmatch(r"[0-9a-f]{40}", args.candidate_sha) is None:
        raise RuntimeError("candidate SHA must be full lowercase hex")
    app = args.app.resolve()
    executable, worker = signed_paths(app)
    worker_cdhash = verify_signed_identity(app, executable, worker)
    os_major = int(
        subprocess.run(
            ["sw_vers", "-productVersion"],
            check=True,
            capture_output=True,
            text=True,
        )
        .stdout.split(".", 1)[0]
    )
    if os_major < 26:
        raise RuntimeError("signed Apple Speech runtime acceptance requires macOS 26+")

    shared_temp = user_temp_root()
    with tempfile.TemporaryDirectory(
        prefix="minutes-apple-speech-acceptance-",
        dir=shared_temp,
    ) as isolated:
        isolated_temp = pathlib.Path(isolated).resolve()
        watcher = SameUidOpenHolder([isolated_temp, shared_temp])
        watcher.start()
        environment = os.environ.copy()
        environment["TMPDIR"] = str(isolated_temp)
        started_at = time.time()
        try:
            acceptance_argument = (
                "--apple-speech-runtime-acceptance"
                if args.runtime
                else "--apple-speech-transport-acceptance"
            )
            result = subprocess.run(
                [str(executable), acceptance_argument],
                env=environment,
                capture_output=True,
                text=True,
                # A helper killed mid multi-byte character yields undecodable
                # bytes. Strict decoding would raise UnicodeDecodeError, which
                # is neither OSError nor SubprocessError nor TimeoutExpired, so
                # it would escape every handler below and reproduce the exact
                # evidence-free failure this harness exists to prevent.
                errors="replace",
                timeout=ACCEPTANCE_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired as expired:
            # TimeoutExpired's repr omits the captured streams, so a wedged
            # helper would otherwise destroy every byte of evidence.
            emit_failure_diagnostics(
                started_at,
                None,
                decode_stream(expired.stdout),
                decode_stream(expired.stderr),
                worker,
            )
            raise RuntimeError(
                "signed Apple Speech transport acceptance timed out after "
                f"{ACCEPTANCE_TIMEOUT_SECONDS}s"
            ) from expired
        except Exception:
            # Defence in depth: any other failure launching or reading the
            # signed app must still leave evidence behind rather than a bare
            # traceback. Deliberately not BaseException, so a cancelled job is
            # not delayed by log collection. The streams are unavailable here,
            # so this reports the system-side evidence only, then re-raises.
            emit_failure_diagnostics(started_at, None, None, None, worker)
            raise
        finally:
            time.sleep(0.1)
            held_contents = watcher.close()

    if result.returncode != 0:
        emit_failure_diagnostics(
            started_at, result.returncode, result.stdout, result.stderr, worker
        )
        raise RuntimeError(
            "signed Apple Speech transport acceptance failed with "
            f"exit {result.returncode}: {result.stderr[-2000:]}"
        )
    expected_marker = (
        "apple-speech-signed-runtime=accepted"
        if args.runtime
        else "apple-speech-signed-byte-transport=accepted"
    )
    if expected_marker not in result.stdout:
        raise RuntimeError("signed app did not emit the content-free acceptance receipt")
    runtime_supported_match = re.search(
        r"^apple-speech-signed-runtime-supported=(true|false)$",
        result.stdout,
        re.MULTILINE,
    )
    if runtime_supported_match is None:
        raise RuntimeError("signed app did not report whether the Speech runtime was supported")
    runtime_supported = runtime_supported_match.group(1) == "true"
    if args.runtime and not runtime_supported:
        raise RuntimeError("signed Apple Speech worker did not run the analyzer")
    if "apple-speech-signed-parent-trusted=true" not in result.stdout:
        raise RuntimeError(
            "signed app did not confirm it evaluated its signing authority as trusted"
        )
    # Echo to stderr as well as the receipt. The receipt is only uploaded as an
    # artifact, so without this a runtimeSupported=false run would read as a
    # clean pass to anyone reading the job log.
    print(
        f"apple-speech-signed-runtime-supported={str(runtime_supported).lower()}",
        file=sys.stderr,
    )
    raw_f32, pcm_i16 = (
        runtime_fixture_patterns() if args.runtime else canary_patterns()
    )
    for contents in held_contents:
        if raw_f32 in contents or pcm_i16 in contents:
            raise RuntimeError(
                "same-UID open holder recovered acceptance audio from a named file"
            )

    receipt = {
        "candidateSha": args.candidate_sha,
        "appIdentifier": detail_value(codesign_details(executable), "Identifier"),
        "teamIdentifier": EXPECTED_TEAM_ID,
        "workerIdentifier": EXPECTED_WORKER_IDENTIFIER,
        "workerCdhash": worker_cdhash,
        "macosMajor": os_major,
        "sameUid": os.geteuid(),
        "heldNewRegularFiles": len(held_contents),
        "namedAudioCanaryObserved": False,
        "productGateExpectedClosed": True,
        "signedByteTransport": "accepted",
        "signedWorkerRuntime": "accepted" if args.runtime else "not-run",
        # Transport-only acceptance intentionally leaves runtimeSupported
        # false. Runtime mode is a real-hardware-only successor that sends the
        # fixed public fixture through the same worker and requires true.
        "runtimeSupported": runtime_supported,
        # The worker derives its peer requirement from the same trusted-
        # distribution verdict, and fails closed when that evaluation cannot
        # complete, so a trusted parent means no identifier-only downgrade.
        "parentTrustedDistribution": True,
    }
    if args.runtime:
        for field, pattern in (
            ("moduleId", r"^apple-speech-module=(.+)$"),
            ("wordCount", r"^apple-speech-word-count=([0-9]+)$"),
            ("segmentCount", r"^apple-speech-segment-count=([0-9]+)$"),
            ("firstResultElapsedMs", r"^apple-speech-first-result-ms=([0-9]+)$"),
            ("totalElapsedMs", r"^apple-speech-total-elapsed-ms=([0-9]+)$"),
        ):
            match = re.search(pattern, result.stdout, re.MULTILINE)
            if match is None:
                raise RuntimeError(f"signed runtime receipt omitted {field}")
            receipt[field] = (
                match.group(1) if field == "moduleId" else int(match.group(1))
            )
    print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
