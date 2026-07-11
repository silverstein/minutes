import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "How to remove AI notetaker bots from your meetings",
  description:
    "Step-by-step: stop Otter, Fireflies, and other AI bots from joining your Zoom, Google Meet, and Teams calls — yours and other people's — plus the capture architecture that never needed a bot in the first place.",
  alternates: {
    canonical: "/resources/remove-ai-notetaker-bots-from-meetings",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "How do I stop Otter from automatically joining my meetings?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "In Otter's settings, disable auto-join for calendar events (Otter Assistant / OtterPilot settings), or disconnect your calendar entirely so Otter can't see meeting links. To remove it from a live call, use the meeting platform's participant list to remove the bot like any attendee. Otter's help center documents both paths.",
      },
    },
    {
      "@type": "Question",
      name: "How do I remove Fireflies (Fred) from a meeting?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Remove the Fireflies notetaker from the participant list as host, and in Fireflies settings change the autojoin rule (or disconnect the calendar) so it stops joining future calls. Fireflies' guide documents removal and autojoin settings.",
      },
    },
    {
      "@type": "Question",
      name: "Can I block all AI notetaker bots from my meetings?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Largely, yes: enable the waiting room, require sign-in, and admit only human participants — bots arrive as visible guest participants and can be denied or removed. But you cannot disable another attendee's bot from your side except by removing it per meeting or setting an organizational policy.",
      },
    },
    {
      "@type": "Question",
      name: "Is there a way to get AI meeting notes without any bot?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Yes — device-side capture. Tools like Minutes (open source, on-device) record audio directly on the participant's machine, so no bot ever appears in the meeting, and with Minutes the audio is also transcribed locally rather than uploaded. You should still tell participants you're recording; the difference is architectural, not a way around consent.",
      },
    },
  ],
} as const;

const sources = [
  {
    label: "Otter help: remove Otter Notetaker from your meeting (Zoom, Meet, Teams)",
    href: "https://help.otter.ai/hc/en-us/articles/14288936562199-Remove-Otter-Notetaker-from-your-meeting-Zoom-Google-Meet-or-Microsoft-Teams",
  },
  {
    label: "Otter help: stop Otter Notetaker from automatically joining meetings",
    href: "https://help.otter.ai/hc/en-us/articles/12906714508823-Stop-Otter-Notetaker-from-automatically-joining-your-meetings",
  },
  {
    label: "Fireflies guide: remove Fireflies from a meeting or stop it from joining",
    href: "https://guide.fireflies.ai/articles/7098191513-how-to-remove-fireflies-from-a-meeting-or-stop-it-from-joining",
  },
  { label: "Minutes security & privacy architecture", href: "/security" },
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

export default function RemoveNotetakerBotsPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(faqJsonLd) }}
      />
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a
            href="/resources/remove-ai-notetaker-bots-from-meetings.md"
            className="hover:text-[var(--accent)]"
          >
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
          How to remove AI notetaker bots from your meetings
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          The most-searched questions about AI notetakers aren&rsquo;t &ldquo;which one is
          best&rdquo; — they&rsquo;re &ldquo;how do I get this thing out of my call.&rdquo;
          Fair. Here&rsquo;s the complete removal guide for the common bots, the settings that
          stop them coming back, and the honest limits of what you can control when the bot
          belongs to someone else.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-07-11
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            How-to guide
          </span>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Why The Bot Keeps Showing Up" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Bot notetakers join meetings the same way a person does: they read a connected
            calendar, find the meeting link, and dial in as a participant. That means there are
            exactly three levers — the calendar connection that feeds it links, the vendor
            setting that tells it to auto-join, and the meeting platform&rsquo;s participant
            controls. Every fix below is one of those three.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Removing Your Own Bot" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Otter (OtterPilot / Otter Notetaker).</span>{" "}
            To stop it joining everything: Otter settings → turn off auto-join for calendar
            events, or disconnect Google/Microsoft calendar entirely so it never sees links. To
            eject it from one live meeting: open the participant list in Zoom/Meet/Teams and
            remove it like any attendee. Both procedures are in Otter&rsquo;s help center,
            linked below.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">Fireflies (Fred).</span> Same
            pattern: Fireflies settings → autojoin rules (change to invite-only or off, or
            disconnect the calendar), and remove the notetaker from the participant list
            mid-call. Fireflies&rsquo; own guide is linked below.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">Anything else.</span> The pattern
            generalizes: find the calendar connection and sever it, find the auto-join rule and
            turn it off. If a bot has no calendar access, it has no way to find your meetings.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Blocking Other People's Bots" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            This is the part vendors&rsquo; help pages soft-pedal: you cannot disable a
            colleague&rsquo;s or client&rsquo;s bot from your side. What you can do as host:
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              Enable the waiting room / lobby and admit only humans — bots arrive as visible
              guest participants with names like &ldquo;Otter Notetaker&rdquo; or
              &ldquo;Fireflies.ai Notetaker.&rdquo;
            </li>
            <li>Require signed-in participants, which blocks most anonymous bot joins.</li>
            <li>Remove the bot from the participant list; in Zoom, removed participants can be barred from rejoining.</li>
            <li>
              Say it out loud: &ldquo;please drop the notetaker for this one&rdquo; is now normal
              meeting etiquette, and the human who owns the bot can kill it in one click.
            </li>
          </ul>
          <p>
            For organizations, the durable fix is policy plus platform controls: several
            platforms let admins restrict which apps and guest domains can join meetings at the
            tenant level.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Version Of This Problem That Solves Itself" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Notice that every step above is managing a symptom. The bot exists because
            cloud notetakers need a way to get your meeting&rsquo;s audio onto their servers,
            and joining the call as a fake participant is that way. Capture the audio on the
            participant&rsquo;s own device instead, and the entire category of problem
            disappears — nothing joins the call, nothing shows up in the participant list,
            nothing needs admitting or ejecting.
          </p>
          <p>
            That&rsquo;s how <span className="font-medium text-[var(--text)]">Minutes</span>{" "}
            works: it records device-side, transcribes locally with whisper.cpp, and writes
            markdown to your own disk — no bot <em>and</em> no cloud. (Granola is also botless,
            though it transcribes in the cloud — see our{" "}
            <a
              href="/compare/granola-vs-minutes"
              className="text-[var(--accent)] hover:underline"
            >
              comparison
            </a>
            .) One thing device-side capture does not change: tell people you&rsquo;re
            recording. The bot&rsquo;s one virtue was announcing itself; without it, consent is
            on you, where it belonged anyway.
          </p>
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="/security"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            How botless capture works
          </a>
          <a
            href="/compare"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Compare notetakers
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

      <PublicFooter />
    </div>
  );
}
