import type { Metadata } from "next";
import data from "./data.json";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Minutes error reference",
  description:
    "Generated reference for stable Minutes core error messages and identifiers.",
  alternates: {
    canonical: "/docs/errors",
  },
};

function SectionLabel({ label }: { label: string }) {
  return (
    <div className="mb-6 flex items-center gap-3">
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
        {label}
      </span>
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}

function LinkPill({ href }: { href: string }) {
  return (
    <a
      href={href}
      className="inline-flex items-center rounded-full bg-[var(--bg)] px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--accent)] hover:bg-[var(--bg-hover)]"
    >
      link
    </a>
  );
}

export default function ErrorsPage() {
  return (
    <div className="mx-auto max-w-[920px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/docs/errors.md" className="hover:text-[var(--accent)]">
            errors.md
          </a>
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
          </a>
        </div>
      </div>

      <section className="max-w-[760px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Generated Reference
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          Minutes error reference
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Generated catalog of stable Minutes core errors. It stays source-backed, but now favors
          actionable user-facing errors over generic wrapper variants so the page is easier to use
          in real troubleshooting.
        </p>
      </section>

      <section className="mt-14">
        <SectionLabel label={`Errors (${data.visibleCount})`} />
        <div className="mb-8 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)]">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Catalog notes
          </p>
          <div className="mt-3 space-y-2 text-[14px] leading-7 text-[var(--text-secondary)]">
            <p>Visible actionable errors: {data.visibleCount}</p>
            <p>Hidden low-signal wrappers: {data.hiddenCount}</p>
          </div>
        </div>

        <div className="space-y-10">
          {data.groups.map((group) => (
            <div key={group.enumName}>
              <SectionLabel label={`${group.enumName} (${group.count})`} />
              <div className="grid gap-4">
                {group.entries.map((entry) => (
                  <div
                    key={entry.anchorId}
                    id={entry.anchorId}
                    className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <p className="font-mono text-[13px] text-[var(--text)]">
                        {entry.enumName}::{entry.variant}
                      </p>
                      <LinkPill href={entry.docsUrl} />
                    </div>
                    <p className="mt-3 whitespace-pre-wrap text-[14px] leading-7 text-[var(--text-secondary)]">
                      {entry.message}
                    </p>
                    <div className="mt-4 space-y-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                      <div>Source: {entry.sourceFile}</div>
                      {entry.cfg ? <div>Platform: {entry.cfg}</div> : null}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </section>

      <PublicFooter />
    </div>
  );
}
