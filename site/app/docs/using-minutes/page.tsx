import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Using Minutes — the desktop app guide",
  description:
    "How to use the Minutes desktop app: recording, the command palette and shortcuts, dictation, voice memos, getting names right, consent settings, and health checks.",
  alternates: {
    canonical: "/docs/using-minutes",
  },
};

const sections = [
  {
    label: "Recording",
    items: [
      [
        "Record a meeting",
        "Click Start Recording in the main window, or use the command palette. Audio is captured and transcribed locally; the meeting lands as markdown in ~/meetings/ with speakers, action items, and decisions.",
      ],
      [
        "Calls (Zoom, Teams, Webex)",
        "Minutes detects desktop call apps and shows a \"Call detected\" banner; record from it to capture both sides natively (no virtual audio devices). Google Meet and Teams-in-browser are experimental toggles in Settings.",
      ],
      [
        "Stop and process",
        "Stop from the window, tray, or palette. Transcription, speaker separation, and structured extraction run locally; the file opens when ready.",
      ],
    ],
  },
  {
    label: "Command palette",
    items: [
      [
        "Open from anywhere: ⌘⇧K",
        "The palette is a global macOS shortcut; it works even when Minutes is in the background. Start or stop recordings, add notes, search transcripts, or jump to the latest meeting without leaving the keyboard.",
      ],
      [
        "If ⌘⇧K collides with your IDE",
        "Settings > Command Palette offers ⌘⇧O and ⌘⇧U as alternates, or disable the global binding entirely ([palette] shortcut_enabled = false in config.toml).",
      ],
      [
        "Context-aware entries",
        "The palette shows what is currently possible: Stop recording only appears while recording; sensitive-meeting controls appear only when that mode is active.",
      ],
    ],
  },
  {
    label: "Dictation",
    items: [
      [
        "Hold the hotkey, speak, release",
        "Dictation sends your words to the clipboard and (optionally) your daily note. Configure the shortcut and destination in Settings > Dictation.",
      ],
      [
        "Quick thoughts",
        "The Quick Thought button (or its palette entry) records a short voice memo and files it with your meetings, transcribed and searchable.",
      ],
    ],
  },
  {
    label: "Voice memos",
    items: [
      [
        "iPhone to desktop",
        "Point the folder watcher at your iCloud Voice Memos folder ([watch] in config.toml, or `minutes watch` from the CLI) and recordings from your phone transcribe automatically on your Mac.",
      ],
    ],
  },
  {
    label: "Getting names right",
    items: [
      [
        "Identity",
        "Settings > Identity holds your name, name variants, and email addresses. These feed transcription hints and how you are labeled in your own meetings.",
      ],
      [
        "Vocabulary",
        "Names and terms the transcriber mishears can be taught: `minutes vocabulary add \"Geert Theys\"` from the CLI, or click Remember next to a correctly attributed speaker in any meeting view. Entries bias future transcription toward the right spelling.",
      ],
      [
        "Calendar attendees",
        "With Full Calendar access granted, attendee names from the current event feed the same hints automatically.",
      ],
    ],
  },
  {
    label: "Consent",
    items: [
      [
        "Recording disclosure",
        "Settings > Privacy holds the consent controls: a reminder mode (default) that shows your disclosure script before meeting recordings, a Require mode that blocks recording until you confirm everyone will be told, and a per-file consent record in every meeting's frontmatter. A disclosure aid, not legal advice.",
      ],
    ],
  },
  {
    label: "Health",
    items: [
      [
        "When something seems off",
        "The Readiness panel (and `minutes health` from the CLI) reports the observed state of models, microphone, calendar access, and the transcription backend, with the exact fix path when something is degraded.",
      ],
    ],
  },
] as const;

export default function UsingMinutesPage() {
  return (
    <div className="mx-auto max-w-[760px] px-6 pb-16 sm:px-8">
      <nav className="flex items-center justify-between border-b border-[color:var(--border)] py-4">
        <a
          href="/"
          className="font-mono text-[15px] font-medium text-[var(--text)]"
        >
          minutes
        </a>
        <div className="flex gap-x-6 text-sm text-[var(--text-secondary)]">
          <a href="/docs" className="hover:text-[var(--accent)]">
            Docs
          </a>
          <a
            href="https://github.com/silverstein/minutes"
            className="hover:text-[var(--accent)]"
          >
            GitHub
          </a>
        </div>
      </nav>

      <header className="pb-8 pt-14">
        <p className="mb-4 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          User guide
        </p>
        <h1 className="font-serif text-[36px] leading-tight tracking-[-0.04em] text-[var(--text)]">
          Using Minutes
        </h1>
        <p className="mt-4 max-w-[600px] text-[15px] leading-7 text-[var(--text-secondary)]">
          The desktop app, end to end: recording, the command palette,
          dictation, voice memos, names, consent, and health. For wiring
          Minutes into Claude, Codex, or other agents, see{" "}
          <a
            href="/for-agents"
            className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
          >
            For agents
          </a>
          .
        </p>
      </header>

      <div className="space-y-12 border-t border-[color:var(--border)] pt-10">
        {sections.map((section) => (
          <section key={section.label}>
            <p className="mb-5 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
              {section.label}
            </p>
            <div className="space-y-4">
              {section.items.map(([title, description]) => (
                <div key={title} className="flex gap-3 text-sm">
                  <span className="mt-0.5 font-mono text-[12px] text-[var(--accent)]">
                    &gt;
                  </span>
                  <p className="leading-6 text-[var(--text-secondary)]">
                    <strong className="font-medium text-[var(--text)]">
                      {title}.
                    </strong>{" "}
                    {description}
                  </p>
                </div>
              ))}
            </div>
          </section>
        ))}
      </div>

      <p className="mt-12 border-t border-[color:var(--border)] pt-6 text-[13px] leading-6 text-[var(--text-secondary)]">
        Deeper reference lives in the repo:{" "}
        <a
          href="https://github.com/silverstein/minutes#readme"
          className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
        >
          README
        </a>
        {" · "}
        <a
          href="https://github.com/silverstein/minutes/blob/main/docs/architecture/config.md"
          className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
        >
          CONFIG.md
        </a>
        {" · "}
        <a
          href="https://github.com/silverstein/minutes/blob/main/docs/architecture/audio-devices.md"
          className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
        >
          Audio device guide
        </a>
      </p>
    </div>
  );
}
