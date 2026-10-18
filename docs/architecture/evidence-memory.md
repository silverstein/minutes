# Evidence-backed conversation memory

This change extends the existing source-authorized architecture. Markdown remains the source of truth. Derived search results, evidence, and decision history must come from an authorized live snapshot and pass the existing publication fence. An index row or an evidence identifier is never an access capability.

## Retrieval contract

Rust and TypeScript implement `literal-and-prefix-v1`. Normalize Unicode to NFD, remove combining marks, lowercase each code point without locale context, and split on non-alphanumeric characters. All query tokens must match across title and body. Only the last token, when at least two code points long, is a prefix. Empty/punctuation-only queries do not match; the query budget is 2,048 UTF-16 code units. The shared fixture at `tests/fixtures/search-contract-v1.json` covers operators, accents, Korean, Greek, names, and the README example.

FTS operators such as AND, OR, and NOT are ordinary words. This deliberately removes Porter stemming from the Rust index: `runs` does not imply `running`. Schema version 3 rebuilds the derived index. Indexed text uses the same folded tokens as queries; returned excerpts always use original source text.

Ranking rewards title and body term frequency plus a contiguous token phrase. The score is not a probability. Ties use newest date then path. This aligns the literal query contract; it does not promise identical candidate inventories for different caller filters or Rust-only alias expansion. SDK identity selection uses exact normalized names or explicitly supplied aliases and rejects ambiguous selectors; an incidental body substring is not identity evidence.

The optional SDK rank-fusion utility accepts already-authorized candidate IDs and supplied ranked lists. It has no filesystem or provider access. It provides a bounded experiment seam for semantic retrieval; it is not an embedding model, vector index, enabled semantic search mode, or an independently measured gain.

## Evidence and temporal state

Evidence includes a source identifier, SHA-256 content revision, stable segment identifier, exact original quote, line range, and UTF-16 span. UTF-16 offsets make the same record usable by JavaScript and Rust; surrogate pairs cannot be split. Timestamps and speaker provenance are emitted when the source supports them. They do not authorize playback or establish that a transcript is acoustically correct. A revision mismatch invalidates the quote against the newer source.

Decision history is a conservative projection of structured source decisions. Only a unique explicit supersession reference with valid chronology closes an older decision. Competing successors and unlinked differing statements remain unresolved. Missing metadata/evidence and future statements remain unknown. Evidence for a structured decision points to its exact YAML source, not a claim that the decision is a verbatim spoken utterance. `current` is the state supported by the available structured records, not proof of organizational authority.

Decision IDs and effective dates are preserved through native frontmatter updates and resummarization. A missing or competing predecessor keeps its descendants unresolved. Generated ASCII slugs never join distinct labels; only exact names, declared canonical IDs and declared aliases provide identity evidence.

## Capture and artifact preservation

Audio files are owner-private at creation. Single-source capture uses a bounded preallocated queue and a writer worker so callbacks do not perform disk writes. Write errors and overflow are latched and surfaced; the single-source WAV worker drains/finalizes within a bounded wait. A timed-out worker retains its exact open file handle, and callers preserve the original path while finalization is pending. This bounds the WAV-worker wait, not all existing sidecar/screen teardown. The native dual-source mixer still writes on its owner loop. WAV finalization flushes buffers; it does not guarantee power-loss durability. The resampler uses an anti-alias filter with fixed state; 16 kHz input has a bit-exact bypass.

Artifact publication uses private exclusive staging, durable flush, and no-replace publication. Rewrites use unique staging and verify the expected original bytes under a cooperative writer lease. This prevents competing Minutes writers and detects external edits observed before publication. Portable POSIX filesystems do not provide a compare-and-swap rename against an unrelated editor: an external write in the final comparison/rename gap remains a limitation. Failed private staging artifacts can remain for recovery; they must not be treated as meetings.

Native call capture independently enforces the configured duration cap and disk floor through normal stop/finalization/processing. It warns at 90 percent and retains an auto-stop reason with the capture warnings. Missing helper telemetry is not treated as silence. Helper-stall detection remains a separate qualification.

On macOS, a recording seals its WAV before retiring its input devices. Driver teardown runs on an owned worker with a two-second wait per device. A pending worker retains the exact stream and prevents another in-process capture until it retires. The recording reports pending device shutdown; it does not claim the OS driver has finished stopping. This boundary applies to single-source input and dual-source stream disposal.

## Summary input coverage

Summary processing partitions the whole input into bounded chunks with separate transcript and notes budgets. Bounded subprocess input/output, call counts, and deadlines prevent runaway work. Failure or incomplete coverage cannot silently publish a complete result. Coverage metadata is carried into enrichment and resummarization. Input coverage means bytes were submitted to successful model calls; it does not establish factual accuracy, entailment, or perfect retention by the model. Enrichment checks the expected source before replacing it.

## Desktop boundary and qualification

Recall's streaming controller is extracted from the large page behind a generated runtime/TypeScript contract. Tests render the production DOM, CSS, controller and Markdown renderer with stubbed IPC. They exercise streaming, errors, reset/stop races, unsafe input and responsive layouts. They do not exercise Tauri, macOS permissions, real audio, or OS text insertion. The existing signed development-app acceptance gate still applies. Backend events do not yet carry a turn ID, so a late event from an earlier backend turn cannot always be identified by the client.

See `tests/eval/v02/README.md` for evaluation commands and their limits.
