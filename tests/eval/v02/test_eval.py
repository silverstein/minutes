"""Metric and fail-closed harness regression tests; no model/audio downloads."""
import hashlib
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import wave

from corpus import synthetic_suite, validate_suite, write_suite
from mcp_eval import McpClient, category_comparisons, citation_valid, exact_substring_baseline, selected_cli_metadata
from metrics import critical_errors, diarization_error, percentile, retrieval_scores, word_errors
from speech_eval import aggregate, gate_report, load_manifest, score_recording
from select_models import select


class MetricTests(unittest.TestCase):
    def test_duplicates_do_not_inflate_recall_or_discounted_gain(self):
        result = retrieval_scores(["a", "a"], {"a": 2, "b": 1}, 2)
        self.assertEqual(result["recall_at_k"], .5)
        self.assertLess(result["ndcg_at_k"], 1)
        self.assertEqual(retrieval_scores(["a", "b"], {"a": 2, "b": 1}, 2)["ndcg_at_k"], 1)

    def test_unknown_questions_require_abstention(self):
        self.assertTrue(retrieval_scores([], {}, 5)["abstention"])
        self.assertFalse(retrieval_scores(["hallucinated"], {}, 5)["abstention"])
        self.assertIsNone(retrieval_scores([], {}, 5)["recall_at_k"])

    def test_wer_errors_and_empty_reference(self):
        result = word_errors("Alice approved 12 units", "Alice denied 12")
        self.assertEqual(result["substitutions"], 1)
        self.assertEqual(result["deletions"], 1)
        self.assertEqual(result["wer"], .5)
        self.assertEqual(word_errors("a", "a a a")["wer"], 2)
        self.assertIsNone(word_errors("", "unexpected")["wer"])

    def test_critical_inserted_negation_and_repeated_number(self):
        score = critical_errors("approve 12", "not approve 12 12", [
            {"text": "not", "kind": "negation"}, {"text": "12", "kind": "number"}])
        self.assertEqual(score["errors"], 2)

    def test_der_maps_speakers_and_scores_overlap_misses_and_false_alarm(self):
        ref = [{"start": 0, "end": 2, "speaker": "Alice"},
               {"start": 1, "end": 3, "speaker": "Bob"}]
        hyp = [{"start": 0, "end": 2, "speaker": "s2"},
               {"start": 1, "end": 3, "speaker": "s1"}]
        self.assertEqual(diarization_error(ref, hyp, 4)["der"], 0)
        self.assertEqual(diarization_error(ref, hyp[:1], 4)["der"], .5)
        extra = hyp + [{"start": 3, "end": 4, "speaker": "s1"}]
        self.assertEqual(diarization_error(ref, extra, 4)["der"], .25)
        wrong = [{"start": 0, "end": 3, "speaker": "one"}]
        self.assertEqual(diarization_error(ref, wrong, 4)["der"], .5)

    def test_der_rejects_bad_intervals(self):
        for end in (-1, 5, float("nan")):
            with self.assertRaises(ValueError):
                diarization_error([{"start": 0, "end": end, "speaker": "s"}], [], 4)

    def test_latency_percentiles_are_nearest_rank(self):
        self.assertEqual(percentile(list(range(1, 101)), .95), 95)
        self.assertIsNone(percentile([], .5))


class CorpusTests(unittest.TestCase):
    def test_cli_provenance_rejects_a_shadowing_release_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = root / "crates/mcp/dist/index.js"
            server.parent.mkdir(parents=True)
            server.write_text("// server")
            filename = "minutes.exe" if sys.platform == "win32" else "minutes"
            debug, release = root / "target/debug" / filename, root / "target/release" / filename
            debug.parent.mkdir(parents=True)
            release.parent.mkdir(parents=True)
            debug.write_bytes(b"candidate")
            with patch("mcp_eval.subprocess.run") as run:
                run.return_value.stdout = "minutes-test"
                metadata = selected_cli_metadata(server, debug)
            self.assertEqual(metadata["sha256"], hashlib.sha256(b"candidate").hexdigest())
            release.write_bytes(b"shadow")
            with self.assertRaises(ValueError):
                selected_cli_metadata(server, debug)

    def test_offline_baseline_reproduces_literal_substring_limitation(self):
        suite = synthetic_suite()
        self.assertEqual(len(exact_substring_baseline(suite, "software billing cadence", 10)), 2)
        self.assertEqual(exact_substring_baseline(suite, "cadence software billing", 10), [])
        self.assertEqual(exact_substring_baseline(suite, "restrictedmarker", 10), [])

    def test_comparisons_count_failed_candidates_as_zero(self):
        report = category_comparisons([{"kind": "retrieval", "category": "token_order", "domain": "museum",
             "passed": False, "offline_substring_baseline": {"recall_at_k": 1, "ndcg_at_k": 1}}])
        self.assertEqual(report["category:token_order"]["delta_recall_at_k"], -1)

    def test_public_synthetic_suite_has_domains_and_realistic_claim_limits(self):
        suite = synthetic_suite()
        validate_suite(suite)
        self.assertFalse(suite["provenance"]["blinded"])
        self.assertEqual(sum(c["kind"] == "retrieval" for c in suite["cases"]), 96)
        self.assertEqual(sum(c["kind"] == "currentness" for c in suite["cases"]), 24)
        self.assertEqual(sum(c["kind"] == "abstention" for c in suite["cases"]), 3)
        joanna = next(doc["content"] for doc in suite["documents"] if doc["path"] == "identity-joanna.md")
        self.assertIn('"name": "Joanna Flores"', joanna)
        self.assertNotIn('"name": "Ann Carter"', joanna)
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(write_suite(suite, Path(directory), 100), 100)
            self.assertEqual(len(list(Path(directory).glob("*.md"))), 100)

    def test_unsafe_fixture_path_rejected(self):
        suite = synthetic_suite()
        suite["documents"][0]["path"] = "../escape.md"
        with self.assertRaises(ValueError):
            validate_suite(suite)

    def test_citations_require_exact_revision_quote_and_utf16_offset(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "source.md"
            raw = "Intro 😀\nExact quote."
            path.write_text(raw)
            start = len("Intro 😀\n".encode("utf-16-le")) // 2
            citation = {"path": str(path), "source_revision": hashlib.sha256(raw.encode()).hexdigest(),
                        "quote": "Exact quote.", "start": start, "end": start + 12, "offset_unit": "utf16",
                        "line_start": 2, "line_end": 2}
            self.assertTrue(citation_valid(citation, root))
            self.assertFalse(citation_valid({**citation, "quote": "Wrong quote."}, root))
            self.assertFalse(citation_valid({**citation, "start": start - 1}, root))
            self.assertFalse(citation_valid({**citation, "line_start": 1}, root))
            path.write_text(raw + " edited")
            self.assertFalse(citation_valid(citation, root))
            path.unlink()
            self.assertFalse(citation_valid(citation, root))

    def test_mcp_uses_handshake_and_tool_protocol(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "server.py"
            script.write_text('''import json,sys
initialized = False
for line in sys.stdin:
    request = json.loads(line)
    if request['method'] == 'notifications/initialized':
        initialized = True
        continue
    result = {'protocolVersion': '2024-11-05'}
    if request['method'] == 'tools/call':
        result = {'structuredContent': {'handshake_seen': initialized, 'args': request['params']}}
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
''')
            client = McpClient([sys.executable, str(script)], root, root, 2)
            try:
                result, _ = client.tool("search_meetings", {"query": "billing"})
                self.assertTrue(result["structuredContent"]["handshake_seen"])
                self.assertEqual(result["structuredContent"]["args"]["name"], "search_meetings")
            finally:
                client.close()


class SpeechTests(unittest.TestCase):
    def test_selection_enforces_constraints_and_ignores_fake_summary(self):
        row = score_recording({"id": "one", "split": "holdout", "_duration": 60,
                               "reference_text": "Ann approves 12", "language": "en",
                               "segments": [{"start": 0, "end": 60, "speaker": "Ann"}]},
                              {"text": "Ann approves 12", "processing_seconds": 10,
                               "segments": [{"start": 0, "end": 60, "speaker": "0"}]})
        report = {"schema_version": 1, "manifest_sha256": "a" * 64,
                  "provenance": {"kind": "consented-recordings", "independent_holdout": True, "synthetic": False},
                  "candidate": {"model": "test", "revision": "1", "hardware": "test-cpu", "platform": "linux", "privacy": "local", "cost_per_audio_hour": 0},
                  "timing_source": "harness-wall-clock", "gate": {"passed": True}, "rows": [row],
                  "summaries": {"holdout": {"wer": -100}}}
        requirements = {"manifest_sha256": "a" * 64, "hardware": "test-cpu", "platform": "linux", "privacy": "local",
                        "max_cost_per_audio_hour": 1, "min_recordings": 1, "min_audio_seconds": 60,
                        "max_wer": .2, "max_der": .2, "max_critical_errors": 0}
        cloud = copy.deepcopy(report)
        cloud["candidate"]["privacy"] = "cloud"
        result = select([("local.json", report), ("cloud.json", cloud)], requirements)
        self.assertEqual(result["recommendation"]["report"], "local.json")
        self.assertEqual(result["recommendation"]["summary"]["wer"], 0)
        self.assertEqual(len(result["excluded"]), 1)
        report["provenance"]["synthetic"] = True
        self.assertIsNone(select([("synthetic.json", report)], requirements)["recommendation"])

    def test_selection_rejects_inconsistent_holdout_coverage(self):
        requirements = {"manifest_sha256": "a" * 64, "hardware": "test", "platform": "linux", "privacy": "local",
                        "max_cost_per_audio_hour": 1, "min_recordings": 1, "min_audio_seconds": 60,
                        "max_wer": .2, "max_der": .2, "max_critical_errors": 0}
        reports = [("one", {"manifest_sha256": "a" * 64, "rows": [{"id": "one", "split": "holdout"}]}),
                   ("two", {"manifest_sha256": "a" * 64, "rows": []})]
        result = select(reports, requirements)
        self.assertIsNone(result["recommendation"])
        self.assertTrue(all("candidate reports disagree on holdout recording coverage" in item["reasons"] for item in result["excluded"]))

    def test_gates_fail_when_metrics_or_holdout_missing(self):
        empty = aggregate([])
        self.assertFalse(gate_report(empty, {"max_wer": .2})["passed"])
        self.assertFalse(gate_report(empty, {"min_recordings": 0})["passed"])
        self.assertFalse(gate_report(empty, None)["passed"])
        with self.assertRaises(ValueError):
            gate_report(empty, {"unknown": 1})

    def test_real_wav_manifest_and_scoring(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio = root / "sample.wav"
            # Synthetic silence tests the harness only; never reported as speech evidence.
            with wave.open(str(audio), "wb") as wav:
                wav.setnchannels(1)
                wav.setsampwidth(2)
                wav.setframerate(8000)
                wav.writeframes(b"\0\0" * 8000)
            row = {"id": "one", "audio": "sample.wav", "sha256": hashlib.sha256(audio.read_bytes()).hexdigest(),
                   "authorization": {"basis": "license", "reference": "test-generated silence"},
                   "reference_text": "Ann approves 12", "language": "en", "split": "holdout",
                   "critical_terms": [{"text": "Ann", "kind": "name"}],
                   "segments": [{"start": 0, "end": 1, "speaker": "Ann"}]}
            path = root / "manifest.json"
            path.write_text(json.dumps({"schema_version": 1, "recordings": [row]}))
            manifest = load_manifest(path)
            scored = score_recording(manifest["recordings"][0], {"text": "Ann not approves 12",
                "segments": [{"start": 0, "end": 1, "speaker": "0"}], "processing_seconds": .5})
            self.assertEqual(scored["critical"]["errors"], 1)
            summary = aggregate([scored])
            self.assertEqual(summary["real_time_factor"], .5)
            self.assertFalse(gate_report(summary, {"max_critical_errors": 0})["passed"])
            self.assertTrue(gate_report(summary, {"max_der": 0})["passed"])
            row["sha256"] = "bad"
            path.write_text(json.dumps({"schema_version": 1, "recordings": [row]}))
            with self.assertRaises(ValueError):
                load_manifest(path)


if __name__ == "__main__":
    unittest.main()
