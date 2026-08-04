#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/minutes-archive-fixture-test.XXXXXX")"
FIXTURE_DIR="$TEST_ROOT/fixtures"

cleanup() {
  chmod -R u+rwX "$TEST_ROOT" 2>/dev/null || true
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

"$REPO_ROOT/scripts/make-archive-qa-fixtures.sh" "$FIXTURE_DIR" \
  >"$TEST_ROOT/generator.out"

grep -Fxq "archive_qa_fixtures=created" "$TEST_ROOT/generator.out"
grep -Fq "ARCHIVE_QA_CANARY_2026_07_30_PETER_PILOT" \
  "$FIXTURE_DIR/01-known-confidentiality.txt"
grep -Fq "separate provisions" "$FIXTURE_DIR/02-split-concepts.md"
unzip -tqq "$FIXTURE_DIR/03-return-and-destruction.docx"
test "$(file -b --mime-type "$FIXTURE_DIR/04-third-party-nda.pdf")" \
  = "application/pdf"
test "$(stat -f '%Lp' "$FIXTURE_DIR/06-permission-denied.txt")" = "0"
test -d "$FIXTURE_DIR/07-unsupported.pages"
test -L "$FIXTURE_DIR/08-linked-copy.txt"
test "$(readlink "$FIXTURE_DIR/08-linked-copy.txt")" \
  = "01-known-confidentiality.txt"

if "$REPO_ROOT/scripts/make-archive-qa-fixtures.sh" "$FIXTURE_DIR" \
  >"$TEST_ROOT/refusal.out" 2>&1; then
  echo "Fixture generator unexpectedly accepted a non-empty directory." >&2
  exit 1
fi
grep -Fq "Refusing to write into a non-empty fixture directory" \
  "$TEST_ROOT/refusal.out"

echo "archive_qa_fixture_generator=passed"
