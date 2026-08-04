#!/bin/bash
set -euo pipefail

CANARY="ARCHIVE_QA_CANARY_2026_07_30_PETER_PILOT"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "Archive QA fixtures must be generated on macOS." >&2
  exit 1
fi

for command_name in textutil cupsfilter; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Required macOS command is unavailable: $command_name" >&2
    exit 1
  fi
done

if [[ $# -gt 1 ]]; then
  echo "Usage: $0 [empty-output-directory]" >&2
  exit 1
fi

if [[ $# -eq 1 ]]; then
  OUTPUT_DIR="$1"
  if [[ -L "$OUTPUT_DIR" ]]; then
    echo "Refusing a symbolic-link output directory: $OUTPUT_DIR" >&2
    exit 1
  fi
  if [[ -e "$OUTPUT_DIR" && ! -d "$OUTPUT_DIR" ]]; then
    echo "Output path exists and is not a directory: $OUTPUT_DIR" >&2
    exit 1
  fi
  mkdir -p "$OUTPUT_DIR"
else
  OUTPUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/minutes-archive-qa.XXXXXX")"
fi

if [[ -n "$(find "$OUTPUT_DIR" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "Refusing to write into a non-empty fixture directory: $OUTPUT_DIR" >&2
  exit 1
fi

cat >"$OUTPUT_DIR/01-known-confidentiality.txt" <<EOF
SYNTHETIC LEGAL FIXTURE — NOT A REAL AGREEMENT
$CANARY

Confidentiality.
Recipient may disclose Confidential Information to its affiliates and their representatives solely on a need-to-know basis, provided those persons are bound by confidentiality duties at least as protective as this Agreement. If disclosure is required by law, subpoena, or court order, Recipient will, to the extent legally permitted, give prompt notice and reasonably cooperate in seeking protective treatment. These obligations survive termination of this Agreement for five years.
EOF

cat >"$OUTPUT_DIR/02-split-concepts.md" <<EOF
# Synthetic decoy agreement

$CANARY

## Affiliates

An affiliate may receive routine operational reports.

## Compelled disclosure

A party may comply with a valid court order after giving notice where permitted.

## Survival

Payment obligations survive termination.

These concepts deliberately occur in separate provisions. This document must
not be presented as one clause satisfying a one-provision conjunction.
EOF

DOCX_SOURCE="$OUTPUT_DIR/.03-return-and-destruction-source.txt"
cat >"$DOCX_SOURCE" <<EOF
SYNTHETIC LEGAL FIXTURE — NOT A REAL AGREEMENT
$CANARY

Return and Destruction.
Within ten business days after written request, Recipient will return or destroy Confidential Information, except for one archival copy maintained solely to satisfy legal retention obligations.
EOF
textutil -convert docx \
  -output "$OUTPUT_DIR/03-return-and-destruction.docx" \
  "$DOCX_SOURCE"
rm -f "$DOCX_SOURCE"

PDF_SOURCE="$OUTPUT_DIR/.04-third-party-nda-source.txt"
cat >"$PDF_SOURCE" <<EOF
SYNTHETIC LEGAL FIXTURE — NOT A REAL AGREEMENT
$CANARY

Third-Party Nondisclosure.
The receiving party shall protect source code using at least the same degree of care it uses for its own information of similar sensitivity, and never less than reasonable care.
EOF
cupsfilter -m application/pdf "$PDF_SOURCE" \
  >"$OUTPUT_DIR/04-third-party-nda.pdf" 2>/dev/null
rm -f "$PDF_SOURCE"

cat >"$OUTPUT_DIR/05-unsupported-legacy.wpd" <<EOF
SYNTHETIC UNSUPPORTED FORMAT SIGNAL
$CANARY
This file is intentionally named as a legacy WordPerfect document. The pilot
must count it as unsupported and must not claim to search it.
EOF

cat >"$OUTPUT_DIR/06-permission-denied.txt" <<EOF
SYNTHETIC PERMISSION TEST
$CANARY
This text must not become searchable while the file has mode 000.
EOF
chmod 000 "$OUTPUT_DIR/06-permission-denied.txt"

mkdir "$OUTPUT_DIR/07-unsupported.pages"
cat >"$OUTPUT_DIR/07-unsupported.pages/package-marker.fixture" <<EOF
SYNTHETIC PACKAGE SIGNAL
$CANARY
The census should treat the Pages package as unsupported coverage, not descend
into it and index this marker.
EOF

ln -s "01-known-confidentiality.txt" \
  "$OUTPUT_DIR/08-linked-copy.txt"

cat >"$OUTPUT_DIR/README.fixture-notes" <<EOF
Minutes Archive synthetic human-QA fixture set

Canary: $CANARY

Expected cases:
- one three-sentence confidentiality provision containing affiliates,
  compelled disclosure, and survival in one provision;
- one Markdown decoy with those ideas split across separate provisions;
- one searchable DOCX;
- one searchable PDF;
- one unsupported legacy extension;
- one unreadable supported file;
- one unsupported Pages package; and
- one symbolic link that must never be followed.

This directory contains no client material. Do not replace these fixtures with
real documents when reporting a product or security defect.
EOF

ABSOLUTE_OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd -P)"
printf 'fixture_dir=%s\n' "$ABSOLUTE_OUTPUT_DIR"
printf 'canary=%s\n' "$CANARY"
printf 'archive_qa_fixtures=created\n'
