# Minutes — Security & Privacy

Last reviewed: 2026-07-11

Cloud notetakers answer the security question with policies: encryption in transit, deletion windows, SOC 2 reports, BAAs. Minutes answers it with architecture. Your conversations are captured, transcribed, diarized, and stored on your own machine. "We delete your audio after processing" is a policy. "We never had your audio" is an architecture.

## The pipeline — every step on your device

1. **Capture** — mic (cpal) and system audio (native macOS capture in the desktop app, or a loopback device), recorded on your machine
2. **Transcribe** — whisper.cpp or parakeet.cpp, running on your CPU/GPU
3. **Diarize** — pyannote ONNX models, local; speaker labels never computed in a cloud
4. **Store** — markdown + YAML frontmatter on your own disk, 0600 owner-only permissions

## What that buys you

- **No audio upload, ever.** There is no code path that sends recordings to a server. The cloud client doesn't exist.
- **Files you own outright.** The durable record is plain markdown in ~/meetings, written with 0600 permissions. Grep it, back it up, delete it.
- **No account, no vendor database.** Nothing to sign up for; no server-side profile of your conversations to breach or subpoena.
- **Open source, MIT.** Every claim here is verifiable in the repository — readable Rust, not a trust-center PDF.

## What does touch the network

- Transcription and diarization models are downloaded once, at setup
- Updates, if you install them
- If you enable automated summarization (off by default), transcript text goes where you point it — a local model via Ollama, an agent CLI you've signed into (claude/codex/gemini — which round-trips through that provider's cloud), or a cloud API if you supply a key

What is never in that traffic: your audio and transcripts, unless you yourself configured a summarizer to receive them. Out of the box, Minutes needs no API key and sends conversation content nowhere. When Claude summarizes a meeting through MCP, it reads local files through tools you granted — visible in your agent's tool log, not a background sync — and what it reads travels to your agent's model provider as conversation context, like anything else you show your agent.

## For regulated work

- **Healthcare.** HIPAA's business-associate machinery exists because vendors receive PHI. On-device processing means no vendor receives anything — no business associate, no BAA to negotiate. Your own obligations (disk encryption, access control, recording consent) remain, as they do for your EHR workstation. See: https://useminutes.app/resources/is-otter-ai-hipaa-compliant
- **Legal.** A transcript that never leaves the attorney's machine involves no third-party disclosure to argue about. (Informational, not legal advice.)
- **EU / GDPR.** No processor, no DPA, no transfer — the data never leaves the controller's machine.

## Verify it yourself

Minutes is MIT-licensed and the pipeline is readable Rust: audio capture in `crates/core/src/capture.rs`, transcription in `crates/core/src/transcribe.rs`. Don't take a web page's word for an architecture claim — read the code, or have your security team do it.

- https://github.com/silverstein/minutes
