# Recording consent and ephemeral audio

Minutes provides a disclosure aid for people who record their own meetings. It
does not decide whether recording is permitted, contact participants, or post a
notice into Zoom, Meet, or another service.

## Before recording

For meeting capture, `[consent].mode` controls the start-time behavior:

- `off` does not show the disclosure flow.
- `remind` shows the configured `disclosure_script` without blocking recording.
- `require` asks for an acknowledgement in an interactive terminal or the
  existing desktop consent sheet. Non-interactive recording remains unblocked
  and is marked `unattested`.

The desktop call banner appears immediately when a call is detected. It can
show the script and provides **Copy** and **Announced** controls without opening
a new modal. In a terminal, `minutes record` prints the script and copies it
when `pbcopy`, `xclip`, `xsel`, or `clip.exe` is available. Use `--announced`
when the script was already read aloud. Minutes records the time of that human
attestation; it never claims that showing or copying text means participants
consented.

Set `[consent].jurisdiction` to an ISO-3166-2-style code such as `US-CA` for an
informational reminder drawn from Minutes' small built-in table. In `remind`
mode, a matching row explains that the jurisdiction is commonly treated as
all-party consent and names the basis the user may wish to record. It never
changes the selected basis or blocks recording. Run `minutes consent show` to
see the effective mode, jurisdiction row, recommended basis, and primary-source
link.

Minutes shows this as a reminder. It is not legal advice, and laws change; confirm the rules that apply to you.

## During recording

Capture stays on the device. Minutes does not send a join-time announcement or
recording command to a meeting provider. The ephemeral-audio option never
deletes audio while recording or while transcription still needs it.

For one meeting, use:

```bash
minutes record --no-keep-audio
```

To make this the default for new meetings, configure:

```toml
[retention]
default_audio_retention = "none"
```

The desktop consent sheet offers the same choice. An explicit
`audio_retention: pinned` on an existing artifact wins over `none`.

## After recording

With `audio_retention: none`, Minutes first finishes the transcript, durably
writes it, re-reads and parses the finished Markdown, and moves any job-owned
audio stems to their final paths. Only then does it delete the exact audio files
owned by that job. It updates the Markdown with `audio_deleted_at` and durably
re-reads that stamp. Voice learning is disabled for ephemeral meetings.

If transcription fails or the job needs review, Minutes keeps the audio so the
meeting can be recovered. The artifact is marked:

```yaml
audio_retention: "none (held: transcription failed)"
```

The existing failed-audio retention window then applies. No background deleter
is added, and the `auto_cleanup` default remains `false`.

## Proving what happened

The meeting's frontmatter is the durable record. A successfully completed
ephemeral meeting includes fields like:

```yaml
consent: verbal_all_parties
consent_announced_at: 2026-09-01T10:00:00-07:00
audio_retention: none
audio_deleted_at: 2026-09-01T10:42:15-07:00
```

`consent_announced_at` means the recorder confirmed that they announced the
script. `audio_deleted_at` is written only after the job-owned audio was
deleted. `minutes health` and job status report `audio: not kept (by request)`
for that completed state.

## For teams and families

The simplest shared practice is explicit: one person operates Minutes and tells
everyone else before recording. Teams can put the same disclosure in recurring
calendar invites and record `notice_in_invite`; families can use the short
spoken script and confirm **Announced**. Keep the human conversation primary:
the recorded basis is an audit note, not an automated consent decision.
