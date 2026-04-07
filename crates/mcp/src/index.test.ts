import { describe, expect, it } from "vitest";

import { MEETING_INSIGHT_KINDS, parseKnowledgeConfig } from "./index.js";

describe("meeting insight contract", () => {
  it("exports only the insight kinds the pipeline emits today", () => {
    expect(MEETING_INSIGHT_KINDS).toEqual(["decision", "commitment", "question"]);
  });
});

describe("parseKnowledgeConfig", () => {
  it("only treats enabled=true inside the knowledge section as enabling the knowledge base", () => {
    const parsed = parseKnowledgeConfig(`
[recording]
enabled = true

[knowledge]
enabled = false
path = "~/kb"
`);

    expect(parsed).toEqual({
      enabled: false,
      path: "~/kb",
      adapter: "wiki",
      engine: "none",
    });
  });

  it("reads knowledge settings from the knowledge section", () => {
    const parsed = parseKnowledgeConfig(`
[knowledge]
enabled = true
path = "~/kb"
adapter = "para"
engine = "agent"
`);

    expect(parsed).toEqual({
      enabled: true,
      path: "~/kb",
      adapter: "para",
      engine: "agent",
    });
  });
});
