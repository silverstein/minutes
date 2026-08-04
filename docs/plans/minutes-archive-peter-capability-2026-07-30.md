# Minutes Archive for Peter: Ideal Legal Capability

Status: product and implementation plan

Date: 2026-07-30

Source baseline: `origin/main` at
`6ac1c4f18361343ff088b4f5fd45cf60dce092a6`

Companion security contract:
[`minutes-archive-discovery-2026-07-30.md`](minutes-archive-discovery-2026-07-30.md)

## Product Decision

Build a private clause-and-precedent workbench for Peter, not a generic chat
window over his hard drive.

Its promise is:

> Ask across decades of work product, get the exact useful language, and see
> where every answer came from without sending the archive away.

The evidence card is the primary product object. Natural language is the query
interface. Generated prose is a secondary, constrained view over exact
evidence.

The first distribution is a separate signed and notarized
`Minutes Archive.app`. It shares narrowly scoped security and indexing
libraries with Minutes but has an independent bundle identity, data directory,
Tauri command manifest, capability profile, release channel, and product name.

## Peter's Three Jobs

### Find language

Examples:

- Find a confidentiality provision no longer than three sentences.
- Find language covering affiliates, compelled disclosure, and survival.
- Show indemnity provisions where defense control remains with the indemnifying
  party.
- Find the shortest limitation-of-liability carve-out for confidentiality.

The system returns exact clause text and proves that all requested concepts
occur in the same provision. It does not return a paragraph assembled from
different documents and present it as a clause.

### Find the right document

Examples:

- Find agreements containing confidentiality, a two-year survival period, and
  New York governing law.
- Show executed consulting agreements for healthcare clients that contain a
  BAA reference.
- Find the document that includes this remembered phrase plus an assignment
  restriction and a change-of-control provision.

The system may match requirements across a document, but the UI distinguishes
document-level conjunction from same-clause conjunction.

### Compare precedent

Examples:

- Compare the five most similar confidentiality clauses.
- What changed between these two versions?
- Which precedent is most protective of the disclosing party, and what exact
  language creates that difference?

The comparison is a table of cited dimensions before it is a generated
summary. Peter can open every source and inspect the exact provision.

## First-Run Experience

Peter should not need Terminal, Python, a development environment, or an
account.

```text
Welcome to Minutes Archive

Find useful language across your private work product.
Your documents remain on this Mac.

[Choose archive locations]
```

The location screen suggests common sources that exist on his Mac:

```text
Archive locations

[Add Documents]
[Add Desktop]
[Add iCloud Drive]
[Add Dropbox or OneDrive]
[Add external drive]
[Choose another folder]
```

Every location uses the macOS folder picker. Nothing is scanned until Peter
approves it. The app does not request Full Disk Access during the ordinary
pilot.

Multiple locations form one vault, but each remains a separate authorization
root. Overlapping roots are rejected or deduplicated by canonical object
identity. Symlinks are never followed to expand scope.

## Census Experience

The first operation is metadata-only:

```text
Private archive census

This step reads file metadata only.
It does not open documents or create a search index.

Documents             Ready
iCloud Drive          1,240 cloud-only items
External drive        Disconnected

[Run census]
```

The result reports:

- counts and bytes by supported format;
- searchable versus likely OCR-required material;
- Apple document packages;
- legacy Word and WordPerfect;
- Outlook or mail containers;
- encrypted or inaccessible items;
- cloud-only placeholders;
- duplicate or overlapping locations; and
- files that require a separate conversion decision.

It emits no filenames, paths, or document text. Peter can export the aggregate
report for us without exposing client information.

Cloud-only items are not silently downloaded. The app reports them and offers a
separate, explicit hydration step only when the retrieval pilot requires their
contents.

## Main Workspace

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Search your archive                                                  │
│ [ Find confidentiality provisions under three sentences that ... ]  │
│                                                                      │
│ Scope: All approved locations   Matter: Any   Type: Clause           │
├──────────────────────────────────────────────────────────────────────┤
│ 18 provisions                                                        │
│                                                                      │
│ Confidentiality · 3 sentences                              96% match │
│ Covers: ✓ affiliates  ✓ compelled disclosure  ✓ survival            │
│ “Recipient shall…”                                                   │
│ Agreement title · §7 · page 12                                      │
│ [Open original] [Compare] [Copy with citation]                       │
│                                                                      │
│ Confidentiality · 2 sentences                              91% match │
│ Covers: ✓ affiliates  ✓ compelled disclosure  — survival            │
│ …                                                                    │
└──────────────────────────────────────────────────────────────────────┘
```

The search box recognizes a small set of legal retrieval constraints:

- same provision versus anywhere in document;
- sentence or word limit;
- must include, should include, and must exclude concepts;
- document type;
- date range;
- executed, draft, or unknown status when evidenced;
- governing law when evidenced;
- selected location or matter; and
- exact remembered language.

Every interpreted constraint remains visible as an editable chip. Peter never
has to guess how the application understood his question.

## Evidence Card Contract

Every result card contains:

- exact source excerpt;
- document title derived locally;
- provision heading and numbering where present;
- page for fixed-layout documents, or stable paragraph/section anchor for
  reflowable documents;
- source revision and converter version;
- match explanation tied to the visible constraints;
- index freshness state;
- Open Original; and
- Copy With Citation.

The card never claims a page number for DOCX or another reflowable source unless
the converter has a stable rendered-page artifact. A paragraph anchor is more
honest than a fabricated page.

An evidence card becomes unavailable if the source has moved, changed, become
inaccessible, or failed policy revalidation. Stale text is not shown with a
small warning; it is withdrawn and reprocessed.

## Retrieval Architecture

The retrieval pipeline is legal-structure-aware:

```text
authorized source
  -> bounded converter
  -> normalized document with source anchors
  -> provision segmentation
  -> exact/Boolean candidate retrieval
  -> optional semantic candidate retrieval
  -> fusion and local reranking
  -> structural constraint verifier
  -> evidence cards
  -> optional evidence-constrained synthesis
```

### Normalization

The normalizer preserves:

- headings and section numbers;
- paragraphs, lists, tables, and signature boundaries;
- defined-term capitalization;
- sentence boundaries;
- page coordinates when the source has stable pages; and
- source byte hash, revision, converter, and warnings.

Documents are data, never agent instructions. Macros, embedded executables,
remote templates, external links, and active content do not execute during
conversion.

### Provision segmentation

Retrieval units are provisions, not arbitrary equal-token chunks. A unit
includes its heading, clause body, parent section context, definitions needed
to understand it, and exact source anchors.

Large schedules and exhibits remain distinct. Defined terms can be linked for
search without silently rewriting the quoted clause.

### Candidate retrieval

The first production-capable path is:

- exact phrase and Boolean search;
- SQLite FTS/BM25;
- document metadata filters; and
- deterministic same-clause and sentence-count checks.

This provides useful legal retrieval before introducing embeddings.

The SOTA path adds:

- local embeddings over provision units;
- query expansion into legal concepts and lexical alternatives;
- reciprocal-rank fusion of lexical and vector candidates;
- a local cross-encoder or constrained reranker; and
- deterministic verification of visible structural constraints after
  reranking.

QMD is a useful reference implementation for expansion, vector retrieval,
fusion, and reranking. Minutes main intentionally disables QMD's persistent
global collections. Attorney data must never enter that registry.

A future QMD-backed path must use an app-owned vault database or isolated local
sidecar and must first prove revocation, corpus fencing, derivative protection,
and exact vault scoping. If it cannot, the application keeps semantic search
disabled and retains exact/FTS retrieval.

### Answer generation

The local model receives only the bounded evidence selected for the current
answer, never the vault root or an unrestricted search tool.

It may:

- summarize differences among cited provisions;
- produce a comparison table;
- explain why results satisfied visible constraints; and
- suggest a narrower follow-up query.

It may not:

- state that a clause is legally sufficient;
- invent missing terms;
- combine fragments into a purported source clause;
- treat an old precedent as current law;
- modify an original; or
- hide an unsupported answer behind fluent prose.

Every visible factual claim closes over exact evidence IDs. If the evidence is
insufficient, the product says so and shows the closest cards.

## Format Plan

The census determines priority, but the intended support tiers are:

| Tier | Formats | Approach |
| --- | --- | --- |
| A | searchable PDF, DOCX, DOC, RTF, TXT, HTML, EML | direct bounded normalization |
| B | scanned PDF, TIFF, JPEG, PNG | local Apple Vision OCR plus page coordinates |
| C | Pages packages, WordPerfect, PST/OLM/MSG, encrypted documents | explicit converter or export workflow |
| D | Time Machine, Apple Mail database, another user's home | not scanned by default; separate reviewed connector |

An unsupported document stays visible as an aggregate count and conversion
status. It is never silently omitted from a claim that the archive has been
searched.

## Matter and Access Model

Peter may begin with one private vault, but the data model must not hard-code
one global authorization scope.

Each source belongs to:

- a vault;
- one authorization root;
- zero or one explicit matter selected or imported from trusted metadata;
- a sensitivity state;
- a source revision; and
- a derivative lifecycle.

Matter assignment is manual or rule-based from trusted paths and metadata.
An LLM suggestion never grants or broadens access.

Every retrieval requires an explicit vault capability. Matter filters are
applied before candidate retrieval. An empty or invalid scope denies the query
rather than searching all documents.

The unlanded Sidekick `GroundingScope` work supplies an important future
pattern: inferred identity cannot establish authority, confirmed participation
does not unlock restricted history, and a scope change discards previously
assembled context. Those ideas should be reconciled into `archive-core`, not
copied blindly from its divergent branch.

## Security Reuse From Shipped Minutes Main

Current main now contains security primitives worth extracting rather than
recreating:

- `policy_fs`: canonical, bounded, fail-closed filesystem policy;
- MCP/SDK `secure-read`: descriptor-bound reads with stable identity and byte
  revision;
- MCP/SDK `corpus-lease`: bounded membership snapshots, watcher coverage,
  resource budgets, and final authorization fences;
- restricted-conversation egress enforcement;
- private filesystem and process-tree enforcement across macOS, Linux, and
  Windows; and
- explicit retirement of persistent global QMD state.

Archive should reuse their invariants through small shared libraries. It should
not inherit Minutes' full meeting command surface or broad desktop capability
file.

The active Silvercloud Privacy-B lane is now designing secure Apple Speech byte
transport. Its relevant general lesson is that a sandboxed broker must not
execute arbitrary third-party helpers and confidential bytes must not be
materialized as a named plaintext temporary file. Legal converters should use
the same narrow-worker principle, even though they process documents rather
than audio.

## Tauri Capability Profile

The Peter app has one main window and a minimal backend surface:

- choose and revoke an authorization root;
- run or cancel a metadata census;
- create, inspect, rebuild, or delete a vault;
- run a scoped query;
- retrieve one currently authorized evidence card;
- open one currently authorized original; and
- export an aggregate report or explicitly selected excerpt.

The webview gets no general filesystem, shell, PTY, opener, network, autostart,
global-shortcut, updater, or arbitrary command access.

Production uses a restrictive CSP, no remote frontend content, no devtools,
private logs, and a separately reviewed signing/update path.

## Local and Cloud Boundary

The Peter pilot is local-only:

- census, conversion, OCR, retrieval, reranking, and answer generation run on
  the Mac;
- model endpoints must resolve to verified loopback;
- telemetry contains no archive metadata or content;
- model downloads occur during setup, before a confidential operation starts;
  and
- the app remains useful with networking disabled.

Future cloud use is a different product mode, not a hidden fallback. It would
require provider-named disclosure, contractual review, client and matter
policy, explicit selection of what leaves the device, retention controls, and
an auditable receipt. General boilerplate consent is not the control.

## Pilot Sequence

### Gate 1: Signed census app

Deliver a one-screen signed and notarized app with multi-root selection,
metadata-only scanning, overlap detection, cloud-placeholder reporting,
progress, cancellation, and aggregate export.

No ingestion or search occurs.

Implementation status: the separate app target, multi-root capability-bound
Rust scanner, cancellation, aggregate UI/export, restrictive CSP, minimal
Tauri capability file, synthetic fixture, and privacy tests are implemented.
The local bundle builds, installs, launches, and verifies with an ad-hoc seal.
Developer ID signing, notarization, and the full native click test remain
release gates; an ad-hoc development bundle is not a Peter handoff.

### Gate 2: Synthetic legal benchmark

Build a public and synthetic corpus covering confidentiality, indemnity,
limitation of liability, assignment, governing law, and BAA language.

Implementation status: the first retrieval slice is implemented in
`minutes-archive-core` as a private in-memory SQLite FTS5 index. It normalizes
bounded UTF-8 fixtures into legal provisions, preserves headings and stable
section anchors, requires an explicit vault, checks current source revisions
at query time, and returns exact evidence cards. Deterministic tests cover
same-clause conjunction, sentence limits, exact phrases, exclusions, source
replacement and withdrawal, malicious prompt-like source text, and wrong or
empty scope. A committed synthetic benchmark now covers confidentiality,
indemnity and defense control, limitation of liability, assignment, governing
law, BAA language, remembered phrases, and prompt-like source text.
Document-level conjunction is a separate result type with criterion evidence;
criteria are grouped inside one document and exclusions apply across its
provisions. Candidate-budget overflow fails closed rather than returning an
apparently complete answer. The proof does not yet persist a protected vault,
open an original in its native application, convert legacy Word or email
containers, or implement OCR; those remain Gate 2 work rather than implied
capability.

The desktop app now exposes this proof only after a completed census and an
explicit content-access action. It ingests bounded searchable `.pdf`, `.docx`,
`.txt`, `.text`, and `.md` files through the retained folder capabilities into
an in-memory index. PDF and DOCX bytes are sent through pipes to a bounded,
network-denied worker; source paths are never passed to the parser. PDF results
carry page anchors and DOCX results carry paragraph anchors plus the converter
version. Before an evidence card is returned, the app revalidates the approved
root, relative membership without links, file identity, current bytes, and
SHA-256 revision. Moved, replaced, mutated, or inaccessible sources are
withdrawn. The browser-facing UI receives document titles and exact evidence
only for current matches; it never receives source paths or general filesystem
access.

The app now includes a deliberately narrower semantic experiment alongside
exact search. Apple's built-in English sentence embedding is pinned to revision
1; the code calls no model asset-request or download API. Provision vectors
are bounded, vault-scoped, held only in memory, removed with their document,
and subjected to the same current-source fence before display. Semantic
results appear in a separate “meaning-similar suggestion” section and state
that they are not determinations of legal sufficiency. They are not fused into
or counted as deterministic constraint matches. QMD, downloaded GGUF models,
cross-encoder reranking, and answer generation remain disabled.

Both indexing-time and query-time embeddings now run in a separate persistent
worker. Before any model construction or confidential input, the worker
installs resource ceilings and a macOS sandbox that denies network access and
reads or writes under user, volume, and network roots. The parent passes only
bounded text through length-framed pipes, never a source path. Binding requires
an immutable private executable snapshot plus a startup self-test proving
localhost binding and `/etc/passwd` reads are denied.

Evaluate:

- exact phrase;
- same-clause concept conjunction;
- document-level conjunction;
- sentence limits;
- exclusion constraints;
- scanned-page OCR;
- source mutation;
- malicious prompt-like document text;
- unsupported formats; and
- wrong-vault and empty-scope denial.

### Gate 3: Bounded Peter subset

Peter chooses one clearly understood folder or a copy containing approximately
100 to 500 documents. The app shows format and conversion coverage before
indexing.

Peter supplies roughly 20 real questions and identifies useful results. He does
not need to create a formal relevance-judgment spreadsheet; the UI captures
useful, wrong result, and missing result locally.

Exact/FTS retrieval remains authoritative. The built-in semantic suggestion
lane is an independent, reversible experiment with no durable vectors.

### Gate 4: Multi-location vault

After the bounded subset proves usefulness and source fidelity, Peter adds
Documents, iCloud Drive, cloud-sync folders, and external drives individually.

The app reports unavailable volumes, cloud-only items, stale indexes, and
coverage gaps. It never claims that the whole computer was searched when a
location was missing or protected.

### Gate 5: Reusable regulated-memory platform

Only after Peter's proof should shared `archive-core` capabilities be framed as
a broader regulated-memory platform. Legal-specific segmentation and result
language remain a domain adapter over generic source authorization,
provenance, derivative lifecycle, and controlled egress.

## Acceptance Contract

The Peter capability is ready for a private pilot only when:

- source files remain byte-identical;
- census output contains no names, paths, or content;
- every result resolves to the exact current source and anchor;
- same-clause requirements are verified within one provision;
- unsupported and cloud-only material is counted honestly;
- source mutation or policy uncertainty withdraws stale evidence;
- empty, wrong-vault, and wrong-matter scopes return no content;
- adversarial document instructions cannot alter orchestration;
- all generated factual claims carry exact evidence IDs;
- local-only operation is verified with networking disabled;
- the signed app passes native macOS interaction and permission testing; and
- an independent security review finds no unresolved high-impact issue.

## What Peter Does

Peter's entire initial contribution is:

1. install a normal Mac app;
2. approve the locations he wants counted;
3. share the aggregate census;
4. choose one bounded pilot folder; and
5. try approximately 20 questions he already cares about.

He does not install developer tools, configure QMD, write prompts for a generic
agent, classify thousands of documents, or move his archive into Minutes.

## Current Build Decision

The multi-root census and the bounded searchable PDF, DOCX, and text evidence
UI now exist in the separate app. Exact retrieval and separately labeled,
revision-pinned on-device semantic suggestions both remain ephemeral.
Live-source resolution and revocation are green in deterministic and
installed-executable synthetic coverage. The installed ad-hoc build also
passes its bundle seal, both converter and semantic worker self-tests, a
three-format end-to-end search and mutation-withdrawal test, and launches with
no open network socket. Native interaction testing is still outstanding.

Next, use the aggregate census to determine whether local OCR, legacy Word,
WordPerfect, email containers, Apple packages, or cloud hydration is the next
coverage constraint. Do not persist source text or vectors, add downloaded
models, model generation, or an Open Original action until their separate
protection and final-fence contracts are proven. Semantic model execution is
now formally network-denied; an end-to-end human test with networking disabled,
Developer ID signing, notarization, and native permission and interaction
testing remain Peter handoff gates.
