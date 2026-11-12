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

Remote transcription reuses the active-corpus policy reader and exact-byte
reauthorization used at existing egress boundaries. Before processing, the
recording must have a matching Markdown policy record: for `recording.wav`
or `recording.mov`, this is `recording.md` beside the audio, inside the
configured output directory. That record must pass the existing normal-only
policy gate. `sensitivity: restricted`, malformed or unknown policy,
`capture: none`, unreadable records, and files outside the active corpus are
refused. Missing sensitivity on an otherwise valid record retains the
existing legacy-normal meaning.

**New recordings without a policy record are refused before upload.** Minutes
does not infer permission from audio, calendar titles, a future transcript,
or the engine setting. Use local transcription until policy is established;
this engine does not add a new classification or consent workflow. All stems
inherit the original recording's policy, which is revalidated immediately
before every chunk upload. Changing the policy between chunks stops the job.

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
