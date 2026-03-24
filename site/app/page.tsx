import { DemoPlayer } from "@/components/demo-player";
import { CopyButton } from "@/components/copy-button";

export default function Home() {
  return (
    <div className="max-w-[800px] mx-auto px-6">
      {/* Nav — sticky with backdrop blur */}
      <nav className="sticky top-0 z-50 flex items-center justify-between py-4 border-b border-white/[0.06] bg-black/80 backdrop-blur-lg">
        <div className="font-mono text-[15px] font-medium text-[#ededed]">
          minutes
        </div>
        <div className="flex gap-6 text-sm max-sm:gap-4 max-sm:text-xs">
          <a href="https://github.com/silverstein/minutes" className="text-[#666] hover:text-[#ededed] transition-colors">GitHub</a>
          <a href="https://github.com/silverstein/minutes#install" className="text-[#666] hover:text-[#ededed] transition-colors">Install</a>
          <a href="https://github.com/silverstein/minutes#claude-integration" className="text-[#666] hover:text-[#ededed] transition-colors">Claude</a>
          <a href="/llms.txt" className="text-[#666] hover:text-[#ededed] transition-colors">llms.txt</a>
        </div>
      </nav>

      {/* Hero */}
      <section className="relative pt-20 pb-14 text-center max-sm:pt-12 max-sm:pb-10">
        {/* Radial glow — more visible */}
        <div className="absolute -top-[30%] left-1/2 -translate-x-1/2 w-[800px] h-[600px] bg-[radial-gradient(ellipse_at_center,rgba(0,112,243,0.12)_0%,rgba(168,85,247,0.06)_35%,transparent_65%)] pointer-events-none" />

        <h1 className="relative text-[44px] max-sm:text-[32px] font-bold leading-[1.15] mb-4 tracking-[-0.04em] bg-gradient-to-b from-white to-[#a1a1a1] bg-clip-text text-transparent">
          Your AI remembers every<br />conversation you&apos;ve had
        </h1>
        <p className="relative text-[17px] max-sm:text-[15px] text-[#a1a1a1] max-w-[540px] mx-auto mb-8 leading-relaxed">
          Agents have run logs. Humans have conversations. Minutes captures the human side — the decisions, the intent, the context — and makes it queryable. Local, open source, free forever.
        </p>

        {/* Primary CTA */}
        <div className="relative flex gap-3 justify-center mb-8 max-sm:flex-col max-sm:items-center">
          <a
            href="https://github.com/silverstein/minutes#install"
            className="inline-flex items-center gap-2 px-6 py-2.5 bg-white text-black text-sm font-medium rounded-lg hover:bg-[#e0e0e0] transition-colors"
          >
            Get started
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className="mt-px"><path d="M6 3l5 5-5 5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/></svg>
          </a>
          <a
            href="https://github.com/silverstein/minutes"
            className="inline-flex items-center gap-2 px-6 py-2.5 border border-white/[0.12] text-sm text-[#a1a1a1] rounded-lg hover:text-[#ededed] hover:border-white/[0.2] transition-colors"
          >
            <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>
            View on GitHub
          </a>
        </div>

        {/* Remotion Player */}
        <div className="relative mb-10">
          <DemoPlayer />
        </div>

        {/* Install commands */}
        <div id="install" className="flex gap-3 justify-center flex-wrap mb-3 max-sm:flex-col max-sm:items-center">
          <CopyButton label="Desktop app" cmd="brew install --cask silverstein/tap/minutes" />
          <CopyButton label="CLI only" cmd="brew tap silverstein/tap && brew install minutes" />
          <CopyButton label="MCP server" cmd="npx minutes-mcp" />
        </div>
        <p className="text-[13px] text-[#666]">
          macOS, Windows, Linux. <code className="font-mono text-[12px] text-[#a1a1a1]">npx</code> works everywhere — Claude Desktop, Cursor, Windsurf, any MCP client.
        </p>

        {/* Works with */}
        <div className="mt-10 pt-8 border-t border-white/[0.04]">
          <p className="text-xs text-[#444] uppercase tracking-widest mb-4">Works with any MCP client</p>
          <div className="flex items-center justify-center gap-8 text-[#555] text-sm max-sm:gap-4 max-sm:text-xs flex-wrap">
            <span className="font-medium">Claude Desktop</span>
            <span className="text-[#333]">/</span>
            <span className="font-medium">Claude Code</span>
            <span className="text-[#333]">/</span>
            <span className="font-medium">Cursor</span>
            <span className="text-[#333]">/</span>
            <span className="font-medium">Windsurf</span>
            <span className="text-[#333]">/</span>
            <span className="font-medium">Cowork</span>
          </div>
        </div>
      </section>

      {/* How it works */}
      <section className="py-14 border-t border-white/[0.06]">
        <h2 className="text-2xl font-semibold mb-6 tracking-[-0.03em]">How it works</h2>
        <pre className="font-mono text-[13px] leading-relaxed text-[#a1a1a1] bg-[#0a0a0a] border border-white/[0.06] rounded-lg p-5 overflow-x-auto mb-4">
{`Audio  →  Transcribe  →  Summarize  →  Structured Markdown
          (local)        (your LLM)     (decisions, action items,
         whisper.cpp    Claude / Ollama   people, entities)`}
        </pre>
        <p className="text-sm text-[#a1a1a1] leading-relaxed">
          Your audio never leaves your machine. Transcription is local via whisper.cpp with GPU acceleration. Summarization is optional — Claude does it conversationally when you ask, using your existing subscription. No API keys needed.
        </p>
      </section>

      {/* Audiences */}
      <section className="py-14 border-t border-white/[0.06]">
        <h2 className="text-2xl font-semibold mb-6 tracking-[-0.03em]">Built for everyone who has conversations</h2>
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
          {[
            {
              icon: "terminal",
              title: "Developers",
              desc: "15 CLI commands. 136 tests. Rust engine, single binary, MIT license. Homebrew, cross-platform CI. TypeScript SDK for agent developers.",
            },
            {
              icon: "mic",
              title: "Knowledge workers",
              desc: "Menu bar app with one-click recording. Calendar integration suggests recording before meetings. Voice memo pipeline from iPhone. Obsidian vault sync.",
            },
            {
              icon: "cpu",
              title: "AI agents",
              desc: "13 MCP tools. 7 resources. Structured intents in YAML. Decision consistency tracking. People profiles. Any agent that speaks MCP can use Minutes as its memory layer.",
            },
          ].map((card) => (
            <div
              key={card.title}
              className="p-5 bg-[#0a0a0a] border border-white/[0.06] rounded-lg transition-colors hover:border-white/[0.12]"
            >
              <div className="text-[#666] mb-3">
                {card.icon === "terminal" && (
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>
                )}
                {card.icon === "mic" && (
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/><line x1="8" y1="23" x2="16" y2="23"/></svg>
                )}
                {card.icon === "cpu" && (
                  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><rect x="4" y="4" width="16" height="16" rx="2"/><rect x="9" y="9" width="6" height="6"/><line x1="9" y1="1" x2="9" y2="4"/><line x1="15" y1="1" x2="15" y2="4"/><line x1="9" y1="20" x2="9" y2="23"/><line x1="15" y1="20" x2="15" y2="23"/><line x1="20" y1="9" x2="23" y2="9"/><line x1="20" y1="14" x2="23" y2="14"/><line x1="1" y1="9" x2="4" y2="9"/><line x1="1" y1="14" x2="4" y2="14"/></svg>
                )}
              </div>
              <h3 className="text-[15px] font-semibold mb-2">{card.title}</h3>
              <p className="text-[13px] text-[#a1a1a1] leading-snug">{card.desc}</p>
            </div>
          ))}
        </div>
      </section>

      {/* Features — two columns on desktop */}
      <section className="py-14 border-t border-white/[0.06]">
        <h2 className="text-2xl font-semibold mb-6 tracking-[-0.03em]">What you get</h2>
        <div className="grid sm:grid-cols-2 gap-x-8 gap-y-4">
          {[
            ["Local transcription", "whisper.cpp with GPU acceleration (Metal, CUDA, CoreML). Your audio never leaves your machine."],
            ["Streaming transcription", "Text appears as you speak. Partial results every 2 seconds, final transcript when you stop."],
            ["Dictation mode", "Hold a hotkey, speak, release. Text goes to clipboard and daily note. Menu bar app or CLI."],
            ["Speaker diarization", "pyannote separates \"who said what\" in multi-person meetings."],
            ["Structured extraction", "Action items, decisions, and commitments as queryable YAML, not buried in prose."],
            ["Cross-meeting intelligence", "Search across all meetings. Build people profiles from every conversation."],
            ["Voice memo pipeline", "iPhone Voice Memos → iCloud → auto-transcribe on Mac. Ideas while walking, searchable by afternoon."],
            ["Desktop app", "Tauri v2 menu bar app. One-click recording, dictation hotkey, calendar integration. macOS and Windows."],
            ["Claude-native", "13 MCP tools for Claude Desktop, Cowork, Dispatch. Claude Code plugin with 12 skills. No API keys."],
            ["Any LLM", "Ollama for local. OpenAI if you prefer. Or skip summarization — the transcript is the artifact."],
            ["Markdown is the truth", "Every meeting saves as markdown with YAML frontmatter. Works with Obsidian, grep, QMD, or anything."],
          ].map(([title, desc]) => (
            <div key={title} className="flex gap-3 items-start text-sm">
              <span className="text-[#333] font-mono text-[13px] mt-0.5 shrink-0">&gt;</span>
              <p className="text-[#a1a1a1] leading-snug">
                <strong className="text-[#ededed] font-medium">{title}</strong> — {desc}
              </p>
            </div>
          ))}
        </div>
      </section>

      {/* Comparison — with subtle background */}
      <section className="py-14 border-t border-white/[0.06]">
        <h2 className="text-2xl font-semibold mb-6 tracking-[-0.03em]">How it compares</h2>
        <div className="overflow-x-auto bg-[#0a0a0a] border border-white/[0.06] rounded-lg p-1">
          <table className="w-full text-[13px] border-collapse">
            <thead>
              <tr>
                <th className="text-left p-3 border-b border-white/[0.06] text-[#666] font-medium text-xs uppercase tracking-wider" />
                <th className="text-left p-3 border-b border-white/[0.06] text-[#666] font-medium text-xs uppercase tracking-wider">Granola</th>
                <th className="text-left p-3 border-b border-white/[0.06] text-[#666] font-medium text-xs uppercase tracking-wider">Otter.ai</th>
                <th className="text-left p-3 border-b border-white/[0.06] text-[#666] font-medium text-xs uppercase tracking-wider">Meetily</th>
                <th className="text-left p-3 border-b border-white/[0.06] text-[#ededed] font-semibold text-xs uppercase tracking-wider">minutes</th>
              </tr>
            </thead>
            <tbody>
              {([
                ["Local transcription", "No", "No", "Yes", "Yes"],
                ["Open source", "No", "No", "Yes", "MIT"],
                ["Free", "$18/mo", "Freemium", "Free", "Free"],
                ["AI agent integration", "No", "No", "No", "13 MCP tools"],
                ["Cross-meeting intelligence", "No", "No", "No", "Yes"],
                ["Dictation mode", "No", "No", "No", "Yes"],
                ["Voice memos", "No", "No", "No", "iPhone pipeline"],
                ["People memory", "No", "No", "No", "Yes"],
                ["Data ownership", "Their servers", "Their servers", "Local", "Local"],
              ] as const).map(([feature, ...values]) => (
                <tr key={feature}>
                  <td className="p-3 border-b border-white/[0.03] text-[#ededed] font-medium">{feature}</td>
                  {values.map((val, i) => {
                    const isMinutes = i === 3;
                    const isYes = val === "Yes" || val === "Local" || val === "Free";
                    const isNo = val === "No";
                    return (
                      <td
                        key={i}
                        className={`p-3 border-b border-white/[0.03] ${
                          isMinutes
                            ? "text-[#ededed] font-semibold"
                            : isYes
                              ? "text-[#00cc88]"
                              : isNo
                                ? "text-[#444]"
                                : "text-[#a1a1a1]"
                        }`}
                      >
                        {val}
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
      <footer className="py-12 border-t border-white/[0.06] text-center text-[13px] text-[#666]">
        <p>minutes is MIT licensed and free forever. Built with Rust, whisper.cpp, and Tauri.</p>
        <p className="mt-2">
          <a href="https://github.com/silverstein/minutes" className="text-[#666] hover:text-[#a1a1a1] transition-colors">GitHub</a>
          {" · "}
          <a href="/llms.txt" className="text-[#666] hover:text-[#a1a1a1] transition-colors">llms.txt</a>
          {" · "}
          <a href="https://github.com/silverstein/minutes/blob/main/CONTRIBUTING.md" className="text-[#666] hover:text-[#a1a1a1] transition-colors">Contribute</a>
        </p>
      </footer>
    </div>
  );
}
