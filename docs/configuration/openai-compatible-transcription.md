# OpenAI-compatible batch transcription

Local transcription remains the default. To explicitly opt in to uploading
batch audio to an OpenAI-compatible server, configure:

```toml
[transcription]
engine = "openai-compatible"
openai_compatible_base_url = "http://localhost:8000/v1"
openai_compatible_model = "whisper-1"
openai_compatible_api_key_env = "TRANSCRIPTION_API_KEY"
openai_compatible_max_upload_bytes = 25000000
```

Set the named environment variable in the process that runs Minutes. Store
only its **name** in configuration; never paste a key into configuration or
Settings. An empty variable name permits unauthenticated loopback servers.
Non-loopback endpoints require HTTPS and a key from the named variable.
There is no desktop keychain or summarization-key fallback for transcription.
The base URL may also end in `/audio/transcriptions`.

Each request is `POST {base}/audio/transcriptions` with multipart WAV audio,
`model`, and `response_format=verbose_json`. A configured transcription
language is included. The limit bounds the entire multipart body, including
the WAV header and form overhead; the default is 25,000,000 bytes. Lower it
for a provider with a smaller limit (maximum configurable limit: 100 MB).
Audio is converted locally to 16 kHz mono PCM16 and split on sample boundaries.
Silence is retained, and returned segment start/end times include each chunk's
source offset. The provider must return timestamped `segments`.

## Conversation policy before upload

New desktop and CLI recordings carry an explicit capture-time policy from
`recording-meta.json` into their processing job. The queue checks that the
metadata matches the capture mode, context session, and recording interval,
so stale metadata cannot authorize a different recording. The policy uses the
same `sensitive-session.json` state that prevents recording during a no-capture
sensitive meeting: no session means normal sensitivity with capture enabled;
a valid sensitive session means restricted sensitivity with capture disabled.
Unreadable or malformed state means unknown. Missing policy on a legacy or
imported job also means unknown, never permission to upload. Calendar events
currently carry no restriction flags and do not grant upload permission.

Before transcription, the engine requires the exact job's saved policy to
say normal sensitivity and capture enabled. Restricted, capture-disabled,
unknown, malformed, or missing job policy is refused before connecting. The
original job policy is carried through prepared audio and both named stems,
and its exact bytes are revalidated immediately before every chunk upload.
A changed or removed job record stops further uploads and preserves audio.
If the job already has a saved meeting artifact, its live normal-only corpus
policy must also authorize every upload; a later restriction takes precedence.

For reprocessing existing recordings outside the job directory, the existing
normal-only active-corpus Markdown policy reader and exact-byte
reauthorization still apply. A matching active record beside the original
audio is required; restricted records, `capture: none`, unreadable or malformed
policy, and files outside the active corpus are refused. Missing sensitivity
on an otherwise valid legacy Markdown record retains its existing normal
meaning. No policy is inferred from audio, calendar titles, a future
transcript, or selecting the remote engine.

## Failure and disclosure

`auto` never chooses this engine. Live transcript and dictation stay local;
recording checkpoints do not upload audio. The engine has no retries,
redirects, or fallback. Any request, response, or policy error fails the job
and leaves the recording and stems on disk. A dead remote worker also requires
manual recovery rather than an automatic repeat upload. Provider error bodies
and transport diagnostics are not copied into errors or job files.

`minutes health`, desktop Settings → Transcription, and recording disclosure
text identify the remote host. The option is absent from onboarding. Health
checks configuration without contacting the provider. Desktop configuration
uses the same fields and accepts an environment variable name, never a key.
