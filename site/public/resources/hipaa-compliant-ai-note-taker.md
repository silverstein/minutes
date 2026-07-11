# HIPAA-compliant AI note takers: the actual state of play

Last reviewed: 2026-07-11 · Not legal advice

Every vendor now says something about HIPAA; almost every summary flattens the conditions that make the claims true or false. The July 2026 state of play, each sourced to the vendor's own documentation.

## Two rules before the list

1. **Nothing is "HIPAA certified."** HHS certifies no product. "Compliant" = vendor self-attestation + a signed BAA.
2. **The plan tier is the whole ballgame.** Every cloud vendor gates its BAA to a top tier. Free/Pro usage with PHI is an impermissible disclosure regardless of security quality.

## Vendor by vendor

| Vendor | Verdict | Condition | Architecture |
|---|---|---|---|
| Otter.ai | Conditional | Enterprise + signed BAA (since July 2025) | Cloud — Otter's servers |
| Fireflies.ai | Conditional | Enterprise ($39/u/mo annual) + Private Storage + BAA, all three at once | Cloud — US AWS/GCP; OpenAI + ASR vendors under zero-retention BAAs |
| Fathom | Conditional | Published blanket BAA; pricing lists HIPAA BAA under Enterprise — plan gating ambiguous, confirm in writing | Cloud — US-only, indefinite default retention; Anthropic/OpenAI/Google |
| Krisp | Conditional | BAA available per its security page (which references a legacy "Business tier"; pricing lists BAA under Enterprise) | Hybrid — on-device noise cancellation, cloud notes (Azure), Krisp Cloud storage once notes are enabled |
| Granola | **No** | None — "cannot sign BAAs," per Granola's own docs | Cloud — Deepgram/AssemblyAI, OpenAI/Anthropic, US AWS |
| Minutes (ours) | **No BAA needed** | Open source, free — vendor never receives the data | On-device — capture to storage on your machine |

Full analyses: /resources/is-otter-ai-hipaa-compliant · /resources/is-fireflies-ai-hipaa-compliant · /resources/is-granola-hipaa-compliant · /compare/fathom-vs-minutes · /compare/krisp-vs-minutes

## How to read this as a buyer

For every cloud vendor, HIPAA is a pricing tier. BAAs carry real legal exposure and vendors charge for it — but it means compliance and procurement are the same question, and downgrades, lapsed contracts, or a clinician's personal account silently break compliance. If you go cloud, put the BAA condition in your renewal checklist.

The alternative is changing the architecture instead of buying the contract: on-device processing means the vendor never receives PHI — no business associate, nothing to lapse. That's Minutes: open source, free, capture-to-storage local. Trade-offs are real (no hosted team features, macOS-first) and the remaining duties are yours (encrypted disk, access control, consent). But the vendor-risk column goes to zero on every "plan."

## Sources

- https://help.otter.ai/hc/en-us/articles/33975072019991-HIPAA-Otter-ai
- https://guide.fireflies.ai/articles/3704059205-set-up-hipaa-compliance-for-your-workspace
- https://www.fathom.ai/baa · https://help.fathom.video/en/articles/5291265
- https://krisp.ai/security-for-ai-meeting-assistant/
- https://docs.granola.ai/help-center/consent-security-privacy/is-granola-hipaa-compliant
- https://www.hhs.gov/hipaa/for-professionals/covered-entities/sample-business-associate-agreement-provisions/index.html
- https://useminutes.app/security
