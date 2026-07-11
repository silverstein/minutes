# AI notetakers and attorney–client privilege

Last reviewed: 2026-07-11 · Not legal advice

Privilege has one load-bearing wall: confidentiality. An AI notetaker that streams client conversations to a vendor's cloud puts a third party inside that wall — and no court has given lawyers a clean answer on what that does to privilege.

## Quick answer

- **Waiver is fact-specific and unsettled — but the risk vector is not.** Privilege requires confidentiality; a cloud notetaker is a disclosure to a third party under that vendor's terms. Whether it defeats privilege depends on the vendor's data handling, your diligence, and a judge — three things you don't fully control.
- **On-device transcription removes the third party entirely.** A transcript generated and stored on the attorney's own machine involves no outside disclosure to argue about. The confidentiality question collapses to the one you already answer daily: is your laptop secure?

## What the bar has actually said

ABA Formal Opinion 512 (July 2024) on generative AI: before client information goes into an AI tool, the lawyer must understand the tool's data handling — who can access inputs, whether they train models, retention — and in some cases obtain informed client consent. Competence about the tool is part of the duty of confidentiality.

Applied to notetakers, read the vendor's terms the way opposing counsel would: staff access for "service improvement"? Model training by them or subprocessors? What happens under a subpoena served on the vendor?

Separately, recording-consent law applies (all-party vs one-party states — see the RCFP state-by-state guide). For client meetings, explicit consent is the professional norm regardless of state minimums.

## The vendor questions that matter

- Where is audio transcribed, and by which subprocessors?
- Is any human review of transcripts possible, ever?
- Are recordings/transcripts used to train models?
- What is the retention period; is deletion verifiable?
- What is their process on receiving legal process for your data?
- Will they sign confidentiality terms that survive their standard ToS?

That list is a due-diligence file you must build for a vendor whose entire involvement is optional.

## The architectural answer

Every question above exists because the conversation leaves your machine. Run transcription on-device and the third party disappears: no vendor receives the communication, no ToS governs it, no vendor subpoena can reach it. That's the design of Minutes — open source, on-device transcription and diarization, transcripts as markdown on your own disk with owner-only permissions.

What on-device does NOT do: satisfy consent law for you, secure an unencrypted laptop, or make a transcript less discoverable — your own files are always discoverable. It removes the third-party disclosure question, which is the one you can't fix after the fact.

## Sources

- ABA Formal Opinion 512 (July 2024): https://www.americanbar.org/content/dam/aba/administrative/professional_responsibility/ethics-opinions/aba-formal-opinion-512.pdf
- RCFP Reporter's Recording Guide: https://www.rcfp.org/reporters-recording-guide/
- Minutes security architecture: https://useminutes.app/security
- Parallel analysis for healthcare: https://useminutes.app/resources/is-otter-ai-hipaa-compliant

Informational only — consult your ethics counsel before adopting any recording tool for client work.
