import type { Metadata } from "next";
import { FaqSection } from "@/components/faq-section";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";
import { faqPageSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "How to remove Otter AI from Zoom",
  description:
    "Remove Otter Notetaker from a Zoom call immediately, even if you are not the host, then stop it coming back. Includes the auto-join setting that silently ignores your change, and what you can actually do about someone else's bot.",
  alternates: {
    canonical: "/resources/remove-otter-ai-from-zoom",
  },
};

const faqs = [
  {
    question: "How do I remove Otter AI from a Zoom meeting?",
    answer: "Type \"stop otter\" into the Zoom meeting chat. Any participant can do this, including people without an Otter account, and the Notetaker leaves immediately. If you are the host you can also open Participants, click the ellipsis next to the Notetaker, and choose Remove.",
  },
  {
    question: "Can I remove Otter from Zoom if I am not the host?",
    answer: "Yes. The \"stop otter\" chat command works for any participant regardless of host status or whether you have an Otter account. This is the only removal method that does not require host controls, which is why it matters when the bot belongs to someone else in the call.",
  },
  {
    question: "Why does Otter keep joining my meetings after I turned auto-join off?",
    answer: "Changing the default auto-join setting only applies to calendar events you have not already customized. Any event where you previously toggled auto-join on keeps that setting. You have to revisit those individual events in the Otter calendar view and turn them off one by one.",
  },
  {
    question: "How do I stop Otter Notetaker joining all my Zoom meetings?",
    answer: "In Otter, go to Integrations, then Meetings, open the Default auto-join settings dropdown, and choose \"Meetings I manually select.\" Disconnecting the calendar entirely is the stronger version: with no calendar access, Otter cannot see your meeting links at all.",
  },
  {
    question: "How do I block other people's AI notetakers from my Zoom meetings?",
    answer: "Enable the Waiting Room and admit only recognized attendees, and require authenticated users. That blocks some standalone bot participants, but it is not a guarantee: notetakers operating through an already-admitted attendee's own authenticated session are unaffected. Admins can disable or remove installed apps at marketplace.zoom.us under Manage, Admin App Management, Apps on Account, which stops internal users from running them but does not restrict external participants. Disabling local recording may block tools that rely on Zoom's recording channel; nothing in Zoom prevents out-of-band capture by a participant or a separate device.",
  },
] as const;

const sources = [
  {
    label: "Otter help: remove Otter Notetaker from your meeting (Zoom, Google Meet, Microsoft Teams)",
    href: "https://help.otter.ai/hc/en-us/articles/14288936562199-Remove-Otter-Notetaker-from-your-meeting-Zoom-Google-Meet-or-Microsoft-Teams",
  },
  {
    label: "Otter help: stop Otter Notetaker from automatically joining your meetings",
    href: "https://help.otter.ai/hc/en-us/articles/12906714508823-Stop-Otter-Notetaker-from-automatically-joining-your-meetings",
  },
  {
    label: "Otter help: choose which meetings Otter Notetaker records",
    href: "https://help.otter.ai/hc/en-us/articles/26010355877911-Choose-which-meetings-Otter-Notetaker-records",
  },
  {
    label: "Zoom Community: disabling AI notetakers (Otter, Read.ai, Fireflies) from joining meetings",
    href: "https://community.zoom.com/meetings-2/how-do-i-disable-ai-notetakers-otter-ai-read-ai-fireflies-ai-etc-from-joining-our-meetings-17388",
  },
  {
    label: "IT@Cornell: strategies to block AI bots from Zoom sessions",
    href: "https://it.cornell.edu/zoom/zoom-block-ai-bots",
  },
  { label: "Minutes security & privacy architecture", href: "/security" },
] as const;


export default function RemoveOtterFromZoomPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(faqPageSchema(faqs)) }}
      />
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a
            href="/resources/remove-otter-ai-from-zoom.md"
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
          How to remove Otter AI from Zoom
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          There is a one-line fix that works even when you are not the host, and almost nobody
          knows it. Below: the immediate removal, the setting that stops Otter coming back
          (including the one that silently ignores you), and an honest account of what you can
          and cannot do about somebody else&rsquo;s bot.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-08-09
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            How-to guide
          </span>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The One-Line Answer" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Type{" "}
            <code className="rounded-[4px] bg-[var(--bg-elevated)] px-1.5 py-0.5 font-mono text-[13px] text-[var(--text)]">
              stop otter
            </code>{" "}
            into the Zoom meeting chat. The Notetaker leaves.
          </p>
          <p>
            The part that matters:{" "}
            <span className="font-medium text-[var(--text)]">
              any participant can do this
            </span>
            . You do not need to be the host. You do not need an Otter account. It works in
            Google Meet and Microsoft Teams chat too. This is the only removal method that
            requires no permissions at all, which makes it the one to remember when the bot
            belongs to somebody else in the call.
          </p>
          <p>
            One caveat worth knowing before you use it: if several Otter Notetakers are in the
            meeting, because more than one attendee brought one,{" "}
            <code className="rounded-[4px] bg-[var(--bg-elevated)] px-1.5 py-0.5 font-mono text-[13px] text-[var(--text)]">
              stop otter
            </code>{" "}
            removes all of them, not just the one you had in mind. To remove a single
            Notetaker, use the participant-list method below.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="If You Are The Host" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Host controls remove one specific bot rather than all of them:
          </p>
          <ol className="list-decimal space-y-2 pl-6">
            <li>Click <span className="font-medium text-[var(--text)]">Participants</span> in the Zoom toolbar.</li>
            <li>Find the Notetaker in the list. It appears as a normal participant with a name like &ldquo;Otter.ai Notetaker.&rdquo;</li>
            <li>Click the <span className="font-medium text-[var(--text)]">ellipsis</span> next to its name.</li>
            <li>Choose <span className="font-medium text-[var(--text)]">Remove</span>.</li>
          </ol>
          <p>
            This ejects the bot from the current meeting only. It does nothing about the next
            one, which is the next section.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Stop It Coming Back" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            In Otter, open <span className="font-medium text-[var(--text)]">Integrations</span>{" "}
            → <span className="font-medium text-[var(--text)]">Meetings</span>, open the{" "}
            <span className="font-medium text-[var(--text)]">Default auto-join settings</span>{" "}
            dropdown, and choose{" "}
            <span className="font-medium text-[var(--text)]">
              &ldquo;Meetings I manually select.&rdquo;
            </span>
          </p>
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              The gotcha that sends people back to Google
            </p>
            <p className="mt-3">
              Changing the default does{" "}
              <span className="font-medium text-[var(--text)]">not</span> apply retroactively to
              calendar events you already customized. If you ever toggled auto-join on for a
              specific recurring meeting, that event keeps its own setting and Otter will keep
              joining it, no matter what the default now says. You have to open the calendar
              view in Otter and turn those events off individually.
            </p>
            <p className="mt-3">
              Otter documents the behavior but publishes no figures on how often it bites. It is
              worth checking first when you believe you disabled Otter and then watch it walk
              into a call the following week.
            </p>
          </div>
          <p>
            The stronger version is to sever the supply line entirely: disconnect the Google or
            Microsoft calendar from Otter. A notetaker that cannot read your calendar cannot
            discover your meeting links, so there is nothing left to auto-join. If you only
            ever want Otter in meetings you deliberately choose, this is the setting that
            actually guarantees it.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Somebody Else's Otter Bot" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            This is the case every vendor help page skips, because the honest answer is
            awkward: you cannot reach into a colleague&rsquo;s or client&rsquo;s Otter account
            and turn their bot off. What you can do, roughly in order of how well it works:
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              <span className="font-medium text-[var(--text)]">Ask.</span> &ldquo;Could you drop
              the notetaker for this one?&rdquo; is ordinary meeting etiquette now, and the
              person who owns the bot can remove it in a single click. For a sensitive
              conversation this is faster and less strange than any technical control.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">
                Type the chat command.
              </span>{" "}
              As above, it works from any seat in the room.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">
                Waiting Room plus authentication.
              </span>{" "}
              The strongest standing control, though not a guarantee. Requiring authenticated
              users blocks some standalone bot participants, and the Waiting Room lets you admit
              only people you recognize before anything hears the room. It does not stop a
              notetaker running through an attendee you have already admitted, and Cornell&rsquo;s
              IT guidance is explicit that this only helps prevent bot access rather than
              eliminating it.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Admin app controls.</span> Zoom
              admins can disable or remove installed apps at{" "}
              <span className="font-medium text-[var(--text)]">
                marketplace.zoom.us → Manage → Admin App Management → Apps on Account
              </span>
              . Read the scope carefully: this stops users on your account from running the app.
              It does not restrict external participants who bring their own.
            </li>
          </ul>
          <p>
            And the limit worth stating plainly, because the pages that sell you a blocker will
            not: some notetakers no longer join as a separate participant at all. They capture
            through an attendee&rsquo;s own authenticated session, so there is no bot in the
            participant list to remove. Admission controls do not reach that case. Disabling
            local recording may block tools that depend on Zoom&rsquo;s own recording channel,
            which is worth doing, but Zoom cannot prevent genuinely out-of-band capture by a
            participant or a separate device in the room. That has always been true of meetings;
            AI just made it cheaper.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Version Of This Problem That Solves Itself" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Every step above manages a symptom. The bot exists because cloud notetakers need
            your meeting&rsquo;s audio on their servers, and joining the call as a synthetic
            participant is how they get it. Capture the audio on your own device instead and the
            entire category disappears: nothing joins, nothing appears in the participant list,
            nothing needs admitting, ejecting, or explaining to a client.
          </p>
          <p>
            That is how <span className="font-medium text-[var(--text)]">Minutes</span> works. It
            records device-side, transcribes locally with whisper.cpp, and writes markdown to
            your own disk. No bot, and no cloud either. Granola is also botless if you want the
            comparison, though it transcribes in the cloud, which is{" "}
            <a
              href="/compare/granola-vs-minutes"
              className="text-[var(--accent)] hover:underline"
            >
              a different trade
            </a>
            . If you are weighing Otter specifically, we keep an{" "}
            <a href="/compare/otter-vs-minutes" className="text-[var(--accent)] hover:underline">
              honest side-by-side
            </a>
            .
          </p>
          <p>
            One thing device-side capture does not change: tell people you are recording. The
            bot&rsquo;s single virtue was announcing itself. Remove it and consent becomes your
            job, which is where it belonged in the first place. If you need the legal detail,
            see{" "}
            <a
              href="/resources/is-it-legal-to-record-a-meeting"
              className="text-[var(--accent)] hover:underline"
            >
              recording consent law by state
            </a>
            .
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
            href="/resources/remove-ai-notetaker-bots-from-meetings"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Other notetakers &amp; platforms
          </a>
        </div>
      </section>

      <FaqSection items={faqs} />

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
