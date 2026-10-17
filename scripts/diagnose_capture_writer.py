#!/usr/bin/env python3
"""Temporary capture-free Windows diagnosis; never changes the stop contract.

Runs original and observed copies of the synthetic fixture, alone and after its
complete libtest prefix. The observer wraps the exact File below BufWriter and
is applied only to this disposable runner checkout, then restored byte-for-byte.
Normal required CI continues to test the committed, uninstrumented source.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time

FIXTURE = "capture::tests::capture_writer_pump_drains_the_final_tail_before_finalization"
ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "crates/core/src/capture.rs"


def once(text, before, after):
    assert text.count(before) == 1, f"unexpected source anchor: {before[:70]}"
    return text.replace(before, after, 1)


def instrument(original):
    # Windows checkout line endings are not part of the source-code premise.
    # Normalize the disposable observer, while retaining original bytes for restore.
    text = original.decode().replace("\r\n", "\n")
    text = text.replace("std::io::BufWriter<std::fs::File>",
                        "std::io::BufWriter<capture_writer_probe::ObservedFile>")
    text = once(text,
                "hound::WavWriter::new(std::io::BufWriter::new(file), wav_spec_16k_mono())",
                "hound::WavWriter::new(std::io::BufWriter::new(capture_writer_probe::ObservedFile::new(file, output_path)), wav_spec_16k_mono())")
    text = once(text, "let thread = std::thread::spawn(move || {\n            let mut batch = [0.0_f32; 1600];",
                "let diagnostic_spawn = Instant::now();\n        let thread = std::thread::spawn(move || {\n            capture_writer_probe::stage(\"worker_started\", sample_count.load(Ordering::Relaxed), worker_queue.samples.len(), Some(diagnostic_spawn.elapsed().as_micros()));\n            let mut batch = [0.0_f32; 1600];")
    text = once(text, "if count > 0 {\n                    write_capture_samples(",
                "if count > 0 {\n                    capture_writer_probe::stage(\"write_batch\", sample_count.load(Ordering::Relaxed), worker_queue.samples.len(), None);\n                    write_capture_samples(")
    text = once(text, "&storage_failed,\n                    );\n                    if storage_failed.load(Ordering::Acquire)",
                "&storage_failed,\n                    );\n                    capture_writer_probe::stage(\"batch_written\", sample_count.load(Ordering::Relaxed), worker_queue.samples.len(), None);\n                    if storage_failed.load(Ordering::Acquire)")
    text = once(text, "if let Err(error) = writer.finalize() {\n                storage_failed.store(true, Ordering::Release);",
                "capture_writer_probe::stage(\"finalize_started\", sample_count.load(Ordering::Relaxed), worker_queue.samples.len(), None);\n            if let Err(error) = writer.finalize() {\n                storage_failed.store(true, Ordering::Release);")
    text = once(text, 'tracing::error!(%error, "recording WAV finalization failed");\n            }\n        });',
                'tracing::error!(%error, "recording WAV finalization failed");\n            }\n            capture_writer_probe::stage("worker_finished", sample_count.load(Ordering::Relaxed), worker_queue.samples.len(), None);\n        });')
    text = once(text, "pump.queue.push_samples(&samples);\n        pump.finish().unwrap();",
                "pump.queue.push_samples(&samples);\n        let diagnostic_stop = Instant::now();\n        let diagnostic_result = pump.finish();\n        capture_writer_probe::report(\"stop_return\", diagnostic_stop.elapsed().as_micros(), &diagnostic_result, count.load(Ordering::Relaxed), pump.queue.samples.len(), failed.load(Ordering::Acquire));\n        if diagnostic_result.is_err() {\n            // Observe retirement after the unchanged deadline; never accept a failed stop.\n            let observation = Instant::now();\n            while !capture_writer_probe::worker_finished() && observation.elapsed() < std::time::Duration::from_secs(5) { std::thread::sleep(std::time::Duration::from_millis(10)); }\n            capture_writer_probe::report(\"post_deadline_observation\", diagnostic_stop.elapsed().as_micros(), &diagnostic_result, count.load(Ordering::Relaxed), pump.queue.samples.len(), failed.load(Ordering::Acquire));\n        }\n        capture_writer_probe::clear();\n        diagnostic_result.unwrap();")
    # Preserve every production guard and final-tail assertion exactly.
    for anchor in ["self.finish_with_timeout(std::time::Duration::from_secs(2))",
                   "return Err(CaptureError::StorageRetiring)",
                   "assert_eq!(count.load(Ordering::Relaxed), samples.len() as u64)",
                   "assert_eq!(reader.duration(), samples.len() as u32)",
                   "crate::overlays::attest_owner_only_file_handle(&file)?",
                   "fs2::FileExt::try_lock_exclusive(&file)?"]:
        assert anchor in text
    return (text + "\n" + (ROOT / "scripts/diagnostics/capture_writer_probe.rs").read_text()).encode()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--prepare-only", action="store_true")
    mode.add_argument("--apply", action="store_true")
    mode.add_argument("--restore", action="store_true")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    saved = output / "original-capture.rs"
    if args.restore:
        if saved.exists():
            original = saved.read_bytes()
            assert SOURCE.read_bytes() in (original, instrument(original)), "unexpected runner edit"
            SOURCE.write_bytes(original)
            assert SOURCE.read_bytes() == original
        return
    original = SOURCE.read_bytes()
    observed = instrument(original)
    provenance = {"original_sha256": hashlib.sha256(original).hexdigest(),
                  "canonical_original_sha256": hashlib.sha256(original.replace(b"\r\n", b"\n")).hexdigest(),
                  "observed_sha256": hashlib.sha256(observed).hexdigest(),
                  "production_timeout_seconds": 2,
                  "observed_file_is_below_bufwriter": True,
                  "real_capture": False}
    (output / "provenance.json").write_text(json.dumps(provenance, indent=2))
    if args.apply:
        assert not saved.exists(), "do not overwrite an earlier diagnostic snapshot"
        saved.write_bytes(original)
        SOURCE.write_bytes(observed)
        print(json.dumps(provenance), flush=True)
        return
    if args.prepare_only:
        (output / "capture-observed.rs").write_bytes(observed)
        return

    cases = []

    def command(name, argv, timeout=900):
        start = time.monotonic()
        result = subprocess.run(argv, cwd=ROOT, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=timeout)
        (output / (name + ".log")).write_text(result.stdout, encoding="utf-8")
        print(json.dumps({"case": name, "exit": result.returncode,
                          "wall_seconds": round(time.monotonic() - start, 3)}), flush=True)
        return result

    def build(name, features):
        argv = ["cargo", "test", "-p", "minutes-core", "--no-default-features", "--lib"]
        if features:
            argv.extend(["--features", features])
        result = command(name + "-build", [*argv, "--no-run", "--message-format=json"])
        assert result.returncode == 0, "diagnostic test binary must build"
        artifacts = []
        for line in result.stdout.splitlines():
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if value.get("reason") == "compiler-artifact" and value.get("executable") and value.get("profile", {}).get("test"):
                artifacts.append(value["executable"])
        assert len(artifacts) == 1
        return artifacts[0]

    def run_cases(label, features):
        executable = build(label, features)
        listing = subprocess.check_output([executable, "--list"], cwd=ROOT, text=True)
        names = re.findall(r"^([^\s]+): test$", listing, re.M)
        assert FIXTURE in names
        prefix = sorted(n for n in names if n <= FIXTURE)
        for suffix, selected in [("isolated", [FIXTURE]), ("prefix", prefix)]:
            argv = [executable, *selected, "--exact", "--test-threads=1", "--nocapture"]
            assert len(subprocess.list2cmdline(argv)) < 30000, "bounded Windows command line"
            name = label + "-" + suffix
            result = command(name, argv)
            measurements = []
            for line in result.stdout.splitlines():
                if "CAPTURE_WRITER_PROBE " in line:
                    measurements.append(json.loads(line.split("CAPTURE_WRITER_PROBE ", 1)[1]))
            cases.append({"case": name, "selected_tests": len(selected), "exit": result.returncode,
                          "test_result": re.findall(r"test result: .*$", result.stdout, re.M),
                          "measurements": measurements})
            (output / "summary.json").write_text(json.dumps({"provenance": provenance, "cases": cases}, indent=2))

    try:
        assert command("build-model-free-cli", ["cargo", "build", "-p", "minutes-cli", "--no-default-features"]).returncode == 0
        run_cases("original-model-free", "")
        run_cases("original-pocketstation", "pocketstation-capture")
        SOURCE.write_bytes(observed)
        run_cases("observed-model-free", "")
        run_cases("observed-pocketstation", "pocketstation-capture")
    finally:
        assert SOURCE.read_bytes() in (original, observed), "preserve any unexpected runner edit"
        SOURCE.write_bytes(original)
        assert SOURCE.read_bytes() == original
    # Failed fixture guards remain failed; this is not a substitute for required CI.
    raise SystemExit(1 if any(case["exit"] for case in cases) else 0)


if __name__ == "__main__":
    main()
