# Minutes meeting workflow audit, October 11, 2026

Minutes already has most of the capabilities needed for a useful meeting companion. The immediate opportunity is to make capture dependable and make preparation, discussion, decisions, and reviewed outputs feel connected. Google Foresight raises an interesting additional question: can an optional assistant enrich the notes a person deliberately writes, while preserving their wording and the evidence? That deserves an experiment after the existing workflow is easier to use.

This is an assessment, not an implementation or a release qualification. It preserves Minutes' open-source scope, local records, provider choices, CLI, portable skills, MCP, dictation, and advanced agent surfaces.

## Evidence and boundaries

Source: `d3285c7990e69ff5db36e1691cca43517b232d66`, current `origin/main` when inspected. Runtime: published Linux CLI 0.28.2 and installed signed macOS Minutes 0.28.2. CLI SHA-256: `f3b8b8f5170ef99f3708f9d2d5304e886a238467b58cc31367ca192775bb6cea`, matching the release checksum.

The installed Mac executable hash was `38ddef7d4f0b12ba5adea2354f690287f8e34bc3c436da0b3c5013c4771950ca`; `codesign --verify --strict` passed. A compact [verification receipt](meeting-workflow-audit-2026-10-11/receipt.json) records results and limits without the unrelated desktop-menu contents.

The ten fictional records in `tests/eval/fixtures/meetings` were copied into disposable Linux and Mac profiles. Data/config paths were overridden and checked; automatic capture, calendar, desktop/screen context, hotkeys, and Coach were disabled. No microphone use, audio playback, actual call, normal meeting-record edit, external message, or application replacement occurred. Native window inspection was explicitly authorized by Mat.

The native app used the existing release bundle with an additional process. These overrides isolate Minutes config and record/assistant directories; they do not prove a separate macOS account or an isolated WebKit preference store. UI-only state changes must not be described as complete account isolation. An invalid duplicate privacy table in the audit-created config initially caused the app's default-config fallback; that instance was stopped, the synthetic TOML was corrected and parsed independently, and only subsequent valid-profile screenshots support the workflow findings. Initial black screenshots were rejected as visual evidence. Screen-share protection was disabled only in the disposable config for the accepted captures. One native Chat question used the existing Claude subscription with synthetic meeting content; this is a cloud-provider test, not on-device inference.

Private raw receipts are under `/home/mat/.codex/artifacts/minutes-workflow-audit-20261011`. Accessibility dumps also contain unrelated system menu entries; those raw dumps are not suitable for publication. The selected screenshots below show synthetic product content only.

Foresight 1.0.0 was opened at first-run. Its Start button accepts Google's terms and starts model downloads. Further acceptance was requested separately and remained pending during this assessment. Accordingly, Foresight's assistance and output quality are vendor claims, not hands-on results. No overall head-to-head quality score is warranted.

## Six steps through the meeting

1. **Preparation — existing capability; navigation friction.** `minutes-prep` already retrieves relationship history, open actions, decision history, and a meeting goal. The native draft chooser already offers Meeting Brief. Runtime `person Riley` returned two relevant meetings and decisions; `research pricing` returned the reversal and earlier experiment with source paths. The idle native home emphasizes recording, a dated list, and losing-touch alerts rather than a visible preparation route. A selected meeting's detail then exposes the useful draft chooser. Recommendation: bring a brief and its goal into the selected meeting workspace, reusing those features. The complete generated prep experience was not exercised.

2. **Capture — current repair credited; live audio not retested.** The native idle UI exposes a microphone selector and Meeting/Memo/Live modes. [Release 0.28.2](https://github.com/silverstein/minutes/releases/tag/v0.28.2) repairs the reported mic-only Go Live path. [Issue #1110's verification](https://github.com/silverstein/minutes/issues/1110) specifically describes AirPods, a solo Meet, and a phrase played through system output. This is meaningful capture evidence, but does not establish a fresh two-human call or mid-call device-route acceptance. Capture confidence remains the highest priority because missing remote speech cannot be recovered by summarization. Readiness and recovery already exist in source; the recommendation is to make their evidence easy to understand at the recording point, not build replacement systems.

3. **Live help — existing surfaces; unfinished integration.** Coach has an optional goal and local-model setup in Settings → AI & Privacy. Terminal Sidekick and native Recall also exist. [RFC 0006](../rfcs/0006-live-sidekick-session-and-eval.md) explicitly defers Native Recall integration with its new live-assistance session contract, foreground-preemption adapters, and signed native acceptance. That does not mean all native chat or all live help is missing. Recommendation: connect the existing brief, selected meeting, recording, and live help through that staged contract; keep optional assistance failure-isolated from capture. No Coach inference, live scheduling, interruption, or cancellation behavior was exercised here.

4. **Debrief — useful structure already visible.** Native meeting detail displayed action items, the annual-only decision, and speaker confidence. The draft chooser provides Debrief Memo, Follow-up Email, Meeting Brief, and Decision Memo. The debrief skill already compares preparation against the outcome. Recommendation: lead the completed meeting view with what was decided, what remains uncertain, who owes what, and the next useful action, with expandable speaker/source details. This is presentation and connection work, not a missing debrief feature.

5. **Follow-up — successful native draft; confirmed date error.** Clicking Follow-up Email created an editable, source-linked Markdown artifact immediately, with no summarization model. Its key points, pricing reversal, and three action items matched the selected synthetic meeting. But a March 25 meeting produced “Thanks again for the conversation today” on October 11. The source date is correct in metadata; the template wording is not date-aware. Opening the draft also puts a long filename and raw frontmatter ahead of the useful email. Recommendation: fix temporal wording and default to a readable editable document with metadata available on demand, retaining source mode and the existing review/revert controls. No email was sent, and review/diff behavior was not exercised.

6. **Next conversation — continuity exists; projections need reconciliation.** The CLI found the current pricing decision and marked its earlier decision explicitly superseded. It returned 24 open action items, while the consistency report returned zero stale commitments because that report reads `intents`, and these fixtures have `action_items` without intents. That is an observable projection difference, not proof of data loss. The people view split abbreviated/full names; explicit `people merge mat-silverstein mat-s` combined them into Mat Silverstein with nine meetings and eight commitments. The correction mechanism already exists. Recommendation: expose confirmed corrections, show which kind of obligation each view counts, and carry the same confirmed state into briefs, detail, and follow-up. Avoid guessing that every similar name is the same person.

   Native Recall also answered a pricing question with the correct annual-only decision and a quote matching `[SPEAKER_0 0:47]` in the selected reversal meeting. It identified the replaced decision and explicitly qualified that the earlier meeting itself was not in its current context. That is one successful source-grounded answer with appropriate uncertainty. It is not a successful all-corpus synthesis benchmark. The source reference in the observed answer was plain meeting-title/timestamp text; a clickable source-open action was not established.

## Current native evidence

Every image below was captured and inspected during this audit. Blank/collapsed captures are excluded from the design assessment.

| Screen | What it establishes | Evidence |
| --- | --- | --- |
| Minutes home | Recording modes, dated list, and ambient alerts; preparation is not prominent here | [Home](meeting-workflow-audit-2026-10-11/home.png) |
| Completed meeting | Decisions/actions/speaker confidence and Create Draft already exist | [Meeting detail](meeting-workflow-audit-2026-10-11/meeting-detail.png) |
| Draft chooser | Four named output types, including the next Meeting Brief | [Draft chooser](meeting-workflow-audit-2026-10-11/draft-chooser.png) |
| Follow-up artifact | Native editable scaffold opens with filename/frontmatter; source link survives | [Follow-up](meeting-workflow-audit-2026-10-11/follow-up.png) |
| Coach settings | Goal and on-device option exist; configuration is in a separate settings flow | [Coach](meeting-workflow-audit-2026-10-11/coach-settings.png) |
| Native Recall | The simpler Chat surface exists alongside Terminal | [Recall](meeting-workflow-audit-2026-10-11/recall.png) |
| Native answer | Correct current decision and transcript timestamp in one synthetic task | [Recall answer](meeting-workflow-audit-2026-10-11/recall-answer.png) |
| Foresight first-run | Focused onboarding and explicit terms/model-download gate | [Foresight](meeting-workflow-audit-2026-10-11/foresight-first-run.png) |

The dark texture and typography give Minutes a recognizable character, but subdued secondary text and several competing green accents weaken hierarchy. Long titles are truncated in detail; artifact filenames dominate the editor. Start with clearer secondary-text contrast, shorter human document titles, consistent sentence case, and one primary action per stage. In Recall, describe the selected scope and processing provider plainly; “private Chat” using a Claude subscription should not be confused with on-device processing. These are qualitative screenshot findings, not measured WCAG contrast results. The screen-share preference also deliberately prevents ordinary capture; black screenshots from that condition are not a visual-design defect.

Accessibility inspection had limits: some Settings snapshots exposed underlying content while the visible modal was open; Foresight's Flutter first-run body was absent from the observed AX tree. These deserve focused foreground keyboard/VoiceOver checks. They do not establish a complete accessibility failure or compliance result. Screen-reader behavior, reduced motion, and a full keyboard journey were not tested.

## What to learn from Foresight

Google describes Foresight as an experimental on-device meeting companion with microphone/system audio, enhanced shorthand notes, live answers, and retrieval across transcripts and private files. Its advertised processing uses EmbeddingGemma 2 and Gemma 4. These are [Google's October 6 announcement](https://developers.googleblog.com/en/google-ai-edge-with-embeddinggemma-2/), not measured results from this audit.

| Idea | Minutes today | Recommendation |
| --- | --- | --- |
| Shorthand notes expanded using the current conversation | Timestamped inline notes exist; the inspected save path stores the entered note through `notes::add_note` | Trial an optional expansion beside the original note. Preserve the user's original text, bind additions to transcript timestamps, and offer accept/edit/reject. |
| Live context that feels part of the meeting | Coach, Sidekick, Recall, prep, and lifecycle pieces exist; the new shared session contract is partly deferred | Finish the narrow native connection before adding another assistant surface. |
| Local multimodal retrieval | Minutes already has documents and optional screen/desktop context | Evaluate an additional local retriever only against existing retrieval, using relevant slides/images and measured answer correctness, latency, memory, and abstention. Do not replace the current system on a model announcement alone. |
| A focused first-run promise | Foresight's observed first-run introduces one personal-knowledge task; Minutes exposes many modes and setup routes | Guide a new user through one useful meeting while keeping all advanced capabilities available. |

The notes experiment should answer a concrete question: does preserving a person's shorthand and adding source-bound context yield a more useful record than the existing summary? It should be judged by factual preservation, decisions, owners, dates, unresolved questions, scan time, and usefulness before the next conversation. Attractive live suggestions alone are insufficient.

## Recommended order and existing ownership

| Order | Change or qualification | Evidence for priority | Existing tracking |
| --- | --- | --- | --- |
| 1 | Qualify both sides of a real call through stop, processing, and reopening; include AirPods/device changes and visible degradation | Historic capture loss is unrecoverable; 0.28.2 fixes it but this audit adds no audio acceptance | Existing capture/native qualification work; #1110 is closed, do not reopen or duplicate without a current reproduction |
| 2 | Make the existing completed-meeting → draft → review route clear; correct stale-meeting email wording | Native draft works, but the default opens raw metadata and uses the wrong temporal wording | `minutes-hua3`, `minutes-8zgi.2` |
| 3 | Reconcile obligation counts and expose existing confirmed identity correction across surfaces | 24 actions versus zero stale intents; explicit person merge succeeds | `minutes-8zgi.2`; assess scope alongside `minutes-u31b`, without claiming it already covers the schema difference |
| 4 | Connect the brief/goal and live session through the staged native contract | Existing features plus explicit RFC integration deferrals | RFC 0006 and existing Sidekick/native journey work |
| 5 | Make the public eval exercise Minutes' actual retrieval boundary | One fixture is excluded by the product although ambient-file agents can read it | `minutes-n5vj`, `minutes-5p8i` |
| 6 | Run the bounded shorthand-enrichment experiment, then consider multimodal retrieval | A plausible way to improve notes; competitor quality remains unverified | Scope as a follow-on experiment after existing cohesion work, not a replacement roadmap |

No new GitHub issue was filed. The breadth of the existing product should stay available; the default path should simply require fewer decisions from the user.

## Verification and remaining uncertainty

The published CLI executed paths, list, search, actions, consistency, research, person, commitments, people, meeting reads, transcript status, and a synthetic person merge. The pricing reversal's `resolution` correctly identifies the superseded decision. Native draft creation was independently observed in the persisted synthetic artifact.

The public voice-memo fixture uses `type: voice_memo`; the current schema accepts `memo`. The released CLI rejected that fixture as policy-invalid and search indexed nine records with one policy-uncertain exclusion. Changing only the type in the disposable copy allowed `get --json` to succeed, after which the copy was restored. This is a fixture/coverage defect, not evidence that recording loses voice memos. The v0.1 README already calls its benchmark scaffolding and says Minutes/MCP are not required: its agent runner grants ambient file access. Extend that existing harness rather than propose a new benchmark from scratch.

Sixteen existing Python live-sidekick privacy/schema tests and 21 existing Node Recall-input/SOTA-fixture tests passed on the assessed source. No Rust build, new model download, model-quality benchmark, live audio test, or release was performed. Draft PR #1107 remains unreleased; at inspection its Ubuntu runtime check failed despite the aggregate CI Gate passing. It must not be counted as shipped keyboard-sound/final-audio behavior.

The Foresight comparison remains incomplete until legal/model setup is authorized and the same synthetic tasks can be exercised. Minutes' complete generated prep, live assistance, review, and broad cross-meeting native answer quality also require further acceptance. This report identifies the best next work from the evidence available; it does not certify either application's complete meeting experience.

Audit ownership: root, branch `audit/meeting-workflow-20261011`, retained worktree `/home/mat/Sites/minutes-worktrees/meeting-workflow-audit-20261011`, based on the source SHA above. The shared checkout and other lanes' work are preserved. Follow-up remains on the existing local Beads tasks; this document is evidence and recommendations, not a replacement task tracker.
