# HIPAA-compliant transcription: what the rule actually requires

Last reviewed: 2026-08-10

Every vendor in this category has a page telling you to look for encryption, SOC 2, and a signed BAA. That list is not wrong, but it skips the question underneath it: whether the vendor should be receiving your patients' audio at all.

Scope: this concerns HIPAA covered entities and their business associates. Not every clinician is a covered entity, since that status also depends on conducting covered electronic transactions. If HIPAA does not apply to you, state law, professional ethics rules, and your own contracts still do.

## Two things to get straight first

**There is no official HIPAA certification.** HHS says no standard requires a covered entity to certify its compliance, that it does not recognize private Security Rule certifications, and that OCR does not endorse or certify particular products. A private certification may still be meaningful evidence of diligence, and some represent substantial audits. What none carries is government recognition, and none prevents an OCR finding afterward. When a vendor leads with a HIPAA badge, ask what program issued it and what it examined.

**Compliance is a property of the arrangement, not the software.** No tool can be compliant on your behalf, because most obligations are about what your organization does: your risk analysis, who can access the files, how the endpoint is configured, whether staff are trained. The question is never "is this app HIPAA compliant." It is "who ends up holding this audio, under what contract, and have I analyzed the risk of the setup I actually have."

## The test the rule applies

Under **45 CFR 160.103**, a business associate is a person who,

> "On behalf of such covered entity… but other than in the capacity of a member of the workforce of such covered entity… creates, receives, maintains, or transmits protected health information for a function or activity regulated by this subchapter…"

Every element matters, and reducing this to "they received PHI" is the most common way to get it wrong. The party must be acting *on your behalf*, must be outside your workforce, and the activity must be one the rule regulates. Someone can receive PHI and still not be a business associate, most obviously another provider receiving it for treatment. A separate branch of the definition covers enumerated professional services, such as legal or accounting work, where providing the service involves disclosure of PHI.

Applied to this category the answer is usually straightforward: a transcription provider that processes identifiable patient recordings for you is ordinarily a business associate, and **45 CFR 164.502(e)** then requires a written contract with satisfactory assurances *before* you disclose the recording.

Notice what the definition does not turn on: encryption strength, data center location, or SOC 2. Those may inform your risk management and vendor diligence, but none determines business associate status, and two of them are not HIPAA requirements at all. The Security Rule requires reasonable and appropriate safeguards, with encryption treated as addressable rather than mandatory in every case.

## The exclusions, and the conduit idea

Paragraph (4) excludes four categories, each with conditions worth reading rather than paraphrasing loosely:

- A health care provider, with respect to disclosures *by a covered entity to that provider* concerning the treatment of the individual
- A plan sponsor, with respect to disclosures *by a group health plan, or by a health insurance issuer or HMO with respect to a group health plan*, to the plan sponsor, and only to the extent the requirements of § 164.504(f) apply and are met
- A government agency, with respect to determining eligibility for or enrollment in a government health plan that provides public benefits and is administered by another government agency, *or collecting PHI for such purposes*, to the extent authorized by law
- A covered entity participating in an organized health care arrangement that performs a function or activity described in paragraph (1)(i) for that arrangement, or provides a service described in paragraph (1)(ii) to or for it, by virtue of those activities or services

Software vendors are not on that list, and vendors sometimes present that as ominous. It is not, and the reason matters: **paragraph (4) is not the only route to not being a business associate**. A vendor that never satisfies the positive definition, because it never creates, receives, maintains, or transmits PHI on your behalf, needs no exception at all. It simply is not one.

The related "conduit" idea is narrower than its reputation. OCR treats it as covering transmission-only services, storage that is temporary and incidental to transmission, and access that is transient or infrequent and necessary to that transmission or required by law. A transcription service that stores audio or transcripts persistently is outside it, and OCR has been explicit that persistent storage defeats conduit status even where the provider holds no decryption key.

## The three architectures

**Human transcription service** — ordinarily a business associate; BAA required before PHI is disclosed. The agency and its transcriptionists receive and store your audio and the finished transcript. Where a use requires a certified transcript this is generally the route to one. Ask who subcontracts, where staff are located, and whether the agency signs BAAs with its own vendors.

**Cloud AI transcription** — ordinarily a business associate; BAA required before PHI is disclosed. The vendor receives your audio, processes it on its servers, and usually stores the transcript. Check whether your plan is one the vendor will actually sign a BAA for, since several offer that only on higher tiers; the plan decides whether the required contract is even available to you.

**On-device transcription** — no vendor in the path, in a genuinely local-only deployment; no BAA with the software vendor provided it never receives PHI. Whether this holds is a fact about your deployment, not a property of the software. Any cloud summarizer, synced folder, hosted backup, or vendor support access puts PHI back in someone else's hands and needs its own analysis. The tradeoff is that the endpoint becomes the security surface, with no vendor contract to fall back on.

A caution for the first two: a signed BAA is necessary when a vendor is a business associate, but not sufficient. The disclosure still has to be permissible, still has to satisfy minimum necessary where that applies, and your own risk analysis and safeguards remain yours. The contract allocates obligations; it does not discharge yours.

## What on-device does not do

Keeping processing local can remove one question, whether a software vendor receives PHI and therefore needs a BAA. It removes nothing else, and it creates work of its own.

- **You owe a fresh risk analysis.** The Security Rule requires an accurate and thorough assessment of risks to all ePHI you hold, and recording consultations creates a new corpus of audio and transcripts on an endpoint that may never have held a designated record set before.
- **Encryption is risk-based.** It is an addressable implementation specification, so the question is what your risk analysis concludes and what you document. In practice, full-disk encryption on a portable endpoint holding patient audio is very hard to reason your way out of.
- **A lost laptop triggers an assessment, not automatic notification.** Breach notification concerns unsecured PHI and requires the regulatory risk assessment; PHI encrypted to the specified standard can fall outside the requirement entirely.
- **The rest of the Security Rule still applies:** access control, audit controls, integrity, authentication, device and media controls, contingency planning and backup, secure disposal. Retention is separate: the six-year rule governs required HIPAA documentation, while medical record retention periods generally come from other federal or state law.
- **Privacy Rule duties follow the record.** If a transcript becomes part of a designated record set, patient access and amendment rights attach.
- **Recording consent is a separate question.** State wiretap and recording law and professional ethics rules apply regardless of where processing happens, and are not the same as a HIPAA authorization. HIPAA permits many treatment, payment, and operations uses without individual authorization; authorization is required where the Privacy Rule does not otherwise permit the use or disclosure.
- **You lose a party to hold accountable.** A business associate under contract carries obligations and liability. Your own endpoint carries none.

Local processing converts a vendor-disclosure problem into an endpoint problem. That can be a very good trade, particularly where the endpoint is already managed to the standard your other clinical systems require. It is a trade, not an exemption, and an unmanaged personal laptop is a materially worse place for this corpus than a managed workstation.

## Where Minutes fits, and where it does not

Minutes is built for the third architecture: records on your device, transcribes locally with whisper.cpp, diarizes with local models, writes markdown into a folder you control. In a local-only deployment we do not receive, maintain, or transmit your PHI, and supplying software to someone who uses it that way does not by itself create a business associate relationship.

That conclusion is conditional on your deployment, and it is worth being blunt about what breaks it. Configure a provider-backed summarizer, which is off by default, and transcript text goes to whichever model provider you chose, whose terms then govern. Connect an AI agent over MCP and ask it to read your meetings, and what it reads travels to that agent's provider as context. Sync your meetings folder to a hosted drive and that host is now storing PHI. Grant anyone remote access to the machine and the same applies. Each needs its own review and ordinarily a BAA with that party.

Where it is the wrong tool:

- You need a certified transcript for a filing or proceeding (requirements vary by jurisdiction and use; generally a human service)
- You want an ambient clinical scribe writing structured notes into your EHR, suggesting codes, or drafting to a SOAP template — Minutes is not a medical scribe and has no EHR integration
- You need centralized audit logs and administrative oversight across a practice; local files give ownership, not governance
- Your posture depends on having a business associate to hold accountable — sometimes that contract is precisely the point
- The endpoint is an unmanaged personal device, in which case you have moved the risk rather than reduced it

## Sources

- 45 CFR 160.103, business associate definition: https://www.ecfr.gov/current/title-45/subtitle-A/subchapter-C/part-160/subpart-A/section-160.103
- 45 CFR 164.502(e), business associate contracts: https://www.ecfr.gov/current/title-45/subtitle-A/subchapter-C/part-164/subpart-E/section-164.502
- HHS FAQ on certifying compliance: https://www.hhs.gov/hipaa/for-professionals/faq/2003/are-we-required-to-certify-our-organizations-compliance-with-the-standards/index.html
- HHS business associate guidance: https://www.hhs.gov/hipaa/for-professionals/privacy/guidance/business-associates/index.html
- OCR guidance on HIPAA and cloud computing: https://www.hhs.gov/hipaa/for-professionals/special-topics/health-information-technology/cloud-computing/index.html
- HHS FAQ on encryption in the Security Rule: https://www.hhs.gov/hipaa/for-professionals/faq/2001/is-the-use-of-encryption-mandatory-in-the-security-rule/index.html
- HHS risk analysis guidance: https://www.hhs.gov/hipaa/for-professionals/security/guidance/guidance-risk-analysis/index.html
- HHS breach notification guidance: https://www.hhs.gov/hipaa/for-professionals/breach-notification/guidance/index.html
- HHS FAQ on consent versus authorization: https://www.hhs.gov/hipaa/for-professionals/faq/264/what-is-the-difference-between-consent-and-authorization/index.html

Informational, not legal advice. HIPAA analysis is fact-specific and turns on your actual deployment; covered-entity status, permitted uses, and state recording law all vary. Your own counsel or compliance officer is the one who signs off.

## Related

- Which AI note takers can be used with PHI, vendor by vendor: https://useminutes.app/resources/hipaa-compliant-ai-note-taker
- What touches the network: https://useminutes.app/security
