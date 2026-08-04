# Minutes Archive pre-review findings log

Status: **NOT AN INDEPENDENT REVIEW**

This is an author-run adversarial pass, written by the implementer. It is input
to the independent review, not a substitute for it and not approval. The
reviewer-owned record is `archive-pilot-review-record-template.md`, and its
`NOT REVIEWED` state is untouched.

Two adversarial agents were run against separate dimensions with instructions
to verify by execution and to use synthetic fixtures only. They are not
independent in the sense the packet requires: they were commissioned by the
author, on the author's code, under the author's framing. Their value is that
they executed attacks and reported measurements, not that they are impartial.

The purpose of this log is so the independent reviewer knows what has already
been probed, what was found, what was changed in response, and — most usefully
— what was **not** reached.

## Findings and disposition

| ID | Severity | Finding | Disposition |
| --- | --- | --- | --- |
| E1 | High | `NSOpenPanel` wrote the approved archive path into `~/Library/Preferences/<bundle-id>.plist` as an `NSOSPLastRootDirectory` bookmark, surviving application exit. Decoding it recovered every path component, the volume name, and the volume UUID. That directory carries no TCC protection. | Fixed in `682a947c`. Keys removed via CFPreferences after each panel closes and again in `purge_session`. Verified end to end against the real panel. |
| I1 | Medium | Hard links escaped the approved root. `is_symlink` is false for a hard link, so a root whose only entry was a hard link to an outside file indexed it and returned it as an evidence card titled as though local. | Fixed in `682a947c`. Multiply-linked regular files are refused and counted as `hard_links_skipped`. Mutation-checked. |
| E2 | Low | A SHA-256 of every matched document's full bytes, plus byte length, ids, and rank, crossed the IPC boundary unrendered. | Fixed in `07bf42c0`. The command returns a projection of the eight fields the interface reads. |
| I2 | Low | The semantic worker's `mach-lookup` denylist named only the legacy ASL service, leaving `logd`, `diagnosticd` and `launchservicesd` reachable. No leak was observed. | Fixed in `07bf42c0`. |
| I3 | Low | The converter's sandbox self-test probed only `TcpListener::bind` and a read of `/etc/passwd`, so a regression to `(allow default)` plus one deny would have passed it. | Fixed in `07bf42c0`, matching the hardening the semantic worker already had. |

## What was probed and held

Verified by execution, reported here so the reviewer can decide whether to
re-run or spend effort elsewhere:

- **Census export privacy.** A 33-artifact adversarial corpus — surnames as
  extensions, multi-dot and trailing-dot names, matter numbers, Cyrillic, an
  RTL-override name, a 216-character surname, packages, hidden files. Zero of
  68 name-derived fragments appeared in the export. Every string value was one
  of 24 fixed values.
- **Canary survival.** A full session over a canary corpus left the canary in
  zero locations across temp dirs, `~/Library` (Application Support, Caches,
  Saved Application State, Preferences, WebKit, Logs, Containers, HTTPStorages,
  Cookies), `~/.minutes`, the app bundle, and crash reports.
- **Logs.** Unified logging during the run and a 12-minute retrospective query
  on the canary and every fixture surname returned nothing.
- **Network.** Zero inet sockets on the app, its WebKit networking helper, and
  both workers, sampled throughout.
- **Authority boundary.** The firmlink bypass is closed by dev/ino identity:
  `/System/Volumes/Data/Users/<user>` is refused, and firmlinked parent/child
  pairs are caught as duplicate or overlapping. APFS case-insensitivity,
  Unicode NFC/NFD, `..` traversal, symlink roots, and TOCTOU replacement all
  fail closed.
- **Hostile documents.** 18 synthetic fixtures — zip bombs, XXE, zip-slip,
  malformed xref, encrypted PDFs, recursive object graphs, 60k-page PDFs,
  `/Launch` and `/JavaScript` actions. Nothing hung, escaped, or wrote outside
  its temp dir. `RLIMIT_AS` and `RLIMIT_CPU` were measured binding before the
  decoder reads attacker bytes.
- **Worker isolation.** Both seatbelt profiles applied verbatim in a standalone
  probe. Only bytes cross into the converter: a live worker showed an empty
  environment, cwd `/`, and only stdin/stdout/stderr plus its own binary.
- **Lifecycle.** 300 sequential conversions mixing clean and aborting
  documents: constant fd count, zero zombies, zero strays.

## Residual risks the reviewer should weigh

1. **`SIGKILL` still leaves the panel preference key.** The fix removes it
   after each panel closes and at graceful exit. A `SIGKILL` runs neither. The
   key is also written while the app is running — AppKit records it after the
   modal returns, so the post-panel call cannot win that race. What is
   guaranteed is that it does not survive a graceful exit.
2. **The app ships with no entitlements, so there is no App Sandbox.** The
   parent process is not OS-prevented from networking; the guarantee is code
   discipline plus CSP, not a kernel control. The two workers *are* sandboxed.
3. **`mobileassetd` is reachable from the semantic worker.** `(deny network*)`
   binds the worker, not `mobileassetd`, which acts on its behalf and has full
   network access. On a Mac where the linguistic asset is already installed
   nothing is fetched; on one lacking it, the behaviour is the OS's to decide.
   The app's own call graph uses only `NLEmbedding::sentenceEmbeddingForLanguage_revision`.
4. **`built_in_os_asset` and `model_download_requested` in the build report are
   compile-time literals**, presented alongside measured fields. They should be
   renamed or measured.
5. **Broad-but-not-home roots remain approvable** — `/Volumes`, `/private/var`,
   `/Library`, `/Applications`, `/dev`, `/.vol`. Each requires an explicit
   picker choice, and the stated claim is about home and filesystem roots, but
   the refusal reads as more complete than it is.
6. **Known retrieval limitation**, disclosed separately in the review packet: a
   same-provision conjunction can span two adjacent clauses in a PDF that
   reports no structure. See `archive-pilot-independent-review.md`.

## Not reached

- The index and search legs **inside the real signed app** were not driven
  end to end by the egress reviewer; identical code and workers were exercised
  through the harness, and the human GUI pass covered the flow on the dev
  build. Whether `WKWebView` persists rendered excerpt text was not directly
  proven, though LocalStorage and IndexedDB were empty and no storage API is
  used.
- Whether the **export save panel** adds a second path record was not tested.
  The same fix now covers it, but the verification was done on the open panel.
- Racing TOCTOU under contention — deterministic swaps all failed closed.
- Non-APFS and case-sensitive volumes.
- Everything about the **notarized artifact**: provenance, Developer ID
  signature, staple, Gatekeeper. No notarized artifact exists yet.

## Performance observations, not security

- Roughly 0.48 s of process-spawn overhead per document. For a 30-year archive
  that is hours of pure overhead.
- Semantic embedding runs about 288 provisions/second; at
  `MAX_SEMANTIC_PROVISIONS` that is ~347 CPU-seconds against a 600-second
  worker budget — under 2× headroom before `SIGXCPU`.
