# Peter private-pilot acceptance run

Peter should receive a normal signed and notarized Mac application, not a
Terminal command, development checkout, QMD setup, or generic ChatGPT upload
workflow. A release operator completes the artifact verification and security
review before this run.

## Before Peter receives the app

The release operator records the exact candidate commit and verifies the
downloaded notarized artifact with
`scripts/verify-archive-pilot-artifact.sh`. An independent reviewer approves the
security packet in `docs/security/archive-pilot-independent-review.md`. The
operator then performs the complete installed-app interaction once with
networking disabled and once with networking enabled under network
observation.

Do not use Peter's documents for those release tests. Use synthetic legal
fixtures with distinctive canary text. The release operator creates the
review folder with `scripts/make-archive-qa-fixtures.sh` and follows
`docs/release/archive-pilot-signing-and-handoff.md`.

## Peter's first session

1. Peter opens the delivered `Minutes Archive` application in Finder. macOS
   should open it normally without an unidentified-developer override.
2. He selects one small, well-understood pilot folder containing roughly
   100–500 documents. He does not need to reorganize or move them.
3. He runs **Private census**. The application reports only aggregate format,
   size, package, placeholder, permission, and error counts.
4. He saves the aggregate census report with the normal Save dialog. Before it
   is shared, the operator confirms that it contains no filename, source path,
   hash, or document text.
5. Peter reviews the supported and unsupported counts. Only then does he choose
   **Build private search index**, which is the separate authorization to read
   supported documents.
6. He tries questions he already knows how to judge, beginning with:
   “Find confidentiality provisions no more than three sentences covering
   affiliates, compelled disclosure, and survival.”
7. For every useful result, he checks the displayed source title and exact
   page, paragraph, or section anchor against the original document before
   relying on it.
8. He closes the window when finished. Closing ends the private session and
   discards the in-memory index.

## Adding the rest of the archive

After the bounded folder proves useful, Peter may add Documents, locally
available iCloud Drive folders, other cloud-sync folders, and external drives
one at a time through the native picker. An iCloud item that has not been
downloaded is counted as a placeholder; it is not silently searched.

The application reports only the locations Peter approved. It must never be
described as searching the whole computer when a folder, external volume,
cloud item, or protected location was unavailable.

## Expected pilot limitations

The initial searchable formats are searchable PDF, DOCX, TXT, TEXT, and
Markdown. Scanned PDFs are reported as requiring OCR. Legacy Word, WordPerfect,
Pages packages, PST/OLM/MSG mail containers, spreadsheets, presentations,
encrypted documents, and other unsupported formats remain coverage signals,
not searchable claims.

Search results are research assistance. They are exact retrieved excerpts, not
legal conclusions, and Peter reviews the source before use. Meaning-similar
suggestions are separately labeled and are never presented as proof that a
clause satisfies the question.

## Stop and contact the pilot operator

Peter should stop the session if macOS shows an unidentified-developer warning,
the app requests network or unrelated privacy permissions, a census export
contains a filename or path, a result lacks a source anchor, a changed source
remains available, the app claims to search an unavailable location, or the
Archive process remains running after its only window closes.

No real client document should be sent to support, copied into an email, or
uploaded to a model to diagnose the issue. Reproduce with a synthetic document
or share only the aggregate census report.
