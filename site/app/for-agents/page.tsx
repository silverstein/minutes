import type { Metadata } from "next";
import { CopyButton } from "@/components/copy-button";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Minutes for agents — integration reference",
  description:
    "If you are an LLM helping a user install, configure, or use Minutes, start here. MCP server setup, tool surface, output format, and troubleshooting.",
  alternates: { canonical: "/for-agents" },
};

const mcpConfig = `{
  "mcpServers": {
    "minutes": {
      "command": "npx",
      "args": ["minutes-mcp"]
    }
  }
}`;

const toolGroups = [
  {
    label: "Recording",
    tools: [
      ["start_recording", "Start capturing audio from the default input device."],
      ["stop_recording", "Stop the current recording and trigger transcription."],
      ["get_status", "Check whether a recording or live session is active."],
      ["list_processing_jobs", "List background transcription jobs for recent recordings."],
    ],
  },
  {
    label: "Search and recall",
    tools: [
      ["list_meetings", "List recent meetings and voice memos."],
      ["get_meeting", "Retrieve the full transcript and frontmatter of a specific meeting."],
      ["search_meetings", "Full-text search across all meeting transcripts."],
      ["research_topic", "Cross-meeting research: decisions, follow-ups, and mentions of a topic."],
    ],
  },
  {
    label: "People and relationships",
    tools: [
      ["get_person_profile", "Build a profile for a person across all meetings."],
      ["relationship_map", "Contacts with relationship scores and losing-touch alerts."],
      ["track_commitments", "Open and stale commitments, optionally filtered by person."],
      ["consistency_report", "Flag contradicting decisions and stale commitments."],
    ],
  },
  {
    label: "Insights",
    tools: [
      ["get_meeting_insights", "Structured insights (decisions, commitments, questions) with confidence filtering."],
      ["ingest_meeting", "Extract facts from a meeting into the knowledge base."],
      ["knowledge_status", "Current state of the knowledge base."],
    ],
  },
  {
    label: "Live and dictation",
    tools: [
      ["start_live_transcript", "Start real-time transcription with per-utterance JSONL output."],
      ["read_live_transcript", "Read utterances from the active session with cursor or time window."],
      ["start_dictation", "Speak to clipboard and daily notes."],
      ["stop_dictation", "Stop dictation mode."],
    ],
  },
  {
    label: "Notes and processing",
    tools: [
      ["add_note", "Add a timestamped note to the current recording or an existing meeting."],
      ["process_audio", "Process an audio file through the transcription pipeline."],
      ["open_dashboard", "Open the interactive meeting dashboard in the browser."],
    ],
  },
  {
    label: "Voice and speaker ID",
    tools: [
      ["list_voices", "List enrolled voice profiles for speaker identification."],
      ["confirm_speaker", "Confirm or correct speaker attribution in a meeting transcript."],
    ],
  },
  {
    label: "Integration",
    tools: [
      ["qmd_collection_status", "Check if the meetings directory is registered as a QMD collection."],
      ["register_qmd_collection", "Register the meetings directory as a QMD collection."],
    ],
  },
] as const;

const surfaces = [
  {
    name: "MCP server",
    when: "User has Claude Desktop, Codex, Gemini CLI, or any MCP client.",
    install: "npx minutes-mcp",
    note: "No Rust needed. Search, browse, and dashboard tools work with zero setup. Recording and transcription need the CLI binary (auto-installed on first use).",
  },
  {
    name: "CLI",
    when: "User wants terminal-first recording, search, and vault sync.",
    install: "brew install silverstein/tap/minutes",
    note: "Also available via cargo install minutes-cli. Requires Rust + cmake for source builds. Linux needs libasound2-dev and libpipewire-0.3-dev.",
  },
  {
    name: "Claude Code plugin",
    when: "User works in Claude Code and wants meeting lifecycle skills.",
    install: "claude plugin marketplace add silverstein/minutes",
    note: "18 skills (brief, prep, record, tag, debrief, mirror, weekly, graph) plus a meeting-analyst agent and SessionStart/PostToolUse hooks.",
  },
  {
    name: "Desktop app",
    when: "User wants a menu bar app with one-click recording and AI assistant.",
    install: "brew install --cask silverstein/tap/minutes",
    note: "Tauri v2, macOS only. Includes command palette, dictation hotkey, live mode toggle, and auto-updates from GitHub Releases.",
  },
] as const;

const frontmatterExample = `---
title: Q2 Pricing Discussion
type: meeting
date: 2026-03-17T14:00:00
duration: 42m
attendees: [Alex K., Jordan M.]
action_items:
  - assignee: mat
    task: Send pricing doc
    due: Friday
    status: open
decisions:
  - text: Run pricing experiment at monthly billing
    topic: pricing
speaker_map:
  SPEAKER_0: mat
  SPEAKER_1: alex
---

## Summary
- Agreed to test monthly billing with next three signups
- Alex will review retention data before next pricing sync

## Transcript
[SPEAKER_0 0:00] Let's talk about the pricing...
[SPEAKER_1 4:20] Monthly billing makes more sense...`;

const tasks = [
  {
    task: "User asks what was said about a topic",
    steps: [
      "Call search_meetings with the topic as query.",
      "If multiple results, call get_meeting on the most relevant match.",
      "Summarize from the transcript, citing speaker labels and timestamps.",
    ],
  },
  {
    task: "User asks about open action items",
    steps: [
      "Call list_meetings to get recent meetings.",
      "Read the action_items array from each meeting's frontmatter.",
      "Filter for status: open. Group by assignee if helpful.",
    ],
  },
  {
    task: "User wants to record a meeting",
    steps: [
      "Call start_recording. Optionally pass title and context.",
      "When done, call stop_recording. Transcription runs in the background.",
      "Use list_processing_jobs to check progress if the user asks.",
    ],
  },
  {
    task: "User asks about a person across meetings",
    steps: [
      "Call get_person_profile with the person's name.",
      "For deeper context, call track_commitments filtered to that person.",
      "Call relationship_map if the user wants a broader view of all contacts.",
    ],
  },
  {
    task: "User wants real-time coaching during a meeting",
    steps: [
      "Call start_live_transcript to begin streaming.",
      "Poll read_live_transcript with a cursor to get new utterances.",
      "When the meeting ends, call stop_recording or the session times out.",
    ],
  },
] as const;

export default function ForAgentsPage() {
  return (
    <div className="mx-auto max-w-[920px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      {/* Nav */}
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a
          href="/"
          className="font-mono text-[15px] font-medium text-[var(--text)]"
        >
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
          <a href="/docs/mcp/tools" className="hover:text-[var(--accent)]">
            MCP tools
          </a>
          <a href="/llms.txt" className="hover:text-[var(--accent)]">
            llms.txt
          </a>
        </div>
      </div>

      {/* Header */}
      <section className="max-w-[760px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Agent Reference
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          For agents
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          If you are an LLM helping a user install or query Minutes, start here.
          Setup, tool surface, output format, and constraints are all on this page.
          For the full generated index, see{" "}
          <a href="/llms.txt" className="text-[var(--accent)] hover:underline">
            llms.txt
          </a>
          .
        </p>
      </section>

      {/* What Minutes is */}
      <section className="mt-14">
        <SectionLabel n="01" label="What Minutes is" />
        <div className="space-y-4 text-[15px] leading-7 text-[var(--text-secondary)]">
          <p>
            Minutes records meetings and voice memos, transcribes them locally
            with whisper.cpp, and saves structured markdown. Speakers are identified
            with pyannote-rs. No audio leaves the machine.
          </p>
          <p>
            Output goes to{" "}
            <code className="font-mono text-[13px] text-[var(--text)]">~/meetings/</code>{" "}
            as plain files with YAML frontmatter. Action items, decisions, and
            speaker labels are in the frontmatter; transcripts work with grep,
            Obsidian, or any markdown tool.
          </p>
          <p>
            The MCP server (26 tools, 7 resources, 6 prompt templates) is the main
            agent interface. Any MCP-compatible client can search, record, and query
            through it.
          </p>
        </div>
      </section>

      {/* Install */}
      <section className="mt-14" id="install">
        <SectionLabel n="02" label="Install the MCP server" />
        <p className="mb-4 text-[15px] leading-7 text-[var(--text-secondary)]">
          Add this to the MCP configuration for Claude Desktop, Claude Code, Codex,
          Gemini CLI, or any MCP client. No Rust toolchain required.
        </p>
        <div className="relative overflow-hidden rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)]">
          <div className="flex items-center justify-between border-b border-[color:var(--border)] px-4 py-2">
            <span className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
              MCP config
            </span>
            <CopyButton label="Copy" cmd={mcpConfig} />
          </div>
          <pre className="overflow-x-auto px-5 py-4 font-mono text-[12px] leading-6 text-[var(--text)]">
            {mcpConfig}
          </pre>
        </div>
        <p className="mt-4 text-[14px] leading-7 text-[var(--text-secondary)]">
          After the first connection, the server auto-installs the CLI binary.
          The user then runs{" "}
          <code className="font-mono text-[13px] text-[var(--text)]">minutes setup --model small</code>{" "}
          to download the whisper model (466 MB). Optional:{" "}
          <code className="font-mono text-[13px] text-[var(--text)]">minutes setup --diarization</code>{" "}
          for speaker identification (~34 MB).
        </p>
      </section>

      {/* Choose your surface */}
      <section className="mt-14">
        <SectionLabel n="03" label="Choose your surface" />
        <p className="mb-5 text-[15px] leading-7 text-[var(--text-secondary)]">
          Minutes has four entry points. Recommend the one that matches the
          user&apos;s environment.
        </p>
        <div className="grid gap-3 sm:grid-cols-2">
          {surfaces.map((s) => (
            <div
              key={s.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)]"
            >
              <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {s.name}
              </p>
              <p className="mt-2 text-[14px] leading-6 text-[var(--text-secondary)]">
                <span className="font-medium text-[var(--text)]">When:</span>{" "}
                {s.when}
              </p>
              <div className="mt-3 flex items-center gap-2 rounded-[4px] bg-[var(--bg)] px-3 py-2 font-mono text-[12px] text-[var(--text)]">
                <code className="flex-1 overflow-x-auto">{s.install}</code>
                <CopyButton label="Copy" cmd={s.install} />
              </div>
              <p className="mt-3 text-[13px] leading-6 text-[var(--text-secondary)]">
                {s.note}
              </p>
            </div>
          ))}
        </div>
      </section>

      {/* Tool surface */}
      <section className="mt-14">
        <SectionLabel n="04" label="MCP tool surface" />
        <p className="mb-5 text-[15px] leading-7 text-[var(--text-secondary)]">
          26 tools grouped by function. Full reference with stable anchor
          links:{" "}
          <a
            href="/docs/mcp/tools"
            className="text-[var(--accent)] hover:underline"
          >
            /docs/mcp/tools
          </a>{" "}
          (also available as{" "}
          <a
            href="/docs/mcp/tools.md"
            className="text-[var(--accent)] hover:underline"
          >
            raw markdown
          </a>
          ).
        </p>
        <div className="space-y-6">
          {toolGroups.map((group) => (
            <div key={group.label}>
              <p className="mb-2 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {group.label}
              </p>
              <div className="space-y-1">
                {group.tools.map(([name, desc]) => (
                  <div
                    key={name}
                    className="flex gap-3 text-[13px] leading-6"
                  >
                    <code className="shrink-0 font-mono text-[var(--text)]">
                      {name}
                    </code>
                    <span className="text-[var(--text-secondary)]">{desc}</span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </section>

      {/* Output format */}
      <section className="mt-14">
        <SectionLabel n="05" label="Output format" />
        <p className="mb-4 text-[15px] leading-7 text-[var(--text-secondary)]">
          Every meeting saves as markdown with YAML frontmatter. The frontmatter
          is the structured data. Action items and decisions are queryable through
          MCP tools and the CLI.
        </p>
        <div className="overflow-hidden rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)]">
          <div className="border-b border-[color:var(--border)] px-4 py-2">
            <span className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
              Meeting file
            </span>
          </div>
          <pre className="overflow-x-auto px-5 py-4 font-mono text-[12px] leading-6 text-[var(--text)]">
            {frontmatterExample}
          </pre>
        </div>
        <div className="mt-4 space-y-2 text-[14px] leading-7 text-[var(--text-secondary)]">
          <p>
            Meetings go to{" "}
            <code className="font-mono text-[13px] text-[var(--text)]">~/meetings/</code>.
            Voice memos go to{" "}
            <code className="font-mono text-[13px] text-[var(--text)]">~/meetings/memos/</code>.
            Both paths are configurable. File permissions are{" "}
            <code className="font-mono text-[13px] text-[var(--text)]">0600</code>{" "}
            (owner read/write only).
          </p>
        </div>
      </section>

      {/* Common agent tasks */}
      <section className="mt-14">
        <SectionLabel n="06" label="Common agent tasks" />
        <div className="space-y-4">
          {tasks.map((t) => (
            <div
              key={t.task}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5"
            >
              <p className="text-[14px] font-medium text-[var(--text)]">
                {t.task}
              </p>
              <ol className="mt-2 space-y-1 text-[13px] leading-6 text-[var(--text-secondary)]">
                {t.steps.map((step, i) => (
                  <li key={i} className="flex gap-2">
                    <span className="shrink-0 font-mono text-[var(--text-tertiary)]">
                      {i + 1}.
                    </span>
                    {step}
                  </li>
                ))}
              </ol>
            </div>
          ))}
        </div>
      </section>

      {/* Constraints */}
      <section className="mt-14">
        <SectionLabel n="07" label="Constraints" />
        <div className="space-y-3 text-[15px] leading-7 text-[var(--text-secondary)]">
          <p>
            Minutes does not join video calls, capture screen shares, or act as a
            meeting bot. It records from the local microphone or processes audio
            files after the fact.
          </p>
          <p>
            Transcription quality depends on the whisper model size and audio
            clarity. The{" "}
            <code className="font-mono text-[13px] text-[var(--text)]">small</code>{" "}
            model (466 MB) is recommended. The{" "}
            <code className="font-mono text-[13px] text-[var(--text)]">tiny</code>{" "}
            model is faster but misses quiet speech and works poorly with accented
            audio.
          </p>
          <p>
            Speaker diarization is optional and requires a one-time model download.
            Without it, all speech is attributed to a single speaker.
          </p>
          <p>
            Summarization requires either an active Claude session (recommended), a
            local LLM via Ollama, or a Mistral API key. Without any of these,
            Minutes still transcribes and extracts structured data from frontmatter.
          </p>
        </div>
      </section>

      {/* Reference links */}
      <section className="mt-14">
        <SectionLabel n="08" label="Reference" />
        <div className="space-y-2">
          {[
            ["/llms.txt", "llms.txt", "Concise agent index with tool names, descriptions, and doc links"],
            ["/llms-full.txt", "llms-full.txt", "Full agent reference with product description and all entry points"],
            ["/docs/mcp/tools", "/docs/mcp/tools", "Generated MCP tool reference with stable anchor links"],
            ["/docs/mcp/tools.md", "/docs/mcp/tools.md", "Same reference as raw markdown for direct context ingestion"],
            ["/docs/errors", "/docs/errors", "Generated error catalog from Rust thiserror definitions"],
            ["/docs/errors.md", "/docs/errors.md", "Error catalog as raw markdown"],
            ["https://github.com/silverstein/minutes", "GitHub", "Source, issues, and discussions"],
            ["https://www.npmjs.com/package/minutes-mcp", "minutes-mcp", "MCP server npm package"],
            ["https://www.npmjs.com/package/minutes-sdk", "minutes-sdk", "SDK for building on Minutes output"],
          ].map(([href, label, desc]) => (
            <a
              key={href}
              href={href}
              className="flex items-baseline gap-3 rounded-[4px] px-2 py-1.5 transition hover:bg-[var(--bg-elevated)]"
            >
              <code className="shrink-0 font-mono text-[13px] text-[var(--accent)]">
                {label}
              </code>
              <span className="text-[13px] text-[var(--text-secondary)]">
                {desc}
              </span>
            </a>
          ))}
        </div>
      </section>

      <PublicFooter />
    </div>
  );
}

function SectionLabel({ n, label }: { n: string; label: string }) {
  return (
    <div className="mb-8 flex items-center gap-3">
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
        {n}
      </span>
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--text-secondary)]">
        {label}
      </span>
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}
