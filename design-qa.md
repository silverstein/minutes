# Sidekick 2 + 1 Design QA

## Comparison target

- Source visual truth:
  - Quiet Signal:
    `/home/mat/.codex/generated_images/019f85e9-3f1b-7962-b58b-045e4433504b/call_k3yjvVBYrLd25JtEts3r7NOB.png`
  - Strategist Thread:
    `/home/mat/.codex/generated_images/019f85e9-3f1b-7962-b58b-045e4433504b/call_iD6jbwCy1RPZ7eUqqc4RWVQz.png`
- Source pixels: 1148 × 1380 for each generated target.
- Intended implementation viewport: native Sidekick window, 720 × 760 CSS
  pixels, responsive down to the existing 480 × 520 minimum.
- Intended states:
  - Resting: quiet listening state with at most one material signal.
  - Engaged: strategist thread after the user asks, corrects, or steers.
- Density normalization: pending a native macOS capture. The implementation
  must be compared at its CSS viewport after normalizing the source targets to
  the same content width.

## Evidence captured

Both source targets were opened at original detail and reviewed. They establish
the desired cream/warm-dark Minutes palette, Geist/Geist Mono/Instrument Serif
hierarchy, one dominant material insight, restrained actions, a readable
context disclosure, and a conversation-first expansion after user engagement.

No implementation screenshot exists yet. The implementation is a Tauri/WebKit
window and repository policy requires the installed signed development app to
be the visual product truth. The automated DOM harness proves state,
interaction, and paint contracts, but it is not acceptable visual evidence.

## Findings

- [P1] Native rendered evidence is still required.
  - Location: installed `Minutes Dev.app` Sidekick window.
  - Evidence: both source visuals are available, but there is no same-state
    native implementation capture to compare.
  - Impact: typography, wrapping, window density, footer height, focus rhythm,
    dark mode, and actual WebKit behavior cannot be graded honestly from source
    code or the DOM harness.
  - Fix: install the branch using `./scripts/install-dev-app.sh` on Silverbook,
    capture the quiet-signal and engaged-thread states at 720 × 760, then run a
    same-canvas comparison and iterate on every P0/P1/P2 mismatch.

## Required fidelity surfaces

- Fonts and typography: source targets use the intended Instrument Serif,
  Geist, and Geist Mono hierarchy; native loading, optical size, wrapping, and
  antialiasing remain unverified.
- Spacing and layout rhythm: the implementation follows the Minutes 4/6/8/10/
  12/16/20 scale in code; native frame proportions and persistent-control
  visibility remain unverified.
- Colors and visual tokens: implementation values match `DESIGN.md`; actual
  light/dark WebKit rendering and contrast remain unverified.
- Image quality and asset fidelity: neither target relies on product imagery or
  decorative raster assets. No image substitution issue is present.
- Copy and content: the resting/engaged contract, primary actions, context
  disclosure, provider status, recovery status, and composer are implemented
  and covered by DOM tests. Glanceability in the installed window remains
  unverified.

## Full-view comparison evidence

Blocked until the installed Sidekick window is captured in both target states.

## Focused-region comparison evidence

Blocked for the same reason. The first focused checks should cover the dominant
insight typography, source/context rail, composer, status line, and the
resting-to-engaged transition.

## Comparison history

1. Initial implementation pass: no native implementation screenshot; visual
   comparison blocked.

## Implementation checklist

1. Install the branch on Silverbook with the stable local build script.
2. Capture light-mode quiet signal and engaged thread at the native window
   size.
3. Capture dark mode and the 480 × 520 minimum-size state.
4. Check keyboard focus, VoiceOver labels, source rail, composer persistence,
   and recovery copy.
5. Compare source and implementation together; fix all P0/P1/P2 drift and
   repeat until clean.

final result: blocked
