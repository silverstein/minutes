# Transcript checkpoints

A host that transcribes a meeting while it records can hand that work to the
queued job, so final processing transcribes only what is left. It is off unless
a host supplies a checkpoint. Without one, processing is unchanged.

## API

- `transcript_checkpoint::transcribe_span(samples, start_sample, source, config, hints)`
  transcribes one span of a source: the microphone stem, the system stem, or a
  single-source meeting recording. `samples` are the source's canonical 16 kHz
  mono samples from `start_sample`. It runs the same meeting VAD chunking, decoder
  settings and per-source options as the final pass (independent stems keep the
  shared clock and have no per-source word minimum). It returns the span's
  `[m:ss]` lines on the source clock, before transcript cleanup.
- `transcript_checkpoint::decode_hints_for_queued_meeting(title, pre_context, config)`
  returns the decode hints a queued meeting job builds when no calendar event
  matches it.
- `jobs::queue_live_capture_with_transcript_checkpoint(.., checkpoint)` queues the
  capture with the spans. They are written beside the job (`<job>.checkpoint.json`,
  owner-only on Unix), removed when the job finishes, and kept after a failure so
  a retry can use them. A checkpoint that cannot be written is dropped with a
  warning; the job still queues.

Cut spans in a pause, so no word is split between two spans.

## Reusing a decoder during capture

A host can keep a `SpanTranscriber::new(config, hints)` on its optional worker
and call its `transcribe_span(samples, start_sample, source)` method for each
completed range. It freezes config and hints for that meeting and lazily loads
one Whisper model context shared by both source stems. The standalone
`transcribe_span` function retains its existing signature and behavior.

`decode_hints_for_queued_meeting_at(title, pre_context, recorded_at, config)`
uses the final pipeline's calendar selection and hint builder for the recording
start time. Call it on the worker because calendar access can wait. A changed
title, context, calendar match, or vocabulary can still invalidate spans at
final processing. The final pass logs that mismatch and decodes again. Never
re-key the old span text to bypass that validation.

## What the final pass accepts

For each ambient meeting source, the final pass loads the final file and accepts
a span only when all of these hold:

- it lies inside the audio and does not overlap an earlier accepted span;
- its settings fingerprint matches the final pass for that source: core version,
  effective engine, the transcription config, and the decode hints (a different
  title, vocabulary or calendar match changes the hints);
- its SHA-256 over the canonical samples matches the same range of the final file.

Ranges that no accepted span covers are VAD-chunked and transcribed as usual.
Span lines and new lines are joined in source order, and the transcript cleanup
pipeline (dedup, noise trimming) then runs once over the whole source, as it does
without checkpoints. Rejected spans are logged with the reason.

Checkpoints are not used for authorized private-audio inputs, for a single
surviving stem, for non-meeting content, or where meeting chunk rotation is
bypassed. Those inputs are transcribed in full.

## Trust

The SHA-256 covers the audio, not the span text. A matching hash proves that a
span was cut from the same samples. It does not prove that its lines are what
Whisper produced for them. The checkpoint file is therefore trusted the way the
job folder is: anyone who can write `<job>.checkpoint.json` can supply any text
for audio that hashes correctly. This stays inside the existing same-user model,
because the file is owner-only and sits beside the job's audio and JSON, and
whoever can write those can already change the meeting. Do not treat the hash as
an integrity check on the text, and do not accept checkpoints from a less
trusted writer than the job itself.

## Equivalence

Chunks are decoded independently, so a span's lines are what the final pass would
produce for the same chunks. VAD chunk boundaries next to a span cut can differ
from a whole-file pass. On synthetic two-stem calls (Whisper `small`, 120 s spans
cut at the quietest 100 ms of their last 15 s), the word error rate against the
reference was 8.4% against 8.3% (20 minutes) and 8.3% against 8.3% (60 minutes),
inside the run-to-run variation of Whisper on Metal.
