# Is Granola HIPAA compliant?

Last reviewed: 2026-07-11

No — and unusually for this category, there's no ambiguity: Granola says so itself.

## Quick answer

- **No, on every plan.** From Granola's own help center: "Granola is not currently HIPAA compliant and should not be used to store or process Protected Health Information (PHI) at this time." Its docs add that Granola "cannot sign Business Associate Agreements (BAAs)" and give no timeline. The $35/user Enterprise tier does not change this.
- **Ignore third-party posts claiming otherwise.** A few SEO articles imply Granola suits medical transcription; they contradict Granola's own documentation.

## What Granola does offer

For non-PHI work, a genuinely decent cloud posture: SOC 2 Type 2 (July 2025), GDPR with a standard DPA, botless capture, audio deleted after transcription, contractual bans on OpenAI/Anthropic training on customer data. The structural facts: cloud transcription (Deepgram/AssemblyAI), note enhancement via OpenAI/Anthropic, transcripts on US AWS servers. That architecture is why the HIPAA answer is what it is.

## If you're a clinician who likes Granola's design

The part you like — device-side capture, no bot — is exactly the part that can be taken all the way. Minutes keeps capture on your device like Granola, then keeps transcription and storage there too: whisper.cpp locally, markdown on your own disk, owner-only permissions, open source. No vendor receives the conversation, so there is no business associate and no BAA question at all. (Your own duties — device encryption, access control, patient consent — remain.)

If you need a cloud product with team features and a BAA, the documented options are Otter (Enterprise + BAA), Fireflies (Enterprise + Private Storage + BAA), and Fathom (published blanket BAA).

## Sources

- https://docs.granola.ai/help-center/consent-security-privacy/is-granola-hipaa-compliant
- https://docs.granola.ai/help-center/consent-security-privacy/security-privacy-data-faqs
- https://www.granola.ai/security · https://www.granola.ai/pricing
- https://useminutes.app/compare/granola-vs-minutes · https://useminutes.app/security
- https://useminutes.app/resources/is-otter-ai-hipaa-compliant · https://useminutes.app/resources/is-fireflies-ai-hipaa-compliant

Informational, not legal advice.
