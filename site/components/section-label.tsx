/** Section headings for marketing and docs pages.
 *
 * These render as `<h2>`, not styled text. Until 2026-08-10 the same component
 * was copy-pasted into 25 page files, all rendering the label inside a `<span>`,
 * so every page on the site exposed exactly one heading (its `<h1>`) and nothing
 * below it. Screen-reader heading navigation had nothing to navigate, and search
 * engines saw no section structure on pages whose whole job is ranking.
 *
 * Tailwind preflight strips the browser's default `h2` size and margin, so the
 * rendered appearance is unchanged from the `<span>` version.
 */

const LABEL_CLASSES =
  "font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]";

/** A titled section divider: the label, then a rule filling the remaining width. */
export function SectionLabel({ label }: { label: string }) {
  return (
    <div className="mb-6 flex items-center gap-3">
      <h2 className={LABEL_CLASSES}>{label}</h2>
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}

/** The numbered variant used by the homepage and /for-agents onramps.
 *
 * The number and the label are one heading, so assistive technology announces
 * "01 Dictation" rather than a bare ordinal followed by unrelated text.
 */
export function NumberedSectionLabel({ n, label }: { n: string; label: string }) {
  return (
    <div className="mb-8 flex items-center gap-3">
      <h2 className="flex items-center gap-3">
        <span className={LABEL_CLASSES}>{n}</span>
        <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--text-secondary)]">
          {label}
        </span>
      </h2>
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}
