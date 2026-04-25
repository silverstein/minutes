import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { mkdtempSync, writeFileSync, mkdirSync, rmSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import {
  splitFrontmatter,
  parseFrontmatter,
  listMeetings,
  searchMeetings,
  getMeeting,
  getMeetingWithOverlays,
  applySpeakerOverlays,
  humanizeTranscript,
  findOpenActions,
  getPersonProfile,
  type MeetingFile,
} from "./reader.js";

// ── Test fixtures ────────────────────────────────────────────

const VALID_MEETING = `---
title: Q2 Pricing Discussion
type: meeting
date: "2026-03-17T14:00:00"
duration: 42m
status: complete
tags:
  - pricing
  - q2
attendees:
  - Alex K.
  - Jordan M.
people:
  - alex-k
  - jordan-m
action_items:
  - assignee: mat
    task: Send pricing doc
    due: Friday
    status: open
  - assignee: sarah
    task: Review competitor grid
    due: March 21
    status: done
decisions:
  - text: Run pricing experiment at monthly billing
    topic: pricing
intents: []
---

## Summary
Alex proposed monthly billing instead of annual.

## Transcript
[SPEAKER_0 0:00] So let's talk about the pricing...
[SPEAKER_1 4:20] I think monthly billing makes more sense...
`;

const MINIMAL_MEETING = `---
title: Quick Sync
type: memo
date: "2026-03-18T09:00:00"
duration: 2m
tags: []
attendees: []
people: []
action_items: []
decisions: []
intents: []
---

Just a quick thought about onboarding.
`;

const EARLIER_MEETING = `---
title: Earlier Meeting
type: meeting
date: "2026-03-10T10:00:00"
duration: 30m
tags: []
attendees: []
people: []
action_items: []
decisions: []
intents: []
---

This happened earlier.
`;

// ── Helpers ──────────────────────────────────────────────────

let tempDir: string;

beforeEach(() => {
  tempDir = mkdtempSync(join(tmpdir(), "minutes-test-"));
});

afterEach(() => {
  rmSync(tempDir, { recursive: true, force: true });
});

function writeMeeting(name: string, content: string): string {
  const path = join(tempDir, name);
  writeFileSync(path, content);
  return path;
}

// ── splitFrontmatter ─────────────────────────────────────────

describe("splitFrontmatter", () => {
  it("splits valid frontmatter from body", () => {
    const { yaml, body } = splitFrontmatter(VALID_MEETING);
    expect(yaml).toContain("title: Q2 Pricing Discussion");
    expect(body).toContain("Alex proposed monthly billing");
  });

  it("returns null yaml for content without frontmatter", () => {
    const { yaml, body } = splitFrontmatter("Just plain text.");
    expect(yaml).toBeNull();
    expect(body).toBe("Just plain text.");
  });

  it("returns null yaml for unclosed frontmatter", () => {
    const { yaml, body } = splitFrontmatter("---\ntitle: Test\nno closing");
    expect(yaml).toBeNull();
  });

  it("handles empty string", () => {
    const { yaml, body } = splitFrontmatter("");
    expect(yaml).toBeNull();
    expect(body).toBe("");
  });
});

// ── parseFrontmatter ─────────────────────────────────────────

describe("parseFrontmatter", () => {
  it("parses valid meeting with all fields", () => {
    const result = parseFrontmatter(VALID_MEETING, "/test/meeting.md");
    expect(result).not.toBeNull();
    expect(result!.frontmatter.title).toBe("Q2 Pricing Discussion");
    expect(result!.frontmatter.type).toBe("meeting");
    expect(result!.frontmatter.duration).toBe("42m");
    expect(result!.frontmatter.tags).toEqual(["pricing", "q2"]);
    expect(result!.frontmatter.attendees).toContain("Alex K.");
    expect(result!.frontmatter.action_items).toHaveLength(2);
    expect(result!.frontmatter.action_items[0].assignee).toBe("mat");
    expect(result!.frontmatter.decisions).toHaveLength(1);
    expect(result!.frontmatter.decisions[0].topic).toBe("pricing");
    expect(result!.body).toContain("Alex proposed monthly billing");
    expect(result!.path).toBe("/test/meeting.md");
  });

  it("parses meeting with minimal fields", () => {
    const result = parseFrontmatter(MINIMAL_MEETING, "/test/memo.md");
    expect(result).not.toBeNull();
    expect(result!.frontmatter.title).toBe("Quick Sync");
    expect(result!.frontmatter.type).toBe("memo");
    expect(result!.frontmatter.action_items).toEqual([]);
  });

  it("returns null for content without frontmatter", () => {
    const result = parseFrontmatter("Just text", "/test/plain.md");
    expect(result).toBeNull();
  });

  it("returns null for malformed YAML", () => {
    const content = "---\ntitle: [invalid yaml{{\n---\n\nBody";
    const result = parseFrontmatter(content, "/test/bad.md");
    expect(result).toBeNull();
  });

  it("returns null for empty file", () => {
    const result = parseFrontmatter("", "/test/empty.md");
    expect(result).toBeNull();
  });

  it("handles missing optional fields gracefully", () => {
    const content = `---
title: Bare Minimum
type: meeting
date: "2026-03-17"
duration: 5m
---

Body text.
`;
    const result = parseFrontmatter(content, "/test/bare.md");
    expect(result).not.toBeNull();
    expect(result!.frontmatter.tags).toEqual([]);
    expect(result!.frontmatter.action_items).toEqual([]);
    expect(result!.frontmatter.decisions).toEqual([]);
  });
});

// ── listMeetings ─────────────────────────────────────────────

describe("listMeetings", () => {
  it("lists meetings sorted by date descending", async () => {
    writeMeeting("earlier.md", EARLIER_MEETING);
    writeMeeting("later.md", VALID_MEETING);

    const meetings = await listMeetings(tempDir, 10);
    expect(meetings).toHaveLength(2);
    expect(meetings[0].frontmatter.title).toBe("Q2 Pricing Discussion");
    expect(meetings[1].frontmatter.title).toBe("Earlier Meeting");
  });

  it("returns empty array for empty directory", async () => {
    const meetings = await listMeetings(tempDir, 10);
    expect(meetings).toEqual([]);
  });

  it("returns empty array for non-existent directory", async () => {
    const meetings = await listMeetings("/nonexistent/path", 10);
    expect(meetings).toEqual([]);
  });

  it("scans subdirectories recursively", async () => {
    const subdir = join(tempDir, "memos");
    mkdirSync(subdir);
    writeMeeting("meeting.md", VALID_MEETING);
    writeFileSync(join(subdir, "memo.md"), MINIMAL_MEETING);

    const meetings = await listMeetings(tempDir, 10);
    expect(meetings).toHaveLength(2);
  });

  it("ignores non-.md files", async () => {
    writeMeeting("meeting.md", VALID_MEETING);
    writeFileSync(join(tempDir, "notes.txt"), "not a meeting");
    writeFileSync(join(tempDir, "image.png"), "not a meeting");

    const meetings = await listMeetings(tempDir, 10);
    expect(meetings).toHaveLength(1);
  });

  it("respects limit parameter", async () => {
    writeMeeting("a.md", VALID_MEETING);
    writeMeeting("b.md", MINIMAL_MEETING);
    writeMeeting("c.md", EARLIER_MEETING);

    const meetings = await listMeetings(tempDir, 2);
    expect(meetings).toHaveLength(2);
  });

  it("skips files with malformed frontmatter", async () => {
    writeMeeting("good.md", VALID_MEETING);
    writeMeeting("bad.md", "---\n[invalid yaml{{\n---\n\nBody");

    const meetings = await listMeetings(tempDir, 10);
    expect(meetings).toHaveLength(1);
    expect(meetings[0].frontmatter.title).toBe("Q2 Pricing Discussion");
  });
});

// ── searchMeetings ───────────────────────────────────────────

describe("searchMeetings", () => {
  it("finds meetings by title match", async () => {
    writeMeeting("pricing.md", VALID_MEETING);
    writeMeeting("memo.md", MINIMAL_MEETING);

    const results = await searchMeetings(tempDir, "Pricing", 10);
    expect(results).toHaveLength(1);
    expect(results[0].frontmatter.title).toBe("Q2 Pricing Discussion");
  });

  it("finds meetings by body text match", async () => {
    writeMeeting("pricing.md", VALID_MEETING);
    writeMeeting("memo.md", MINIMAL_MEETING);

    const results = await searchMeetings(tempDir, "onboarding", 10);
    expect(results).toHaveLength(1);
    expect(results[0].frontmatter.title).toBe("Quick Sync");
  });

  it("performs case-insensitive search", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const results = await searchMeetings(tempDir, "pricing", 10);
    expect(results).toHaveLength(1);
  });

  it("returns empty array for no matches", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const results = await searchMeetings(tempDir, "nonexistent query", 10);
    expect(results).toEqual([]);
  });

  it("returns empty array for empty query", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const results = await searchMeetings(tempDir, "", 10);
    expect(results).toEqual([]);
  });

  it("handles special characters in query without crashing", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    // These would crash if using RegExp — String.includes() is safe
    const results = await searchMeetings(tempDir, "C++ meeting (test)", 10);
    expect(results).toEqual([]);
  });
});

// ── getMeeting ───────────────────────────────────────────────

describe("getMeeting", () => {
  it("returns parsed meeting for valid path", async () => {
    const path = writeMeeting("meeting.md", VALID_MEETING);

    const result = await getMeeting(path);
    expect(result).not.toBeNull();
    expect(result!.frontmatter.title).toBe("Q2 Pricing Discussion");
  });

  it("returns null for non-existent file", async () => {
    const result = await getMeeting("/nonexistent/file.md");
    expect(result).toBeNull();
  });

  it("returns null for malformed file", async () => {
    const path = writeMeeting("bad.md", "not yaml frontmatter at all");

    const result = await getMeeting(path);
    expect(result).toBeNull();
  });
});

// ── findOpenActions ──────────────────────────────────────────

describe("findOpenActions", () => {
  it("finds open action items", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const actions = await findOpenActions(tempDir);
    expect(actions).toHaveLength(1);
    expect(actions[0].item.assignee).toBe("mat");
    expect(actions[0].item.task).toBe("Send pricing doc");
  });

  it("filters by assignee", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const actions = await findOpenActions(tempDir, "mat");
    expect(actions).toHaveLength(1);

    const noActions = await findOpenActions(tempDir, "nobody");
    expect(noActions).toEqual([]);
  });
});

// ── getPersonProfile ─────────────────────────────────────────

describe("getPersonProfile", () => {
  it("builds profile from meeting attendees", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const profile = await getPersonProfile(tempDir, "Alex");
    expect(profile.meetings).toHaveLength(1);
    expect(profile.meetings[0].title).toBe("Q2 Pricing Discussion");
    expect(profile.topics).toContain("pricing");
  });

  it("returns empty profile for unknown person", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const profile = await getPersonProfile(tempDir, "UnknownPerson");
    expect(profile.meetings).toHaveLength(0);
  });
});

// ── speaker_map parsing ──────────────────────────────────────

describe("parseFrontmatter speaker_map", () => {
  const MEETING_WITH_SPEAKERS = `---
title: Speaker Test
type: meeting
date: "2026-04-25T10:00:00"
duration: 10m
tags: []
attendees: []
people: []
action_items: []
decisions: []
intents: []
speaker_map:
  - speaker_label: SPEAKER_0
    name: Speaker 0
    confidence: medium
    source: llm
  - speaker_label: SPEAKER_1
    name: Alex Kim
    confidence: high
    source: manual
---

## Transcript

SPEAKER_0: hello
`;

  it("parses speaker_map entries when present", () => {
    const result = parseFrontmatter(MEETING_WITH_SPEAKERS, "/t/m.md");
    expect(result?.frontmatter.speaker_map).toHaveLength(2);
    expect(result?.frontmatter.speaker_map?.[0]).toEqual({
      speaker_label: "SPEAKER_0",
      name: "Speaker 0",
      confidence: "medium",
      source: "llm",
    });
    expect(result?.frontmatter.speaker_map?.[1].source).toBe("manual");
  });

  it("returns undefined speaker_map when YAML omits the field", () => {
    const stripped = MEETING_WITH_SPEAKERS.replace(
      /speaker_map:[\s\S]*?(?=---)/,
      ""
    );
    const result = parseFrontmatter(stripped, "/t/m.md");
    expect(result?.frontmatter.speaker_map).toBeUndefined();
  });

  it("falls back to safe defaults for unknown confidence/source values", () => {
    const sketchy = MEETING_WITH_SPEAKERS.replace(
      /confidence: medium/,
      "confidence: bogus"
    ).replace(/source: llm/, "source: aliens");
    const result = parseFrontmatter(sketchy, "/t/m.md");
    expect(result?.frontmatter.speaker_map?.[0].confidence).toBe("medium");
    expect(result?.frontmatter.speaker_map?.[0].source).toBe("llm");
  });
});

// ── applySpeakerOverlays ─────────────────────────────────────

describe("applySpeakerOverlays", () => {
  function meetingWith(speakers: any[] | undefined): MeetingFile {
    return {
      frontmatter: {
        title: "T",
        type: "meeting",
        date: "2026-04-25T10:00:00",
        duration: "1m",
        tags: [],
        attendees: [],
        people: [],
        action_items: [],
        decisions: [],
        intents: [],
        speaker_map: speakers as any,
      },
      body: "## Transcript\n\nSPEAKER_0: hi\n",
      path: "/t/m.md",
    };
  }

  it("returns the input meeting unchanged when no confirmations", () => {
    const meeting = meetingWith([
      { speaker_label: "SPEAKER_0", name: "Alex", confidence: "low", source: "llm" },
    ]);
    expect(applySpeakerOverlays(meeting, [])).toBe(meeting);
  });

  it("overrides existing speaker_map entries with high/manual", () => {
    const meeting = meetingWith([
      { speaker_label: "SPEAKER_0", name: "Speaker 0", confidence: "medium", source: "llm" },
    ]);
    const out = applySpeakerOverlays(meeting, [
      { speaker_label: "SPEAKER_0", name: "Alex Kim", previous_name: "Speaker 0" },
    ]);
    expect(out.frontmatter.speaker_map?.[0]).toEqual({
      speaker_label: "SPEAKER_0",
      name: "Alex Kim",
      confidence: "high",
      source: "manual",
    });
  });

  it("appends confirmations for speakers not yet in the map", () => {
    const meeting = meetingWith([]);
    const out = applySpeakerOverlays(meeting, [
      { speaker_label: "SPEAKER_2", name: "Jordan" },
    ]);
    expect(out.frontmatter.speaker_map).toHaveLength(1);
    expect(out.frontmatter.speaker_map?.[0].name).toBe("Jordan");
  });

  it("does not mutate the input meeting object", () => {
    const meeting = meetingWith([
      { speaker_label: "SPEAKER_0", name: "Speaker 0", confidence: "medium", source: "llm" },
    ]);
    applySpeakerOverlays(meeting, [
      { speaker_label: "SPEAKER_0", name: "Alex Kim" },
    ]);
    expect(meeting.frontmatter.speaker_map?.[0].name).toBe("Speaker 0");
  });

  it("handles meetings whose frontmatter has no speaker_map", () => {
    const meeting = meetingWith(undefined);
    const out = applySpeakerOverlays(meeting, [
      { speaker_label: "SPEAKER_0", name: "Alex Kim" },
    ]);
    expect(out.frontmatter.speaker_map).toEqual([
      {
        speaker_label: "SPEAKER_0",
        name: "Alex Kim",
        confidence: "high",
        source: "manual",
      },
    ]);
  });

  it("ignores confirmations missing speaker_label or name", () => {
    const meeting = meetingWith([]);
    const out = applySpeakerOverlays(meeting, [
      { speaker_label: "", name: "Ghost" },
      { speaker_label: "SPEAKER_3", name: "" },
      { speaker_label: "SPEAKER_4", name: "Real" },
    ]);
    expect(out.frontmatter.speaker_map).toEqual([
      {
        speaker_label: "SPEAKER_4",
        name: "Real",
        confidence: "high",
        source: "manual",
      },
    ]);
  });
});

// ── humanizeTranscript ───────────────────────────────────────

describe("humanizeTranscript", () => {
  const HIGH_ALEX = {
    speaker_label: "SPEAKER_0",
    name: "Alex Kim",
    confidence: "high" as const,
    source: "manual" as const,
  };
  const HIGH_JORDAN = {
    speaker_label: "SPEAKER_1",
    name: "Jordan Park",
    confidence: "high" as const,
    source: "manual" as const,
  };
  const MEDIUM_ALEX = {
    speaker_label: "SPEAKER_0",
    name: "Alex Kim",
    confidence: "medium" as const,
    source: "llm" as const,
  };

  it("rewrites bracketed speaker prefixes for high-confidence entries", () => {
    const body = "[SPEAKER_0 0:00] hello\n[SPEAKER_1 0:05] hi\n";
    const out = humanizeTranscript(body, [HIGH_ALEX, HIGH_JORDAN]);
    expect(out).toBe("[Alex Kim 0:00] hello\n[Jordan Park 0:05] hi\n");
  });

  it("leaves medium/low-confidence speakers untouched", () => {
    const body = "[SPEAKER_0 0:00] hello\n";
    expect(humanizeTranscript(body, [MEDIUM_ALEX])).toBe(body);
  });

  it("returns body unchanged when speaker_map is empty or undefined", () => {
    const body = "[SPEAKER_0 0:00] hello\n";
    expect(humanizeTranscript(body, undefined)).toBe(body);
    expect(humanizeTranscript(body, [])).toBe(body);
  });

  it("preserves non-lexical event tags inside the body", () => {
    const body = "[SPEAKER_0 0:00] [laughter]\n[SPEAKER_0 0:05] real words\n";
    const out = humanizeTranscript(body, [HIGH_ALEX]);
    expect(out).toBe("[SPEAKER_0 0:00] [laughter]\n[Alex Kim 0:05] real words\n");
  });

  it("leaves non-bracketed lines (headings, prose, blanks) alone", () => {
    const body = "## Transcript\n\nSome free-form note.\n[SPEAKER_0 0:00] hi\n";
    const out = humanizeTranscript(body, [HIGH_ALEX]);
    expect(out).toBe(
      "## Transcript\n\nSome free-form note.\n[Alex Kim 0:00] hi\n"
    );
  });

  it("is idempotent on already-humanized text", () => {
    const body = "[Alex Kim 0:00] hello\n";
    expect(humanizeTranscript(body, [HIGH_ALEX])).toBe(body);
  });

  it("is non-mutating on the input string", () => {
    const original = "[SPEAKER_0 0:00] hello\n";
    const body = original;
    humanizeTranscript(body, [HIGH_ALEX]);
    expect(body).toBe(original);
  });

  it("handles malformed bracket lines gracefully", () => {
    const body = "[SPEAKER_0 hello\n[NoTimestamp]\n[SPEAKER_0 0:00] real\n";
    const out = humanizeTranscript(body, [HIGH_ALEX]);
    expect(out).toBe(
      "[SPEAKER_0 hello\n[NoTimestamp]\n[Alex Kim 0:00] real\n"
    );
  });
});

// ── getMeetingWithOverlays graceful fallback ─────────────────

describe("getMeetingWithOverlays", () => {
  it("falls back to plain getMeeting when the CLI is unavailable", async () => {
    const path = writeMeeting("meeting.md", VALID_MEETING);
    // Point at a binary that definitely doesn't exist so the helper's
    // execFile path errors and the function falls back cleanly.
    const out = await getMeetingWithOverlays(path, {
      minutesBin: "/nonexistent/minutes-binary-for-test",
      timeoutMs: 2000,
    });
    expect(out?.frontmatter.title).toBe("Q2 Pricing Discussion");
  });

  it("returns null for a nonexistent meeting file even with overlay flag", async () => {
    const out = await getMeetingWithOverlays(
      join(tempDir, "does-not-exist.md"),
      { minutesBin: "/nonexistent" }
    );
    expect(out).toBeNull();
  });
});
