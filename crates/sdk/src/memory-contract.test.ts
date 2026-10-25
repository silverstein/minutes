import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { lexicalMatch, queryTokens, compareSearchMatches, contentQuery, relaxedQueries } from "./retrieval.js";
import { buildSourceEvidence, evidenceHash, evidenceForSpan } from "./evidence.js";
import { buildDecisionHistory } from "./decision-history.js";
import { parseFrontmatter } from "./reader.js";

function source(name: string, date: string, decisions: string, body = "[SPEAKER_0 0:12] Café 😀 roadmap.") {
  const content = `---\ntitle: Example\ntype: meeting\ndate: ${date}\ndecisions:\n${decisions}\n---\n${body}\n`;
  const meeting = parseFrontmatter(content, `/meetings/${name}.md`)!;
  return { ...meeting, content };
}

describe("literal retrieval contract", () => {
  it("conforms to the shared Rust/SDK/MCP fixtures", () => {
    const fixtures = JSON.parse(readFileSync(new URL("../../../tests/fixtures/search-contract-v1.json", import.meta.url), "utf8"));
    for (const fixture of fixtures.cases) {
      expect(lexicalMatch(fixture.title, fixture.body, fixture.query) !== null, fixture.id).toBe(fixture.matches);
    }
  });
  it("matches AND tokens across title/body with a final prefix", () => {
    expect(lexicalMatch("Pricing", "The roadmap changed", "pricing road")).not.toBeNull();
    expect(lexicalMatch("Pricing", "Roadmap", "pric roadmap")).toBeNull();
    expect(lexicalMatch("Pricing", "Elsewhere", "pricing road")).toBeNull();
  });
  it("treats operators as literals, preserves unicode and accents", () => {
    expect(lexicalMatch("AND decisions", "", "AND")).not.toBeNull();
    expect(lexicalMatch("decisions", "", "NOT decisions")).toBeNull();
    expect(lexicalMatch("café", "東京", "CAFE 東京")).not.toBeNull();
    expect(queryTokens("(R) C++ 😀")).toEqual(["r", "c"]);
    expect(lexicalMatch("road", "", "r")).toBeNull();
    expect(lexicalMatch("R", "", "r")).not.toBeNull();
  });
  it("rejects unbounded input and has deterministic rank ties", () => {
    expect(() => queryTokens("x".repeat(2049))).toThrow(RangeError);
    expect(lexicalMatch("anything", "", "!? ")).toBeNull();
    expect(compareSearchMatches({ score: 1, date: "2020-01-01", path: "a" }, { score: 1, date: "2020-01-01", path: "b" })).toBeLessThan(0);
    // Tier outranks score: the exact title phrase beats a chatty transcript.
    const chatty = "let me show you. also the media plan. let me know. ".repeat(200);
    const titleHit = lexicalMatch("Also let me know if or when there's time", "short note", "also let me")!;
    const scattered = lexicalMatch("gg media", chatty, "also let me")!;
    expect(scattered.score).toBeGreaterThan(titleHit.score);
    expect([titleHit.tier, scattered.tier]).toEqual([3, 0]);
    expect(compareSearchMatches(
      { tier: titleHit.tier, score: titleHit.score, date: "2020-01-01", path: "a" },
      { tier: scattered.tier, score: scattered.score, date: "2026-01-01", path: "b" }
    )).toBeLessThan(0);
    expect(lexicalMatch("Malso let me", "", "also let me")).toBeNull();
    expect(lexicalMatch("pricing", "", "pricing")!.score).toBeGreaterThan(lexicalMatch("", "pricing", "pricing")!.score);
    // Stopwords are optional except the final (still typed) token; mirrors Rust.
    expect(contentQuery("Planning a ch")).toBe("planning ch");
    expect(contentQuery("pricing for")).toBe("pricing for");
    expect(contentQuery("to be or")).toBe("to be or");
    expect(lexicalMatch("Planning Chaos Harnesses", "", contentQuery("planning a ch"))!.tier).toBe(3);
    expect(relaxedQueries("planning chaos desktop")).toEqual(["chaos desktop", "planning desktop", "planning chaos"]);
    expect(relaxedQueries("planning")).toEqual([]);
    // Plural title words rank like the Rust (stemmed) match; scoring is unchanged.
    expect(lexicalMatch("Planning Chaos Harnesses for Codex", "chaos harness codex", "chaos harness codex")!.tier).toBe(2);
  });
});

describe("snapshot-bound evidence", () => {
  it("matches the Rust evidence golden across UTF-8 and UTF-16", () => {
    const body = "# Notes\n[SPEAKER_1 01:02.5] Café 😀 launch.\n[01:03] Decision: ship Friday.\n";
    const bundle = buildSourceEvidence({ path: "/meetings/emoji.md", content: `---\ntitle: Plan\n---\n${body}`, body }, { query: "cafe" });
    expect(bundle.source_id).toBe("2e0d3ab9af2dcb5cbbab8c73b995ce98ab2ddf621ad6d991c96de20a1f7dc4ff");
    expect(bundle.source_revision).toBe("85803628fcb381e7e2767a56499435b8d8a815d638a58b4fdb85a5ededeb640e");
    expect(bundle.segments[0]).toMatchObject({ id: "fe300c55ff9e0a6b219ede5bb70da3676327241596784d4d2586320e13cda582", start: 28, end: 63, line_start: 5, timestamp_seconds: 62.5 });
  });
  it("binds exact raw spans and provenance to a stable content revision", () => {
    const doc = source("a", "2020-01-01", "  []");
    const bundle = buildSourceEvidence({ ...doc, speakerMap: [{ speaker_label: "SPEAKER_0", name: "Ann", confidence: "high", source: "manual" }] }, { query: "cafe" });
    const segment = bundle.segments[0];
    expect(bundle.source_revision).toBe(evidenceHash(doc.content));
    expect(doc.content.slice(segment.start, segment.end)).toBe(segment.quote);
    expect(segment.timestamp_seconds).toBe(12);
    expect(segment.speaker).toEqual({ label: "SPEAKER_0", name: "Ann", confidence: "high", provenance: "manual" });
    expect(buildSourceEvidence(doc).segments[0].id).toBe(segment.id);
    expect(buildSourceEvidence({ ...doc, content: doc.content + "\n", body: doc.body + "\n" }).segments[0].id).not.toBe(segment.id);
  });
  it("never upgrades uncertain speaker labels or invents audio access", () => {
    const doc = source("a", "2020-01-01", "  []");
    const bundle = buildSourceEvidence({ ...doc, speakerMap: [{ speaker_label: "SPEAKER_0", name: "Ann", confidence: "low", source: "llm" }] });
    expect(bundle.segments[0].speaker?.name).toBeUndefined();
    expect(JSON.stringify(bundle)).not.toContain("audio");
    expect(() => buildSourceEvidence({ ...doc, body: "invented" })).toThrow(/snapshot/);
  });
  it("centers long-line evidence on a matching token and rejects split characters", () => {
    const doc = source("long", "2020-01-01", "  []", "x".repeat(2000) + " runway 😀");
    const proof = buildSourceEvidence(doc, { query: "runway" }).segments[0];
    expect(proof.quote).toContain("runway");
    expect(doc.content.slice(proof.start, proof.end)).toBe(proof.quote);
    const emoji = doc.content.indexOf("😀");
    expect(() => evidenceForSpan(doc, emoji + 1, emoji + 2)).toThrow(/Unicode/);
    expect(() => evidenceForSpan(doc, emoji, emoji + 1)).toThrow(/Unicode/);
  });
  it("bounds long quotes without splitting emoji, exposes truncation", () => {
    const doc = source("a", "2020-01-01", "  []", "x".repeat(1199) + "😀" + "x".repeat(2000));
    const bundle = buildSourceEvidence(doc);
    expect(bundle.truncated).toBe(true);
    expect(bundle.segments[0].quote.length).toBe(1199);
    expect(bundle.segments[0].quote_truncated).toBe(true);
  });
});

describe("explicit temporal decision history", () => {
  it("resolves a unique explicit predecessor and preserves source evidence", () => {
    const old = source("old", "2020-01-01", "  - id: price-v1\n    text: Monthly allowed\n    topic: pricing");
    const newer = source("new", "2020-02-01", "  - text: Annual only\n    topic: pricing\n    supersedes: price-v1\n    effective_date: 2020-02-02");
    const history = buildDecisionHistory([old, newer], { asOf: "2020-03-01" });
    expect(history.map((row) => row.status)).toEqual(["current", "superseded"]);
    expect(history[0].supersedes).toBe(history[1].id);
    expect(history[1].superseded_by).toBe(history[0].id);
    expect(history[0].effective_date_basis).toBe("explicit");
    const quote = history[0].evidence[0];
    expect(newer.content.slice(quote.start, quote.end)).toBe(quote.quote);
    expect(quote.quote).toContain("Annual only");
  });
  it("does not infer contradiction or let an ambiguous link choose an owner", () => {
    const a = source("a", "2020-01-01", "  - id: shared\n    text: Monthly\n    topic: pricing");
    const b = source("b", "2020-01-02", "  - id: shared\n    text: Quarterly\n    topic: pricing");
    const c = source("c", "2020-02-01", "  - text: Annual\n    topic: pricing\n    supersedes: shared");
    const history = buildDecisionHistory([a, b, c], { asOf: "2020-03-01" });
    expect(history.every((entry) => entry.status === "unresolved")).toBe(true);
    expect(history[0].unresolved_reason).toBe("ambiguous-supersedes");
    expect(history.some((entry) => entry.superseded_by)).toBe(false);
  });
  it("leaves competing explicit successors unresolved independent of source ordering", () => {
    const a = source("a", "2020-01-01", "  - id: base\n    text: Monthly\n    topic: pricing");
    const b = source("b", "2020-02-01", "  - text: Quarterly\n    topic: pricing\n    supersedes: base");
    const c = source("c", "2020-03-01", "  - text: Annual\n    topic: pricing\n    supersedes: base");
    const history = buildDecisionHistory([a, b, c], { asOf: "2020-04-01" });
    expect(history).toEqual(buildDecisionHistory([a, c, b], { asOf: "2020-04-01" }));
    expect(history.every((entry) => entry.status === "unresolved" && !entry.supersedes && !entry.superseded_by)).toBe(true);
  });
  it("keeps missing and competing supersession ancestry unresolved", () => {
    const missing = source("missing", "2020-01-01", "  - id: base\n    text: Annual\n    topic: pricing\n    supersedes: absent");
    const next = source("next", "2020-02-01", "  - text: Lifetime\n    topic: pricing\n    supersedes: base");
    const history = buildDecisionHistory([next, missing], { asOf: "2020-04-01" });
    expect(history.every((entry) => entry.status === "unresolved")).toBe(true);
    expect(history[0].unresolved_reason).toBe("unresolved-supersession-ancestry");
    expect(history.some((entry) => entry.supersedes || entry.superseded_by)).toBe(false);
    const base = source("base", "2020-01-01", "  - id: base\n    text: Monthly\n    topic: pricing");
    const a = source("a", "2020-02-01", "  - id: a\n    text: Annual\n    topic: pricing\n    supersedes: base");
    const b = source("b", "2020-02-02", "  - text: Lifetime\n    topic: pricing\n    supersedes: base");
    const c = source("c", "2020-03-01", "  - text: Weekly\n    topic: pricing\n    supersedes: a");
    const competing = buildDecisionHistory([c, b, base, a], { asOf: "2020-04-01" });
    expect(competing.every((entry) => entry.status === "unresolved")).toBe(true);
    expect(competing.some((entry) => entry.supersedes || entry.superseded_by)).toBe(false);
    expect(competing).toEqual(buildDecisionHistory([base, a, b, c], { asOf: "2020-04-01" }));
  });

  it("resolves a complete multi-generation chain regardless of source order", () => {
    const a = source("a", "2020-01-01", "  - id: a\n    text: Monthly\n    topic: pricing");
    const b = source("b", "2020-02-01", "  - id: b\n    text: Annual\n    topic: pricing\n    supersedes: a");
    const c = source("c", "2020-03-01", "  - text: Lifetime\n    topic: pricing\n    supersedes: b");
    const history = buildDecisionHistory([c, a, b], { asOf: "2020-04-01" });
    expect(history.map((entry) => entry.status)).toEqual(["current", "superseded", "superseded"]);
    expect(history).toEqual(buildDecisionHistory([a, b, c], { asOf: "2020-04-01" }));
  });

  it("rejects nonexistent calendar dates and JavaScript date shorthand", () => {
    for (const value of ["2026-02-30", "12", "2026-02-01T24:00:00Z"]) {
      const doc = source("bad-date", "2020-01-01", `  - text: Decision\n    topic: plan\n    effective_date: "${value}"`);
      expect(buildDecisionHistory([doc], { asOf: "2027-01-01" })[0]).toMatchObject({ status: "unknown", unresolved_reason: "invalid-effective-date" });
      expect(() => buildDecisionHistory([doc], { asOf: value })).toThrow(/valid date/);
    }
  });
  it("keeps future and topicless decisions unknown and checks limits", () => {
    const doc = source("future", "2020-01-01", "  - text: Future\n    topic: plan\n    effective_date: 2030-01-01\n  - text: No topic");
    expect(buildDecisionHistory([doc], { asOf: "2020-01-02" }).every((entry) => entry.status === "unknown")).toBe(true);
    expect(() => buildDecisionHistory([doc], { asOf: "not a date" })).toThrow();
    expect(() => buildDecisionHistory([doc], { limit: 0 })).toThrow();
  });
});
