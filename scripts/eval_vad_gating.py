"""Generate and score the local whole-file Whisper VAD evaluation corpus."""

import argparse
import json
import math
import os
import random
import re
import subprocess
import sys
import tempfile
import time
import wave
from array import array
from collections import defaultdict
from dataclasses import dataclass
from itertools import groupby
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "tests/eval/fixtures"
MANIFESTS = ("dictation-benchmark-corpus.json", "apple-speech-corpus.json")
RATE = 16_000
VAD_MODEL = "silero-v6.2.0"
TIMESTAMP = re.compile(r"^\[(\d+):([0-5]\d)\]\s*")
COLLAPSE = re.compile(r"^\[\.\.\.\]|\[[^\]]*\bcollapsed\]", re.IGNORECASE)
OTHER_ANNOTATION = re.compile(
    r"^\[(?:Recording ended - \d+ lines of trailing noise|\d+ scattered non-speech markers) removed\]$"
)


@dataclass(frozen=True)
class Case:
    name: str
    audio: Path
    category: str
    reference: str | None
    gaps_seconds: tuple[tuple[float, float], ...] = ()


def source_cases(directory):
    manifests = [directory / "corpus.json"]
    if not manifests[0].is_file():
        manifests = [directory / name for name in MANIFESTS if (directory / name).is_file()]
    if not manifests:
        raise ValueError(f"No corpus manifest found in {directory}")

    cases = {}
    for manifest in manifests:
        entries = json.loads(manifest.read_text(encoding="utf-8"))
        if not isinstance(entries, list):
            raise ValueError(f"{manifest} must contain a JSON array")
        for entry in entries:
            audio = (manifest.parent / entry["audioPath"]).resolve()
            if directory == FIXTURES and not audio.is_file():
                # apple-speech-corpus.json uses paths from the repository root.
                audio = (ROOT / entry["audioPath"]).resolve()
            reference = entry["referenceText"]
            if not isinstance(reference, str):
                raise ValueError(f"referenceText must be a string for {audio}")
            category = "clean" if words(reference) else "noise-only"
            case = Case(entry.get("id", audio.stem), audio, category, reference)
            previous = cases.setdefault(audio, case)
            if previous.reference != reference:
                raise ValueError(f"Conflicting references for {audio}")

    if directory == FIXTURES:
        assets = ROOT / "crates/assets"
        meeting = FIXTURES / "audio/apple-speech-meeting.wav"
        demo = assets / "demo.wav"
        # Use the bundled asset once when it is identical to the meeting fixture.
        if meeting in cases and demo.read_bytes() == meeting.read_bytes():
            reference = cases.pop(meeting).reference
            cases[demo] = Case("demo", demo, "clean", reference)
            quiet = assets / "parity_low_volume.wav"
            cases[quiet] = Case(quiet.stem, quiet, "quiet-speech", reference)
        for audio in sorted(assets.glob("*.wav")):
            cases.setdefault(audio, Case(audio.stem, audio, "asset-unscored", None))
    return list(cases.values())


def read_pcm(path):
    with wave.open(str(path), "rb") as source:
        if (source.getnchannels(), source.getsampwidth(), source.getframerate()) != (1, 2, RATE):
            raise ValueError(f"Expected mono 16 kHz 16-bit PCM WAV: {path}")
        frames = source.getnframes()
        pcm = source.readframes(frames)
        if frames == 0 or len(pcm) != 2 * frames:
            raise ValueError(f"Empty or truncated WAV: {path}")
        return pcm


def silence(seconds):
    return bytes(seconds * RATE * 2)


def pink_noise(seconds, dbfs):
    # Sum octave-spaced random sample-and-holds, then set the RMS level.
    rng = random.Random(1417)
    bands = [rng.uniform(-1, 1) for _ in range(12)]
    level = sum(bands)
    signal = array("f")
    for index in range(1, seconds * RATE + 1):
        band = (index & -index).bit_length() - 1
        if band < len(bands):
            updated = rng.uniform(-1, 1)
            level += updated - bands[band]
            bands[band] = updated
        signal.append(level)
    mean = sum(signal) / len(signal)
    rms = math.sqrt(sum((sample - mean) ** 2 for sample in signal) / len(signal))
    scale = 32768 * 10 ** (dbfs / 20) / rms
    pcm = array("h", (round((sample - mean) * scale) for sample in signal))
    if sys.byteorder != "little":
        pcm.byteswap()
    return pcm.tobytes()


def write_case(directory, name, category, reference, parts):
    path = directory / f"{name}.wav"
    offset = 0
    gaps = []
    with wave.open(str(path), "wb") as output:
        output.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        for pcm, is_gap in parts:
            end = offset + len(pcm) // 2
            if is_gap:
                gaps.append((offset / RATE, end / RATE))
            output.writeframesraw(pcm)
            offset = end
    return Case(name, path, category, reference, tuple(gaps))


def stress_cases(directory, speech_cases):
    first = speech_cases[0]
    second = speech_cases[1] if len(speech_cases) > 1 else first
    left, right = read_pcm(first.audio), read_pcm(second.audio)
    reference = f"{first.reference} {second.reference}"
    for seconds in (20, 60):
        yield write_case(directory, f"silence-{seconds}s", "silence-gap", reference,
                         [(left, False), (silence(seconds), True), (right, False)])
        for dbfs in (-50, -40):
            yield write_case(directory, f"pink-{seconds}s-{abs(dbfs)}dbfs", "pink-noise-gap", reference,
                             [(left, False), (pink_noise(seconds, dbfs), True), (right, False)])
    yield write_case(directory, "silence-30s-each-edge", "edge-silence", reference,
                     [(silence(30), True), (left, False), (right, False), (silence(30), True)])
    yield write_case(directory, "pink-only-90s-40dbfs", "noise-only", "",
                     [(pink_noise(90, -40), False)])


def words(text):
    return re.findall(r"[^\W_]+", text.casefold().replace("'", "").replace("’", ""))


def edit_distance(reference, hypothesis):
    previous = list(range(len(hypothesis) + 1))
    for index, expected in enumerate(reference, 1):
        current = [index]
        for column, actual in enumerate(hypothesis, 1):
            current.append(min(previous[column] + 1, current[-1] + 1,
                               previous[column - 1] + (expected != actual)))
        previous = current
    return previous[-1]


def score(reference, transcript, gaps):
    collapse_lines = 0
    gap_lines = []
    spoken = []
    timestamps_complete = True
    for line in transcript.splitlines():
        line = line.strip()
        stamp = TIMESTAMP.match(line)
        content = line[stamp.end():].strip() if stamp else line
        collapse = COLLAPSE.search(content) is not None
        if collapse or OTHER_ANNOTATION.fullmatch(content):
            collapse_lines += collapse
            spoken.append("")  # An annotation interrupts a run of identical lines.
            continue
        spoken.append(content)
        if not words(content):
            continue
        if stamp is None:
            timestamps_complete = False
            continue
        start = int(stamp[1]) * 60 + int(stamp[2])
        if any(begin <= start < end for begin, end in gaps):
            gap_lines.append(line)

    hypothesis = words(" ".join(spoken))
    expected = words(reference) if reference is not None else None
    edits = edit_distance(expected, hypothesis) if expected is not None else None
    has_gap_score = bool(gaps) and timestamps_complete
    return {
        "reference_words": len(expected) if expected is not None else None,
        "word_edits": edits,
        "wer": edits / len(expected) if expected else None,
        "noise_only_words": len(hypothesis) if expected == [] else 0,
        "collapse_lines": collapse_lines,
        "repeated_line_runs": sum(
            1 for content, lines in groupby(spoken) if content and sum(1 for _ in lines) >= 3
        ),
        "gap_lines": gap_lines if has_gap_score else None,
        "gap_words": sum(len(words(TIMESTAMP.sub("", line))) for line in gap_lines)
        if has_gap_score else None,
    }


def transcribe(binary, case, model, model_files):
    with tempfile.TemporaryDirectory(prefix="minutes-vad-eval-") as directory:
        home = Path(directory)
        models = home / ".minutes/models"
        models.mkdir(parents=True)
        for source in model_files:
            (models / source.name).symlink_to(source)
        config = home / ".config/minutes/config.toml"
        config.parent.mkdir(parents=True)
        config.write_text(
            '[transcription]\nengine = "whisper"\n'
            f'model = "{model}"\nvad_model = "{VAD_MODEL}"\n'
            f'model_path = {json.dumps(str(models), ensure_ascii=False)}\n',
            encoding="utf-8",
        )
        env = {
            "PATH": os.environ.get("PATH", os.defpath),
            "HOME": directory,
            "TMPDIR": directory,
            "XDG_CONFIG_HOME": str(home / ".config"),
            "XDG_DATA_HOME": str(home / ".local/share"),
            "XDG_CACHE_HOME": str(home / ".cache"),
            "XDG_STATE_HOME": str(home / ".local/state"),
            "MINUTES_DATA_DIR": str(models.parent),
            "MINUTES_CONFIG_PATH": str(config),
        }
        start = time.perf_counter()
        result = subprocess.run([str(binary), "transcribe", str(case.audio)],
                                cwd=home, env=env, capture_output=True, encoding="utf-8")
        elapsed = time.perf_counter() - start
    if result.returncode != 0:
        raise ValueError(f"{binary} failed on {case.audio} (exit {result.returncode}):\n"
                         f"{result.stderr.strip() or result.stdout.strip()}")
    if result.stdout.startswith("[Transcription placeholder"):
        raise ValueError(f"{binary} needs to be built with Whisper support")
    return {
        "transcript": result.stdout.rstrip(),
        "wall_seconds": elapsed,
        **score(case.reference, result.stdout, case.gaps_seconds),
    }


def summarize(rows):
    totals = {}
    for side in ("baseline", "candidate"):
        metrics = [row[side] for row in rows]
        eligible = [entry for entry in metrics if entry["reference_words"]]
        reference_words = sum(entry["reference_words"] for entry in eligible)
        gap_words = [row[side]["gap_words"] for row in rows if row["gaps_seconds"]]
        totals[side] = {
            "wer": sum(entry["word_edits"] for entry in eligible) / reference_words
            if reference_words else None,
            "gap_words": sum(gap_words) if gap_words and None not in gap_words else None,
            **{key: sum(entry[key] for entry in metrics) for key in
               ("noise_only_words", "collapse_lines", "repeated_line_runs", "wall_seconds")},
        }
    return {"files": len(rows), **totals, "wer_delta": wer_delta(totals)}


def wer_delta(row):
    old, new = (row[side]["wer"] for side in ("baseline", "candidate"))
    return new - old if old is not None and new is not None else None


def metric_cells(row):
    old, new = row["baseline"], row["candidate"]

    def display(value, spec):
        return "n/a" if value is None else format(value, spec)

    def pair(key, spec=""):
        return f"{display(old[key], spec)} / {display(new[key], spec)}"

    delta = row["wer_delta"]
    return [
        display(old["wer"], ".1%"), display(new["wer"], ".1%"),
        "n/a" if delta is None else f"{delta * 100:+.1f} pp",
        pair("noise_only_words"), pair("gap_words"), pair("collapse_lines"),
        pair("repeated_line_runs"), pair("wall_seconds", ".2f"),
    ]


def markdown(model, categories, rows):
    columns = ["Baseline WER", "Candidate WER", "WER delta", "Noise-only words", "Gap words",
               "Collapse lines", "Repeated runs", "Wall seconds"]

    def table(labels, entries):
        return "\n".join("| " + " | ".join(line) + " |" for line in
                         [labels + columns, ["---"] * (len(labels) + len(columns)), *entries])

    def cell(value):
        return " ".join(value.splitlines()).replace("|", "&#124;")

    summary = table(["Category", "Files"], [
        [name, str(row["files"]), *metric_cells(row)] for name, row in categories.items()
    ])
    per_file = table(["File", "Category"], [
        [cell(row["file"]), row["category"], *metric_cells(row)] for row in rows
    ])
    return (
        f"# Whole-file Whisper VAD evaluation\n\nModel `{model}` with VAD `{VAD_MODEL}`. "
        "Paired values show baseline / candidate. Category WER is total edits divided by total "
        "reference words. Empty and missing references have no WER. "
        "Gap words use reported line starts at whole-second precision. "
        "See `results.json` for transcripts and gap boundaries.\n\n"
        f"## By category\n\n{summary}\n\n## By file\n\n{per_file}\n"
    )


def self_test():
    assert score("Hello, WORLD!", "[0:00] hello world", ())["wer"] == 0
    assert score("one two", "one three", ())["wer"] == 0.5
    assert score("one two", "one", ())["word_edits"] == 1
    assert score("one", "one two", ())["word_edits"] == 1
    empty = score("", "[0:00] stray words", ())
    assert empty["wer"] is None and empty["word_edits"] == empty["noise_only_words"] == 2
    assert score("", "", ())["noise_only_words"] == 0
    assert score(None, "unscored", ())["wer"] is None
    repeated = score("yes yes yes", "[0:00] yes\n[0:01] yes\n[0:02] yes\n"
                     "[...] [2 lines collapsed]\n[0:03] yes\n[0:04] yes", ())
    assert repeated["repeated_line_runs"] == repeated["collapse_lines"] == 1
    assert repeated["word_edits"] == 2
    gaps = ((20.0, 40.0),)
    gap = score("hello", "[0:19] hello\n[0:20] two words\n[0:40] goodbye", gaps)
    assert gap["gap_words"] == 2 and gap["gap_lines"] == ["[0:20] two words"]
    annotated = score("hello", "[0:19] hello\n[Recording ended - 1 lines of trailing noise removed]\n"
                      "[3 scattered non-speech markers removed]", gaps)
    assert annotated["wer"] == annotated["gap_words"] == annotated["collapse_lines"] == 0
    assert score("hello", "untimed text", gaps)["gap_words"] is None
    assert score("", "", gaps)["gap_words"] == 0
    print("VAD eval self-test passed (WER, empty references, collapse lines, repeated runs, gap words)")


def evaluate(args):
    binaries = {side: getattr(args, side).resolve() for side in ("baseline", "candidate")}
    for binary in binaries.values():
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise ValueError(f"Expected an executable Minutes binary: {binary}")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", args.model):
        raise ValueError("--model must be a Whisper model name")
    models = Path.home() / ".minutes/models"
    names = (f"ggml-{args.model}.bin", f"whisper-{args.model}.bin", f"{args.model}.bin")
    whisper = next((models / name for name in names if (models / name).is_file()), None)
    silero = models / f"ggml-{VAD_MODEL}.bin"
    if whisper is None or not silero.is_file():
        raise ValueError(f"Need a local Whisper {args.model} model and {silero.name} in {models}")

    cases = source_cases(args.corpus.resolve())
    speech = [case for case in cases if case.category == "clean"]
    if not speech:
        raise ValueError("The corpus needs at least one speech WAV with a nonempty reference")
    for case in cases:
        read_pcm(case.audio)

    out = args.out.resolve()
    out.mkdir(mode=0o700, parents=True, exist_ok=True)
    generated = Path(tempfile.mkdtemp(prefix="corpus-", dir=out))
    cases.extend(stress_cases(generated, speech))

    rows = []
    by_category = defaultdict(list)
    for index, case in enumerate(cases):
        print(f"Transcribing {case.audio.name} ({index + 1}/{len(cases)})", flush=True)
        row = {
            "id": case.name, "file": case.audio.name, "audio": str(case.audio),
            "category": case.category, "reference": case.reference, "gaps_seconds": case.gaps_seconds,
        }
        # Alternate which binary runs first to distribute process and cache warm-up effects.
        order = tuple(binaries) if index % 2 == 0 else tuple(reversed(binaries))
        for side in order:
            row[side] = transcribe(binaries[side], case, args.model, (whisper, silero))
        row["wer_delta"] = wer_delta(row)
        by_category[case.category].append(row)
        rows.append(row)

    categories = {name: summarize(entries) for name, entries in by_category.items()}
    results = {
        "model": args.model, "vad_model": VAD_MODEL, "corpus": str(args.corpus.resolve()),
        "binaries": {side: str(path) for side, path in binaries.items()},
        "categories": categories, "files": rows,
    }
    (out / "results.json").write_text(json.dumps(results, indent=2, ensure_ascii=False) + "\n",
                                      encoding="utf-8")
    (out / "report.md").write_text(markdown(args.model, categories, rows), encoding="utf-8")
    print(f"Wrote {out / 'report.md'} and {out / 'results.json'}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--model", default="small")
    parser.add_argument("--corpus", type=Path, default=FIXTURES)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not (args.baseline and args.candidate and args.out):
        parser.error("--baseline, --candidate and --out are required unless running --self-test")
    try:
        evaluate(args)
    except (OSError, ValueError, KeyError, TypeError, wave.Error, EOFError) as error:
        parser.exit(1, f"VAD evaluation failed: {error}\n")


if __name__ == "__main__":
    main()
