import type { ReactNode } from "react";
import { CopyButton } from "@/components/copy-button";
import { HomeHero, SiteHeader } from "@/components/home-hero";
import {
  APPLE_SILICON_DOWNLOAD_PATH,
  WINDOWS_INSTALLER_DOWNLOAD_URL,
} from "@/lib/downloads";
import { MINUTES_MCP_TOOL_COUNT, MINUTES_TEST_COUNT } from "@/lib/release";
import { organizationSchema, softwareApplicationSchema } from "@/lib/schema";

const linkClass =
  "text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-4 hover:decoration-[color:var(--text)]";

function SectionHeading({ title, muted }: { title: string; muted: string }) {
  return (
    <h2 className="max-w-[880px] text-[32px] font-semibold leading-[1.08] tracking-[-0.035em] text-[var(--text)] sm:text-[44px]">
      {title} <span className="text-[var(--text-tertiary)]">{muted}</span>
    </h2>
  );
}

function Section({ id, children }: { id?: string; children: ReactNode }) {
  return (
    <section id={id} className="scroll-mt-20 border-t border-[color:var(--border)] py-20 sm:py-28">
      {children}
    </section>
  );
}

function Panel({ title, meta, children }: { title: string; meta?: string; children: ReactNode }) {
  return (
    <div className="flex flex-col overflow-hidden rounded-[12px] border border-[color:var(--border)] bg-[var(--bg-elevated)]">
      <div className="flex items-center justify-between gap-3 border-b border-[color:var(--border)] px-4 py-2.5 font-mono text-[11px] text-[var(--text-secondary)]">
        <span className="truncate">{title}</span>
        {meta ? <span className="shrink-0 text-[var(--text-tertiary)]">{meta}</span> : null}
      </div>
      <div className="flex-1 p-4">{children}</div>
    </div>
  );
}

const steps = [
  {
    n: "1",
    title: "Capture",
    body: "Record a meeting, drop in a voice memo from your iPhone, or dictate. Transcription runs on your machine.",
  },
  {
    n: "2",
    title: "One Markdown file",
    body: "Speakers, transcript, decisions, and action items, in a file you can open, grep, and keep for ten years.",
  },
  {
    n: "3",
    title: "Recall with a source",
    body: "Ask Recall in the app, or your own AI assistant, and the answer points back to the file it came from.",
  },
] as const;

// Both pairs are verbatim cases from the dictation cleanup tests
// (crates/core/src/dictation_cleanup.rs), so the demo shows real output.
const corrections = [
  ["Meet on Tuesday, actually Thursday.", "Meet on Thursday."],
  ["This is a recording test—actually, a dictation test.", "This is a dictation test."],
] as const;

const dictationEditing = [
  ["Self-corrections", "Say “actually” or “scratch that” and the earlier words go."],
  ["Voice commands", "“New line”, “new paragraph”, “bullet”, and your own snippets."],
  ["Fits the app", "On a Mac, terminals get exactly what you said; chats and AI prompts get tidied prose."],
  ["No AI rewriting", "Every edit is a rule on your device. Nothing is sent anywhere to clean it up."],
] as const;

// Brand marks from Simple Icons (CC0, v16.36.0), drawn in the text color so
// they read as labels. Codex has no mark: OpenAI's logo is not in Simple
// Icons, and it is not redrawn here.
const agentSurfaces = [
  { name: "Claude", logo: "m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z" },
  { name: "Codex" },
  { name: "Gemini CLI", logo: "M11.04 19.32Q12 21.51 12 24q0-2.49.93-4.68.96-2.19 2.58-3.81t3.81-2.55Q21.51 12 24 12q-2.49 0-4.68-.93a12.3 12.3 0 0 1-3.81-2.58 12.3 12.3 0 0 1-2.58-3.81Q12 2.49 12 0q0 2.49-.96 4.68-.93 2.19-2.55 3.81a12.3 12.3 0 0 1-3.81 2.58Q2.49 12 0 12q2.49 0 4.68.96 2.19.93 3.81 2.55t2.55 3.81" },
  { name: "OpenCode", logo: "M22 24H2V0h20zM17 4.8H7v14.4h10z" },
  { name: "Cursor" },
  { name: "Pi" },
  { name: "Any MCP client" },
] as const;

type FlowStep = { label: string; detail: string; offDevice?: boolean; yourChoice?: boolean };

const cloudFlow: FlowStep[] = [
  { label: "Capture", detail: "your mic" },
  { label: "Transcribe", detail: "their servers", offDevice: true },
  { label: "Summarize", detail: "hosted AI", offDevice: true },
  { label: "Store", detail: "their workspace", offDevice: true },
];

const minutesFlow: FlowStep[] = [
  { label: "Capture", detail: "your mic and system audio" },
  { label: "Transcribe", detail: "on-device speech model" },
  { label: "Store", detail: "Markdown in ~/meetings" },
  { label: "Recall", detail: "the assistant you pick sees what you share", yourChoice: true },
];

function FlowList({ name, steps: flow }: { name: string; steps: FlowStep[] }) {
  return (
    <div className="rounded-[12px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
      <p className="text-[15px] font-semibold text-[var(--text)]">{name}</p>
      <ol className="mt-4 space-y-2">
        {flow.map((step) => (
          <li
            key={step.label}
            className="flex items-center justify-between gap-4 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg)] px-3 py-2.5"
          >
            <span className="text-[14px] text-[var(--text)]">
              {step.label} <span className="text-[var(--text-tertiary)]">· {step.detail}</span>
            </span>
            <span
              className={`shrink-0 font-mono text-[10px] uppercase tracking-[0.12em] ${
                step.offDevice || step.yourChoice ? "text-[var(--text-tertiary)]" : "text-[var(--accent)]"
              }`}
            >
              {step.offDevice ? "Cloud" : step.yourChoice ? "Your choice" : "Your machine"}
            </span>
          </li>
        ))}
      </ol>
    </div>
  );
}

// Homepage storage and agent-access rows spot-checked October 19, 2026.
// Sources: granola.ai/security, help.granola.ai/article/granola-mcp,
// help.otter.ai/hc/en-us/articles/35287607569687-Otter-MCP-Server,
// otter.ai/pricing, anarlog.so/privacy and anarlog.so.
const comparisons = [
  ["Product", "Hosted meeting notepad", "Hosted meeting assistant", "Local meeting notepad", "Local conversation memory"],
  ["Agent access", "Hosted MCP", "Hosted MCP and integrations", "CLI + MCP", `Files + ${MINUTES_MCP_TOOL_COUNT} MCP tools`],
  ["Primary storage", "Hosted workspace", "Hosted workspace", "Local SQLite + files", "Local Markdown + YAML"],
  ["Local AI", "Cloud transcription", "Cloud transcription", "With local providers", "Local transcription; AI provider of your choice"],
] as const;

const compareLinks = [
  ["Granola", "/compare/granola-vs-minutes"],
  ["Otter", "/compare/otter-vs-minutes"],
  ["Fathom", "/compare/fathom-vs-minutes"],
  ["Anarlog", "/compare/hyprnote-vs-minutes"],
  ["Wispr Flow", "/compare/wispr-flow-vs-minutes"],
] as const;

const trust = [
  {
    title: "Consent in every file",
    body: "Each recording stamps its consent basis into the file. Sensitive meetings keep structured notes without audio, and Require mode blocks recording until consent is confirmed.",
    href: "/writing/governance-built-in-not-retrofitted",
    cta: "Why we built it this way",
  },
  {
    title: `${MINUTES_TEST_COUNT.toLocaleString()} tests and a public eval`,
    body: "Run the demo, read the test suite, and check the limits we publish before you trust it with a real meeting.",
    href: "/proof",
    cta: "See the proof",
  },
  {
    title: "Files that outlast the app",
    body: "Plain Markdown with YAML frontmatter. Working Mem0 and Graphiti examples read the same folder.",
    href: "https://github.com/silverstein/minutes/tree/main/examples",
    cta: "Browse the examples",
  },
] as const;

export default function Home() {
  return (
    <div>
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{
          __html: JSON.stringify([organizationSchema(), softwareApplicationSchema()]),
        }}
      />
      <SiteHeader />
      <HomeHero />

      <main className="mx-auto max-w-[1200px] px-6">
        <Section id="product">
          <SectionHeading
            title="One conversation. One file."
            muted="Recall with a source, months later."
          />
          <div className="mt-10 grid gap-4 md:grid-cols-3">
            {steps.map((step) => (
              <div key={step.n} className="border-t border-[color:var(--border-mid)] pt-5">
                <p className="font-mono text-[12px] text-[var(--text-tertiary)]">{step.n}</p>
                <p className="mt-2 text-[18px] font-semibold text-[var(--text)]">{step.title}</p>
                <p className="mt-2 text-[15px] leading-7 text-[var(--text-secondary)]">{step.body}</p>
              </div>
            ))}
          </div>
          <div className="mt-10 grid gap-4 lg:grid-cols-[minmax(0,4fr)_minmax(0,5fr)_minmax(0,4fr)]">
            <Panel title="pricing-idea.m4a" meta="0:46">
              <p className="text-[15px] leading-7 text-[var(--text)]">
                &ldquo;Next three consultant signups get monthly. Annual stays the
                enterprise default.&rdquo;
              </p>
              <p className="mt-3 font-mono text-[11px] text-[var(--text-tertiary)]">
                Voice memo · transcribed on your Mac
              </p>
            </Panel>
            <Panel title="~/meetings/2026-02-28-pricing-strategy.md" meta="0600">
              <pre className="overflow-x-auto font-mono text-[12px] leading-6 text-[var(--text-secondary)]">
                <code>{`---
title: Pricing Strategy — Monthly Billing Test
type: meeting
consent: verbal_all_parties
decisions:
  - text: Launch monthly billing alongside
      annual, starting with the next three
      consultant signups
    authority: high
---

## Transcript
[SPEAKER_0 1:02] Next three consultant
signups get monthly.`}</code>
              </pre>
            </Panel>
            <Panel title="Any MCP client" meta="minutes">
              <p className="text-[14px] text-[var(--text-secondary)]">
                What did we decide in the February 28 pricing meeting?
              </p>
              <p className="mt-4 text-[15px] leading-7 text-[var(--text)]">
                Launch monthly billing for the next three consultant signups.
                Annual remains the enterprise default.
              </p>
              <p className="mt-3 font-mono text-[11px] leading-5 text-[var(--text-tertiary)]">
                Source: 2026-02-28-pricing-strategy.md · decision
              </p>
            </Panel>
          </div>
        </Section>

        <Section id="dictation">
          <div className="grid gap-12 lg:grid-cols-2">
            <div>
              <SectionHeading
                title="Dictation that stays on your computer."
                muted="Hold a key, talk, and the text lands where you're typing."
              />
              <p className="mt-6 max-w-[540px] text-[16px] leading-8 text-[var(--text-secondary)]">
                On a Mac it types at your cursor; on Windows and Linux it goes to
                the clipboard. Every dictation is also saved as a Markdown file, so
                your AI assistant can find it later with your meetings and memos.
              </p>
              <dl className="mt-8 grid gap-x-8 gap-y-6 sm:grid-cols-2">
                {dictationEditing.map(([term, detail]) => (
                  <div key={term}>
                    <dt className="text-[15px] font-semibold text-[var(--text)]">{term}</dt>
                    <dd className="mt-1 text-[14px] leading-6 text-[var(--text-secondary)]">{detail}</dd>
                  </div>
                ))}
              </dl>
              <p className="mt-8 text-[15px] text-[var(--text-secondary)]">
                Coming from Wispr Flow?{" "}
                <a href="/compare/wispr-flow-vs-minutes" className={linkClass}>
                  See how they compare
                </a>{" "}
                or{" "}
                <a href="/resources/open-source-wispr-flow-alternatives" className={linkClass}>
                  open-source alternatives
                </a>
                .
              </p>
            </div>

            <div className="space-y-4">
              <Panel title="What you said → what lands" meta="on-device cleanup">
                <div className="space-y-4">
                  {corrections.map(([raw, cleaned]) => (
                    <div key={raw} className="font-mono text-[13px] leading-6">
                      <p className="text-[var(--text-tertiary)] line-through decoration-[color:var(--border-mid)]">{raw}</p>
                      <p className="text-[var(--text)]">{cleaned}</p>
                    </div>
                  ))}
                </div>
              </Panel>
              <Panel title="Time to decode one dictation" meta="Apple Silicon">
                <div className="space-y-4">
                  {[
                    ["Parakeet v3", "51 ms", "30%"],
                    ["Whisper small", "168 ms", "100%"],
                  ].map(([engine, ms, width]) => (
                    <div key={engine}>
                      <div className="flex justify-between text-[14px]">
                        <span className="text-[var(--text)]">{engine}</span>
                        <span className="font-mono text-[var(--text-secondary)]">{ms}</span>
                      </div>
                      <div className="mt-2 h-2 rounded-full bg-[var(--bg-hover)]">
                        <div className="h-2 rounded-full bg-[var(--accent)]" style={{ width }} />
                      </div>
                    </div>
                  ))}
                </div>
                <p className="mt-4 text-[13px] leading-6 text-[var(--text-secondary)]">
                  Median model time on 39 of our own dictation clips, M4 Max.
                  Parakeet v3 is a one-click install on Apple Silicon.{" "}
                  <a href="/writing/whisper-cpp-vs-parakeet-cpp" className={linkClass}>
                    How we measured
                  </a>
                </p>
              </Panel>
            </div>
          </div>
        </Section>

        <Section id="agents">
          <div className="grid gap-12 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)] lg:items-center">
            <div>
              <SectionHeading
                title="Your AI reads the folder."
                muted="And cites the file it read."
              />
              <ul className="mt-6 flex flex-wrap gap-x-5 gap-y-2 text-[15px] text-[var(--text)]" aria-label="Works with">
                {agentSurfaces.map((surface) => (
                  <li key={surface.name} className="inline-flex items-center gap-2">
                    {"logo" in surface ? (
                      <svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor" className="h-4 w-4">
                        <path d={surface.logo} />
                      </svg>
                    ) : null}
                    {surface.name}
                  </li>
                ))}
              </ul>
              <p className="mt-6 max-w-[520px] text-[16px] leading-8 text-[var(--text-secondary)]">
                {MINUTES_MCP_TOOL_COUNT} MCP tools let your assistant search
                meetings, memos, and dictation, follow a live transcript, and cite
                the file every answer came from. Summaries are optional: Claude can
                write them on your existing subscription, with no API key.
              </p>
              <div className="mt-8 flex flex-wrap items-center gap-3">
                <CopyButton label="Try it with five sample meetings" cmd="npx minutes-mcp --demo" acquisitionTarget="mcp" />
              </div>
              <p className="mt-6 text-[15px]">
                <a href="/for-agents" className={linkClass}>
                  Set up your agent
                </a>
              </p>
            </div>
            <Panel title="claude" meta="~/projects/pricing">
              <pre className="overflow-x-auto whitespace-pre-wrap font-mono text-[12.5px] leading-6 text-[var(--text-secondary)]">
                <code>
                  <span className="text-[var(--text)]">&gt; what did we decide about pricing?</span>
                  {"\n\n"}
                  <span className="text-[var(--accent)]">⏺</span>
                  {" minutes - search_meetings (MCP)(query: \"pricing\")\n  ⎿  2 results\n\n"}
                  <span className="text-[var(--accent)]">⏺</span>
                  <span className="text-[var(--text)]">
                    {" Annual-only billing, decided March 25. It reversed the\n  February 28 test of monthly billing for the next three\n  consultant signups."}
                  </span>
                  {"\n\n  Source: 2026-03-25-pricing-reversal.md"}
                </code>
              </pre>
            </Panel>
          </div>
        </Section>

        <Section id="local">
          <SectionHeading
            title="Recording and transcription run on your machine."
            muted="Your history is Markdown on your own disk."
          />
          <div className="mt-10 grid gap-4 md:grid-cols-2">
            <FlowList name="A cloud notetaker" steps={cloudFlow} />
            <FlowList name="Minutes" steps={minutesFlow} />
          </div>
          <p className="mt-5 max-w-[760px] text-[14px] leading-7 text-[var(--text-secondary)]">
            If you pick a cloud model for summaries, or ask a cloud assistant about
            a meeting, the context you share goes to that provider. Skip
            summaries or use a local model and nothing leaves.
          </p>
        </Section>

        <Section id="compare">
          <SectionHeading title="How it compares." muted="Where your meetings live, and who can read them." />
          <div className="mt-10 overflow-x-auto rounded-[12px] border border-[color:var(--border)]">
            <table className="w-full min-w-[680px] border-collapse text-[14px]">
              <thead>
                <tr className="bg-[var(--bg-elevated)] text-left">
                  <th className="p-4" />
                  {["Granola", "Otter.ai", "Anarlog"].map((name) => (
                    <th key={name} className="p-4 font-medium text-[var(--text-secondary)]">
                      {name}
                    </th>
                  ))}
                  <th className="p-4 font-semibold text-[var(--text)]">Minutes</th>
                </tr>
              </thead>
              <tbody>
                {comparisons.map(([feature, ...values]) => (
                  <tr key={feature} className="border-t border-[color:var(--border)]">
                    <td className="p-4 font-medium text-[var(--text)]">{feature}</td>
                    {values.map((value, index) => (
                      <td
                        key={`${feature}-${index}`}
                        className={`p-4 ${
                          index === 3 ? "font-medium text-[var(--text)]" : "text-[var(--text-secondary)]"
                        }`}
                      >
                        {value}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="mt-6 flex flex-wrap items-center gap-x-5 gap-y-2 text-[14px] text-[var(--text-secondary)]">
            <a href="/compare" className={linkClass}>
              All comparisons
            </a>
            {compareLinks.map(([name, href]) => (
              <a key={href} href={href} className="hover:text-[var(--text)]">
                vs {name}
              </a>
            ))}
          </div>
        </Section>

        <Section>
          <SectionHeading title="Built to be checked." muted="Not just trusted." />
          <div className="mt-10 grid gap-4 md:grid-cols-3">
            {trust.map((item) => (
              <a
                key={item.title}
                href={item.href}
                className="group rounded-[12px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 hover:border-[color:var(--border-mid)]"
              >
                <p className="text-[17px] font-semibold text-[var(--text)]">{item.title}</p>
                <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">{item.body}</p>
                <p className="mt-4 text-[14px] text-[var(--text)]">
                  {item.cta} <span aria-hidden="true" className="inline-block group-hover:translate-x-0.5">→</span>
                </p>
              </a>
            ))}
          </div>
        </Section>

        <Section id="why">
          <SectionHeading title="Why it's free." muted="Memory you don't rent." />
          <div className="mt-10 max-w-[680px] space-y-5 text-[17px] leading-8 text-[var(--text-secondary)]">
            <p>
              Soon your agents will run on what you&apos;ve said out loud: meetings, calls, the idea you talked
              through on a walk. That&apos;s the most personal data you&apos;ll ever make. You shouldn&apos;t have
              to rent access to it, and nobody should get to read it just because you used their app.
            </p>
            <p>
              So it&apos;s a file. Plain Markdown on your disk, MIT licensed. Switch agents next year and nothing
              has to move. Share it with whoever you want, or with no one. There&apos;s no Minutes cloud for it to
              sit in.
            </p>
            <p>
              Good tools outlive the people who make them. If I stopped tomorrow, your meetings would still open
              and the code would still run.
            </p>
            <p className="text-[var(--text)]">
              <a href="https://github.com/silverstein" className="hover:underline">
                Mat Silverstein
              </a>
            </p>
          </div>
        </Section>

        <section id="install" className="scroll-mt-20 border-t border-[color:var(--border)] py-20 text-center sm:py-28">
          <h2 className="mx-auto max-w-[760px] text-[36px] font-semibold leading-[1.05] tracking-[-0.04em] text-[var(--text)] sm:text-[56px]">
            Free, open source, and yours.{" "}
            <span className="text-[var(--text-tertiary)]">No account, no API key, no subscription.</span>
          </h2>
          <div className="mt-10 flex flex-wrap justify-center gap-3">
            <a
              href={APPLE_SILICON_DOWNLOAD_PATH}
              data-download-target="mac"
              className="inline-flex h-12 items-center rounded-full bg-[var(--accent)] px-7 text-[16px] font-medium text-[var(--marketing-button-text)] hover:bg-[var(--accent-hover)]"
            >
              Download for Mac
            </a>
            <a
              href={WINDOWS_INSTALLER_DOWNLOAD_URL}
              data-download-target="windows"
              className="inline-flex h-12 items-center rounded-full border border-[color:var(--border-mid)] px-7 text-[16px] font-medium text-[var(--text)] hover:bg-[var(--bg-hover)]"
            >
              Download for Windows
            </a>
          </div>
          <p className="mt-5 text-[14px] text-[var(--text-secondary)]">
            Apple Silicon Macs and Windows. The first launch downloads a speech model.
          </p>
          <div className="mt-8 flex flex-wrap justify-center gap-3">
            <CopyButton
              label="Homebrew (desktop)"
              acquisitionTarget="desktop_cli"
              cmd="brew install --cask silverstein/tap/minutes"
            />
            <CopyButton
              label="Homebrew (CLI)"
              acquisitionTarget="desktop_cli"
              cmd="brew tap silverstein/tap && brew install minutes"
            />
          </div>
          <p className="mx-auto mt-4 max-w-[640px] text-[13px] leading-6 text-[var(--text-tertiary)]">
            If Homebrew warns about the tap, run{" "}
            <span className="font-mono text-[var(--text-secondary)]">brew trust silverstein/tap</span> once.
            Minutes won&apos;t open?{" "}
            <a href="/download/recover" className={linkClass}>
              Reinstall and keep your meetings
            </a>
            .
          </p>
          <p className="mx-auto mt-6 max-w-[420px] rounded-[12px] border border-[color:var(--border)] px-4 py-3 text-[14px] leading-6 text-[var(--text-secondary)] sm:hidden">
            Reading on your phone? Minutes installs on Mac and Windows.{" "}
            <a href="https://github.com/silverstein/minutes" className={linkClass}>
              Star the repo
            </a>{" "}
            so it is waiting at your desk.
          </p>
        </section>

        <footer className="flex flex-col gap-6 border-t border-[color:var(--border)] py-12 text-[14px] text-[var(--text-secondary)] sm:flex-row sm:justify-between">
          <div>
            <p className="text-[var(--text)]">Minutes is MIT licensed and free.</p>
            <p className="mt-1">
              Built by{" "}
              <a href="https://github.com/silverstein" className="hover:text-[var(--text)]">
                Mat Silverstein
              </a>
              , founder of{" "}
              <a href="https://x1wealth.com" className="hover:text-[var(--text)]">
                X1 Wealth
              </a>
              .
            </p>
          </div>
          <nav aria-label="Footer" className="grid grid-cols-2 gap-x-10 gap-y-2 sm:grid-cols-3">
            {[
              ["For agents", "/for-agents"],
              ["MCP docs", "/docs/mcp/tools"],
              ["Errors", "/docs/errors"],
              ["Compare", "/compare"],
              ["Resources", "/resources"],
              ["Writing", "/writing"],
              ["GitHub", "https://github.com/silverstein/minutes"],
              ["llms.txt", "/llms.txt"],
              ["Contribute", "https://github.com/silverstein/minutes/blob/main/CONTRIBUTING.md"],
            ].map(([label, href]) => (
              <a key={label} href={href} className="hover:text-[var(--text)]">
                {label}
              </a>
            ))}
          </nav>
        </footer>
      </main>
    </div>
  );
}
