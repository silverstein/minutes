# Transcript checkpoints

A host that transcribes a meeting while it records can hand that work to the
queued job, so final processing transcribes only what is left. It is off unless
a host supplies a checkpoint. Without one, processing is unchanged.

## API

- `transcript_checkpoint::transcribe_span(samples, start_sample, source, config, hints)`
  transcribes one span of a source: the microphone stem, the system stem, or a
  single-source meeting recording. `samples` are the source's canonical 16 kHz
  mono samples from `start_sample`. It runs the same decode plan (meeting VAD
  chunks packed into decode groups), decoder settings and per-source options as
  the final pass (independent stems keep the shared clock and have no per-source
  word minimum). It returns the span's `[m:ss]` lines on the source clock, before
  transcript cleanup.
- `transcript_checkpoint::decode_hints_for_queued_meeting(title, pre_context, config)`
  returns the decode hints a queued meeting job builds when no calendar event
  matches it.
- `jobs::queue_live_capture_with_transcript_checkpoint(.., checkpoint)` queues the
  capture with the spans. They are written beside the job (`<job>.checkpoint.json`,
  owner-only on Unix), removed when the job finishes, and kept after a failure so
  a retry can use them. A checkpoint that cannot be written is dropped with a
  warning; the job still queues.

Start and end a span only at a cut of the source's decode plan:
`decode_groups::DecodePlan::for_growing_source(samples, target).cuts()`, where
`samples` start at the source start or at an earlier cut. See
[Decode groups](#decode-groups).

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
- its SHA-256 over the canonical samples matches the same range of the final file;
- its decode groups are the final pass's groups for that range: no group crosses
  either edge of the span.

The settings fingerprint also names the decode strategy: one call per chunk, or
groups with their version, target, kept pause, loop thresholds and retry policy. A span made with
another strategy is rejected even when the core version, settings and audio match.

The final pass plans the decode groups of the whole source once. Groups that no
accepted span covers are transcribed as usual.
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

## Decode groups

Whisper encodes a 30 s window for every call, so one call per short VAD chunk
spends most of its work on padding. The Whisper meeting path packs adjacent VAD
chunks into decode groups (`decode_groups`), and decodes each group in one call.
The same plan is used by the whole-file pass, checkpoint spans and the gaps
between accepted spans.

- Groups are packed greedily from the start of each source. A chunk joins the
  open group while the audio that Whisper gets for the group stays at most 30 s
  (`WHISPER_GROUP_TARGET_SAMPLES`). That audio is the chunks and the kept pauses
  between them.
- 30 s is a packing target, not a maximum. The VAD chunker splits speech without
  a pause at 45 s, and such a chunk is a group of its own.
- Whisper gets the chunks of a group with the pauses between them. A pause of
  up to 1 s (`GROUP_PAUSE_SAMPLES`) is kept whole. A longer pause is cut to 1 s:
  0.5 s after the chunk before and 0.5 s before the chunk after. Without pauses,
  Whisper `medium` dropped a one-word answer and timed short answers early in a
  test with 60 s and 45 s pauses. A stem of a call is quiet while the other side
  speaks, so whole pauses would use up the 30 s window.
- Each line is mapped back to the source clock, sample for sample. Whisper
  starts a segment where the segment before it ended, which is often in a pause
  or in the quiet end of the speech before. A VAD chunk is not one stretch of
  speech: it ends with 0.5 s of quiet, and a one-word answer shorter than 1 s
  stays in one chunk with the sentence after a long silence. So the group
  decoder finds the stretches of speech inside each chunk. A line starts in the
  first stretch that its segment holds at least 0.3 s of. A line that starts
  before that stretch gets the start of the stretch. A line that starts inside
  it keeps its exact time.
- The clock of the call does not change when Silero is not installed and energy
  stripping removes silence before the decode. The stripped times are mapped
  back (`whisper_guard::strip_silence_with_map`). A cleanup annotation in the
  output of a call does not change the exact times of the other lines.
- A cut is the point after a group where the VAD chunker resets: 800 ms after the
  group's last chunk, or at its end for a 45 s split. A span from one cut to
  another gets the same chunks and groups as the whole source. During a
  recording, a group whose end is not known yet, because the open chunk after it
  could still join it, has no cut. Stop leaves it to the final pass.
- Loop guard: when a line of a group with more than one chunk is a repetition
  loop inside one segment (`whisper_guard::find_repetition_loop`: a phrase 4+
  times back to back, 16+ words), the group is decoded again once, one call per
  chunk. That output is kept, even if it loops again, because it is what one
  call per chunk produced before groups. The retry happens in the same way in
  all three paths.
- Other engines keep one call per chunk: their plan has one chunk per group.

## Equivalence

Groups are decoded independently, so a span's lines are what the final pass would
produce for the same groups. Before decode groups, VAD chunk boundaries next to a
span cut could differ from a whole-file pass. On synthetic two-stem calls
(Whisper `small`, 120 s spans cut at the quietest 100 ms of their last 15 s), the
word error rate against the reference was 8.4% against 8.3% (20 minutes) and 8.3%
against 8.3% (60 minutes), inside the run-to-run variation of Whisper on Metal.
Spans now end only at cuts, and the final pass rejects a span whose groups differ.
