# Jev qualification: retrieval and computer interaction

## Verified provider shape

Jev is a structured evaluation model, not a replacement conversational model,
embedding database or browser driver. Vercel AI Gateway exposes `typesafe-ai/jev`
through its v4 evaluation-model protocol. Questions return choice, boolean or
score answers. OpenCode integration has not been verified in this work.

Primary references:
- https://vercel.com/ai-gateway/models/jev
- https://typesafe.ai/blog/introducing-system-one-models-and-jev
- https://docs.typesafe.ai/introduction/quickstart
- Published `@ai-sdk/gateway` 4.0.85 and `@ai-sdk/provider` 4.0.17 contracts.
- https://github.com/awlevin/typesafe-computer-use (author's implementation,
  not an independently verified benchmark or a dependency installed here).

The synthetic-only evaluator is `tooling/voice-evals/jev.mjs`. It sends no
meetings, screen captures, clipboard, private documents or session transcripts.
Run its offline contract test with `node tooling/voice-evals/jev.mjs --self-test`.
An explicitly invoked `--live` evaluation uses `AI_GATEWAY_API_KEY` from the
launch environment. Never commit credentials or print them in receipts.

## September 17 measurement

Seven sequential synthetic cases passed: attendee versus mention, semantic
recall, 404 detection, incorrect paste despite API success, stale foreground
target, page-content instruction injection, and ambiguous target clarification.
End-to-end milliseconds: 1365, 592, 611, 285, 331, 606, 597.
Median 597 ms; maximum 1365 ms. This small hand-authored set establishes an API
and behavior smoke test, not production accuracy or a broad speed benchmark.

## Integration boundary

Use a future opt-in adapter to rank already-policy-eligible snippets and judge
observed DOM or accessibility evidence. Preserve lexical search, exact identity,
participant/date constraints and a no-match/clarify option. A typed answer can
still be factually wrong. Do not let it authorize writes, relax permissions,
invent targets, decide that a send is allowed, or substitute for exact readback.

For computer use the host remains responsible for stable observed references,
focus, revision, origin and freshness checks, deterministic actions, and readback.
For second-brain retrieval, filter access policy before any remote evaluation;
send only the minimum candidates after separate provider consent. Do not index
or transmit the private corpus by turning this experiment on implicitly.

Not yet qualified: broad paraphrase recall, large candidate sets, adversarial
screens, images, real-world ambiguity, repeated latency distributions, provider
failure fallback, cost budgets, or private-data deployment. There is deliberately
no runtime Jev configuration change in this tranche.

The computer-use example combines local Vision OCR, accessibility and active
window observations with choices over known actions, then executes them locally.
Its author reports about 1.5 seconds end-to-end per step on its demonstration,
versus 0.13-0.38 seconds for classification alone. Those numbers are not our
measurements. Its limitations include icon-only controls, duplicate labels and
focus conflicts. Its raw screenshots and payload logging must not be copied into
Minutes' default logging. Prefer existing AX/DOM references to OCR coordinates,
keep send/submit gates independent, and verify exact effects after each write.
