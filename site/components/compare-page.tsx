type ComparisonRow = {
  label: string;
  competitor: string;
  minutes: string;
};

type SourceLink = {
  label: string;
  href: string;
};

type ComparePageProps = {
  competitorName: string;
  competitorLabel: string;
  markdownHref: string;
  heroSummary: string;
  quickVerdictCompetitor: string;
  quickVerdictMinutes: string;
  comparisonRows: ComparisonRow[];
  competitorWins: string[];
  minutesWins: string[];
  workflowSection: string[];
  chooseSection: string[];
  notRightFitSection: string[];
  evaluatedSection: string[];
  sources: SourceLink[];
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

export function ComparePage({
  competitorName,
  competitorLabel,
  markdownHref,
  heroSummary,
  quickVerdictCompetitor,
  quickVerdictMinutes,
  comparisonRows,
  competitorWins,
  minutesWins,
  workflowSection,
  chooseSection,
  notRightFitSection,
  evaluatedSection,
  sources,
}: ComparePageProps) {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href={markdownHref} className="hover:text-[var(--accent)]">
            page.md
          </a>
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
          </a>
          <a href="/docs/mcp/tools" className="hover:text-[var(--accent)]">
            MCP docs
          </a>
        </div>
      </div>

      <section className="max-w-[780px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Comparison
        </p>
        <h1 className="mt-4 font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          Minutes vs {competitorLabel}
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          {heroSummary}
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-04-09
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Fit-based comparison
          </span>
        </div>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Quick verdict
        </p>
        <div className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Choose <span className="font-medium text-[var(--text)]">{competitorName}</span> if{" "}
            {quickVerdictCompetitor}
          </p>
          <p>
            Choose <span className="font-medium text-[var(--text)]">Minutes</span> if{" "}
            {quickVerdictMinutes}
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="At A Glance" />
        <div className="overflow-x-auto rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] shadow-[var(--shadow-panel)]">
          <table className="min-w-full border-collapse text-left">
            <thead>
              <tr className="border-b border-[color:var(--border)]">
                <th className="px-4 py-3 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  Category
                </th>
                <th className="px-4 py-3 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  {competitorName}
                </th>
                <th className="px-4 py-3 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  Minutes
                </th>
              </tr>
            </thead>
            <tbody>
              {comparisonRows.map((row) => (
                <tr key={row.label} className="border-b border-[color:var(--border)] last:border-b-0">
                  <td className="px-4 py-4 align-top text-[14px] font-medium text-[var(--text)]">
                    {row.label}
                  </td>
                  <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text-secondary)]">
                    {row.competitor}
                  </td>
                  <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text-secondary)]">
                    {row.minutes}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="mt-14 grid gap-6 lg:grid-cols-2">
        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Where {competitorName} wins
          </p>
          <ul className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
            {competitorWins.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>

        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Where Minutes wins
          </p>
          <ul className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
            {minutesWins.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Workflows" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          {workflowSection.map((paragraph) => (
            <p key={paragraph}>{paragraph}</p>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Which Should You Pick?" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          {chooseSection.map((paragraph) => (
            <p key={paragraph}>{paragraph}</p>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="When Minutes Is Not The Right Fit" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          {notRightFitSection.map((paragraph) => (
            <p key={paragraph}>{paragraph}</p>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="How We Evaluated" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          {evaluatedSection.map((paragraph) => (
            <p key={paragraph}>{paragraph}</p>
          ))}
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="/for-agents"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            See agent setup
          </a>
          <a
            href="/docs/mcp/tools"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Browse MCP docs
          </a>
          <a
            href="/compare"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            All comparisons
          </a>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Sources" />
        <ul className="space-y-2 text-[14px] leading-7 text-[var(--text-secondary)]">
          {sources.map((source) => (
            <li key={source.href}>
              <a href={source.href} className="text-[var(--accent)] hover:underline">
                {source.label}
              </a>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
