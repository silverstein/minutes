import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { collectPolicyToolSearchSnapshots, policyDecisionHistory } from "./index.js";

const content = (title: string, body: string, restricted = false) => `---\ntitle: ${JSON.stringify(title)}\ntype: meeting\ndate: 2020-01-01T00:00:00Z\nsensitivity: ${restricted ? "restricted" : "normal"}\n---\n${body}`;
const snapshot = (value: string) => ({ canonicalRoot: "/corpus", files: [{ path: "/corpus/source.md", relativePath: "source.md", content: value }] });

describe("MCP shared memory contract", () => {
  it("uses the same literal query semantics as Rust and SDK", () => {
    const fixture = JSON.parse(readFileSync(new URL("../../../tests/fixtures/search-contract-v1.json", import.meta.url), "utf8"));
    for (const item of fixture.cases) {
      const result = collectPolicyToolSearchSnapshots(snapshot(content(item.title, item.body)), false, { query: item.query });
      expect(result.length > 0, item.id).toBe(item.matches);
    }
  });
  it("ranks relevant titles ahead of newer incidental mentions", () => {
    const corpus = snapshot(content("Pricing", ""));
    corpus.files.push({ path: "/corpus/new.md", relativePath: "new.md", content: content("Unrelated", "pricing").replace("2020-01-01", "2020-02-01") });
    expect(collectPolicyToolSearchSnapshots(corpus, false, { query: "pricing" })[0].path).toBe("/corpus/source.md");
  });
  it("does not use restricted or inactive sources to settle a decision", () => {
    const base = content("Public", "").replace("---\n\n", "---\n");
    const raw = base.replace("sensitivity: normal", "sensitivity: normal\ndecisions:\n  - text: Annual\n    topic: pricing\n    supersedes: secret");
    const corpus = snapshot(raw);
    const privateDoc = content("Private", "PRIVATE_CANARY", true).replace("sensitivity: restricted", "sensitivity: restricted\ndecisions:\n  - id: secret\n    text: PRIVATE_CANARY\n    topic: pricing");
    corpus.files.push({ path: "/corpus/private.md", relativePath: "private.md", content: privateDoc });
    corpus.files.push({ path: "/corpus/archive/old.md", relativePath: "archive/old.md", content: privateDoc.replace("restricted", "normal") });
    expect(collectPolicyToolSearchSnapshots(corpus, false, { query: "PRIVATE_CANARY" })).toEqual([]);
    const history = policyDecisionHistory(corpus);
    expect(history).toHaveLength(1);
    expect(history[0].status).toBe("unresolved");
    expect(history[0].supersedes).toBeUndefined();
    expect(JSON.stringify(history)).not.toContain("PRIVATE_CANARY");
  });
});
