# Is Otter.ai HIPAA compliant?

Last reviewed: 2026-07-11

Short answer: yes, since July 2025 — but only on the Enterprise plan, and only with a signed Business Associate Agreement (BAA). Most answers online are out of date in one direction or the other.

## Quick answer

- **Otter.ai is HIPAA compliant only for Enterprise customers with a signed BAA.** Otter announced HIPAA compliance in July 2025 following an independent assessment. Per its help center, the BAA is available exclusively on the Enterprise plan, arranged through sales.
- **On Basic, Pro, and Business plans, the answer is no.** No BAA is available on those tiers, so recording conversations containing protected health information (PHI) on them is an impermissible disclosure to a vendor — regardless of how good Otter's general security is.

## What a BAA does — and what it doesn't

A BAA makes it lawful for a vendor to receive and process PHI on your behalf, and binds the vendor to HIPAA's safeguards. It is a legal instrument, not an architectural one. With a signed BAA, patient conversations still travel to Otter's cloud, are still transcribed on Otter's servers, and still live in Otter's storage — the disclosure is permitted and governed, not eliminated. A BAA obligates the vendor to report breaches; it does not make the vendor unbreachable.

## The checklist if you stay with Otter

Using Otter with PHI compliantly requires all of:

- An Enterprise plan — not Basic, Pro, or Business
- A BAA actually signed before any PHI is recorded
- Workspace policies keeping PHI recordings inside the covered workspace (a clinician's personal Pro account doesn't inherit the organization's BAA)
- Your own HIPAA obligations: recording consent, access controls, minimum-necessary practices

## The question under the question

The reason a BAA is needed at all is that the audio leaves your machine. The second way to resolve the HIPAA question: transcription that runs entirely on your own device, so no vendor ever receives the conversation. No third party, no business associate, no BAA — the legal question dissolves because the disclosure never happens.

That's the architecture Minutes is built on: audio is captured and transcribed on-device with whisper.cpp, and the record is markdown on your own disk with owner-only file permissions. It's open source (MIT), so the claim is verifiable in code. To be precise: no local tool is "HIPAA certified" — HIPAA governs covered entities and their vendors. On-device processing removes the vendor from the equation; securing the device (disk encryption, access control) remains your responsibility, as it already is for your EHR workstation.

## Which fits your situation

- Large org already on Otter Enterprise wanting team features → sign the BAA, lock recording to the covered workspace, audit regularly
- Solo practitioner or small practice on a consumer/Pro plan → stop recording PHI with it today; no BAA is available at your tier. Consider on-device transcription
- Any practice whose bar is "patient audio never touches a third party" → on-device tools are the only architecture that meets it

## Sources

- https://help.otter.ai/hc/en-us/articles/33975072019991-HIPAA-Otter-ai
- https://otter.ai/blog/otter-ai-achieves-hipaa-compliance
- https://www.hhs.gov/hipaa/for-professionals/covered-entities/sample-business-associate-agreement-provisions/index.html
- https://useminutes.app/security
- https://github.com/silverstein/minutes

This page is informational, not legal advice. Verify plan details and BAA terms with Otter and your compliance counsel before recording PHI with any tool.
