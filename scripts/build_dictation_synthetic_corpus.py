#!/usr/bin/env python3
"""Build an explicitly synthetic, local-only dictation benchmark on macOS.

Uses installed `say` voices. Never records a microphone, downloads a voice,
changes Minutes settings, or sends text to a service. This is a reproducible
smoke corpus; it cannot establish real-speaker accuracy or a Wispr comparison.
"""
import argparse
import hashlib
import json
import platform
import shutil
import subprocess
from pathlib import Path

SENTENCES = [
    ("chat", "Can you check the latest dictation changes in Minutes?", ["Minutes"], []),
    ("chat", "I will be there in ten minutes. Please wait for me.", ["ten"], []),
    ("negation", "Do not send the message. Keep the original draft.", ["not", "original"], []),
    ("negation", "I am not agreeing to delete the file.", ["not"], []),
    ("negation", "Never overwrite the recording, even if the summary fails.", ["Never", "recording"], []),
    ("numbers", "The invoice is one hundred twenty dollars and fifty cents.", ["hundred", "fifty"], []),
    ("numbers", "We need twelve seats, not twenty seats.", ["twelve", "not", "twenty"], []),
    ("numbers", "The meeting starts at nine thirty tomorrow morning.", ["nine", "thirty"], []),
    ("names", "Please ask Elodie and Mat about the Minutes release.", ["Elodie", "Mat", "Minutes"], []),
    ("names", "Wispr Flow and CleanShot have useful interaction details.", ["Wispr", "CleanShot"], []),
    ("names", "OpenAI, GitHub, and Ghostty should keep their own spelling.", ["OpenAI", "GitHub", "Ghostty"], []),
    ("email", "Hello Alex. Thank you for reviewing this. Best, Mat.", ["Alex", "Mat"], []),
    ("email", "Please attach the notes and reply only to the original sender.", ["only", "original"], []),
    ("agent", "Check the source and run the focused tests before changing the default.", ["focused", "default"], []),
    ("agent", "Preserve the dirty worktree and use a separate branch for this change.", ["dirty", "separate"], []),
    ("correction", "Actually, leave the first version alone. That was only an example.", ["Actually", "alone", "only"], []),
    ("correction", "I said keep it, not remove it. Please show me a preview first.", ["keep", "not", "preview"], []),
    ("multiline", "First check the microphone. Then recover the saved text. Finally copy it.", ["microphone", "recover", "copy"], []),
    ("technical", "The setting is disabled by default and stored on this computer.", ["disabled", "default"], []),
    ("long", "Before we send anything, check the names, numbers, and meaning. Keep every recording safe. A failed optional feature must never interrupt capture. If a result is uncertain, show the original text and let me choose what to use.", ["never", "original", "uncertain"], []),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--voices", nargs="+", default=["Samantha", "Daniel"])
    parser.add_argument("--rates", nargs="+", type=int, default=[140, 180, 220])
    args = parser.parse_args()
    if platform.system() != "Darwin" or not shutil.which("say") or not shutil.which("afconvert"):
        parser.error("Run on macOS with the built-in say and afconvert commands.")
    if not args.voices or any(rate < 80 or rate > 300 for rate in args.rates):
        parser.error("Choose at least one installed voice and rates from 80 to 300.")
    output = args.output.resolve()
    if (output / "corpus.json").exists():
        parser.error("This corpus already exists. Use a new output directory.")
    audio = output / "audio"
    audio.mkdir(parents=True, exist_ok=True)
    cases, receipts = [], []
    for voice_index, voice in enumerate(args.voices):
        for rate in args.rates:
            for index, (category, text, required, forbidden) in enumerate(SENTENCES):
                case_id = f"synthetic-{category}-{index:02d}-v{voice_index}-r{rate}"
                aiff = audio / f"{case_id}.aiff"
                wav = audio / f"{case_id}.wav"
                subprocess.run(["say", "-v", voice, "-r", str(rate), "-o", str(aiff), text], check=True)
                subprocess.run(["afconvert", "-f", "WAVE", "-d", "LEI16@16000", "-c", "1", str(aiff), str(wav)], check=True)
                aiff.unlink()
                cases.append({"id": case_id, "audioPath": f"audio/{wav.name}", "contentType": "dictation", "locale": "en-US", "referenceText": text, "requiredTerms": required, "forbiddenTerms": forbidden})
                receipts.append({"id": case_id, "category": category, "voice": voice, "rate": rate, "sha256": hashlib.sha256(wav.read_bytes()).hexdigest()})
    (output / "corpus.json").write_text(json.dumps(cases, indent=2) + "\n")
    (output / "provenance.json").write_text(json.dumps({"synthetic": True, "generator": "macOS say and afconvert", "cases": receipts, "limitations": "Clean synthetic speech, not human voices, accents, ambient noise, microphone latency, or Wispr performance."}, indent=2) + "\n")
    print(json.dumps({"cases": len(cases), "synthetic": True, "corpus": str(output / "corpus.json")}))


if __name__ == "__main__":
    main()
