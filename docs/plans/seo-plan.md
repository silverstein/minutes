# SEO Plan — useminutes.app

*Researched 2026-07-11 with Ahrefs (US, subdomains mode). Volumes are monthly US searches; difficulty (KD) is Ahrefs 0–100; CPC in USD.*

## TL;DR

useminutes.app has **370 live referring domains but ranks for 1 keyword (~19 visits/mo)** — link authority with no content to spend it on. Competitors split into two groups: the funded cloud players (Otter, Fireflies) win non-branded search with utility pages and templates, while the close-in competitors (Granola, superwhisper, Fathom) do **essentially zero non-branded SEO** — Granola's entire footprint is its own brand name. The privacy/compliance and local/on-device clusters are nearly uncontested (KD 0–13) and are queries cloud competitors structurally cannot win. Plan: ~15 pages across 5 clusters, comparison and compliance first.

## Baseline (2026-07-11)

| Metric | useminutes.app |
|---|---|
| Live referring domains | 370 (991 backlinks) |
| Organic keywords | 1 |
| Organic traffic | ~19/mo |
| Existing SEO-relevant routes | `/compare`, `/writing`, `/docs`, `/proof`, `/for-agents` |

The refdomain-to-keyword ratio is the headline: most young sites have the opposite problem. New pages here should rank unusually fast for their difficulty tier.

## Competitive findings

### Granola (granola.ai)
~100% branded traffic ("granola ai" 15k/mo etc.). Non-branded attempts are weak: a follow-up-email-template post and accidental rankings for "claude dangerously skip permissions". **No moat in non-branded search.** They rank #14+ for "ai note taking" (KD 83) and gain nothing from it.

### superwhisper (superwhisper.com)
Branded ("superwhisper" 1.9k/mo) plus scattered low positions on whisper-adjacent terms. Does not own "whisper dictation app", "local speech to text", or even "mac whisper" (MacWhisper outranks them). The dictation long tail is open.

### Otter (otter.ai) — the playbook to learn from
Non-branded traffic comes from three page types:
1. **Utility/tool pages**: `/free-ai-transcription` (298 kw), `/mp3-to-text` (178 kw), `/audio-to-text`, `/video-to-text`, `/podcast-transcription`. Format-intent pages that convert.
2. **Legality content**: one post ("is it illegal to record someone without permission") ranks for 132 keywords. Recording-consent law is a durable, linkable topic.
3. **Anti-bot pain, hosted on their own help center**: "remove otter ai from teams", "how to disable otter ai", "stop otter notetaker from joining meetings" — hundreds of monthly searches from people trying to *get rid of* meeting bots. Otter is forced to rank for its own churn queries. **Minutes' botless capture is the product-answer to this search demand.**

### Fireflies (fireflies.ai)
Templates work: "meeting minutes templates" post ranks #2 for "meeting minutes" (13k/mo, 142 keywords). The rest is thin listicle spam (memes, team names) — volume without intent; do not copy.

### Fathom (fathom.video)
Nearly all branded + help-center. No content program to speak of.

**Net read:** only the cloud incumbents do real SEO, and they cannot follow Minutes into the privacy/local clusters without arguing against their own architecture.

## Keyword clusters (priority order)

### Cluster 1 — Alternative/comparison (bottom-funnel, effectively free)

| Keyword | Vol/mo | KD | Notes |
|---|---|---|---|
| granola alternative | 210 | 0 | `/compare/granola-vs-minutes` exists (#431) |
| otter alternative | 90 | 2 | Cluster TP ~900 |
| superwhisper alternative | 53 | ~0 | Dictation beachhead |
| whisper app alternative(s) | 165 | 0 | Careful: mixed intent with defunct confession app |
| is otter ai hipaa compliant | 50 | 0 | Attack page; bridges to Cluster 2 |

**Pages:** `/compare/granola`, `/compare/otter`, `/compare/superwhisper`, `/compare/macwhisper`, `/compare/fireflies`, `/compare/fathom`. One honest axis per page: on-device vs cloud, owned markdown vs SaaS lock-in, botless capture vs meeting bots. Include a real feature table and a "when X is the better choice" section — honesty ranks and earns AI-answer citations.

### Cluster 2 — Privacy/compliance for regulated industries (the wedge)

| Keyword | Vol/mo | KD | CPC | Notes |
|---|---|---|---|---|
| hipaa compliant ai note taker | 200 | — | — | Core wedge term |
| hipaa compliant transcription | 150 | 2 | $5.00 | KD 2 at $5 CPC = advertisers pay, nobody competes organically |
| legal transcription software | 84 | 10 | $8.00 | Cluster traffic potential 44,000 |
| is otter ai hipaa compliant | 50 | 0 | — | The honest answer ("only on Business tier w/ BAA, audio still leaves the device") sells Minutes |
| ai notes for therapists | 46 | 59 | $9.00 | Crowded by medical-scribe SaaS; angle: notes that never leave the laptop |
| attorney client privilege ai | 30 | — | — | Legal privilege + AI notetakers is a genuinely unanswered question |
| offline transcription software | 20 | 0 | $1.50 | |

**Pages:** a `/private` or `/security` architecture pillar page (on-device processing, 0600 perms, no cloud, open source = auditable), plus posts: "HIPAA-compliant AI note-taking: why on-device changes the analysis", "AI notetakers and attorney–client privilege", "Is [Otter/Fireflies/Granola] HIPAA compliant?" (one page per competitor, factual). Important nuance: **do not claim "HIPAA compliant" as a product certification** — the accurate and *stronger* claim is that audio/transcripts never leave the device, so there's no third-party disclosure and no BAA needed. That framing is also unique content no cloud vendor can write.

### Cluster 3 — Botless capture / anti-meeting-bot (demand proven by competitors' churn queries)

Search demand: "remove otter ai from teams", "how to disable otter ai", "stop fireflies from joining meetings" — plus etiquette queries about bots in meetings. Cloud vendors rank for these defensively; Minutes can rank for them offensively.

**Pages:** "How to remove AI notetaker bots from your meetings (Zoom/Meet/Teams)" — a genuinely helpful guide that ends with "or use a notetaker that never joins as a participant". Plus "AI meeting notes without a bot" as a positioning page.

### Cluster 4 — Local/on-device + whisper.cpp (owns the developer/self-hoster identity)

| Keyword | Vol/mo | KD |
|---|---|---|
| local speech to text | 100 | 13 |
| whisper transcription app | 150 | 55 |
| whisper dictation app | 99 | — |
| best local speech to text | 60 | — |
| whisper.cpp local speech to text | 45 | — |
| mac dictation app | 50 | — |
| on device transcription | 20 | — |
| self hosted transcription | 10 | — |

**Pages:** "The best local speech-to-text apps (2026)" (include competitors honestly), "whisper.cpp vs parakeet.cpp for local transcription" (unique first-party expertise — nobody else ships both), "Local dictation on macOS: complete guide". These earn developer links, which compounds every other cluster.

### Cluster 5 — Head terms (later, via the clusters above)

"ai meeting notes" (1,146/mo, KD 52, TP 22k), "meeting transcription software" (478/mo, KD 53), "ai note taker" (12.6k/mo, KD 72). Don't target directly in year one — the homepage + internal links from clusters 1–4 accrue relevance; revisit when 30+ pages are live and DR-supporting links land.

**Skip:** Fireflies-style filler (memes, team names), "ai medical scribe" (KD 65, funded vertical SaaS — Freed/Heidi territory; Minutes is not an EHR scribe).

## GEO (AI-answer optimization)

For this audience, being the cited answer in ChatGPT/Claude/Perplexity for "private Granola alternative" or "AI notetaker that doesn't send audio to the cloud" likely matters as much as Google position.

- No Brand Radar report exists in the Ahrefs account yet — **set one up** tracking Minutes vs Granola/Otter/Fireflies/Fathom/superwhisper on prompts like "best private ai note taker", "granola alternative", "local transcription app", "hipaa compliant meeting notes".
- Comparison pages should use extractable structures: direct answer in the first paragraph, feature tables, FAQ blocks with schema.org markup.
- The GitHub repo + docs are already the strongest GEO asset (LLMs cite OSS readmes heavily). `llms.txt` + `llms-full.txt` already ship (generated); keep README claims aligned with site claims.

## Technical/site checklist

- Per-page `metadata` (title/description/OG) for every new route; comparison pages need `Product` + `FAQPage` structured data.
- `sitemap.xml` + `robots.txt` (verify present in `site/`).
- `/writing` becomes the blog surface; `/compare` becomes an index of per-competitor pages.
- Internal linking: every cluster page links to the relevant compare pages and to download/quick-start. Homepage links to the pillar pages.
- All pages follow DESIGN.md (cream, Instrument Serif/Sans, Geist Mono transcript cards — the transcript card is itself a differentiated SERP screenshot asset).

## Sequencing

**Weeks 1–2 (highest ROI):** retarget `/compare` → `/compare/granola` + add Otter and superwhisper pages; publish "Is Otter AI HIPAA compliant?" and the on-device security pillar. ~6 pages, all KD ≤ 2 targets.

**Weeks 3–6:** compliance posts (HIPAA analysis, attorney–client privilege), anti-bot guide, "best local speech to text" roundup, whisper.cpp vs parakeet.cpp. Set up Brand Radar + llms.txt.

**Weeks 7–12:** remaining compare pages (Fireflies, Fathom, MacWhisper, Krisp), mac dictation guide, legal-transcription page, meeting-minutes template page (Fireflies-proven, aligns with the product's markdown output).

**Measure:** Ahrefs rank tracking on ~30 target keywords; Brand Radar share-of-voice monthly; conversion = downloads/GitHub stars from organic landing pages.

## Expectations

Honest ceiling: this niche's search volumes are hundreds, not tens of thousands, per term. Realistic outcome at 90 days: 1,000–3,000 organic visits/mo, heavily switch-intent, plus AI-answer citations that don't show in analytics. The strategic value is owning the *category language* ("on-device conversation memory") before a funded competitor decides to.
