# Minutes vs Fathom

Last reviewed: 2026-07-11

Fathom is the strongest free offer in cloud meeting notes — unlimited recording at $0, polished summaries, CRM sync, and now a bot-free capture option (in beta). Minutes draws the line somewhere Fathom doesn't: recording, transcription, and storage all happen on your own machine, and your audio is never uploaded.

## Quick verdict

- Choose **Fathom** if you want excellent free meeting summaries with CRM workflows and you're comfortable with recordings living in a US cloud, processed by major AI providers under no-training contracts.
- Choose **Minutes** if your conversations must never leave your machine and you want inspectable markdown your own agents query locally.

## Where your conversation goes

**Fathom** (leaves your device): capture (bot in the call, or bot-free via desktop app, beta) → transcribe + summarize in Fathom's cloud (AI via Anthropic/OpenAI/Google) → store on Fathom's US servers, indefinitely by default (auto-delete rules are Business+). SOC 2 Type II, published blanket HIPAA BAA, no-training contracts with LLM subprocessors — a serious cloud posture, but a cloud posture. Fathom improves its own models on de-identified customer data unless you opt out.

**Minutes** (stays on device): capture device audio (no bot, works offline/in person) → transcribe + diarize on-device (sealed local whisper.cpp + pyannote) → store markdown on your disk (0600 permissions). Nothing is uploaded by default — the only network traffic is one-time model downloads, plus transcript text if you explicitly configure an LLM summarizer (local via Ollama, or a provider you choose). See https://useminutes.app/security for the complete list.

## At a glance

- Capture — Fathom: bot, or bot-free via desktop app (beta), Zoom/Meet/Teams/Slack Huddles; Minutes: always botless, in-person too
- Processing — Fathom: cloud; Minutes: on-device (audio never uploaded)
- Storage — Fathom: US servers, indefinite default retention; Minutes: your disk
- Data residency — Fathom: US only; Minutes: moot
- Model training — Fathom: subprocessors barred, internal de-identified training with opt-out; Minutes: no vendor exists
- Compliance — Fathom: SOC 2 Type II, published blanket BAA (pricing lists HIPAA BAA under Enterprise); Minutes: no vendor in the loop
- Open source — Fathom: no; Minutes: MIT
- API/MCP — Fathom: public API + first-party MCP over its cloud; Minutes: MCP (31 tools) + CLI + SDK over local files
- Pricing — Fathom: Free (unlimited recording), Premium $20, Team $19, Business $34/user/mo billed monthly ($16/$15/$25 annually), Enterprise custom; Minutes: free, open source

## Where Fathom wins

- Genuinely exceptional free tier: unlimited recordings, transcription, storage at $0 — nobody matches it, including us
- Real sales/CRM depth: HubSpot/Salesforce sync, coaching metrics, scorecards
- Good agent citizenship: public API + first-party MCP; bot-free capture option (beta)

## Where Minutes wins

- Your audio never leaves your machine — Fathom's bot-free mode still uploads to its cloud; Minutes' capture-to-storage pipeline has no upload step
- No vendor retention to audit: Fathom keeps recordings by default until deleted; Minutes' only copy is yours
- Open source, free forever, and captures in-person conversations and voice memos — no meeting link required

## When Minutes is not the right fit

- Maximum polish for zero dollars on video calls — Fathom's free tier is unbeatable on that axis
- CRM-feeding sales workflows — Fathom's tooling has no Minutes equivalent

## Sources

- https://fathom.ai/ · https://fathom.ai/pricing · https://www.fathom.ai/baa
- https://help.fathom.video/en/articles/296512 (security) · /5291265 (HIPAA) · /296448 (retention)
- https://developers.fathom.ai/mcp-docs
- https://useminutes.app/for-agents · https://useminutes.app/docs/mcp/tools · https://useminutes.app/security
