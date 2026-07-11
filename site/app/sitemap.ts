import type { MetadataRoute } from "next";

const BASE_URL = "https://useminutes.app";

const routes = [
  "/",
  "/compare",
  "/compare/fathom-vs-minutes",
  "/compare/fireflies-vs-minutes",
  "/compare/granola-vs-minutes",
  "/compare/hyprnote-vs-minutes",
  "/compare/krisp-vs-minutes",
  "/compare/macwhisper-vs-minutes",
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
  "/resources/ai-notetakers-attorney-client-privilege",
  "/resources/best-local-speech-to-text",
  "/resources/best-mcp-meeting-memory-tools",
  "/resources/best-meeting-tools-for-claude-code-and-codex",
  "/resources/is-fireflies-ai-hipaa-compliant",
  "/resources/is-granola-hipaa-compliant",
  "/resources/is-otter-ai-hipaa-compliant",
  "/resources/legal-transcription-software",
  "/resources/local-dictation-macos",
  "/resources/meeting-minutes-template",
  "/resources/open-source-alternatives-to-granola-ai",
  "/resources/remove-ai-notetaker-bots-from-meetings",
  "/security",
  "/writing",
  "/writing/governance-built-in-not-retrofitted",
  "/writing/whisper-cpp-vs-parakeet-cpp",
] as const;

export default function sitemap(): MetadataRoute.Sitemap {
  return routes.map((route) => ({
    url: `${BASE_URL}${route}`,
    changeFrequency: "weekly",
    priority: route === "/" ? 1 : 0.7,
  }));
}
