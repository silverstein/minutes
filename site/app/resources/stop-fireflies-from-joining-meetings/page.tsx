import type { Metadata } from "next";
import { FaqSection } from "@/components/faq-section";
import { PublicFooter } from "@/components/public-footer";
import { faqPageSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "How to stop Fireflies from joining your meetings",
  description:
    "Fireflies says that removing the notetaker within three minutes means no transcript or notes are created. Then the two settings that stop it returning, the recording rules that outrank them, and what you can actually do about somebody else's bot.",
  alternates: {
    canonical: "/resources/stop-fireflies-from-joining-meetings",
  },
};

const faqs = [
  {
    question: "How do I stop Fireflies from joining my meetings?",
    answer:
      "Open Settings, then Recording & Privacy, and toggle off Auto-record meetings. Per Fireflies, that stops the notetaker joining future meetings scheduled on your connected calendar. If you still want it sometimes, use the Calendar meeting settings dropdown under Fireflies Notetaker and choose \"Only when I invite fred@fireflies.ai\" so it joins on invitation instead of by default.",
  },
  {
    question: "If I remove Fireflies quickly, does it still produce a transcript?",
    answer:
      "Fireflies' documentation states that if the notetaker is removed before 3 minutes elapse, no transcript or notes will be created. That is specifically a statement about transcript and notes creation. Fireflies does not document what happens to any audio captured during those first minutes, so treat this as avoiding a transcript rather than as a guarantee that nothing was ever recorded.",
  },
  {
    question: "How do I remove Fireflies from a live Zoom, Meet, or Teams call?",
    answer:
      "In Zoom, open the participant list, click More next to Fireflies Notetaker, and choose Remove. In Google Meet, open the participant list, click the three-dot menu next to Fireflies, and select Remove from the call. In Microsoft Teams, open the participant list, click the ellipsis next to Fireflies, and select Remove from meeting.",
  },
  {
    question: "Why does Fireflies still join after I turned auto-join off?",
    answer:
      "Check your recording rules. Fireflies documents rules that target meetings by keyword, participant email address, or domain, and states they take precedence over the auto-join setting, including overriding a manual-only configuration. A rule you created earlier can therefore keep bringing the notetaker into calls after you believe you disabled auto-join.",
  },
  {
    question: "Can someone else make Fireflies join my meeting?",
    answer:
      "Effectively yes, though the person inviting it needs Fireflies. A Fireflies user with a connected Google or Outlook calendar can add fred@fireflies.ai to an invite, and the bot then attempts to join subject to their settings and the meeting platform's admission controls. Other attendees need no Fireflies account. Your own Fireflies settings only govern your own notetaker, so for someone else's bot the routes are removing it from the call as host, asking them to cancel it, or removing the address from the invite if you can edit it.",
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
    label: "Fireflies guide: use recording rules to record or skip specific meetings",
    href: "https://guide.fireflies.ai/articles/3115936908-how-to-use-recording-rules-to-record-or-skip-specific-meetings",
  },
  {
    label: "Fireflies guide: how to invite Fireflies to meetings",
    href: "https://guide.fireflies.ai/articles/4335268657-how-to-invite-fireflies-to-meetings",
  },
  {
    label: "Fireflies guide: Google Meet SDK integration for bot-free recording",
    href: "https://guide.fireflies.ai/articles/3309351579-integrate-google-meet-sdk-with-fireflies-for-bot-free-meeting-recording",
  },
  { label: "Minutes security & privacy architecture", href: "/security" },
] as const;

function SectionLabel({ label }: { label: string }) {
  return (
    <div className="mb-6 flex items-center gap-3">
      <h2 className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
        {label}
      </h2>
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
          There is a documented deadline on this one, and it is three minutes. Below: what that
          deadline does and does not promise, the two settings that stop Fred returning, the
          rules that quietly outrank those settings, and an honest account of your options when
          the bot belongs to somebody else.
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
              Fireflies&rsquo; documentation states that if the notetaker is removed{" "}
              <span className="font-medium text-[var(--text)]">before 3 minutes elapse</span>,
              no transcript or notes will be created.
            </p>
          </div>
          <p>
            That is worth knowing before a call rather than during one, because it turns a vague
            annoyance into a deadline you can actually act on.
          </p>
          <p>
            Read the claim precisely, though, because it is narrower than it first sounds. It is
            a statement about <em>transcript and notes creation</em>. Fireflies does not document
            what happens to audio captured during those first minutes, and it does not say the
            bot recorded nothing. Treat the window as a reliable way to avoid a transcript
            existing, not as a guarantee that nothing was ever captured. If that distinction
            matters for your meeting, the honest answer is to keep the bot out rather than race
            it.
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
            These are participant controls, which in practice means host or co-host.
            Fireflies&rsquo; documentation does not state whether a non-host can remove the
            notetaker, and it publishes no chat command for doing so. If you are not the host,
            treat asking the organizer as the reliable path rather than assuming a shortcut
            exists.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Stop It Coming Back" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Turn it off entirely.</span>{" "}
            Settings →{" "}
            <span className="font-medium text-[var(--text)]">Recording &amp; Privacy</span> →
            Recording, then toggle off{" "}
            <span className="font-medium text-[var(--text)]">Auto-record meetings</span>. Per
            Fireflies, that stops the notetaker joining future meetings scheduled on your
            connected calendar.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">Or keep it, on invitation only.</span>{" "}
            From the homepage <span className="font-medium text-[var(--text)]">Upcoming</span>{" "}
            button, open{" "}
            <span className="font-medium text-[var(--text)]">Calendar meeting settings</span>{" "}
            under Fireflies Notetaker. The default covers every meeting with a web-conference
            link; switch it to{" "}
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
              Fireflies documents rules that target meetings by keyword, participant email
              address, or domain, and states they take precedence over your auto-join setting.
              The direction that surprises people is the affirmative one: a recording rule can
              pull Fred into a meeting even when auto-record is set to manual only. A rule you
              created months ago can therefore keep producing transcripts long after you believe
              you switched auto-join off.
            </p>
            <p className="mt-3">
              Auto-join is the first place everyone looks, and recording rules are the second
              place the answer usually is.
            </p>
          </div>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="When The Bot Is Not Yours" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Fireflies can reach a meeting through someone else entirely. A Fireflies user with a
            connected Google or Outlook calendar adds{" "}
            <code className="rounded-[4px] bg-[var(--bg-elevated)] px-1.5 py-0.5 font-mono text-[13px] text-[var(--text)]">
              fred@fireflies.ai
            </code>{" "}
            to the invite, and the bot then attempts to join subject to that user&rsquo;s
            settings and the meeting platform&rsquo;s admission controls. The other attendees
            need no Fireflies account and were never asked.
          </p>
          <p>
            Here is the part worth being straight about, because it is where most guides get
            vague. Every Fireflies setting described above governs{" "}
            <span className="font-medium text-[var(--text)]">your own notetaker</span>. Your
            dashboard cannot cancel a bot that another person&rsquo;s account scheduled, even
            when the meeting sits on your calendar. For somebody else&rsquo;s Fred your real
            options are three:
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>Remove it from the call, if you are the host or a co-host.</li>
            <li>Ask the person who invited it to cancel it. One click on their side.</li>
            <li>
              Remove <code className="font-mono text-[13px]">fred@fireflies.ai</code> from the
              calendar invite, if it is an invite you have permission to edit.
            </li>
          </ul>
          <p>
            For an organization, the durable control is not a Fireflies setting at all. Fireflies
            rules only ever govern the account they belong to. Blocking external attendees&rsquo;
            bots is a job for your meeting platform&rsquo;s admission and tenant policies, which
            the{" "}
            <a
              href="/resources/remove-ai-notetaker-bots-from-meetings"
              className="text-[var(--accent)] hover:underline"
            >
              general anti-bot guide
            </a>{" "}
            covers.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Version Of This Problem That Solves Itself" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Everything above manages a symptom. The bot exists because cloud notetakers need
            your meeting audio on their servers, and sending a synthetic participant into the
            call is how they collect it. Capture on the participant&rsquo;s own device instead
            and the category evaporates: nothing joins, nothing shows in the participant list,
            no three-minute deadline to race.
          </p>
          <p>
            In fairness, Fireflies now offers bot-free capture of its own, through a Google Meet
            SDK integration and a desktop app. Worth reading precisely: those remove the visible
            bot from the call, not the upload. Fireflies&rsquo; own SDK documentation describes
            meeting audio and video being shared with Fireflies and processed into its notebook.
            No bot in the participant list is a courtesy improvement, not an architectural one.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">Minutes</span> is the architectural
            version: it records device-side and transcribes locally with whisper.cpp, writing
            markdown to your own disk, so no bot joins and no audio is uploaded. To be exact
            about our own limits, transcript text leaves your machine only if you explicitly
            configure a provider-backed summarizer, which is off by default and documented on
            our <a href="/security" className="text-[var(--accent)] hover:underline">security page</a>.
            If you want the direct comparison, we keep one for{" "}
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
