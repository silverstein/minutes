import type { MetadataRoute } from "next";

const BASE_URL = "https://useminutes.app";

const routes = [
  "/",
  "/compare",
  "/compare/fireflies-vs-minutes",
  "/compare/granola-vs-minutes",
  "/compare/hyprnote-vs-minutes",
  "/compare/otter-vs-minutes",
  "/compare/superwhisper-vs-minutes",
  "/docs",
  "/docs/agent-integrations",
  "/docs/errors",
  "/docs/mcp/tools",
  "/docs/using-minutes",
  "/dojo",
  "/for-agents",
  "/proof",
  "/resources/best-mcp-meeting-memory-tools",
  "/resources/best-meeting-tools-for-claude-code-and-codex",
  "/resources/is-otter-ai-hipaa-compliant",
  "/resources/open-source-alternatives-to-granola-ai",
  "/security",
  "/writing",
  "/writing/governance-built-in-not-retrofitted",
] as const;

export default function sitemap(): MetadataRoute.Sitemap {
  return routes.map((route) => ({
    url: `${BASE_URL}${route}`,
    changeFrequency: "weekly",
    priority: route === "/" ? 1 : 0.7,
  }));
}
