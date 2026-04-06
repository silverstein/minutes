import { DemoPlayer } from "@/components/demo-player";
import { CopyButton } from "@/components/copy-button";
import { TopologyLoader } from "@/components/topology-loader";

function SectionLabel({ n, label }: { n: string; label: string }) {
  return (
    <div className="flex items-center gap-3 mb-8">
      <span className="font-mono text-[11px] text-[#444]">{n}</span>
      <span className="text-xs text-[#444] uppercase tracking-[0.15em]">{label}</span>
      <div className="flex-1 h-px bg-white/[0.04]" />
    </div>
  );
}

export default function Home() {
  return (
    <div className="max-w-[800px] mx-auto px-6 relative z-10">
      {/* Nav */}
      <nav className="sticky top-0 z-50 flex items-center justify-between py-4 border-b border-white/[0.06] bg-black/80 backdrop-blur-lg">
        <div className="font-mono text-[15px] font-medium text-[#ededed]">
          minutes
        </div>
        <div className="flex gap-6 text-sm max-sm:gap-4 max-sm:text-xs">
          <a href="https://github.com/silverstein/minutes" className="text-[#555] hover:text-[#ededed] transition-colors">GitHub</a>
          <a href="https://github.com/silverstein/minutes#install" className="text-[#555] hover:text-[#ededed] transition-colors">Install</a>
          <a href="https://github.com/silverstein/minutes#claude-integration" className="text-[#555] hover:text-[#ededed] transition-colors">Claude</a>
          <a href="/llms.txt" className="text-[#555] hover:text-[#ededed] transition-colors">llms.txt</a>
        </div>
      </nav>

      {/* Hero */}
      <section className="relative pt-24 pb-16 text-center max-sm:pt-14 max-sm:pb-10">
        <TopologyLoader />
        <div className="absolute -top-[30%] left-1/2 -translate-x-1/2 w-[800px] h-[600px] bg-[radial-gradient(ellipse_at_center,rgba(0,112,243,0.06)_0%,rgba(168,85,247,0.03)_35%,transparent_65%)] pointer-events-none" />

        <h1 className="relative text-[52px] max-sm:text-[36px] font-bold leading-[1.05] mb-5 tracking-[-0.045em] bg-gradient-to-b from-white to-[#999] bg-clip-text text-transparent">
          Open-source conversation<br />memory.
        </h1>
        <p className="relative text-[17px] max-sm:text-[15px] text-[#888] max-w-[520px] mx-auto mb-10 leading-relaxed">
          Agents have run logs. Humans have conversations. Minutes captures the human side and makes it queryable. Local, open source, free forever.
        </p>

        {/* CTAs */}
        <div className="relative flex gap-3 justify-center mb-10 max-sm:flex-col max-sm:items-center">
          <a
            href="https://github.com/silverstein/minutes#install"
            className="inline-flex items-center gap-2 px-6 py-2.5 bg-white text-black text-sm font-medium rounded-[3px] hover:bg-[#e0e0e0] transition-colors"
          >
            Get started
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className="mt-px"><path d="M6 3l5 5-5 5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/></svg>
          </a>
          <a
            href="https://github.com/silverstein/minutes"
            className="inline-flex items-center gap-2 px-6 py-2.5 border border-white/[0.12] text-sm text-[#888] rounded-[3px] hover:text-[#ededed] hover:border-white/[0.2] transition-colors"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>
            View on GitHub
          </a>
        </div>

        {/* Demo */}
        <div className="relative mb-12">
          <DemoPlayer />
        </div>

        {/* Download */}
        <div id="install" className="flex gap-3 justify-center flex-wrap mb-4 max-sm:flex-col max-sm:items-center">
          <a
            href="https://github.com/silverstein/minutes/releases/latest/download/Minutes_0.10.1_aarch64.dmg"
            className="inline-flex items-center gap-2 px-5 py-2 bg-[#111] border border-white/[0.1] text-sm text-[#ededed] rounded-[3px] hover:bg-[#1a1a1a] hover:border-white/[0.18] transition-colors"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
            Mac (Apple Silicon)
          </a>
          <a
            href="https://github.com/silverstein/minutes/releases/latest/download/minutes-desktop-windows-x64-setup.exe"
            className="inline-flex items-center gap-2 px-5 py-2 bg-[#111] border border-white/[0.1] text-sm text-[#ededed] rounded-[3px] hover:bg-[#1a1a1a] hover:border-white/[0.18] transition-colors"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
            Windows
          </a>
        </div>
        <p className="text-[12px] text-[#444] mb-6">Download, install, done. First launch downloads a speech model.</p>

        {/* Developer install */}
        <div className="flex gap-3 justify-center flex-wrap mb-3 max-sm:flex-col max-sm:items-center">
          <CopyButton label="Homebrew (desktop)" cmd="brew install --cask silverstein/tap/minutes" />
          <CopyButton label="Homebrew (CLI)" cmd="brew tap silverstein/tap && brew install minutes" />
          <CopyButton label="MCP server" cmd="npx minutes-mcp" />
        </div>
        <p className="text-[13px] text-[#555]">
          macOS, Windows, Linux. <code className="font-mono text-[12px] text-[#888]">npx</code> works everywhere.
        </p>

        {/* Works with */}
        <div className="mt-12 pt-8 border-t border-white/[0.04]">
          <p className="text-[10px] text-[#444] uppercase tracking-[0.2em] mb-4">Works with any MCP client</p>
          <div className="flex items-center justify-center gap-8 text-[#444] text-sm max-sm:gap-4 max-sm:text-xs flex-wrap">
            <span>Claude Code</span>
            <span className="text-[#222]">/</span>
            <span>Codex</span>
            <span className="text-[#222]">/</span>
            <span>Gemini CLI</span>
            <span className="text-[#222]">/</span>
            <span>Claude Desktop</span>
            <span className="text-[#222]">/</span>
            <span>Cowork</span>
          </div>
        </div>
      </section>

      {/* 01 — How it works */}
      <section className="py-16 border-t border-white/[0.06]">
        <SectionLabel n="01" label="Pipeline" />
        <h2 className="text-[32px] font-semibold mb-6 tracking-[-0.035em] leading-tight">How it works</h2>
        <pre className="font-mono text-[13px] leading-relaxed text-[#888] bg-[#0a0a0a] border border-white/[0.06] rounded-[2px] p-5 overflow-x-auto mb-5">
{`Audio → Transcribe → Diarize → Summarize → Markdown → Relationship Graph
        (local)      (local)    (your LLM)  (decisions,  (people, commitments,
       whisper.cpp  pyannote   Claude /      action items) topics, scores)
                               Ollama`}
        </pre>
        <p className="text-sm text-[#888] leading-relaxed max-w-[640px]">
          Your audio never leaves your machine. Transcription is local via whisper.cpp with GPU acceleration. Summarization is optional. Claude does it conversationally when you ask, using your existing subscription. No API keys needed.
        </p>
      </section>

      {/* 02 — Audiences */}
      <section className="py-16 border-t border-white/[0.06]">
        <SectionLabel n="02" label="Audiences" />
        <h2 className="text-[32px] font-semibold mb-8 tracking-[-0.035em] leading-tight">Built for everyone who<br />has conversations</h2>
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-px bg-white/[0.06]">
          {[
            {
              icon: "cpu",
              title: "AI agents",
              desc: "24 MCP tools. 7 resources. Relationship graph. Commitment tracking. Any agent that speaks MCP can use Minutes as its memory layer.",
            },
            {
              icon: "terminal",
              title: "Developers",
              desc: "41 CLI commands. 400+ tests. Rust engine, single binary, MIT license. Homebrew, cross-platform CI. TypeScript SDK.",
            },
            {
              icon: "mic",
              title: "Everyone else",
              desc: "Menu bar app with one-click recording. Calendar integration suggests recording before meetings. Voice memo pipeline from iPhone.",
            },
          ].map((card) => (
            <div
              key={card.title}
              className="p-6 bg-[#0a0a0a] transition-colors hover:bg-[#0d0d0d]"
            >
              <div className="text-[#555] mb-4">
                {card.icon === "terminal" && (
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>
                )}
                {card.icon === "mic" && (
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/></svg>
                )}
                {card.icon === "cpu" && (
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6"/><line x1="9" y1="1" x2="9" y2="4"/><line x1="15" y1="1" x2="15" y2="4"/><line x1="9" y1="20" x2="9" y2="23"/><line x1="15" y1="20" x2="15" y2="23"/><line x1="20" y1="9" x2="23" y2="9"/><line x1="20" y1="14" x2="23" y2="14"/><line x1="1" y1="9" x2="4" y2="9"/><line x1="1" y1="14" x2="4" y2="14"/></svg>
                )}
              </div>
              <h3 className="text-[15px] font-semibold mb-2 text-[#ededed]">{card.title}</h3>
              <p className="text-[13px] text-[#888] leading-snug">{card.desc}</p>
            </div>
          ))}
        </div>
      </section>

      {/* 03 — Features */}
      <section className="py-16 border-t border-white/[0.06]">
        <SectionLabel n="03" label="Features" />
        <h2 className="text-[32px] font-semibold mb-10 tracking-[-0.035em] leading-tight">What you get</h2>
        <div className="space-y-10">
          {/* Capture */}
          <div>
            <h3 className="font-mono text-[11px] text-[#555] uppercase tracking-[0.15em] mb-4">Capture</h3>
            <div className="grid sm:grid-cols-2 gap-x-8 gap-y-3">
              {[
                ["Local transcription", "whisper.cpp with GPU acceleration. Your audio never leaves your machine."],
                ["Streaming transcription", "Text appears as you speak. Partial results every 2 seconds."],
                ["Dictation mode", "Hold a hotkey, speak, release. Text goes to clipboard and daily note."],
                ["Speaker diarization", "pyannote separates who said what in multi-person meetings."],
              ].map(([title, desc]) => (
                <div key={title} className="flex gap-3 items-start text-sm">
                  <span className="text-[#333] font-mono text-[12px] mt-0.5 shrink-0">&gt;</span>
                  <p className="text-[#888] leading-snug">
                    <strong className="text-[#ededed] font-medium">{title}.</strong> {desc}
                  </p>
                </div>
              ))}
            </div>
          </div>

          {/* Intelligence */}
          <div>
            <h3 className="font-mono text-[11px] text-[#555] uppercase tracking-[0.15em] mb-4">Intelligence</h3>
            <div className="grid sm:grid-cols-2 gap-x-8 gap-y-3">
              {[
                ["Structured extraction", "Action items, decisions, and commitments as queryable YAML."],
                ["Relationship memory", "Track people, commitments, topics across meetings. Losing-touch alerts."],
                ["Cross-meeting search", "Search across all meetings. Build people profiles from every conversation."],
                ["Voice memo pipeline", "iPhone Voice Memos to iCloud to auto-transcribe on Mac."],
              ].map(([title, desc]) => (
                <div key={title} className="flex gap-3 items-start text-sm">
                  <span className="text-[#333] font-mono text-[12px] mt-0.5 shrink-0">&gt;</span>
                  <p className="text-[#888] leading-snug">
                    <strong className="text-[#ededed] font-medium">{title}.</strong> {desc}
                  </p>
                </div>
              ))}
            </div>
          </div>

          {/* Integration */}
          <div>
            <h3 className="font-mono text-[11px] text-[#555] uppercase tracking-[0.15em] mb-4">Integration</h3>
            <div className="grid sm:grid-cols-2 gap-x-8 gap-y-3">
              {[
                ["Desktop app", "Tauri v2 menu bar app. One-click recording, dictation hotkey, calendar integration."],
                ["Claude-native", "24 MCP tools for Claude Desktop, Cowork, Dispatch. 12 Claude Code skills."],
                ["Any LLM", "Ollama for local. OpenAI if you prefer. Or skip summarization entirely."],
                ["Markdown is the truth", "YAML frontmatter. Works with Obsidian, grep, QMD, or anything."],
              ].map(([title, desc]) => (
                <div key={title} className="flex gap-3 items-start text-sm">
                  <span className="text-[#333] font-mono text-[12px] mt-0.5 shrink-0">&gt;</span>
                  <p className="text-[#888] leading-snug">
                    <strong className="text-[#ededed] font-medium">{title}.</strong> {desc}
                  </p>
                </div>
              ))}
            </div>
          </div>
        </div>
      </section>

      {/* 04 — Comparison */}
      <section className="py-16 border-t border-white/[0.06]">
        <SectionLabel n="04" label="Comparison" />
        <h2 className="text-[32px] font-semibold mb-8 tracking-[-0.035em] leading-tight">How it compares</h2>
        <div className="overflow-x-auto border border-white/[0.06] rounded-[2px]">
          <table className="w-full text-[13px] border-collapse">
            <thead>
              <tr className="bg-[#0a0a0a]">
                <th className="text-left p-3 border-b border-white/[0.06] text-[#555] font-medium text-[10px] uppercase tracking-[0.15em]" />
                <th className="text-left p-3 border-b border-white/[0.06] text-[#555] font-medium text-[10px] uppercase tracking-[0.15em]">Granola</th>
                <th className="text-left p-3 border-b border-white/[0.06] text-[#555] font-medium text-[10px] uppercase tracking-[0.15em]">Otter.ai</th>
                <th className="text-left p-3 border-b border-white/[0.06] text-[#555] font-medium text-[10px] uppercase tracking-[0.15em]">Meetily</th>
                <th className="text-left p-3 border-b border-white/[0.06] text-[#ededed] font-semibold text-[10px] uppercase tracking-[0.15em]">minutes</th>
              </tr>
            </thead>
            <tbody>
              {([
                ["Local transcription", "No", "No", "Yes", "Yes"],
                ["Open source", "No", "No", "Yes", "MIT"],
                ["Free", "$18/mo", "Freemium", "Free", "Free"],
                ["AI agent integration", "No", "No", "No", "24 MCP tools"],
                ["Cross-meeting intelligence", "No", "No", "No", "Yes"],
                ["Dictation mode", "No", "No", "No", "Yes"],
                ["Voice memos", "No", "No", "No", "iPhone pipeline"],
                ["People memory", "No", "No", "No", "Yes"],
                ["Data ownership", "Their servers", "Their servers", "Local", "Local"],
              ] as const).map(([feature, ...values]) => (
                <tr key={feature} className="hover:bg-white/[0.015] transition-colors">
                  <td className="p-3 border-b border-white/[0.03] text-[#ededed] font-medium">{feature}</td>
                  {values.map((val, i) => {
                    const isMinutes = i === 3;
                    const isNo = val === "No";
                    return (
                      <td
                        key={i}
                        className={`p-3 border-b border-white/[0.03] ${
                          isMinutes
                            ? "text-[#ededed] font-semibold"
                            : isNo
                              ? "text-[#333]"
                              : "text-[#888]"
                        }`}
                      >
                        {isNo ? "—" : val}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      {/* Footer */}
      <footer className="py-14 border-t border-white/[0.06] text-center text-[13px] text-[#555]">
        <p>minutes is MIT licensed and free forever.</p>
        <p className="mt-1">Built by <a href="https://github.com/silverstein" className="text-[#777] hover:text-[#ededed] transition-colors">Mat Silverstein</a>, founder of <a href="https://x1wealth.com" className="text-[#777] hover:text-[#ededed] transition-colors">X1 Wealth</a></p>
        <p className="mt-3">
          <a href="https://github.com/silverstein/minutes" className="text-[#555] hover:text-[#ededed] transition-colors">GitHub</a>
          {" · "}
          <a href="/llms.txt" className="text-[#555] hover:text-[#ededed] transition-colors">llms.txt</a>
          {" · "}
          <a href="https://github.com/silverstein/minutes/blob/main/CONTRIBUTING.md" className="text-[#555] hover:text-[#ededed] transition-colors">Contribute</a>
        </p>
      </footer>
    </div>
  );
}
