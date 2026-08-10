/** Section headings for marketing and docs pages.
 *
 * Until 2026-08-10 this component was copy-pasted into 25 page files. Nearly
 * every copy rendered the section title inside a `<span>`, so most pages exposed
 * only their `<h1>` and no structure beneath it: screen-reader heading
 * navigation had nothing to move through, and search engines saw no sections on
 * pages whose entire job is ranking.
 *
 * Heading level is a prop rather than hard-coded, because a label is not always
 * a top-level section title. Docs pages nest group labels inside a section, and
 * the homepage pairs a numbered kicker with a real `<h2>` title underneath it.
 * Promoting either to `<h2>` would announce a section that does not exist, which
 * is the same class of error as the `<span>` it replaced, just in the opposite
 * direction.
 *
 * Tailwind preflight strips the browser's default heading size and margin, so
 * these render identically to the original `<span>` markup.
 */

const LABEL_CLASSES =
  "font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]";

/** A titled section divider: the label, then a rule filling the remaining width.
 *
 * Pass `level={3}` when the label names a group nested inside a section that
 * already has its own `<h2>`.
 */
export function SectionLabel({
  label,
  level = 2,
}: {
  label: string;
  level?: 2 | 3;
}) {
  return (
    <div className="mb-6 flex items-center gap-3">
      {level === 3 ? (
        <h3 className={LABEL_CLASSES}>{label}</h3>
      ) : (
        <h2 className={LABEL_CLASSES}>{label}</h2>
      )}
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}

/** The numbered variant used by the homepage and /for-agents onramps.
 *
 * On /for-agents the numbered label is the section's only title, so it is the
 * heading. On the homepage each one sits above a separate `<h2>`, making it a
 * kicker; pass `heading={false}` there so the page does not report two peer
 * headings per section. When it is a heading, the ordinal and label live inside
 * one element so assistive tech announces "01 Dictation" rather than a bare
 * ordinal followed by unrelated text.
 */
export function NumberedSectionLabel({
  n,
  label,
  heading = true,
}: {
  n: string;
  label: string;
  heading?: boolean;
}) {
  const parts = (
    <>
      <span className={LABEL_CLASSES}>{n}</span>
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--text-secondary)]">
        {label}
      </span>
    </>
  );

  return (
    <div className="mb-8 flex items-center gap-3">
      {heading ? <h2 className="flex items-center gap-3">{parts}</h2> : parts}
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}
