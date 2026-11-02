# Minutes security and privacy

Last reviewed: 2026-09-05

Capture, transcription, diarization, and storage run locally. If you choose a cloud AI assistant or summarizer, the meeting context you authorize is sent to that provider. Use a local model or skip summarization for an on-device workflow.

## What stays local

The recording and transcription pipeline processes audio on your device. Minutes writes Markdown and YAML records to your own disk. You can inspect the source, choose your storage folder, and read the files without Minutes.

## What can use the network

Setup and updates can download models and software. Connected cloud assistants and summarizers receive authorized meeting context. Files copied into synced folders follow the sync provider's settings. This website counts visits with Cloudflare Web Analytics, which sets no cookies and does not identify you. It is separate from the desktop app, which sends nothing to us.

## Your responsibilities

Local processing is not a compliance certification. Review device access, encryption, consent, retention, backups, sync, and AI providers against your organization's requirements. Local files are not immune from disclosure obligations.

## Verify the data path

- [Source code](https://github.com/silverstein/minutes)
- [Agent integration guide](https://useminutes.app/docs/agent-integrations)
- [Security page](https://useminutes.app/security)
