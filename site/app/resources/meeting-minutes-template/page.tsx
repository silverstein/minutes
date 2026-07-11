import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Meeting minutes templates (markdown, copy-paste ready)",
  description:
    "Four meeting minutes templates in clean markdown — standard team meeting, board meeting, action-item-focused, and 1:1 — plus what actually belongs in minutes and what doesn't.",
  alternates: {
    canonical: "/resources/meeting-minutes-template",
  },
};

const templates = [
  {
    title: "Standard team meeting",
    body: `# Team Meeting — {date}

**Attendees:** {names}
**Facilitator:** {name}

## Agenda
1. {topic}
2. {topic}

## Discussion
- {topic}: {key points, disagreements, context}

## Decisions
- {decision} — decided by {who}, because {why}

## Action items
- [ ] {task} — @{owner}, due {date}

## Parking lot
- {deferred topic}`,
  },
  {
    title: "Board / formal meeting",
    body: `# {Organization} Board Meeting Minutes

**Date/Time:** {date, start–end}
**Location:** {place / video}
**Present:** {names, roles}
**Absent:** {names}
**Quorum:** {yes/no}

## Call to order
Called to order at {time} by {chair}.

## Approval of prior minutes
Minutes of {date} were {approved / amended}.

## Reports
- {Officer/Committee}: {summary}

## Motions
- MOTION: {text}. Moved {name}, seconded {name}.
  Vote: {for}–{against}–{abstain}. {Carried/Failed}.

## Adjournment
Adjourned at {time}. Next meeting: {date}.

Respectfully submitted, {secretary}`,
  },
  {
    title: "Action-item-focused (standup / working session)",
    body: `# {Project} Working Session — {date}

## What changed since last time
- {update}

## Blockers
- {blocker} — needs {who/what}

## Action items
- [ ] {task} — @{owner}, due {date}
- [ ] {task} — @{owner}, due {date}

## Next checkpoint
{date} — success looks like: {criteria}`,
  },
  {
    title: "1:1 meeting",
    body: `# 1:1 — {name} & {name}, {date}

## Their agenda
- {topic}

## My agenda
- {topic}

## Notes
- {what was actually said}

## Commitments
- [ ] {mine} — due {date}
- [ ] {theirs} — due {date}

## Follow up next time
- {thread to pull}`,
  },
] as const;

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

export default function MeetingMinutesTemplatePage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/resources/meeting-minutes-template.md" className="hover:text-[var(--accent)]">
            page.md
          </a>
          <a href="/security" className="hover:text-[var(--accent)]">
            security
          </a>
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
        </div>
      </div>

      <section className="max-w-[800px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Resource
        </p>
        <h1 className="mt-4 font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          Meeting minutes templates that people actually fill in
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Four templates in plain markdown — copy them straight into your notes app, wiki, or
          repo. They&rsquo;re deliberately short: the graveyard of meeting documentation is full
          of beautiful templates nobody filled in twice. Below them, the two-minute version of
          what belongs in minutes at all.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-07-11
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Copy-paste ready
          </span>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="The Templates" />
        <div className="grid gap-5">
          {templates.map((t) => (
            <div
              key={t.title}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] shadow-[var(--shadow-panel)]"
            >
              <div className="border-b border-[color:var(--border)] px-5 py-3">
                <h2 className="font-serif text-[19px] text-[var(--text)]">{t.title}</h2>
              </div>
              <div className="overflow-x-auto p-5">
                <pre className="whitespace-pre font-mono text-[12px] leading-6 text-[var(--text-secondary)]">
                  {t.body}
                </pre>
              </div>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What Belongs In Minutes" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Minutes are not a transcript. They exist to answer three future questions: what did
            we decide, who owes what by when, and why did we choose this over the alternative.
            Everything else — the winding discussion, the tangents — is context that a template
            can safely drop. Write decisions with their reasons (&ldquo;chose monthly billing
            because annual was blocking mid-market deals&rdquo;), action items with a single
            owner and a date, and nothing without one of those.
          </p>
          <p>
            The formal board template is the exception: it&rsquo;s a legal record, so it captures
            motions, votes, and quorum precisely and skips discussion detail almost entirely.
            Don&rsquo;t use it for working meetings; don&rsquo;t use the working templates for
            board meetings.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Or Stop Filling In Templates" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Full disclosure: we publish this page and we also build the tool that makes it
            partly obsolete. <span className="font-medium text-[var(--text)]">Minutes</span>{" "}
            (open source, free) records the meeting, transcribes it on your device, and — once
            you connect an assistant (Claude via MCP, or a local LLM) — fills in the structure
            above automatically: attendees, decisions, and action items as structured YAML in a
            markdown file on your own disk. The template becomes the output format, not
            homework. Templates still win for meetings you don&rsquo;t record, and formal board
            minutes where a human secretary is the point.
          </p>
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="https://github.com/silverstein/minutes"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            Generate these automatically
          </a>
          <a
            href="/"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            See the output format
          </a>
        </div>
      </section>

      <PublicFooter />
    </div>
  );
}
