import type { Metadata } from "next";
import { FaqSection } from "@/components/faq-section";
import { PublicFooter } from "@/components/public-footer";
import { faqPageSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "How to stop Fireflies from joining your meetings",
  description:
    "Remove Fred within three minutes and Fireflies creates no transcript at all. Then the two settings that stop it returning, the recording rules that outrank them, and what to do when the bot belongs to someone else.",
  alternates: {
    canonical: "/resources/stop-fireflies-from-joining-meetings",
  },
};

const faqs = [
  {
    question: "How do I stop Fireflies from joining my meetings?",
    answer:
      "Open Settings, then Recording & Privacy, and toggle off Auto-record meetings. That stops Fireflies joining any future meeting on your connected calendar. If you still want it sometimes, use the Calendar meeting settings dropdown under Fireflies Notetaker and choose \"Only when I invite fred@fireflies.ai\" so it joins on invitation instead of automatically.",
  },
  {
    question: "If I remove Fireflies from a meeting, does it keep the recording?",
    answer:
      "Not if you are quick. Fireflies' own documentation states that if the notetaker is removed before 3 minutes elapse, no transcript or notes will be created. After that window a transcript exists and removing the bot only stops it capturing more.",
  },
  {
    question: "How do I remove Fireflies from a live Zoom, Meet, or Teams call?",
    answer:
      "In Zoom, open the participant list, click More next to Fireflies Notetaker, and choose Remove. In Google Meet, open the participant list, click the three-dot menu next to Fireflies, and select Remove from the call. In Microsoft Teams, open the participant list, click the ellipsis next to Fireflies, and select Remove from meeting.",
  },
  {
    question: "Why does Fireflies still join after I turned auto-join off?",
    answer:
      "Check your recording rules. Fireflies supports rules that target meetings by keyword, participant email address, or domain, and per its documentation those rules override the auto-join setting. A rule you created earlier can therefore keep pulling the notetaker into calls after you believe you disabled it.",
  },
  {
    question: "Can someone else make Fireflies join my meeting?",
    answer:
      "Yes. Fireflies joins when fred@fireflies.ai is added as a guest on the calendar invite, so any organizer can bring it into a meeting you attend without you having a Fireflies account. If it is on your calendar you can find the event under Upcoming Meetings in Fireflies and switch it off; otherwise you are down to removing the bot in the call or asking the organizer.",
  },
] as const;

const sources = [
  {
    label: "Fireflies guide: how to remove Fireflies from a meeting (or stop it from joining)",
    href: "https://guide.fireflies.ai/articles/7098191513-how-to-remove-fireflies-from-a-meeting-or-stop-it-from-joining",
  },
  {
    label: "Fireflies guide: how to disable the Fireflies auto-join settings",
    href: "https://guide.fireflies.ai/articles/8587670572-how-to-disable-the-fireflies-auto-join-settings",
  },
  {
    label: "Fireflies guide: learn about Fireflies auto-join settings",
    href: "https://guide.fireflies.ai/articles/5074225515-learn-about-fireflies-auto-join-settings",
  },
  {
    label: "Fireflies guide: how Fireflies joins and records your meetings (FAQs)",
    href: "https://guide.fireflies.ai/articles/9554534786-how-fireflies-joins-and-records-your-meetings-faqs",
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

export default function StopFirefliesJoiningPage() {
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
            href="/resources/stop-fireflies-from-joining-meetings.md"
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
          How to stop Fireflies from joining your meetings
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          There is a deadline on this one. Get Fred out fast enough and Fireflies keeps nothing
          at all. Below: the three-minute rule, the two settings that stop it returning, the
          rules that quietly outrank those settings, and what you can do when the bot belongs to
          somebody else.
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
        <SectionLabel label="The Three-Minute Rule" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
            <p>
              Fireflies&rsquo; own documentation states that if the notetaker is removed{" "}
              <span className="font-medium text-[var(--text)]">before 3 minutes elapse</span>,
              no transcript or notes will be created.
            </p>
          </div>
          <p>
            That makes the first three minutes of a call qualitatively different from every
            minute after. Eject Fred inside the window and there is nothing to delete, nothing
            to request, nothing sitting in someone else&rsquo;s workspace. Miss it and a
            transcript exists; removing the bot then only stops it capturing more.
          </p>
          <p>
            Worth knowing before the meeting rather than during it, which is the entire reason
            this page leads with it. If a call turns sensitive in the first minute, you have a
            real deadline and it is short.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Remove It Now" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>The bot appears as an ordinary participant. Per platform:</p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              <span className="font-medium text-[var(--text)]">Zoom.</span> Open the participant
              list, click <span className="font-medium text-[var(--text)]">More</span> next to
              Fireflies Notetaker, choose{" "}
              <span className="font-medium text-[var(--text)]">Remove</span>.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Google Meet.</span> Open the
              participant list, click the three-dot menu next to Fireflies, select{" "}
              <span className="font-medium text-[var(--text)]">Remove from the call</span>.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Microsoft Teams.</span> Open the
              participant list, click the ellipsis next to Fireflies, select{" "}
              <span className="font-medium text-[var(--text)]">Remove from meeting</span>.
            </li>
          </ul>
          <p>
            These are host-side participant controls. Fireflies&rsquo; documentation does not
            state whether a non-host can remove the notetaker, and it publishes no chat command
            for doing so, so if you are not the host, treat asking the organizer as the reliable
            path rather than assuming a shortcut exists.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Stop It Coming Back" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Turn it off entirely.</span>{" "}
            Settings → <span className="font-medium text-[var(--text)]">Recording &amp; Privacy</span>{" "}
            → Recording, then toggle off{" "}
            <span className="font-medium text-[var(--text)]">Auto-record meetings</span>. Per
            Fireflies, that stops the notetaker joining any future meeting scheduled on your
            connected calendar.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">Or keep it, on invitation only.</span>{" "}
            From the homepage <span className="font-medium text-[var(--text)]">Upcoming</span>{" "}
            button, open{" "}
            <span className="font-medium text-[var(--text)]">Calendar meeting settings</span>{" "}
            under Fireflies Notetaker. The default is every meeting with a web-conference link;
            switch it to{" "}
            <span className="font-medium text-[var(--text)]">
              &ldquo;Only when I invite fred@fireflies.ai&rdquo;
            </span>{" "}
            and it joins when you ask rather than by default. This is usually the setting people
            actually wanted.
          </p>
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              If it still joins, check your recording rules
            </p>
            <p className="mt-3">
              Fireflies supports rules that target meetings by keyword, participant email
              address, or domain, and per its documentation those rules{" "}
              <span className="font-medium text-[var(--text)]">override auto-join settings</span>.
              A rule you set up months ago can keep pulling Fred into calls long after you
              believe you switched auto-join off. Auto-join is the first place people look and
              the second place the answer usually is.
            </p>
          </div>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="When The Bot Is Not Yours" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Fireflies has a route into your meeting that does not require you to use Fireflies
            at all: it joins when{" "}
            <code className="rounded-[4px] bg-[var(--bg-elevated)] px-1.5 py-0.5 font-mono text-[13px] text-[var(--text)]">
              fred@fireflies.ai
            </code>{" "}
            is added as a guest on the calendar invite. Any organizer can do that. You do not
            need an account, and you were never asked.
          </p>
          <p>
            If the meeting is on your own connected calendar, Fireflies lets you find the event
            under <span className="font-medium text-[var(--text)]">Upcoming Meetings</span> and
            switch it off so the notetaker does not attend. If it is not your calendar, your
            options narrow to the two honest ones: remove the bot in the call if you are host,
            or say something. &ldquo;Could we do this one without the notetaker?&rdquo; costs
            nothing and works immediately, and the person who invited Fred can uninvite him in a
            click.
          </p>
          <p>
            For an organization, the durable version is a recording rule scoped to a domain,
            which is the same mechanism that causes the surprise above, used deliberately.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Version Of This Problem That Solves Itself" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Everything above manages a symptom. The bot exists because cloud notetakers need
            your meeting audio on their servers, and sending a synthetic participant into the
            call is how they collect it. Capture on your own device instead and the whole
            category evaporates: nothing joins, nothing shows in the participant list, no
            three-minute deadline, nothing to explain to a client.
          </p>
          <p>
            In fairness, Fireflies now offers bot-free capture of its own, through a Google Meet
            SDK integration and a desktop app. Worth knowing, and worth reading precisely: those
            remove the visible bot from the call, not the upload. The audio still goes to
            Fireflies.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">Minutes</span> removes both. It
            records device-side, transcribes locally with whisper.cpp, and writes markdown to
            your own disk, so there is no bot and no vendor copy. If you want the direct
            comparison, we keep one for{" "}
            <a
              href="/compare/fireflies-vs-minutes"
              className="text-[var(--accent)] hover:underline"
            >
              Fireflies and Minutes
            </a>
            .
          </p>
          <p>
            One thing device-side capture does not change: tell people you are recording. The
            bot&rsquo;s single virtue was announcing itself. Without it, consent is your job,
            which is where it belonged anyway. The legal detail is in{" "}
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

      <FaqSection items={faqs} />

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
