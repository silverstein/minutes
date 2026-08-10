import type { Metadata } from "next";
import { FaqSection } from "@/components/faq-section";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";
import { faqPageSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "How to turn off built-in AI notetakers in Zoom and Teams",
  description:
    "Zoom AI Companion and Teams Copilot are platform features, not bots you can remove from the participant list. The exact admin paths, the Teams setting that is only a default rather than an enforcement, and why your Zoom toggle is grayed out.",
  alternates: {
    canonical: "/resources/turn-off-built-in-ai-notetakers",
  },
};

const faqs = [
  {
    question: "How do I disable the AI notetaker in Zoom?",
    answer:
      "Sign in to the Zoom web portal and open Admin Center then Settings for the whole account, Admin Center then Users then Groups for one group, or your personal Settings for just yourself. Open Zoom AI and toggle the meeting summary features off. Admins can also click the lock icon to stop users changing it.",
  },
  {
    question: "Why is the Zoom AI Companion setting grayed out?",
    answer:
      "Because it has been locked above you. Zoom's documentation says that if a feature is grayed out it has been locked at the account or group level and must be changed by an admin. This catches account owners too, since a setting locked at group level still shows as unavailable in a personal settings page.",
  },
  {
    question: "How do I turn off Copilot in Microsoft Teams meetings?",
    answer:
      "In the Teams admin center go to Meetings, then Meeting policies, then the Recording & transcription section, and set Copilot. The dropdown offers On, On with saved transcript required, On with transcript saved by default, and Off. Assign the policy to the users or groups it should cover.",
  },
  {
    question: "Does setting Teams Copilot to Off actually stop it?",
    answer:
      "Not necessarily. Microsoft states that the only Copilot policy setting you can enforce is \"On with saved transcript required\"; the others create a default that your organizers can change. So an admin who selects Off has expressed a default, not a guarantee, and an organizer can still turn Copilot on for their own meeting.",
  },
  {
    question: "Can I remove a built-in AI notetaker from a meeting like a bot?",
    answer:
      "No, and this is the main difference from tools like Otter or Fireflies. Those join as participants, so you can remove them from the participant list. Zoom AI Companion and Teams Copilot are features of the platform itself, running inside the service that hosts your call. There is nothing in the participant list to eject, so the only controls are the account, policy, and per-meeting settings.",
  },
] as const;

const sources = [
  {
    label: "Zoom support: enabling or disabling Zoom AI meeting summary (account, group, and user level)",
    href: "https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0057623",
  },
  {
    label: "Microsoft Learn: manage Microsoft 365 Copilot in Teams meetings and events",
    href: "https://learn.microsoft.com/en-us/microsoftteams/copilot-teams-transcription",
  },
  {
    label: "Microsoft Learn: admins, manage transcription and captions for Teams meetings",
    href: "https://learn.microsoft.com/en-us/microsoftteams/meeting-transcription-captions",
  },
  {
    label: "Microsoft Learn: recording and transcription options for sensitive meetings",
    href: "https://learn.microsoft.com/en-us/microsoftteams/manage-meeting-recording-options",
  },
  { label: "Minutes security & privacy architecture", href: "/security" },
] as const;

export default function BuiltInAiNotetakersPage() {
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
            href="/resources/turn-off-built-in-ai-notetakers.md"
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
          How to turn off built-in AI notetakers in Zoom and Teams
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Zoom AI Companion and Teams Copilot are a different problem from Otter or Fireflies.
          Those arrive as participants you can eject. These are features of the platform
          hosting your call, so there is nothing in the participant list to remove, and the
          controls live in settings you may not own. Here are the exact paths, and the two
          places where turning it off does not do what it looks like.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-08-10
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            How-to guide
          </span>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Why This One Is Different" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            A third-party notetaker has to get into your meeting the way a person does, by
            dialing in as a participant. That is what makes it removable: it shows up in the
            list, and you can eject it. Built-in AI is not in the list because it never joined.
            It runs inside the service that is already carrying your audio.
          </p>
          <p>
            The practical consequence is that every control here is a setting rather than an
            action, and the setting is often owned by someone else. If you are not an admin, the
            most reliable move in the room is still the social one: ask the organizer to turn it
            off, and confirm they did.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Zoom AI Companion" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>Sign in to the Zoom web portal, then take the path that matches your scope:</p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              <span className="font-medium text-[var(--text)]">Whole account:</span>{" "}
              Admin Center → Settings
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">One group:</span> Admin Center →
              Users → Groups, then select the group
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Just you:</span> Settings
            </li>
          </ul>
          <p>
            In any of those, open{" "}
            <span className="font-medium text-[var(--text)]">Zoom AI</span> and toggle the
            meeting summary features off. Admins changing an account or group can also click the
            lock icon, which prevents users below them from turning it back on.
          </p>
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              If the toggle is grayed out
            </p>
            <p className="mt-3">
              It has been locked above you. Zoom&rsquo;s documentation is explicit: if a feature
              is grayed out, it has been locked at the account or group level and must be
              changed by an admin. This is worth knowing because it catches account owners too,
              who reasonably expect their own settings page to be authoritative. A setting locked
              at the group level still reads as unavailable in your personal settings, so the
              fix is to change it at the level where the lock was applied rather than the level
              where you noticed it.
            </p>
          </div>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Microsoft Teams Copilot" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            In the Teams admin center, go to{" "}
            <span className="font-medium text-[var(--text)]">
              Meetings → Meeting policies
            </span>
            , open the{" "}
            <span className="font-medium text-[var(--text)]">Recording &amp; transcription</span>{" "}
            section, and set{" "}
            <span className="font-medium text-[var(--text)]">Copilot</span>. The dropdown offers
            four options:
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>On</li>
            <li>On with saved transcript required</li>
            <li>On with transcript saved by default</li>
            <li>Off</li>
          </ul>
          <p>Assign the policy to the users or groups it should apply to.</p>
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              &ldquo;Off&rdquo; is a default, not an enforcement
            </p>
            <p className="mt-3">
              This is the detail that catches people, and it comes from Microsoft&rsquo;s own
              documentation:{" "}
              <span className="font-medium text-[var(--text)]">
                the only Copilot policy setting you can enforce is &ldquo;On with saved
                transcript required.&rdquo;
              </span>{" "}
              The others create a default that your organizers can change.
            </p>
            <p className="mt-3">
              So an admin who selects Off has expressed a preference, not a guarantee. An
              organizer can still switch Copilot on for their own meeting. If your compliance
              posture assumes the tenant setting is binding, that assumption is worth testing
              before you rely on it, and enforcement of the outcome you actually want may need
              to come from policy and training rather than the dropdown.
            </p>
          </div>
          <p>
            Because Copilot builds on the transcript, transcription policy is the other lever
            worth reviewing at the same time. Check what your meeting policies set for
            transcription rather than assuming it matches the Copilot setting.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What If You Are Not The Admin" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Most people asking this question do not run the tenant. Honest options, in order of
            how well they work:
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              <span className="font-medium text-[var(--text)]">Ask the organizer.</span> They
              hold the per-meeting controls in both platforms. &ldquo;Could we run this one
              without the AI summary?&rdquo; is a normal request now.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Say it before the substance.</span>{" "}
              If a call turns sensitive, raising it at that moment is late but still better than
              not raising it; summaries are generated from the whole meeting.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Escalate to whoever owns the tenant.</span>{" "}
              For a recurring concern, the durable fix is a policy change, not a per-meeting
              request repeated forever.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Move the conversation.</span> For
              genuinely confidential discussion, a platform whose vendor is not summarizing by
              default is a cleaner answer than fighting settings you do not control.
            </li>
          </ul>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Version Of This Problem That Solves Itself" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Notice what all of the above has in common: the notes are being produced by the
            company hosting your call, using settings owned by someone in your organization, and
            your ability to say no depends on where you sit in an admin hierarchy. That is a
            governance arrangement, not a product feature you can toggle away.
          </p>
          <p>
            The alternative is to let the platform do the call and keep the record yourself.{" "}
            <span className="font-medium text-[var(--text)]">Minutes</span> records device-side,
            transcribes locally with whisper.cpp, and writes markdown to your own disk, so the
            notes exist because you made them rather than because a tenant policy allowed it. It
            does not disable anything the platform is doing, and it should not be read as a way
            around your organization&rsquo;s rules. It changes who holds the copy you rely on.
          </p>
          <p>
            To be exact about our own limits: transcript text leaves your machine only if you
            deliberately configure a provider-backed summarizer, which is off by default and
            documented on our{" "}
            <a href="/security" className="text-[var(--accent)] hover:underline">
              security page
            </a>
            . And for third-party bots, which are a different problem with different fixes, see{" "}
            <a
              href="/resources/remove-ai-notetaker-bots-from-meetings"
              className="text-[var(--accent)] hover:underline"
            >
              removing AI notetaker bots
            </a>
            .
          </p>
          <p>
            One thing device-side capture does not change: tell people you are recording, and
            follow your own organization&rsquo;s policy on it. The legal detail is in{" "}
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
            Third-party bots
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
        <p className="mt-6 text-[13px] leading-7 text-[var(--text-secondary)]">
          Admin interfaces in both products change often. Verify the current path in your own
          tenant before relying on any of this, and confirm the effect on a test meeting rather
          than assuming a saved setting took effect.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
