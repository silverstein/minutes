# Minutes Archive private-pilot security review packet

This packet is for the independent reviewer of the Peter pilot. It describes
the exact product under review and the evidence required before the installer
may be handed to an attorney. It is not a certification, legal opinion, or
substitute for the attorney's professional-responsibility analysis.

## Review object

The review object is one notarized `Minutes Archive.app` produced from an exact
40-character candidate commit by the protected
`Signed Archive Pilot Acceptance` workflow. The download must contain:

- `minutes-archive-pilot-notarized.zip`;
- `minutes-archive-pilot-notarized.zip.sha256`; and
- `signed-archive-provenance.txt`.

Run the repository's verifier on a Mac before opening the application:

```sh
./scripts/verify-archive-pilot-artifact.sh /path/to/downloaded-artifact-directory
```

The verifier fails unless the zip digest, provenance, Developer ID signature,
Minutes Team ID `63TMLKT8HN`, production identifier
`com.useminutes.archive`, notarization ticket, staple, Gatekeeper assessment,
executable digest, and forbidden-entitlement checks all agree.

## Product boundary to verify

The pilot is a separate Minutes target. It does not import material into the
Minutes meeting store and has no cloud mode.

| Boundary | Required behavior | Primary evidence |
| --- | --- | --- |
| Location authority | Only folders chosen in the native picker are scanned; overlapping roots are rejected | root-approval tests and native picker test |
| Census privacy | Census reads directory entries and file metadata, not regular-file bytes; export contains no names, paths, hashes, or content | census unit tests and exported synthetic report inspection |
| Content authority | Opening documents is a separate action available only after a reviewed census | UI smoke and native interaction test |
| Parser isolation | PDF and DOCX bytes cross bounded pipes into a resource-limited, network-denied worker; paths do not | converter worker tests and sandbox source review |
| Semantic isolation | Apple's built-in revision-pinned model runs in a separate resource-limited, network-denied worker; no model download API is called | semantic worker tests and dependency/source review |
| Derivative lifetime | Source text, FTS rows, and vectors are in memory only; closing the sole window exits the process | persistence search, native close lifecycle smoke, process inspection |
| Evidence fidelity | Results are exact excerpts with source revision and page, paragraph, or section anchors | legal benchmark and document-vault smoke |
| Live-source fence | Root membership, link status, file identity, bytes, and SHA-256 are rechecked before display; stale evidence is withdrawn | mutation/replacement tests and document-vault smoke |
| Webview authority | No filesystem, shell, opener, updater, autostart, global shortcut, or network capability is exposed | Tauri capability and CSP inspection |
| Distribution | Exact protected commit is built and exercised before credentials unlock; candidate code is not executed afterward | fixed workflow policy tests and run log |

## Adversarial review cases

The reviewer should independently attempt at least these cases:

| Case | Required result |
| --- | --- |
| Add a parent and its child as separate roots | overlap is refused |
| Place a symbolic link inside an approved root | link is skipped and never traversed |
| Cancel a census | no partial report is retained for export |
| Cancel content indexing | no partial vault remains searchable |
| Use a PDF containing prompt-like instructions | text is treated as evidence, never orchestration |
| Replace or mutate a matched source after indexing | result is withdrawn |
| Remove or disconnect an approved root | results from that root become unavailable |
| Search with a wrong or empty vault scope | no content is returned |
| Ask for three required concepts in one clause | only one-provision conjunctions qualify |
| Ask for criteria anywhere in one document | each criterion is tied to exact evidence in that same document |
| Exceed the candidate budget | search fails closed rather than claiming completeness |
| Disable networking for the entire installed-app session | census, indexing, exact search, and supported semantic suggestions still work |
| Close the only window after indexing | process exits and cannot answer without rebuilding the vault |

## Egress and observation checks

Use synthetic documents only for security testing. With networking disabled,
exercise census, content authorization, PDF and DOCX conversion, semantic
suggestions, export, and close. Repeat with networking enabled while observing
the app and both workers. Any network connection attributable to the Archive
processes is a stop-ship finding.

Inspect unified logs, crash reports, window titles, exported census JSON, and
temporary directories for synthetic canary strings, filenames, source paths,
extracted text, prompts, and vectors. Any confidential derivative outside the
authorized evidence UI or explicit export is a stop-ship finding.

## Known retrieval limitations disclosed to the reviewer

These are open defects the implementation author found and did not fix. They
are listed so the reviewer tests them deliberately rather than discovering them
as surprises, and so the boundary between "known and bounded" and "stop-ship"
is drawn by the reviewer rather than assumed.

**A same-provision conjunction can span two adjacent clauses in a PDF that
reports no structure.** The segmenter closes a provision at a heading. Where a
PDF has one uniform font size and section captions that no lexical rule
recognises -- title case, no numbering -- neither the file nor the text offers
a boundary, and a provision can run past the end of one clause into the next.
A conjunction is then asserted across text the document never joined.

Scope and mitigation, all verified:

- DOCX is unaffected: `w:pStyle` reports the structure directly.
- PDFs with numbered captions ("7. CONFIDENTIALITY") or real heading styles
  are unaffected.
- The excerpt is always displayed, so the reader can see both clauses. This is
  a visible overstatement, not a hidden one.
- Cards making a conjunction claim on a provision with no caption now say so:
  "This provision carries no section caption, so its extent was inferred from
  the page layout; check the excerpt that the terms are in one clause."

A reproduction is checked in at `tests/fixtures/archive-real-pdf/list-tail-merge.pdf`
with an `#[ignore]`d test in `crates/archive-core/tests/real_pdf_segmentation.rs`.
A geometric converter that fixed this was built, reviewed, and reverted: it
silently deleted section captions from documents with no running header, which
is worse. The reviewer should judge whether the disclosure is sufficient for
the pilot or whether this is stop-ship under "make a materially broader claim
than the tested format and location coverage".

**PDF page-boundary segmentation is layout-derived.** Provision extents in PDFs
come from page and paragraph layout, not from a structure the file declares.
The "Evidence fidelity" row above is exact about excerpts, revisions, and
anchors -- the excerpt is genuinely the source text at the cited anchor -- but
provision *extent* in a structureless PDF is inferred.

## Stop-ship criteria

The pilot must not be delivered if the reviewer finds any unresolved issue
that can:

- disclose a filename, path, document byte, excerpt, prompt, or derivative
  outside the approved local operation;
- escape an approved root or follow a link or reparse point;
- return evidence after its source is no longer current and authorized;
- persist source text or vectors across application exit;
- execute document text or candidate-controlled code as instructions;
- silently use a network or downloaded model;
- present a generated legal conclusion as source evidence;
- bypass the protected signing, notarization, or provenance boundary; or
- make a materially broader claim than the tested format and location coverage.

## Review record

The final review report should identify the candidate commit, notarized zip
SHA-256, executable SHA-256, macOS version, test Mac architecture, review date,
reviewer, methods used, findings with severity, fixes retested, residual risks,
and a clear approve or do-not-approve decision. The reviewer—not the
implementation author—owns that decision.

An author-run adversarial pass was completed before this review and is logged
in `docs/security/archive-pilot-pre-review-findings.md`. It records five
findings and their fixes, what was probed and held, the residual risks, and --
most usefully -- what it did not reach. It is explicitly not an independent
review and not approval: it was commissioned by the implementer, on the
implementer's code. Read it to avoid duplicating covered ground, not to
shorten the review.

Use `docs/security/archive-pilot-review-record-template.md` as the reviewer-
owned record. Its initial `NOT REVIEWED` state is intentional and must never be
treated as approval.
