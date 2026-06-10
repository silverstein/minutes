import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Governance built in, not retrofitted — minutes",
  description:
    "Default-on recording at work is coming. The controls cannot be bolted on afterward, because the record's primary reader is now an agent.",
  alternates: {
    canonical: "/writing/governance-built-in-not-retrofitted",
  },
};

export default function Post() {
  return (
    <div className="mx-auto max-w-[680px] px-6 pb-16 sm:px-8">
      <nav className="flex items-center justify-between border-b border-[color:var(--border)] py-4">
        <a
          href="/"
          className="font-mono text-[15px] font-medium text-[var(--text)]"
        >
          minutes
        </a>
        <div className="flex gap-x-6 text-sm text-[var(--text-secondary)]">
          <a href="/writing" className="hover:text-[var(--accent)]">
            Writing
          </a>
          <a
            href="https://github.com/silverstein/minutes"
            className="hover:text-[var(--accent)]"
          >
            GitHub
          </a>
        </div>
      </nav>

      <article className="pt-14">
        <p className="mb-4 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--text-secondary)]">
          2026-06-10 · Mat Silverstein
        </p>
        <h1 className="font-serif text-[36px] leading-[1.05] tracking-[-0.04em] text-[var(--text)] sm:text-[42px]">
          Governance built in, not retrofitted
        </h1>

        <div className="mt-8 space-y-5 text-[16px] leading-[1.75] text-[var(--text-secondary)]">
          <p>
            Every meeting I&apos;ve taken since March sits in plain markdown on
            my laptop.
          </p>
          <p>
            I built a small open source tool called Minutes to do it: local
            transcription, speaker labels, action items, nothing leaves the
            machine. Three months of dogfooding taught me something I
            didn&apos;t expect. I almost never reread a transcript. The value
            shows up later, when an agent answers &quot;what did we decide
            about pricing in April&quot; from files it can actually read.
          </p>
          <p>
            That experience is why one line in a16z&apos;s piece{" "}
            <a
              href="https://www.a16z.news/p/everything-is-recorded-now"
              className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
            >
              &quot;Everything Is Recorded Now&quot;
            </a>{" "}
            stuck with me. David Haber argues default-on recording is
            inevitable and that a new system of record will form around voice.
            He&apos;s right, and the piece is worth reading. But he also
            concedes the governance controls will get &quot;retrofitted on
            top&quot; after adoption wins.
          </p>
          <p>
            Retrofitting worked when the reader of a record was a person. It
            breaks when the reader is an agent, because an agent will happily
            surface the one conversation you never should have captured, in a
            context you never imagined, years later. Whoever holds the corpus
            sets the rules, and if the corpus lives on someone else&apos;s
            servers, the rules were never yours to set.
          </p>
          <p>
            So we&apos;re building the controls into the record itself. Every
            Minutes recording now carries its consent basis in the file&apos;s
            frontmatter. Next: meetings you designate sensitive produce
            structured notes with no audio captured at all, plus a sensitivity
            field the agent layer is required to respect. A restricted meeting
            simply never appears in search results, graph queries, or anything
            an agent assembles.
          </p>
          <p>
            Recording everything is coming either way. I&apos;d rather own my
            record, and have my agents respect boundaries I set, before the
            default flips.
          </p>
          <p>
            Minutes is MIT licensed. If you&apos;re building in this space, the{" "}
            <a
              href="https://github.com/silverstein/minutes/blob/main/docs/plans/consent-layer-spec-2026-06-04.md"
              className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
            >
              consent spec
            </a>{" "}
            and the{" "}
            <a
              href="https://github.com/silverstein/minutes/blob/main/docs/plans/consent-layer-phase2-2026-06-10.md"
              className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
            >
              phase 2 plan
            </a>{" "}
            are in the repo. Point Claude or Codex at it and spin up your own
            version.
          </p>
        </div>

        <p className="mt-10 border-t border-[color:var(--border)] pt-6 text-[13px] leading-6 text-[var(--text-secondary)]">
          The consent layer is a disclosure aid, not legal advice; make sure
          everyone present has agreed where required.
        </p>
      </article>
    </div>
  );
}
