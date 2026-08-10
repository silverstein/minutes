import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";

export const metadata: Metadata = {
  title: "Minutes proof",
  description:
    "Run the Minutes demo, inspect the source meetings, and see what proof is real today versus still in progress.",
  alternates: {
    canonical: "/proof",
  },
};

const proofRows = [
  {
    label: "60-second demo",
    status: "Runnable now",
    body:
      "npx minutes-mcp --demo installs a five-meeting fixture corpus into ~/.minutes/demo/ and prints an MCP config pointed at that corpus. A new evaluator can try search and recall without donating a real meeting first.",
    href: "/for-agents#try",
    link: "Try it",
  },
  {
    label: "Agent eval v0.1",
    status: "Smoke test",
    body:
      "The current eval has 10 fictional meeting files, 20 maintainer-authored questions, a runner, and a provisional Claude-on-Claude 20/20 pre-grade. The harness runs and gives skeptics something concrete to poke at; it is not independent benchmark evidence.",
    href: "https://github.com/silverstein/minutes/blob/main/docs/eval/results-v0.1.md",
    link: "Read results",
  },
  {
    label: "Reference adapters",
    status: "Baseline examples",
    body:
      "Mem0 and Graphiti adapters show how Minutes markdown maps into external memory systems. They are intentionally small examples, not a supported SDK. Identity-aware ingestion, idempotency, and pinned adapter tests are the next v2 milestone.",
    href: "https://github.com/silverstein/minutes/tree/main/examples",
    link: "See adapters",
  },
] as const;

const sourceFiles = [
  {
    date: "2026-02-28",
    label: "monthly billing launched",
    href: "https://github.com/silverstein/minutes/blob/main/crates/mcp/fixtures/demo/2026-02-28-pricing-strategy.md",
  },
  {
    date: "2026-03-25",
    label: "monthly billing reversed",
    href: "https://github.com/silverstein/minutes/blob/main/crates/mcp/fixtures/demo/2026-03-25-pricing-reversal.md",
  },
  {
    date: "2026-04-17",
    label: "Q2 priorities locked",
    href: "https://github.com/silverstein/minutes/blob/main/crates/mcp/fixtures/demo/2026-04-17-prioritization.md",
  },
] as const;

const nextMilestones = [
  {
    title: "Eval v0.2",
    body:
      "Multi-corpus questions, blind-authored holdouts, hallucination traps, noisy transcript variants, multi-model runs, and head-to-head baselines.",
  },
  {
    title: "Adapter v2",
    body:
      "Per-attendee identity mapping, duplicate-safe manifests, exact version pins, CI dry-runs, and simpler Graphiti setup paths.",
  },
  {
    title: "Human review",
    body:
      "Independent human sign-off on eval runs before any result is treated as more than a provisional smoke test.",
  },
] as const;


export default function ProofPage() {
  return (
    <div className="mx-auto max-w-[920px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
          </a>
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
          <a href="/docs/mcp/tools" className="hover:text-[var(--accent)]">
            MCP docs
          </a>
        </div>
      </div>

      <section className="max-w-[780px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Proof
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          Run the demo. Inspect the receipts.
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          The proof is the part you can run. Seed a small meeting corpus, ask a
          question where the old answer is wrong, then open the markdown files
          behind the answer. That is the bet: your meeting memory should be
          inspectable, not just confidently summarized.
        </p>
      </section>

      <section className="mt-14">
        <SectionLabel label="Try the proof loop" />
        <div className="grid gap-5 lg:grid-cols-[1fr_1fr]">
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
            <p className="text-[15px] leading-8 text-[var(--text-secondary)]">
              First, install the bundled demo corpus. It writes five fictional
              meetings to{" "}
              <code className="font-mono text-[13px] text-[var(--text)]">
                ~/.minutes/demo/
              </code>{" "}
              and prints the MCP config for your agent host.
            </p>
            <div className="mt-4 rounded-[6px] bg-[var(--bg)] px-4 py-3 font-mono text-[13px] text-[var(--text)]">
              npx minutes-mcp --demo
            </div>
            <p className="mt-5 text-[15px] leading-8 text-[var(--text-secondary)]">
              Then ask:
            </p>
            <div className="mt-3 rounded-[6px] border border-[color:var(--border)] bg-[var(--bg)] px-4 py-3 text-[14px] leading-7 text-[var(--text)]">
              What did we decide about pricing, and which decision is current?
            </div>
          </div>
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              The answer should catch the reversal
            </p>
            <p className="mt-4 text-[15px] leading-8 text-[var(--text-secondary)]">
              The current pricing decision is annual-only. The February meeting
              tested monthly billing for three consultant signups. The March
              follow-up reversed that decision after only four signups and worse
              churn. The March file explicitly supersedes February.
            </p>
            <p className="mt-4 text-[13px] leading-6 text-[var(--text-secondary)]">
              That is the point. The agent is not hallucinating from a summary
              blob; it is walking the same receipts you can open.
            </p>
          </div>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Inspect the source" />
        <div className="grid gap-3">
          {sourceFiles.map((file) => (
            <a
              key={file.href}
              href={file.href}
              className="flex flex-col gap-2 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)] sm:flex-row sm:items-center sm:justify-between"
            >
              <div>
                <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                  {file.date}
                </p>
                <p className="mt-2 text-[15px] text-[var(--text)]">
                  {file.label}
                </p>
              </div>
              <p className="font-mono text-[12px] uppercase tracking-[0.12em] text-[var(--text-secondary)]">
                open markdown
              </p>
            </a>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Current evidence" />
        <div className="grid gap-4">
          {proofRows.map((row) => (
            <a
              key={row.label}
              href={row.href}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
            >
              <div className="flex flex-wrap items-center gap-3">
                <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                  {row.label}
                </p>
                <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--accent)]">
                  {row.status}
                </span>
              </div>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">
                {row.body}
              </p>
              <p className="mt-4 font-mono text-[12px] uppercase tracking-[0.12em] text-[var(--text)]">
                {row.link}
              </p>
            </a>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="What not to overclaim" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            v0.1 is useful, but it is not a category benchmark. The corpus,
            questions, and rubrics are maintainer-authored, and the published
            grade is same-family model pre-grading with human sign-off still
            pending.
          </p>
          <p>
            The reference adapters show the file contract works, but v2 still
            has work to do: identity mapping, idempotency, pinned dependencies,
            and CI dry-run coverage.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Next proof milestones" />
        <div className="grid gap-4 md:grid-cols-3">
          {nextMilestones.map((milestone) => (
            <div
              key={milestone.title}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)]"
            >
              <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {milestone.title}
              </p>
              <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">
                {milestone.body}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Why this matters
        </p>
        <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          Most meeting tools sell the polished summary. Minutes is about the
          layer underneath: source material your agents can read and you can
          audit. If the demo feels obvious, the product is the same loop pointed
          at your real meetings.
        </p>
        <div className="mt-5 flex flex-wrap gap-3">
          <a
            href="/for-agents#try"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            Try the demo
          </a>
          <a
            href="https://github.com/silverstein/minutes/blob/main/docs/eval/results-v0.1.md"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Audit v0.1
          </a>
        </div>
      </section>

      <PublicFooter />
    </div>
  );
}
