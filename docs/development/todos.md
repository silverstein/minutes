# Todos — Minutes

## P1: Dictation Shortcut Settings Simplification
**What:** Merge the two independent shortcut systems (standard shortcut + raw key hotkey) into one unified dropdown with value-prefix dispatch. See `docs/plans/dictation-ux.md` for full spec.
**Why:** Current 6-control settings section is confusing. Users don't know which shortcut system to use.
**Effort:** M (human: ~1 week / CC: ~30 min)
**Depends on:** Nothing — can be done independently.
**Context:** Identified during dictation UX design review (2026-03-24). Streaming whisper engine is shipped (v0.7.2). Overlay UX improvements (streaming text, loading state, silence countdown) are implemented.

## P2: Full Ambient Memory (Voice Memo Intelligence)
**What:** Upgrade voice memo pipeline with LLM auto-classification (person, project, topic tags), intent/decision extraction on voice memos, and include voice memos in `/minutes weekly` synthesis alongside meetings.
**Why:** Transforms voice memos from "searchable text" to "intelligent entries" that Claude can reason about structurally. The ghost context layer (Approach B) establishes the capture pipeline; this adds the intelligence layer on top.
**Pros:** Voice memos become first-class meeting intelligence. Auto-tagging eliminates manual organization. Weekly synthesis surfaces cross-memo patterns.
**Cons:** LLM classification adds latency (~2-5s per memo) and cost. May be overkill for very short memos (<15s). Needs careful prompt engineering to avoid over-extraction.
**Context:** Deferred as Approach C during cross-device ghost context CEO review (2026-03-24). Wait for usage feedback on the ghost context pipeline before adding intelligence.
**Effort:** L (human: ~3 weeks / CC: ~3-4 hours)
**Depends on:** Cross-device ghost context layer shipped (v0.7.0). Needs usage data showing people actually capture voice memos.

## P3: Weekly Synthesis as First-Class Recall Panel View — BLOCKED
**What:** Add a "Weekly" phase to the Recall panel that renders the weekly synthesis directly, rather than only running as a CLI skill in the terminal.
**Why:** Completes the lifecycle loop (prep → record → debrief → weekly) in the UI.
**Status:** Blocked on Recall panel being built. The weekly skill works fine in the terminal for now.
**Effort:** M (human: ~1 week / CC: ~30 min)
**Depends on:** Recall panel shipping.

## P3: WASM compilation of minutes-reader for SDK — BLOCKED
**What:** Compile `minutes-reader` (Rust crate, no audio deps) to WASM and use it as the npm SDK's parsing core instead of the TypeScript reimplementation.
**Why:** Eliminates TS/Rust parsing divergence, guarantees exact parity with the Rust pipeline.
**Status:** Blocked on SDK having real users who surface parsing edge cases. Cool engineering, zero user impact right now.
**Effort:** M (human: ~1 week / CC: ~30 min)
**Depends on:** SDK adoption + user feedback on parsing edge cases.

## P4: Multi-Thread Conversations (Per-Meeting Chat History) — DEFERRED
**What:** Instead of one singleton PTY session, each meeting gets its own conversation thread.
**Why:** Clean context separation when switching between meetings in the Recall panel.
**Status:** Deferred. Major architectural change that nobody has asked for. Needs Recall panel + usage data.
**Effort:** L (human: ~2 weeks / CC: ~2 hours)
**Depends on:** Recall panel + evidence that users want per-meeting threads.

---

## Completed (2026-03-24)

- ~~P1: Agent Memory SDK~~ — `minutes-sdk@0.7.1` on npm
- ~~P2: Claude Code Plugin Distribution~~ — `claude plugin marketplace add silverstein/minutes`
- ~~P2: Cross-Device Dictation~~ — Ghost context layer, phone → desktop pipeline (v0.7.0)
- ~~P3: Publish to crates.io~~ — `cargo install minutes-cli`
- ~~P3: Open Source Interactive Skill Template~~ — `docs/development/skill-template-interactive.md`
- ~~P3: Create DESIGN.md~~ — Design tokens, components, conventions
