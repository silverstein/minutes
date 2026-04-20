import type { Metadata } from "next";
import { CopyButton } from "@/components/copy-button";
import { PublicFooter } from "@/components/public-footer";
import surfaces from "@/lib/product-surfaces.json";

export const metadata: Metadata = {
  title: "Minutes — the audio layer for agent memory",
  description:
    "The open-source audio layer for agent memory. Minutes captures meetings locally and writes structured markdown to ~/meetings/. Claude Code, Codex, Gemini CLI, Cursor, and OpenCode all read from the same folder. No cloud, no SDK, no API key.",
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
  - speaker_label: SPEAKER_0
    name: mat
    confidence: high
    source: manual
  - speaker_label: SPEAKER_1
    name: alex
    confidence: medium
    source: llm
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
          Open-source. MCP-native.
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          The meeting corpus your agents{" "}
          <span className="italic text-[var(--accent)]">read as files</span>.
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Cloud meeting tools hold your conversations in their database behind
          their API. Minutes writes every meeting and voice memo to structured
          markdown in{" "}
          <code className="font-mono text-[15px] text-[var(--text)]">~/meetings/</code>
          {" "}on your own disk. Claude Code, Codex, Gemini CLI, Cursor, OpenCode,
          and any MCP-compatible client read from the same folder. No SDK. No
          API key. Your corpus survives tools, vendors, and hype cycles.
        </p>
        <p className="mt-4 text-[15px] leading-7 text-[var(--text-secondary)]">
          This page is the integration reference. MCP config, tool surface,
          frontmatter schema, and task recipes below. For the full generated
          index, see{" "}
          <a href="/llms.txt" className="text-[var(--accent)] hover:underline">
            llms.txt
          </a>
          .
        </p>
      </section>

      {/* Try in 60 seconds */}
      <section className="mt-8 max-w-[760px]">
        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Try it in 60 seconds
          </p>
          <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">
            One command drops a 5-meeting fixture corpus into{" "}
            <code className="font-mono text-[12px] text-[var(--text)]">
              ~/.minutes/demo/
            </code>
            , prints the MCP config with a{" "}
            <code className="font-mono text-[12px] text-[var(--text)]">MEETINGS_DIR</code>{" "}
            env override, and lists questions to ask. No signup, no API key.
            Basic search and list tools work immediately. Structured tools
            (consistency report, person profiles) auto-install the Minutes CLI
            on first call.
          </p>
          <div className="mt-4 flex items-center gap-2 rounded-[6px] bg-[var(--bg)] px-4 py-3 font-mono text-[13px] text-[var(--text)]">
            <code className="flex-1 overflow-x-auto">
              npx minutes-mcp --demo
            </code>
            <CopyButton label="Copy" cmd="npx minutes-mcp --demo" />
          </div>
          <p className="mt-4 text-[12px] leading-6 text-[var(--text-secondary)]">
            Paste the printed config into your agent host. Try:{" "}
            <em className="font-normal text-[var(--text)]">
              &ldquo;What did we decide about pricing? Which decision is
              current?&rdquo;
            </em>
          </p>
        </div>
      </section>

      {/* Shape of the category */}
      <section className="mt-10 max-w-[760px]">
        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Shape of the category
          </p>
          <div className="mt-4 grid gap-x-6 gap-y-3 text-[13px] leading-6 sm:grid-cols-[200px_1fr]">
            <div className="font-mono text-[var(--text-secondary)]">
              Granola, Fireflies, Otter
            </div>
            <div className="text-[var(--text)]">
              Cloud database. Closed API. Data lives in their app.
            </div>
            <div className="font-mono text-[var(--text-secondary)]">
              Agent-memory SDKs
            </div>
            <div className="text-[var(--text)]">
              Cloud-hosted memory. Proprietary SDK. API key required.
            </div>
            <div className="font-mono text-[var(--accent)]">Minutes</div>
            <div className="text-[var(--text)]">
              Local capture. Markdown on your disk. Any agent reads from{" "}
              <code className="font-mono text-[13px]">~/meetings/</code>. MIT.
            </div>
          </div>
          <p className="mt-5 text-[13px] leading-6 text-[var(--text-secondary)]">
            Ten years from now,{" "}
            <code className="font-mono text-[13px] text-[var(--text)]">grep</code>{" "}
            still works on your corpus.
          </p>
        </div>
      </section>

      {/* Agent compatibility */}
      <section className="mt-10 max-w-[760px]">
        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Agent compatibility
          </p>
          <p className="mt-3 text-[13px] leading-6 text-[var(--text-secondary)]">
            First-class support across every major agent runtime. Same folder,
            different hosts, no vendor lock-in.
          </p>
          <div className="mt-4 overflow-x-auto">
            <table className="w-full min-w-[520px] border-collapse text-[12px]">
              <thead>
                <tr className="border-b border-[color:var(--border)]">
                  <th className="py-2 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                    Agent
                  </th>
                  <th className="py-2 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                    Native skills
                  </th>
                  <th className="py-2 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                    MCP tools
                  </th>
                  <th className="py-2 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                    Setup
                  </th>
                </tr>
              </thead>
              <tbody className="font-mono leading-6 text-[var(--text)]">
                <tr className="border-b border-[color:var(--border)]">
                  <td className="py-2 pr-3">Claude Code</td>
                  <td className="py-2 pr-3">18 skills + 2 hooks</td>
                  <td className="py-2 pr-3">26 tools</td>
                  <td className="py-2">
                    <code className="text-[11px]">/plugin install minutes@minutes</code>
                  </td>
                </tr>
                <tr className="border-b border-[color:var(--border)]">
                  <td className="py-2 pr-3">Claude Desktop</td>
                  <td className="py-2 pr-3 text-[var(--text-secondary)]">—</td>
                  <td className="py-2 pr-3">26 tools + MCP App</td>
                  <td className="py-2">
                    <code className="text-[11px]">npx minutes-mcp</code>{" "}
                    <span className="text-[var(--text-secondary)]">or .mcpb</span>
                  </td>
                </tr>
                <tr className="border-b border-[color:var(--border)]">
                  <td className="py-2 pr-3">Codex</td>
                  <td className="py-2 pr-3">18 skills via <code className="text-[11px]">.agents/</code></td>
                  <td className="py-2 pr-3">26 tools</td>
                  <td className="py-2">
                    <code className="text-[11px]">npx minutes-mcp</code>
                  </td>
                </tr>
                <tr className="border-b border-[color:var(--border)]">
                  <td className="py-2 pr-3">Gemini CLI</td>
                  <td className="py-2 pr-3">18 skills via <code className="text-[11px]">.agents/</code></td>
                  <td className="py-2 pr-3">26 tools</td>
                  <td className="py-2">
                    <code className="text-[11px]">npx minutes-mcp</code>
                  </td>
                </tr>
                <tr className="border-b border-[color:var(--border)]">
                  <td className="py-2 pr-3">Cursor</td>
                  <td className="py-2 pr-3 text-[var(--text-secondary)]">—</td>
                  <td className="py-2 pr-3">26 tools</td>
                  <td className="py-2">
                    <code className="text-[11px]">npx minutes-mcp</code>{" "}
                    <span className="text-[var(--text-secondary)]">in Cursor MCP settings</span>
                  </td>
                </tr>
                <tr className="border-b border-[color:var(--border)]">
                  <td className="py-2 pr-3">OpenCode</td>
                  <td className="py-2 pr-3">
                    18 skills + <code className="text-[11px]">/minutes-*</code> commands
                  </td>
                  <td className="py-2 pr-3">26 tools</td>
                  <td className="py-2">
                    <code className="text-[11px]">.opencode/</code>{" "}
                    <span className="text-[var(--text-secondary)]">auto-discovered</span>
                  </td>
                </tr>
                <tr>
                  <td className="py-2 pr-3">Any MCP client</td>
                  <td className="py-2 pr-3 text-[var(--text-secondary)]">—</td>
                  <td className="py-2 pr-3">26 tools</td>
                  <td className="py-2">
                    <code className="text-[11px]">npx minutes-mcp</code>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
          <p className="mt-4 text-[12px] leading-6 text-[var(--text-secondary)]">
            Every agent reads the same{" "}
            <code className="font-mono text-[12px] text-[var(--text)]">~/meetings/</code>{" "}
            folder. Switch hosts without migrating data.
          </p>
        </div>
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
          user&apos;s environment. This matrix is source-backed so the install
          steps stay aligned with the docs index and generated agent artifacts.
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
              <p className="mt-2 text-[13px] leading-6 text-[var(--text-secondary)]">
                <span className="font-medium text-[var(--text)]">Best for:</span>{" "}
                {s.activation}
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
          MCP tools and the CLI. The full field-by-field schema — every required
          and optional field, with examples and stability guarantees — is at{" "}
          <a
            href="https://github.com/silverstein/minutes/blob/main/docs/frontmatter-schema.md"
            className="text-[var(--accent)] hover:underline"
          >
            docs/frontmatter-schema.md
          </a>
          . That page is the interop contract: any tool that wants to read or
          produce Minutes-compatible output should target it.
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
