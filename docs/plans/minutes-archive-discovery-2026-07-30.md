# Minutes Archive: Discovery and Security Contract

Status: implementation underway; native census and capability-bound searchable
PDF, DOCX, and text evidence pilot implemented

Date: 2026-07-30

Implementation baseline: `origin/main` at
`6ac1c4f18361343ff088b4f5fd45cf60dce092a6`

## Recommendation

Build the first archive proof as a separate `Minutes Archive.app` Tauri target
in the Minutes workspace. Share narrowly scoped Rust libraries and design
tokens with Minutes, but give the archive app its own bundle identifier, data
directory, command manifest, capabilities, release channel, and threat model.

Do not put archive documents in `~/meetings`, add legal-only concepts to the
conversation schema, expose the archive through the current Recall window, or
fork the Minutes repository during discovery.

The reusable product thesis is private, owned memory with provenance, scoped
authority, and controlled egress. Conversation memory is one source adapter.
Document archives are another. Legal is the first validation setting, not a
hard-coded product identity.

## Shipped Facts and Unlanded Inputs

The implementation baseline already has local Markdown artifacts, a SQLite
search index, sensitivity filtering, Tauri packaging, a shared Ollama adapter,
capability-bound secure reads, bounded corpus leases, restrictive filesystem
policy, sealed private-audio processing, and restricted-conversation egress
enforcement.

QMD is not currently the implementation behind the public
`minutes_core::search` path. That path opens the Minutes SQLite search index
directly. Current main explicitly disables persistent QMD collections because
QMD's global index cannot guarantee revocation after an external policy change.
Legacy registry and mirror handling is retirement and cleanup machinery, not
proof that Recall or `minutes search` performs hybrid retrieval.

This contract also incorporates scoped-history invariants observed in the
unlanded Silvercloud Sidekick branch. That branch is design and test evidence,
not shipped product behavior. No implementation should depend on its code until
it is reconciled with fresh main and independently reviewed.

The reusable unlanded invariants are:

- authority and scope are established before retrieval;
- an unscoped request fails closed rather than searching broadly;
- inferred identity never grants access;
- restricted, malformed, unreadable, stale, or policy-uncertain content fails
  before prompt assembly or process or network invocation;
- source bytes and authorization are revalidated immediately before egress;
- derived data inherits source sensitivity and provenance;
- a scope or policy change invalidates context assembled under the old scope;
- local execution is claimed only after loopback is verified; and
- provenance describes evidence actually supplied rather than implying
  exhaustive citation coverage.

## Concrete User Flow

1. The attorney chooses an archive folder. The app receives only the explicit
   folder capability; it does not scan the home directory implicitly.
2. The app runs a metadata-only census. It reports aggregate formats, sizes,
   age bands, packages, placeholders, symlinks, and failures. It emits no
   filenames, paths, or document text.
3. The app previews conversion coverage and storage requirements before
   reading document contents.
4. The attorney creates a vault and chooses its matter policy. Originals stay
   read-only. The app creates private normalized sidecars with source identity,
   hashes, page or paragraph coordinates, converter version, and sensitivity.
5. The attorney can first run exact and Boolean searches over the normalized
   corpus. Results are evidence cards containing an excerpt, source title,
   source location, and an Open Original action.
6. Hybrid semantic search is enabled only after the derivative-index gate is
   satisfied. A local answerer may synthesize from retrieved excerpts, but
   unsupported claims produce an explicit insufficient-evidence response.

Example:

> Find confidentiality provisions that are no more than three sentences and
> also cover affiliates, compelled disclosure, and survival.

The result is a ranked set of exact provisions. Each card shows why it matched,
the three-sentence excerpt, document and page, and whether all three requested
concepts occur in the same provision. A generated comparison is secondary to
the evidence cards.

## Architecture Boundary

```text
Minutes workspace
|
|-- minutes-core                 shared low-level utilities only
|-- archive-core                 inventory, policy, provenance, corpus leases
|-- archive-convert              isolated format conversion workers
|-- archive-retrieve             exact, Boolean, and gated hybrid retrieval
|-- archive-answer               evidence-constrained local synthesis
|
|-- Minutes.app                  conversation product
`-- Minutes Archive.app          independent least-privilege Tauri target
```

`archive-core` owns authorization and source identity. Retrieval backends
receive an already-authorized corpus lease rather than a raw filesystem root.
The UI never receives unrestricted filesystem or shell access.

Converters run out of process with no network, bounded time and output, and a
fresh temporary directory. Their output is untrusted until validated.
Documents and retrieved text are data, never instructions to an agent or
converter orchestrator.

## Source and Derivative Classes

| Class | Examples | Required treatment |
| --- | --- | --- |
| Original | DOC, PDF, email, scan, package | Read-only; never rewritten |
| Normalized | Markdown or text sidecar, OCR text | Same sensitivity as source |
| Retrieval | FTS rows, chunks, embeddings, reranker cache | Same sensitivity as source; revocable |
| Answer | excerpt card, generated comparison, export | Carries exact source provenance |
| Operational | counts, health, converter status | Must not contain names, paths, or excerpts |

Indexes, embeddings, OCR text, caches, logs, crash reports, and backups are
confidential derivatives. Protecting only the original documents is a failed
design.

## Retrieval Contract

The first proof uses exact and SQLite FTS retrieval because its membership and
revocation behavior can be kept inside the application boundary.

Hybrid retrieval may use QMD only through an application-owned, vault-specific
database or a locked local sidecar. The global QMD collection registry is not a
security boundary and must not contain attorney material.

Before hybrid retrieval is enabled, the implementation must prove:

- every query names an exact vault and authorized matter scope;
- no unscoped fallback searches other collections;
- the index database is private and excluded from telemetry and general
  workspaces;
- source removal or sensitivity change revokes the corresponding chunks;
- source and index membership are fenced against concurrent change;
- symlink, hard-link, root replacement, and stale-index cases fail closed;
- model downloads are pinned and occur before confidential processing or
  through an explicit setup-only network capability; and
- clearing a vault removes its normalized and retrieval derivatives.

If these properties cannot be established, downloaded-model hybrid semantic
indexing remains disabled. The later built-in Apple suggestion experiment is a
separate, ephemeral lane and is never treated as a verified constraint match.

## Tauri Security Profile

The archive target must not inherit the existing all-window capability file or
the full Minutes command registry.

Its production profile has:

- a restrictive CSP and no remote frontend content;
- no devtools, PTY, shell, global shortcut, autostart, or general opener
  capability;
- no network capability during census, conversion, indexing, or local search;
- explicit application commands in the Tauri manifest;
- a file-picker-granted root narrowed to read-only backend commands;
- no frontend access to arbitrary paths;
- a separate updater decision and signing identity; and
- no document content in logs, analytics, crash metadata, window titles, or
  notifications.

Opening an original is a backend action against a current authorized source
identity, not a general `shell:open` grant to the webview.

## Threat Model

| Threat | Required control |
| --- | --- |
| Compromised frontend | Narrow command manifest, strict CSP, window-specific capability |
| Malicious document prompt injection | Treat content as data; fixed orchestration; evidence-only generation |
| Converter exploit or decompression bomb | Networkless child, resource ceilings, validated bounded output |
| Cross-matter retrieval | Capability before retrieval; separate vault or matter lease; deny unscoped calls |
| Stale semantic index | Membership manifest, policy invalidation, pre-answer revalidation |
| Symlink or root replacement | Canonical identities, no link following, initial and final fences |
| Another local user or backup reader | OS account boundary, encrypted device and backup, private permissions |
| Remote-model disclosure | Cloud disabled by default; verified loopback for local inference |
| Misleading AI answer | Exact excerpts, page anchors, claim-evidence closure, insufficient-evidence state |
| Lost or altered source | Read-only originals, hash-backed provenance, no silent repair |

## Compliance Boundary

The product may be described during discovery as local-first and designed for
confidential professional material. It must not be described as compliant,
privilege-preserving, HIPAA compliant, or approved for a regulated industry
until the applicable control and legal reviews are complete.

Technical release gates include a documented data-flow inventory, independent
security review, dependency and model inventory, signed builds, encrypted
device and backup guidance, retention and deletion verification, incident
response behavior, access-log review, and adversarial cross-vault tests.

Professional-use gates include jurisdiction-specific ethics review, firm
policy, client or engagement requirements, supervision, human verification,
and informed consent wherever information would leave the firm or another
rule requires it.

## Discovery Gates

The Peter pilot advances in four bounded stages.

### Stage 0: Synthetic census

The metadata-only census passes privacy tests and runs against a synthetic
archive containing modern, legacy, package, scan, email, placeholder, and link
cases.

Implemented on the discovery branch in `minutes-archive-core` with a committed
synthetic UI fixture. The Rust suite covers content-canary exclusion,
unreadable regular files, package non-traversal, multiple roots, duplicate and
overlapping roots, root and nested links, artifact bounds, and cancellation.

### Stage 1: Attorney-run aggregate census

The attorney opens the signed `Archive Census.app`, approves one or more
locations through the native macOS picker, runs the census, reviews the
aggregate, and chooses Export Aggregate Report. Only the aggregate JSON is
reviewed. No source filename, path, or content is transferred.

Peter is not asked to install a development environment or use Terminal. The
maintainer-only dogfood build is:

```bash
./scripts/install-archive-dev-app.sh
```

The Python census remains a reference implementation and independent privacy
test surface. It is not the customer workflow.

### Stage 2: Synthetic retrieval proof

Converters and exact retrieval are evaluated on public or synthetic legal
documents. Every result opens the correct source location, and adversarial
documents cannot alter system behavior.

The exact-retrieval slice is implemented with an in-memory, vault-scoped SQLite
FTS5 index and deterministic provision-level verification. The lexical lane
has no model, and no attorney derivative is persisted. Tests prove exact
excerpts and anchors, same-provision concept conjunction, sentence limits,
explicit vault denial, stale-revision withdrawal, transactional source
replacement, and inert prompt-like source text. Document-level conjunction is
implemented as a separate evidence type that groups criteria inside one
document, applies exclusions across its provisions, and never assembles a
match across documents. A committed legal fixture exercises the provision and
document modes, and an overflowing lexical candidate set fails closed.
Additional legacy and email format conversion, protected persistence, and OCR
remain unimplemented Stage 2 gates.

The separate desktop app now makes content access a distinct post-census
action. It can build an in-memory index from bounded searchable `.pdf`, `.docx`,
`.txt`, `.text`, and `.md` sources using the retained folder-picker
authorities. Traversal skips links and packages, deduplicates file identities,
holds read-only source handles, and applies a final root, membership, identity,
byte, and SHA-256 revision fence before returning any evidence card. A moved,
replaced, mutated, or inaccessible result is withdrawn. The aggregate build
report contains no source name or path and states that neither source text nor
the index was persisted.

PDF and DOCX parsers run only in a self-executed worker snapshot. The parent
binds and re-hashes the immutable executable snapshot, clears the environment,
uses stdin and stdout pipes rather than source paths or named plaintext
temporary files, caps source and output bytes, enforces a deadline and process
group, and requires a real startup self-test. On macOS the worker installs CPU,
file, descriptor, and measured address-space limits plus a deny-by-default
Seatbelt profile before reading source bytes. The self-test proves both an
`/etc/passwd` read and a localhost listener are denied. Synthetic end-to-end
coverage sends TXT, DOCX, and PDF through the actual app executable, verifies
page or paragraph anchors, retrieves all three, mutates the PDF, and observes
its evidence withdrawal. Scanned PDFs are reported as OCR-required rather than
silently treated as searchable.

The app also has a reversible, in-memory semantic-suggestion lane. It pins
Apple's built-in English `NLEmbedding` sentence model to revision 1, calls no
asset-request or model-download API, caps each provision input, normalizes and
bounds the vector count, and binds every vector to a vault, document,
provision, and current source revision. Meaning-similar excerpts are displayed
below and separately from deterministic matches with an explicit
attorney-review warning. They are never represented as satisfying a legal
constraint. Tests prove model identity, vector bounds, vault denial, exact
excerpt return, stale-source withdrawal, and a legal paraphrase ranking above
unrelated text.

Model execution is now isolated from the desktop process. The parent binds and
re-hashes a private read-only snapshot of the installed app executable, starts
a persistent length-framed pipe worker, clears its environment, applies
resource ceilings, and installs the sandbox before constructing the Apple
model or reading provision text. The macOS profile denies all network access
and reads or writes under user, volume, and network roots; the worker receives
text only through a bounded pipe and never receives source paths. Its startup
self-test must prove that localhost binding and `/etc/passwd` reads fail.
Provision and query embeddings both use this worker. The vectors remain
process memory only. A downloaded embedding model, QMD integration, hybrid
fusion, reranking, answer generation, and durable vector storage remain
separate gates.

A RustSec audit on 2026-07-30 found vulnerable older `quick-xml` versions in
the shared workspace lock. The Archive dependency tree's `plist` was updated
from 1.8.0 to 1.10.0, removing `quick-xml` 0.38.4; both the Archive converter
and the macOS app tree now resolve `quick-xml` 0.41.0, which contains the
relevant denial-of-service fixes. `quick-xml` 0.37.5 remains in the whole
workspace lock through the Windows-only notification path of the main Minutes
app, so a whole-workspace `cargo audit` still returns nonzero even though that
crate is absent from the macOS Archive app tree. Informational unmaintained and
unsound transitive warnings also remain for independent review; this is not a
claim that the dependency review is complete.

### Stage 3: Controlled private pilot

The attorney selects a bounded copy or read-only subset after reviewing the
coverage report. Downloaded-model hybrid retrieval and local generation remain
separately gated. No cloud provider receives archive content.

## Pilot Success Measures

- Zero modification of source documents.
- Zero filename, path, or content disclosure from the census.
- Zero cross-vault or restricted-source results in deterministic adversarial
  tests.
- Every displayed excerpt resolves to the exact current source and location.
- Every generated factual claim identifies the excerpts that support it.
- Exact and Boolean search handles concept conjunction within one provision,
  not merely across a document.
- Unsupported or stale evidence produces a refusal rather than a plausible
  clause.
- The observed corpus format coverage is high enough to define the private
  pilot without guessing.

## Explicit Non-Goals for Discovery

- General home-directory or whole-disk scanning.
- Automatic upload, sync, collaboration, or remote administration.
- Autonomous legal advice, filing, drafting, or source modification.
- Matter classification inferred solely by an LLM.
- A shared multi-user service.
- Marketing a regulatory certification.
- Replacing the current Minutes conversation product or importing legal
  material into `~/meetings`.

## First Decision After the Census

The aggregate format distribution determines the converter plan. A corpus
dominated by searchable PDF and DOCX can move directly to a small synthetic
retrieval proof. A corpus dominated by scanned PDFs, legacy Word, WordPerfect,
Outlook containers, Apple document packages, or cloud placeholders requires a
conversion and hydration proof before any retrieval UI work.
