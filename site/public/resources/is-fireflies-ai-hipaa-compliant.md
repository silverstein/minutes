# Is Fireflies.ai HIPAA compliant?

Last reviewed: 2026-07-11

Yes — with three conditions that must all hold at once, and a price tier attached.

## Quick answer

- **Fireflies can be HIPAA-compliant, but only in one configuration:** an active Enterprise plan ($39/user/mo, billed annually) + Private Storage enabled + a signed BAA — all three simultaneously. Fireflies' own setup guide states compliance is disabled if any single requirement is removed, downgraded, or expires.
- **On Free, Pro, and Business plans, the answer is no.** Recording PHI on those tiers is a disclosure to a vendor with no BAA in effect — regardless of Fireflies' generally strong security posture (SOC 2 Type II, zero-retention agreements with AI vendors).

## What the compliant configuration involves

Credit where due: Fireflies publishes a self-serve BAA page (fireflies.ai/baa) and documents the requirements plainly. The compliant setup: Enterprise pricing, Private Storage (dedicated or bring-your-own AWS S3/GCS, EU region options), signed BAA, all kept alive. Fireflies states it has downstream BAAs with OpenAI and its ASR vendors with zero-retention terms.

What it doesn't change: patient conversations are still processed in Fireflies' US cloud (AWS/GCP default) and still pass through third-party AI vendors. The BAA chain makes disclosures lawful and governed — not not-disclosures.

Nuance: Fireflies' healthcare marketing describes features "available to all customers," while the compliance docs gate HIPAA to Enterprise. The features are broad; the compliance is not. Read the setup guide, not the press release.

## The question under the question

A three-condition checklist exists because the audio leaves your machine. On-device transcription never gives any vendor the conversation — no business associate, no BAA, no storage tier, no configuration to keep alive. That's Minutes: open source, on-device transcription and diarization, markdown on your own disk. (No tool of any kind is "HIPAA certified" — HHS certifies nobody.) Device encryption, access control, and consent remain your responsibilities.

## Sources

- https://fireflies.ai/security · https://fireflies.ai/hipaa · https://fireflies.ai/baa · https://fireflies.ai/pricing
- https://guide.fireflies.ai/articles/3704059205-set-up-hipaa-compliance-for-your-workspace
- https://guide.fireflies.ai/articles/9596505232-learn-about-data-storage-and-transfer
- https://useminutes.app/security · https://useminutes.app/resources/is-otter-ai-hipaa-compliant

Informational, not legal advice.
