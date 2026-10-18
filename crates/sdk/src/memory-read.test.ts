import { afterEach, describe, expect, it } from "vitest";
import { mkdtempSync, writeFileSync, rmSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { getDecisionHistory, getMeeting, getPersonProfile, searchMeetings } from "./reader.js";

const dirs: string[] = [];
const corpus = () => { const dir = mkdtempSync(join(tmpdir(), "minutes-memory-contract-")); dirs.push(dir); return dir; };
afterEach(() => { for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true }); });
const markdown = (sensitivity = "normal", text = "Monthly billing", supersedes = "") => `---
title: Café planning
type: meeting
date: 2020-01-01T12:00:00Z
sensitivity: ${sensitivity}
attendees: [Joanna]
decisions:
  - text: ${text}
    topic: pricing
${supersedes ? `    supersedes: ${supersedes}\n` : ""}---
[SPEAKER_0 0:15] The roadmap and pricing changed.
`;

describe("authorized memory API", () => {
  it("returns shared lexical matches with exact source evidence", async () => {
    const dir = corpus(); const path = join(dir, "source.md"); const content = markdown();
    writeFileSync(path, content);
    const results = await searchMeetings(dir, "CAFE road");
    expect(results).toHaveLength(1);
    expect(results[0].retrieval?.contract).toBe("literal-and-prefix-v1");
    const proof = results[0].evidence!.segments[0];
    expect(content.slice(proof.start, proof.end)).toBe(proof.quote);
    const exact = await getMeeting(path, { rootDir: dir });
    expect(exact && !exact.restricted_stub && exact.evidence?.source_revision).toBe(results[0].evidence!.source_revision);
    expect((await getPersonProfile(dir, "Ann")).meetings).toEqual([]);
    await expect(getPersonProfile(dir, " ")).rejects.toThrow(/empty/);
  });
  it("revokes evidence and history when sensitivity changes", async () => {
    const dir = corpus(); const path = join(dir, "source.md");
    writeFileSync(path, markdown());
    expect(await getDecisionHistory(dir)).toHaveLength(1);
    expect(await searchMeetings(dir, "pricing")).toHaveLength(1);
    writeFileSync(path, markdown("restricted", "PRIVATE_CANARY"));
    expect(await getDecisionHistory(dir)).toEqual([]);
    expect(await searchMeetings(dir, "pricing")).toEqual([]);
    const exact = await getMeeting(path, { rootDir: dir });
    expect(exact?.restricted_stub).toBe(true);
    expect(JSON.stringify(exact)).not.toMatch(/PRIVATE_CANARY|source_revision|segments|source\.md/);
    const overridden = await getDecisionHistory(dir, { includeRestricted: true });
    expect(overridden[0].text).toBe("PRIVATE_CANARY");
  });
  it("cannot resolve a visible decision through a hidden or restricted predecessor", async () => {
    const dir = corpus();
    writeFileSync(join(dir, "public.md"), markdown("normal", "Annual billing", "PRIVATE_CANARY"));
    writeFileSync(join(dir, "private.md"), markdown("restricted", "PRIVATE_CANARY"));
    mkdirSync(join(dir, "archive"));
    writeFileSync(join(dir, "archive", "old.md"), markdown("normal", "PRIVATE_CANARY"));
    const history = await getDecisionHistory(dir);
    expect(history).toHaveLength(1);
    expect(history[0].status).toBe("unresolved");
    expect(history[0].supersedes).toBeUndefined();
    expect(history[0].unresolved_reason).toBe("unresolved-supersedes");
  });
});
