import type { Metadata } from "next";
import { collectionPageSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "Writing — minutes",
  description:
    "Essays from building Minutes: local-first conversation memory, agent-readable records, and governance that lives in the data.",
  alternates: {
    canonical: "/writing",
  },
};

const posts = [
  {
    slug: "building-local-dictation",
    date: "2026-11-03",
    title: "Building local dictation in Minutes",
    summary:
      "How dictation in Minutes got to where it is: the Parakeet v3 numbers on real dictation clips, how text reaches the app you are using, bugs found in daily use, and two features that failed their tests and were dropped.",
  },
  {
    slug: "whisper-cpp-vs-parakeet-cpp",
    date: "2026-10-27",
    title: "Whisper vs Parakeet v3 for local transcription",
    summary:
      "Parakeet v3 now runs inside Minutes on Apple Silicon. Published benchmarks and our own Mac measurements against Whisper, where Whisper still wins, and what Minutes picks by default. Read this if you are choosing a local transcription engine.",
  },
  {
    slug: "governance-built-in-not-retrofitted",
    date: "2026-06-10",
    title: "Governance built in, not retrofitted",
    summary:
      "a16z says everything at work will be recorded and the controls will get bolted on afterward. The retrofit assumption breaks the moment the record's reader is an agent. The essay sets out what consent, authority, and provenance have to look like inside the file itself.",
  },
] as const;

export default function WritingIndex() {
  return (
    <div className="mx-auto max-w-[720px] px-6 pb-16 sm:px-8">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(collectionPageSchema({ metadata, path: "/writing", items: posts.map((post) => ({ title: post.title, href: `/writing/${post.slug}` })) })) }}
      />
      <nav className="flex items-center justify-between border-b border-[color:var(--border)] py-4">
        <a
          href="/"
          className="font-mono text-[15px] font-medium text-[var(--text)]"
        >
          minutes
        </a>
        <div className="flex gap-x-6 text-sm text-[var(--text-secondary)]">
          <a
            href="https://github.com/silverstein/minutes"
            className="hover:text-[var(--accent)]"
          >
            GitHub
          </a>
          <a href="/" className="hover:text-[var(--accent)]">
            Home
          </a>
        </div>
      </nav>

      <header className="pb-10 pt-14">
        <p className="mb-4 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Writing
        </p>
        <h1 className="font-serif text-[36px] leading-tight tracking-[-0.04em] text-[var(--text)]">
          Notes from building Minutes
        </h1>
        <p className="mt-4 max-w-[560px] text-[15px] leading-7 text-[var(--text-secondary)]">
          Essays on local-first conversation memory, agent-readable records,
          and where the recorded workplace is heading.
        </p>
        <p className="mt-4 max-w-[560px] text-[15px] leading-7 text-[var(--text-secondary)]">
          These are longer and more opinionated than the{" "}
          <a href="/resources" className="text-[var(--accent)] hover:underline">
            resource guides
          </a>
          . A resource answers one question with sources. An essay argues for a
          position we hold about how recorded conversations should work, using
          the numbers and design decisions from building Minutes as evidence.
          Each one is dated, and we revise rather than quietly rewrite.
        </p>
      </header>

      <div className="space-y-6 border-t border-[color:var(--border)] pt-10">
        {posts.map((post) => (
          <a
            key={post.slug}
            href={`/writing/${post.slug}`}
            className="block rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
          >
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
              {post.date}
            </p>
            <h2 className="mt-2 font-serif text-[24px] leading-7 text-[var(--text)]">
              {post.title}
            </h2>
            <p className="mt-3 text-[14px] leading-6 text-[var(--text-secondary)]">
              {post.summary}
            </p>
          </a>
        ))}
      </div>
    </div>
  );
}
