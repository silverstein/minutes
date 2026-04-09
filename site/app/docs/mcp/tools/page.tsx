import type { Metadata } from "next";
import data from "./data.json";

export const metadata: Metadata = {
  title: "Minutes MCP tools",
  description:
    "Generated reference for Minutes MCP tools, resources, and prompt templates.",
  alternates: {
    canonical: "/docs/mcp/tools",
  },
};

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

function LinkPill({ href }: { href: string }) {
  return (
    <a
      href={href}
      className="inline-flex items-center rounded-full bg-[var(--bg)] px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--accent)] hover:bg-[var(--bg-hover)]"
    >
      anchor
    </a>
  );
}

export default function MpcToolsPage() {
  const toolGroups = Object.entries(
    data.tools.reduce((acc, tool) => {
      acc[tool.group] ||= [];
      acc[tool.group].push(tool);
      return acc;
    }, {} as Record<string, typeof data.tools>)
  );
  const resourceGroups = Object.entries(
    data.resources.reduce((acc, resource) => {
      acc[resource.group] ||= [];
      acc[resource.group].push(resource);
      return acc;
    }, {} as Record<string, typeof data.resources>)
  );
  const promptGroups = Object.entries(
    data.prompts.reduce((acc, prompt) => {
      acc[prompt.group] ||= [];
      acc[prompt.group].push(prompt);
      return acc;
    }, {} as Record<string, typeof data.prompts>)
  );

  return (
    <div className="mx-auto max-w-[920px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/docs/mcp/tools.md" className="hover:text-[var(--accent)]">
            tools.md
          </a>
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
          </a>
        </div>
      </div>

      <section className="max-w-[760px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Generated Reference
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          Minutes MCP tools
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Canonical reference for the Minutes MCP server. This page is generated from the shipped
          product surface, so tool counts and names stay aligned with the actual server.
        </p>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Install
        </p>
        <pre className="mt-4 overflow-x-auto rounded-[6px] bg-[var(--bg)] p-4 font-mono text-[12px] leading-6 text-[var(--text)]">
{`{
  "mcpServers": {
    "minutes": {
      "command": "npx",
      "args": ["minutes-mcp"]
    }
  }
}`}
        </pre>
        <p className="mt-4 text-[14px] leading-7 text-[var(--text-secondary)]">
          Full agent entry point:{" "}
          <a href={data.documentationUrl} className="text-[var(--accent)] hover:underline">
            {data.documentationUrl}
          </a>
        </p>
      </section>

      <section className="mt-14">
        <SectionLabel label={`Tools (${data.tools.length})`} />
        <div className="space-y-8">
          {toolGroups.map(([group, tools]) => (
            <div key={group}>
              <SectionLabel label={group} />
              <div className="grid gap-4">
                {tools.map((tool) => (
                  <div
                    key={tool.name}
                    id={tool.anchorId}
                    className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <p className="font-mono text-[13px] text-[var(--text)]">{tool.name}</p>
                      <LinkPill href={tool.docsUrl} />
                    </div>
                    <p className="mt-2 text-[14px] leading-7 text-[var(--text-secondary)]">
                      {tool.description}
                    </p>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label={`Resources (${data.resources.length})`} />
        <div className="space-y-8">
          {resourceGroups.map(([group, resources]) => (
            <div key={group}>
              <SectionLabel label={group} />
              <div className="grid gap-4">
                {resources.map((resource) => (
                  <div
                    key={resource.uri}
                    id={resource.anchorId}
                    className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <p className="font-mono text-[13px] text-[var(--text)]">{resource.uri}</p>
                      <LinkPill href={resource.docsUrl} />
                    </div>
                    <p className="mt-2 text-[14px] leading-7 text-[var(--text-secondary)]">
                      {resource.description}
                    </p>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label={`Prompt Templates (${data.prompts.length})`} />
        <div className="space-y-8">
          {promptGroups.map(([group, prompts]) => (
            <div key={group}>
              <SectionLabel label={group} />
              <div className="grid gap-4">
                {prompts.map((prompt) => (
                  <div
                    key={prompt.name}
                    id={prompt.anchorId}
                    className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5"
                  >
                    <div className="flex items-start justify-between gap-3">
                      <p className="font-mono text-[13px] text-[var(--text)]">{prompt.name}</p>
                      <LinkPill href={prompt.docsUrl} />
                    </div>
                    <p className="mt-2 text-[14px] leading-7 text-[var(--text-secondary)]">
                      {prompt.description}
                    </p>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
