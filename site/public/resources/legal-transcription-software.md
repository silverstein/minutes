# Legal transcription software: what confidentiality actually requires

Last reviewed: 2026-07-11 · Not legal advice

Most "legal transcription software" roundups compare turnaround times and per-minute prices, and skip the only question that can end a career: where does the privileged audio go?

## Quick answer

- **Certified transcripts are a human service, not a software feature.** When the transcript must be evidence — proceedings, certified depositions — hire a certified transcriptionist or court reporter.
- **Everything else is a software job, and architecture is the buying criterion.** Client meetings, dictated memos, internal case discussions: cloud transcription tools put a third party inside privileged conversations; on-device tools involve no outside disclosure at all.

## The three jobs firms actually have

1. **Court-grade transcripts** — human, certified, jurisdiction-formatted. Budget for a service.
2. **Working transcripts of privileged material** — client calls, strategy discussions, dictated notes. Speed matters, confidentiality governs. ABA Formal Opinion 512 requires understanding a tool's data handling before client information goes in — an analysis that gets one sentence long when the tool never transmits anything.
3. **Non-privileged volume** — public hearings, CLEs, marketing content. Any decent tool; pick on price.

## Where Minutes fits — and doesn't

Minutes is built for job #2: records and transcribes on your own machine (whisper.cpp — the audio has no network path), labels speakers, extracts action items, stores markdown on your disk with owner-only permissions — greppable and queryable by your AI assistant without leaving your control. Open source (MIT): your security review can read the code.

Not the tool for: certified transcripts (hire a human), jurisdiction-formatted verbatim output, medical-legal templating. Your own obligations — consent, device encryption, matter-based access — stay with you.

## Sources

- Privilege analysis: https://useminutes.app/resources/ai-notetakers-attorney-client-privilege
- ABA Formal Opinion 512: https://www.americanbar.org/content/dam/aba/administrative/professional_responsibility/ethics-opinions/aba-formal-opinion-512.pdf
- Architecture: https://useminutes.app/security
- https://github.com/silverstein/minutes
