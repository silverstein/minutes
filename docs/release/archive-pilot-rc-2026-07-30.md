# Minutes Archive private pilot release-candidate record

Date: 2026-07-30

Branch: `feat/minutes-archive-discovery`

Installed development surface:
`~/Applications/Minutes Archive Dev.app`

Bundle identifier: `com.useminutes.archive.dev`

This record distinguishes the verified local development candidate from a
Peter-ready distribution. The current app is intentionally a separate Minutes
target and does not read from, write to, or import content into the Minutes
meeting store.

## Implemented boundary

- Native multi-folder selection for Documents, iCloud Drive, external drives,
  or other individually approved locations.
- A first-pass metadata-only census that emits aggregate counts but no names,
  paths, hashes, or content.
- Explicit, separate content authorization after census review.
- In-memory legal provision and document-level retrieval for searchable PDF,
  DOCX, TXT, TEXT, and Markdown.
- Exact excerpts with document title, stable page, paragraph, or section
  anchor, source revision, and converter version.
- Final approved-root, membership, no-link, identity, byte, and SHA-256
  revision checks before any result reaches the webview.
- Automatic withdrawal of moved, replaced, mutated, or inaccessible evidence.
- PDF and DOCX conversion in a deny-by-default, network-denied, resource-limited
  parser worker.
- Separately labeled meaning-similar suggestions using Apple's pinned built-in
  English sentence model. Provision and query embeddings run in a second
  deny-by-default worker: no filesystem write is permitted anywhere, reads are
  limited to the paths Apple's model needs, and network is denied. Its
  self-test probes paths the profile does not name, so a regression to
  allow-by-default fails it. `mach-lookup` cannot be denied outright without
  breaking the model, so the pasteboard, distributed notification centre and
  syslog are denied by name -- a denylist, and therefore weaker than the file
  rules. The primary control is that the worker performs no logging or IPC at
  all; `os_log` can reach the unified log without a `mach-lookup`, so that
  discipline rather than the profile is what keeps text out of
  `/var/db/diagnostics`. An independent reviewer should re-verify it.
- No source content, FTS rows, or semantic vectors persisted.
- Closing the Archive window releases the session and exits. Purge is explicit,
  not a destructor: `exit(0)` does not unwind, so nothing written as `Drop`
  would run. It is invoked from both the window-close event and `RunEvent::Exit`
  (Cmd-Q maps to `[NSApp terminate:]`, which never fires a close event), it
  recovers from a poisoned session lock, and it drains a registry of worker
  snapshot directories populated before indexing starts, so an exit during a
  build does not strand them. It does not cover `SIGKILL`.
- No downloaded model, QMD runtime, cloud AI, generated legal answer, shell,
  opener, broad filesystem permission, or webview network permission.

## Corrections from adversarial review

Independent review after 2026-07-30 disproved several claims this record
previously made. They are listed because a reviewer needs to know what was
wrong, not only what is now asserted.

- The census exported client surnames. Filenames follow legal filing
  conventions -- `Ltr to A.Weinstein`, `Retainer.Rothschild` -- and any
  lowercase tail after the final dot was emitted verbatim as an extension,
  each with `files: 1`, alongside a hardcoded `filenames_emitted: false`.
  Only extensions the format taxonomy recognizes are now emitted.
- `approve_roots` refused the home directory by canonical path string. On
  macOS `/System/Volumes/Data/Users/<user>` is a firmlink, so the same
  directory reached that way was approved, as were `/Users` and the whole
  data volume. Approval now compares device and inode against every
  directory containing home, including the firmlinked spelling.
- Overlapping roots reached through a firmlink were accepted, double-counting
  every artifact in the intersection.
- The semantic worker ran `(allow default)`; everything outside three
  subtrees was readable and writable, including `$TMPDIR`, while the worker
  received verbatim privileged text. Its self-test could not detect this: it
  probed `/etc/passwd`, a path denied by an explicit literal written to
  satisfy that test.
- Evidence cards asserted concepts matched "in the same provision" that their
  own excerpt did not contain, so a struck clause under a heading naming its
  subjects read as present. Excerpts now carry the text that was matched.
- DOCX `paragraph:NNNNNN` anchors counted paragraphs emitted rather than
  `<w:p>` elements, so empty spacers and table cells shifted them.
- Worker snapshots -- two 40 MB copies of the executable -- survived the
  process in `$TMPDIR`, because `exit(0)` skips every destructor.
- `scripts/verify-archive-pilot-artifact.sh` skipped its entire
  forbidden-entitlement check whenever `codesign` failed, and never asserted
  the hardened runtime.

## Reproducible verification

Run:

```sh
./scripts/verify-archive-dev-app.sh
```

The verifier checks the installed bundle seal; runs the focused Rust tests,
legal benchmark, both real worker tests, and strict Clippy; rejects vulnerable
`quick-xml 0.37.5` if it enters the macOS Archive dependency tree; exercises
TXT, DOCX, and PDF through the installed app executable; verifies current
evidence and mutation withdrawal; runs the deterministic UI interaction smoke;
runs an installed native-window lifecycle smoke that requires a visible main
window, a real close event, and process exit; and prints the bundle identity
and executable SHA-256.

Observed on 2026-07-30:

- Focused Rust tests: 47 passed, 0 failed.
- Legal retrieval benchmark: passed.
- Installed-executable document/worker smoke:
  `document_vault_smoke=passed indexed=3 current_after_mutation=2`.
- Deterministic UI interaction smoke: one approved location, two evidence
  cards, search view visible.
- Installed native lifecycle smoke:
  `archive_native_lifecycle=passed window=visible close=purged`.
- Installed bundle seal: valid and satisfies its designated requirement.
- Installed executable SHA-256:
  `54797b481c2eb09e8e72197f5d3623f999ab3b4abf3c723b2301278093410f3f`.
- Fresh installed app process: no open network socket observed.
- macOS Archive tree: `quick-xml 0.41.0`; no `quick-xml 0.37.5`.

The whole Minutes workspace audit still reports two high-severity advisories
for `quick-xml 0.37.5`. That version is retained by the main Minutes app's
Windows-only notification dependency and is not in the macOS Archive app tree.
The whole workspace also contains informational unmaintained and unsound
transitive warnings. These facts are recorded, not waived.

## Independent review and handoff packet

- `scripts/verify-archive-pilot-artifact.sh` verifies the downloaded
  notarized zip, fixed provenance, Developer ID signature, reviewed Team ID and
  bundle identifier, notarization staple, Gatekeeper acceptance, executable
  digest, and forbidden-entitlement boundary before the app is opened.
- `scripts/verify-archive-pilot-artifact.test.sh` exercises its success shape
  with mocked platform attestations and proves fail-closed behavior for a
  mismatched zip, wrong provenance team, wrong signature team, and unexpected
  provenance field.
- `docs/security/archive-pilot-independent-review.md` defines the independent
  review object, adversarial cases, observation checks, and stop-ship criteria.
- `docs/security/archive-pilot-review-record-template.md` provides a
  reviewer-owned record that is explicitly `NOT REVIEWED` until completed.
- `scripts/make-archive-qa-fixtures.sh` creates a deterministic, client-free
  folder for Finder QA across exact, decoy, PDF, DOCX, unsupported-package,
  permission, and symlink cases.
- `docs/release/archive-pilot-signing-and-handoff.md` binds candidate freeze,
  protected authorization, signing, artifact verification, offline QA,
  independent review, and delivery-hash confirmation into one operator run.
- `docs/release/archive-peter-acceptance.md` gives the release operator and
  Peter a Finder-first, no-Terminal pilot run.

## Unclosed Peter handoff gates

- The current bundle is ad-hoc signed. An Apple Development identity exists in
  the local keychain but was unavailable to the non-interactive signer
  (`errSecInternalComponent`) and is not a Developer ID distribution identity.
- No accessible `Developer ID Application` identity was found. Developer ID
  signing, notarization, staple verification, and another installed-artifact
  hash are required before sending the app to Peter.
- `.github/workflows/signed-archive-acceptance.yml` provides the bounded
  distribution path after review and merge: it accepts only an exact candidate
  protected by `acceptance-<sha>`, builds and exercises the app before any
  credential is unlocked, pauses at the existing reviewed
  `signed-dev-acceptance` environment, then signs and notarizes only the inert
  provenance-bound artifact. It cannot be dispatched until the fixed workflow
  is present on `main`, and the environment reviewer must explicitly approve
  the signing job.
- Native Computer Use could not start its host pipe. The deterministic Chrome
  interaction test and installed native window lifecycle test passed, but the
  console session was locked and a human must still click-test the installed
  Tauri app, native folder picker, cancellation, export, supported indexing,
  search, and source withdrawal.
- The end-to-end workflow has not yet been exercised by a human with networking
  disabled. Worker network denial is enforced and self-tested, but that does
  not replace the full installed-app offline test.
- Independent security review remains required. The implementation author
  cannot satisfy the independence condition.
- Real format coverage is unknown until Peter runs the metadata-only census.
  OCR, legacy Word, WordPerfect, email-container parsing, Apple packages, and
  iCloud hydration must be prioritized from those aggregate counts rather than
  guessed.

This is therefore a strong local development release candidate, not a
production claim, regulatory certification, or attorney-ready distribution.
