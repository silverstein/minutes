import type { FaqItem } from "@/lib/schema";

/** Visible FAQ block, rendered from the same array that produces FAQPage markup.
 *
 * Google requires FAQPage structured data to describe content the user can
 * actually see. Rendering and markup therefore read from one array: see
 * `faqPageSchema` in `@/lib/schema`.
 *
 * Native `<details>` keeps the answers in the DOM whether or not they are
 * expanded, so crawlers and answer engines read them without running JS.
 */
export function FaqSection({ items }: { items: ReadonlyArray<FaqItem> }) {
  if (items.length === 0) return null;

  return (
    <section className="mt-14 max-w-[800px]">
      <div className="mb-6 flex items-center gap-3">
        <h2 className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Common questions
        </h2>
        <div className="h-px flex-1 bg-[var(--border)]" />
      </div>
      <div className="divide-y divide-[color:var(--border)] border-y border-[color:var(--border)]">
        {items.map((item) => (
          <details key={item.question} className="faq-disclosure group py-4">
            <summary className="flex cursor-pointer list-none items-start justify-between gap-4 text-[15px] font-medium leading-7 text-[var(--text)] marker:content-none">
              <span>{item.question}</span>
              <span
                aria-hidden="true"
                className="mt-1 shrink-0 font-mono text-[13px] text-[var(--text-secondary)] transition-transform group-open:rotate-45"
              >
                +
              </span>
            </summary>
            <div className="mt-3 pr-8 text-[15px] leading-8 text-[var(--text-secondary)]">
              {item.answer}
            </div>
          </details>
        ))}
      </div>
    </section>
  );
}
