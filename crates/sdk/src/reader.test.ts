import { describe, it, expect, expectTypeOf, beforeEach, afterEach, vi } from "vitest";
import type { ExactMeetingResult, ReadOptions, RestrictedMeetingStub } from "./index.js";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "fs";
import { join } from "path";
import { tmpdir } from "os";
import {
  splitFrontmatter,
  parseFrontmatter,
  parseAttributionSource,
  listMeetings,
  searchMeetings,
  getMeeting,
  getMeetingWithOverlays,
  applySpeakerOverlays,
  humanizeTranscript,
  findOpenActions,
  getPersonProfile,
  findDecisions,
  listVoiceMemos,
  isRestricted,
  canonicalPathWireEquals,
  normalizeCanonicalPathWire,
  SDK_DECISION_RESULT_MAX,
  SDK_OPEN_ACTION_RESULT_MAX,
  SDK_PERSON_PROFILE_MEETING_MAX,
  SDK_PERSON_PROFILE_OPEN_ACTION_MAX,
  SDK_PERSON_PROFILE_TOPIC_MAX,
  SDK_VOICE_MEMO_LOOKBACK_MAX_DAYS,
  SDK_VOICE_MEMO_RESULT_MAX,
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

// A meeting designated restricted (Wave 2 sensitivity enforcement). Carries a
// searchable title/body, an open action, a decision, and an attendee so the
// exclusion can be asserted across every agent-facing read surface.
const RESTRICTED_MEETING = `---
title: Board Pricing Strategy
type: meeting
date: "2026-03-19T16:00:00"
duration: 25m
capture: none
sensitivity: restricted
tags:
  - pricing
attendees:
  - Alex K.
people:
  - alex-k
action_items:
  - assignee: mat
    task: Draft confidential pricing memo
    due: Monday
    status: open
decisions:
  - text: Hold the secret pricing floor at cost plus ten
    topic: pricing
intents: []
---

## Notes
- [0:01] Confidential board pricing discussion.
`;

// ── Helpers ──────────────────────────────────────────────────

let tempDir: string;
let previousMeetingsDir: string | undefined;

beforeEach(() => {
  tempDir = mkdtempSync(join(tmpdir(), "minutes-test-"));
  previousMeetingsDir = process.env.MEETINGS_DIR;
  process.env.MEETINGS_DIR = tempDir;
});

afterEach(() => {
  if (previousMeetingsDir === undefined) {
    delete process.env.MEETINGS_DIR;
  } else {
    process.env.MEETINGS_DIR = previousMeetingsDir;
  }
  rmSync(tempDir, { recursive: true, force: true });
});

function writeMeeting(name: string, content: string): string {
  const path = join(tempDir, name);
  writeFileSync(path, content);
  return path;
}

function restrictedMeetingWithInvalidUtf8Key(canary: string): Buffer {
  const bytes = Buffer.from(RESTRICTED_MEETING.replace(
    "Confidential board pricing discussion.",
    canary
  ));
  const key = Buffer.from("sensitivity");
  const keyOffset = bytes.indexOf(key);
  if (keyOffset < 0) throw new Error("sensitivity fixture key missing");
  bytes[keyOffset + 5] = 0xff;
  return bytes;
}

function collectionMeeting({
  title = "Collection Bounds",
  type = "meeting",
  date = "2026-07-16T12:00:00Z",
  actionCount = 0,
  decisionCount = 0,
  tagCount = 0,
  person = "Alex",
}: {
  title?: string;
  type?: "meeting" | "memo";
  date?: string;
  actionCount?: number;
  decisionCount?: number;
  tagCount?: number;
  person?: string;
} = {}): string {
  const tags = Array.from({ length: tagCount }, (_, index) => `  - topic-${index}`);
  const actions = Array.from(
    { length: actionCount },
    (_, index) =>
      `  - assignee: ${person}\n    task: task-${index}\n    status: open`
  );
  const decisions = Array.from(
    { length: decisionCount },
    (_, index) => `  - text: decision-${index}\n    topic: topic-${index}`
  );
  return [
    "---",
    `title: ${title}`,
    `type: ${type}`,
    `date: ${date}`,
    "duration: 1m",
    "tags:",
    ...(tags.length > 0 ? tags : ["  []"]),
    "attendees:",
    `  - ${person}`,
    "people: []",
    "action_items:",
    ...(actions.length > 0 ? actions : ["  []"]),
    "decisions:",
    ...(decisions.length > 0 ? decisions : ["  []"]),
    "intents: []",
    "---",
    "",
    `${person} collection body.`,
  ].join("\n");
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

  it("parses note artifacts in the meeting corpus", () => {
    const content = `---
title: Launch Prep
type: note
date: "2026-09-15T10:00:00Z"
---

Launch checklist.
`;
    const result = parseFrontmatter(content, "/test/launch-prep.md");
    expect(result).not.toBeNull();
    expect(result!.frontmatter.type).toBe("note");
    expect(result!.body).toContain("Launch checklist");
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

  it("fails closed when required policy-bearing meeting fields are missing or invalid", () => {
    const valid = `---
title: Required fields
type: meeting
date: "2026-07-15T10:00:00Z"
---

REQUIRED_FIELD_CANARY
`;
    for (const content of [
      valid.replace("title: Required fields\n", ""),
      valid.replace("type: meeting\n", ""),
      valid.replace('date: "2026-07-15T10:00:00Z"\n', ""),
      valid.replace("title: Required fields", "title: [not, text]"),
      valid.replace("type: meeting", "type: private-meeting"),
      valid.replace('date: "2026-07-15T10:00:00Z"', "date: not-a-date"),
    ]) {
      expect(parseFrontmatter(content, "/test/policy-uncertain.md")).toBeNull();
    }
  });

  it("preserves sensitive no-capture frontmatter", () => {
    const content = `---
title: Board Sync
type: meeting
date: "2026-06-10T12:00:00-07:00"
duration: 12m
capture: none
sensitivity: restricted
debrief: pending
---

## Notes

- [0:01] Marker.
`;
    const result = parseFrontmatter(content, "/test/sensitive.md");
    expect(result).not.toBeNull();
    expect(result!.frontmatter.capture).toBe("none");
    expect(result!.frontmatter.sensitivity).toBe("restricted");
    expect(result!.frontmatter.debrief).toBe("pending");
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

  it("lists note artifacts from the meeting corpus", async () => {
    writeMeeting(
      "launch-prep.md",
      MINIMAL_MEETING.replace("Quick Sync", "Launch Prep").replace("type: memo", "type: note")
    );

    const meetings = await listMeetings(tempDir, 10);
    expect(meetings).toHaveLength(1);
    expect(meetings[0].frontmatter.type).toBe("note");
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

  it("prunes every inactive corpus directory, including mixed-case spellings", async () => {
    writeMeeting("live.md", VALID_MEETING);
    for (const [index, directory] of [
      "archive",
      "Processed",
      "FAILED",
      "Failed-Captures",
      ".git",
      ".private",
    ].entries()) {
      const inactive = join(tempDir, directory);
      mkdirSync(inactive);
      writeFileSync(
        join(inactive, `private-${index}.md`),
        MINIMAL_MEETING.replace("Quick Sync", `INACTIVE-CANARY-${index}`)
      );
    }

    const meetings = await listMeetings(tempDir, 100);
    expect(meetings.map((meeting) => meeting.frontmatter.title)).toEqual([
      "Q2 Pricing Discussion",
    ]);
    expect(JSON.stringify(meetings)).not.toContain("INACTIVE-CANARY");
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

  it("orders ISO offsets by instant and breaks equal-instant ties by path", async () => {
    const template = (title: string, date: string) =>
      MINIMAL_MEETING.replace("Quick Sync", title).replace(
        /date: .+/,
        `date: ${date}`
      );
    writeMeeting(
      "z-early.md",
      template("Earlier instant", "2026-01-01T10:00:00+10:00")
    );
    writeMeeting(
      "m-late.md",
      template("Later instant", "2026-01-01T02:00:00-10:00")
    );
    writeMeeting(
      "b-tie.md",
      template("Tie B", "2026-01-01T12:00:00Z")
    );
    writeMeeting(
      "a-tie.md",
      template("Tie A", "2026-01-01T07:00:00-05:00")
    );

    const meetings = await listMeetings(tempDir, 10);
    expect(meetings.map((meeting) => meeting.frontmatter.title)).toEqual([
      "Tie A",
      "Tie B",
      "Later instant",
      "Earlier instant",
    ]);
  });

  it("rejects non-positive, fractional, and excessive result limits", async () => {
    writeMeeting("meeting.md", VALID_MEETING);
    for (const invalid of [0, -1, 1.5, 10_001]) {
      await expect(listMeetings(tempDir, invalid)).rejects.toThrow(/limit must be/i);
      await expect(searchMeetings(tempDir, "pricing", invalid)).rejects.toThrow(
        /limit must be/i
      );
    }
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
  it("uses MEETINGS_DIR as the authoritative root for a valid exact path", async () => {
    const path = writeMeeting("meeting.md", VALID_MEETING);

    const result = await getMeeting(path);
    expect(result).not.toBeNull();
    expect(result!.frontmatter.title).toBe("Q2 Pricing Discussion");
  });

  it("returns null for non-existent file", async () => {
    const result = await getMeeting("/nonexistent/file.md");
    expect(result).toBeNull();
  });

  it("rejects exact paths outside the configured root and in inactive directories", async () => {
    const outsideDir = mkdtempSync(join(tmpdir(), "minutes-exact-outside-"));
    try {
      const outside = join(outsideDir, "outside.md");
      writeFileSync(outside, VALID_MEETING);

      expect(await getMeeting(outside)).toBeNull();
      for (const directory of ["archive", "ArChIvE", ".recovery"]) {
        const inactive = join(tempDir, directory);
        mkdirSync(inactive);
        const inactiveMeeting = join(inactive, "meeting.md");
        writeFileSync(inactiveMeeting, VALID_MEETING);
        expect(await getMeeting(inactiveMeeting)).toBeNull();
        // Default macOS volumes are case-insensitive, so exercise mixed-case
        // spellings sequentially rather than assuming both directory names
        // can coexist in one corpus.
        rmSync(inactive, { recursive: true, force: true });
      }
    } finally {
      rmSync(outsideDir, { recursive: true, force: true });
    }
  });

  it("accepts an explicit authoritative root without consulting MEETINGS_DIR", async () => {
    const configuredElsewhere = mkdtempSync(join(tmpdir(), "minutes-configured-elsewhere-"));
    try {
      process.env.MEETINGS_DIR = configuredElsewhere;
      const path = writeMeeting("meeting.md", VALID_MEETING);

      expect(await getMeeting(path)).toBeNull();
      expect(await getMeeting(path, { rootDir: tempDir })).not.toBeNull();
    } finally {
      rmSync(configuredElsewhere, { recursive: true, force: true });
    }
  });

  it("returns null for malformed file", async () => {
    const path = writeMeeting("bad.md", "not yaml frontmatter at all");

    const result = await getMeeting(path);
    expect(result).toBeNull();
  });

  it.skipIf(process.platform === "win32")(
    "rejects a symlink to a valid meeting outside the active root",
    async () => {
      const outsideDir = mkdtempSync(join(tmpdir(), "minutes-outside-"));
      try {
        const outside = join(outsideDir, "outside.md");
        writeFileSync(outside, VALID_MEETING);
        const linked = join(tempDir, "linked.md");
        symlinkSync(outside, linked);

        expect(await getMeeting(linked)).toBeNull();
        expect(await listMeetings(tempDir, 10)).toEqual([]);
      } finally {
        rmSync(outsideDir, { recursive: true, force: true });
      }
    }
  );
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

  it("bounds collection before appending more actions and rejects invalid limits", async () => {
    writeMeeting(
      "actions.md",
      collectionMeeting({ actionCount: 4 })
    );

    const actions = await findOpenActions(tempDir, undefined, { limit: 2 });
    expect(actions.map((action) => action.item.task)).toEqual(["task-0", "task-1"]);
    for (const invalid of [0, -1, 1.5, SDK_OPEN_ACTION_RESULT_MAX + 1, Infinity]) {
      await expect(
        findOpenActions(tempDir, undefined, { limit: invalid })
      ).rejects.toThrow(/findOpenActions limit must be/i);
    }
  });
});

// ── getPersonProfile ─────────────────────────────────────────

describe("getPersonProfile", () => {
  it("builds profile from meeting attendees", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const profile = await getPersonProfile(tempDir, "Alex K.");
    expect(profile.meetings).toHaveLength(1);
    expect(profile.meetings[0].title).toBe("Q2 Pricing Discussion");
    expect(profile.topics).toContain("pricing");
  });

  it("returns empty profile for unknown person", async () => {
    writeMeeting("meeting.md", VALID_MEETING);

    const profile = await getPersonProfile(tempDir, "UnknownPerson");
    expect(profile.meetings).toHaveLength(0);
  });

  it("independently bounds meetings, actions, and topics", async () => {
    writeMeeting(
      "old.md",
      collectionMeeting({
        title: "Old",
        date: "2026-07-14T12:00:00Z",
        actionCount: 3,
        tagCount: 3,
      })
    );
    writeMeeting(
      "new.md",
      collectionMeeting({
        title: "New",
        date: "2026-07-16T12:00:00Z",
        actionCount: 3,
        tagCount: 3,
      })
    );
    writeMeeting(
      "middle.md",
      collectionMeeting({
        title: "Middle",
        date: "2026-07-15T12:00:00Z",
        actionCount: 3,
        tagCount: 3,
      })
    );

    const profile = await getPersonProfile(tempDir, "Alex", {
      meetingLimit: 2,
      openActionLimit: 2,
      topicLimit: 2,
    });
    expect(profile.meetings.map((meeting) => meeting.title)).toEqual([
      "New",
      "Middle",
    ]);
    expect(profile.openActions).toHaveLength(2);
    expect(profile.topics).toEqual(["topic-0", "topic-1"]);
  });

  it("rejects invalid independent profile limits", async () => {
    const cases = [
      ["meetingLimit", SDK_PERSON_PROFILE_MEETING_MAX],
      ["openActionLimit", SDK_PERSON_PROFILE_OPEN_ACTION_MAX],
      ["topicLimit", SDK_PERSON_PROFILE_TOPIC_MAX],
    ] as const;
    for (const [field, max] of cases) {
      for (const invalid of [0, -1, 1.5, max + 1, Infinity]) {
        await expect(
          getPersonProfile(tempDir, "Alex", { [field]: invalid })
        ).rejects.toThrow(/getPersonProfile .* limit must be/i);
      }
    }
  });
});

describe("listVoiceMemos", () => {
  it("bounds recent memo results and preserves newest-first ordering", async () => {
    for (const [name, title, date] of [
      ["old.md", "Old memo", "2026-07-14T12:00:00Z"],
      ["new.md", "New memo", "2026-07-16T12:00:00Z"],
      ["middle.md", "Middle memo", "2026-07-15T12:00:00Z"],
    ] as const) {
      writeMeeting(name, collectionMeeting({ title, type: "memo", date }));
    }

    const memos = await listVoiceMemos(tempDir, {
      days: SDK_VOICE_MEMO_LOOKBACK_MAX_DAYS,
      limit: 2,
    });
    expect(memos.map((memo) => memo.frontmatter.title)).toEqual([
      "New memo",
      "Middle memo",
    ]);
  });

  it("rejects invalid result limits and lookback windows", async () => {
    for (const invalid of [0, -1, 1.5, SDK_VOICE_MEMO_RESULT_MAX + 1, Infinity]) {
      await expect(listVoiceMemos(tempDir, { limit: invalid })).rejects.toThrow(
        /listVoiceMemos limit must be/i
      );
    }
    for (const invalid of [
      -1,
      1.5,
      SDK_VOICE_MEMO_LOOKBACK_MAX_DAYS + 1,
      Infinity,
    ]) {
      await expect(listVoiceMemos(tempDir, { days: invalid })).rejects.toThrow(
        /listVoiceMemos days must be/i
      );
    }
  });
});

describe("findDecisions", () => {
  it("bounds decisions during newest-first collection", async () => {
    writeMeeting(
      "old.md",
      collectionMeeting({
        title: "Old",
        date: "2026-07-14T12:00:00Z",
        decisionCount: 3,
      })
    );
    writeMeeting(
      "new.md",
      collectionMeeting({
        title: "New",
        date: "2026-07-16T12:00:00Z",
        decisionCount: 3,
      })
    );

    const decisions = await findDecisions(tempDir, undefined, 2);
    expect(decisions).toHaveLength(2);
    expect(decisions.every((decision) => decision.title === "New")).toBe(true);
  });

  it("rejects invalid decision limits", async () => {
    for (const invalid of [0, -1, 1.5, SDK_DECISION_RESULT_MAX + 1, Infinity]) {
      await expect(
        findDecisions(tempDir, undefined, invalid)
      ).rejects.toThrow(/findDecisions limit must be/i);
    }
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

  it("parses new attribution sources explicitly", () => {
    expect(parseAttributionSource("ml-bleed-degraded")).toBe("ml-bleed-degraded");
    expect(parseAttributionSource("stem-recovery")).toBe("stem-recovery");
    expect(parseAttributionSource("aliens")).toBe("llm");
  });

  it("preserves recording_health enum fields", () => {
    const content = MEETING_WITH_SPEAKERS.replace(
      "speaker_map:",
      `recording_health:
  voice_stem_active_ratio: 0.31
  system_stem_active_ratio: 0
  system_dominant_ratio: 0.12
  capture_warnings:
    - kind: silent
      source: system
      message: System audio was silent during capture.
      diagnostic_confidence: inferred
  diarization_path: ml-bleed-degraded
speaker_map:`
    );
    const result = parseFrontmatter(content, "/t/m.md");

    expect(result?.frontmatter.recording_health).toEqual({
      voice_stem_active_ratio: 0.31,
      system_stem_active_ratio: 0,
      system_dominant_ratio: 0.12,
      capture_warnings: [
        {
          kind: "silent",
          source: "system",
          message: "System audio was silent during capture.",
          diagnostic_confidence: "inferred",
        },
      ],
      diarization_path: "ml-bleed-degraded",
    });
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
  it("compares only equivalent Rust and Node Windows canonical wire prefixes", () => {
    const drive = "C:\\Meetings\\normal.md";
    const unc = "\\\\server\\share\\normal.md";
    expect(normalizeCanonicalPathWire(`\\\\?\\${drive}`)).toBe(drive);
    expect(
      normalizeCanonicalPathWire("\\\\?\\UNC\\server\\share\\normal.md")
    ).toBe(unc);
    expect(canonicalPathWireEquals(`\\\\?\\${drive}`, drive)).toBe(true);
    expect(canonicalPathWireEquals(drive, drive.toLowerCase())).toBe(false);
    expect(canonicalPathWireEquals(drive, drive.replaceAll("\\", "/"))).toBe(
      false
    );
    expect(
      canonicalPathWireEquals("\\\\?\\GLOBALROOT\\Device\\x", "GLOBALROOT\\Device\\x")
    ).toBe(false);
  });

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

  it.skipIf(process.platform === "win32")(
    "never returns a stale normal snapshot after any failed overlay attempt",
    async () => {
      const cases: Array<{
        name: string;
        script: string;
        timeoutMs: number;
        expected: "restricted" | "unreadable";
      }> = [
        {
          name: "nonzero",
          script: `writeFileSync(process.argv[3], ${JSON.stringify(RESTRICTED_MEETING)}); process.exit(7);`,
          timeoutMs: 2_000,
          expected: "restricted",
        },
        {
          name: "timeout",
          script: `writeFileSync(process.argv[3], ${JSON.stringify(RESTRICTED_MEETING)}); setTimeout(() => {}, 60_000);`,
          timeoutMs: 500,
          expected: "restricted",
        },
        {
          name: "empty-stdout",
          script: `writeFileSync(process.argv[3], ${JSON.stringify(RESTRICTED_MEETING)});`,
          timeoutMs: 2_000,
          expected: "restricted",
        },
        {
          name: "invalid-json",
          script: `writeFileSync(process.argv[3], ${JSON.stringify(RESTRICTED_MEETING)}); process.stdout.write("{invalid-json");`,
          timeoutMs: 2_000,
          expected: "restricted",
        },
        {
          name: "malformed-source",
          script: `writeFileSync(process.argv[3], "not meeting markdown"); process.exit(9);`,
          timeoutMs: 2_000,
          expected: "unreadable",
        },
        {
          name: "removed-source",
          script: `rmSync(process.argv[3]); process.stdout.write("{invalid-json");`,
          timeoutMs: 2_000,
          expected: "unreadable",
        },
      ];

      for (const testCase of cases) {
        const path = writeMeeting("meeting.md", VALID_MEETING);
        const fakeMinutes = join(tempDir, `fake-${testCase.name}-minutes.mjs`);
        writeFileSync(
          fakeMinutes,
          `#!/usr/bin/env node\n` +
            `import { rmSync, writeFileSync } from "node:fs";\n` +
            `${testCase.script}\n`
        );
        chmodSync(fakeMinutes, 0o700);

        const out = await getMeetingWithOverlays(path, {
          rootDir: tempDir,
          minutesBin: fakeMinutes,
          timeoutMs: testCase.timeoutMs,
        });

        if (testCase.expected === "restricted") {
          expect(out?.restricted_stub, testCase.name).toBe(true);
        } else {
          expect(out, testCase.name).toBeNull();
        }
        expect(JSON.stringify(out), testCase.name).not.toContain(
          "Alex proposed monthly billing"
        );
      }
    }
  );

  it("returns null for a nonexistent meeting file even with overlay flag", async () => {
    const out = await getMeetingWithOverlays(
      join(tempDir, "does-not-exist.md"),
      { minutesBin: "/nonexistent" }
    );
    expect(out).toBeNull();
  });

  it("does not invoke the overlay path for a source outside the authoritative root", async () => {
    const outsideDir = mkdtempSync(join(tmpdir(), "minutes-overlay-outside-"));
    try {
      const outside = join(outsideDir, "outside.md");
      const marker = join(outsideDir, "overlay-invoked");
      const fakeMinutes = join(outsideDir, "fake-minutes.mjs");
      writeFileSync(outside, VALID_MEETING);
      writeFileSync(
        fakeMinutes,
        `#!/usr/bin/env node\n` +
          `import { writeFileSync } from "node:fs";\n` +
          `writeFileSync(${JSON.stringify(marker)}, "invoked");\n` +
          `process.stdout.write(JSON.stringify({ frontmatter: { speaker_map: [] } }));\n`
      );
      chmodSync(fakeMinutes, 0o700);

      const out = await getMeetingWithOverlays(outside, {
        rootDir: tempDir,
        minutesBin: fakeMinutes,
      });

      expect(out).toBeNull();
      expect(existsSync(marker)).toBe(false);
    } finally {
      rmSync(outsideDir, { recursive: true, force: true });
    }
  });

  it("uses the same explicit root for initial read and overlay reauthorization", async () => {
    const configuredElsewhere = mkdtempSync(join(tmpdir(), "minutes-overlay-config-"));
    try {
      process.env.MEETINGS_DIR = configuredElsewhere;
      const path = writeMeeting("meeting.md", VALID_MEETING);
      const fakeMinutes = join(tempDir, "fake-overlay-minutes.mjs");
      writeFileSync(
        fakeMinutes,
        `#!/usr/bin/env node\n` +
          `import { createHash } from "node:crypto";\n` +
          `import { readFileSync } from "node:fs";\n` +
          `const source = readFileSync(process.argv[3]);\n` +
          `process.stdout.write(JSON.stringify({ path: process.argv[3], overlay_applied: true, overlay_source_sha256: createHash("sha256").update(source).digest("hex"), frontmatter: { speaker_map: [{ speaker_label: "SPEAKER_0", name: "EXPLICIT-ROOT-OVERLAY", confidence: "high", source: "manual" }] } }));\n`
      );
      chmodSync(fakeMinutes, 0o700);

      const out = await getMeetingWithOverlays(path, {
        rootDir: tempDir,
        minutesBin: fakeMinutes,
      });

      if (!out || out.restricted_stub) {
        throw new Error("normal overlay read must return a full meeting");
      }
      expect(out.frontmatter.speaker_map?.[0]?.name).toBe(
        "EXPLICIT-ROOT-OVERLAY"
      );
    } finally {
      rmSync(configuredElsewhere, { recursive: true, force: true });
    }
  });

  it("rejects a path-only overlay response without an exact source proof", async () => {
    const path = writeMeeting("meeting.md", VALID_MEETING);
    const fakeMinutes = join(tempDir, "fake-stale-overlay-minutes.mjs");
    writeFileSync(
      fakeMinutes,
      `#!/usr/bin/env node\n` +
        `process.stdout.write(JSON.stringify({ overlay_applied: true, overlay_source_sha256: "${"0".repeat(64)}", frontmatter: { speaker_map: [{ speaker_label: "SPEAKER_0", name: "STALE-PRIVATE-CANARY", confidence: "high", source: "manual" }] } }));\n`
    );
    chmodSync(fakeMinutes, 0o700);

    const out = await getMeetingWithOverlays(path, {
      minutesBin: fakeMinutes,
      timeoutMs: 2000,
    });

    expect(JSON.stringify(out)).not.toContain("STALE-PRIVATE-CANARY");
  });

  it("rejects an exact-byte overlay attributed to a different canonical source", async () => {
    const path = writeMeeting("meeting.md", VALID_MEETING);
    const other = writeMeeting("duplicate.md", VALID_MEETING);
    const fakeMinutes = join(tempDir, "fake-wrong-source-overlay-minutes.mjs");
    writeFileSync(
      fakeMinutes,
      `#!/usr/bin/env node\n` +
        `import { createHash } from "node:crypto";\n` +
        `import { readFileSync } from "node:fs";\n` +
        `const source = readFileSync(process.argv[3]);\n` +
        `process.stdout.write(JSON.stringify({ path: ${JSON.stringify(other)}, overlay_applied: true, overlay_source_sha256: createHash("sha256").update(source).digest("hex"), frontmatter: { speaker_map: [{ speaker_label: "SPEAKER_0", name: "WRONG-SOURCE-CANARY", confidence: "high", source: "manual" }] } }));\n`
    );
    chmodSync(fakeMinutes, 0o700);

    const out = await getMeetingWithOverlays(path, {
      rootDir: tempDir,
      minutesBin: fakeMinutes,
      timeoutMs: 2000,
    });

    expect(JSON.stringify(out)).not.toContain("WRONG-SOURCE-CANARY");
  });

  it.skipIf(process.platform === "win32")(
    "invokes overlay enrichment with the initially canonical source path",
    async () => {
      const path = writeMeeting("canonical.md", VALID_MEETING);
      const alias = join(tempDir, "alias.md");
      const marker = join(tempDir, "overlay-argv.txt");
      symlinkSync(path, alias);
      const fakeMinutes = join(tempDir, "fake-canonical-argv-minutes.mjs");
      writeFileSync(
        fakeMinutes,
        `#!/usr/bin/env node\n` +
          `import { createHash } from "node:crypto";\n` +
          `import { readFileSync, writeFileSync } from "node:fs";\n` +
          `const source = readFileSync(process.argv[3]);\n` +
          `writeFileSync(${JSON.stringify(marker)}, process.argv[3]);\n` +
          `process.stdout.write(JSON.stringify({ path: process.argv[3], overlay_applied: true, overlay_source_sha256: createHash("sha256").update(source).digest("hex"), frontmatter: { speaker_map: [{ speaker_label: "SPEAKER_0", name: "CANONICAL-OVERLAY", confidence: "high", source: "manual" }] } }));\n`
      );
      chmodSync(fakeMinutes, 0o700);

      const out = await getMeetingWithOverlays(alias, {
        rootDir: tempDir,
        minutesBin: fakeMinutes,
      });
      if (!out || out.restricted_stub) {
        throw new Error("normal overlay read must return a full meeting");
      }
      expect(readFileSync(marker, "utf8")).toBe(path);
      expect(out.frontmatter.speaker_map?.[0]?.name).toBe("CANONICAL-OVERLAY");
    }
  );

  it.skipIf(process.platform === "win32")(
    "reauthorizes the exact source after the overlay CLI returns",
    async () => {
      const path = writeMeeting("meeting.md", VALID_MEETING);
      const fakeMinutes = join(tempDir, "fake-minutes.mjs");
      writeFileSync(
        fakeMinutes,
        `#!/usr/bin/env node\n` +
          `import { writeFileSync } from "node:fs";\n` +
          `writeFileSync(process.argv[3], ${JSON.stringify(RESTRICTED_MEETING)});\n` +
          `process.stdout.write(JSON.stringify({ frontmatter: { speaker_map: [{ speaker_label: "SPEAKER_0", name: "OVERLAY-PRIVATE-CANARY", confidence: "high", source: "manual" }] } }));\n`
      );
      chmodSync(fakeMinutes, 0o700);

      const out = await getMeetingWithOverlays(path, {
        minutesBin: fakeMinutes,
        timeoutMs: 2000,
      });

      expect(out?.restricted_stub).toBe(true);
      expect(out && "speaker_map" in out.frontmatter).toBe(false);
      expect(JSON.stringify(out)).not.toContain("OVERLAY-PRIVATE-CANARY");
      expect(JSON.stringify(out)).not.toContain(
        "Confidential board pricing discussion"
      );
    }
  );
});

// ── Sensitivity enforcement (restricted meetings) ────────────

describe("sensitivity enforcement", () => {
  it("isRestricted reflects the sensitivity frontmatter", () => {
    const restricted = parseFrontmatter(RESTRICTED_MEETING, "r.md");
    const normal = parseFrontmatter(VALID_MEETING, "v.md");
    expect(isRestricted(restricted!)).toBe(true);
    expect(isRestricted(normal!)).toBe(false);
  });

  it("listMeetings excludes restricted meetings by default", async () => {
    writeMeeting("valid.md", VALID_MEETING);
    writeMeeting("restricted.md", RESTRICTED_MEETING);
    const meetings = await listMeetings(tempDir, 10);
    const titles = meetings.map((m) => m.frontmatter.title);
    expect(titles).toContain("Q2 Pricing Discussion");
    expect(titles).not.toContain("Board Pricing Strategy");
  });

  it("does not publish corpus authorization hooks through ReadOptions", () => {
    type PublicOptionsExposeHooks =
      "corpusLeaseHooks" extends keyof ReadOptions ? true : false;
    const exposesHooks: PublicOptionsExposeHooks = false;
    expect(exposesHooks).toBe(false);
  });

  it("fails closed on an explicit unknown sensitivity value", async () => {
    const malformed = VALID_MEETING.replace(
      "duration: 42m",
      "duration: 42m\nsensitivity: confidential"
    );
    const path = writeMeeting("unknown-sensitivity.md", malformed);

    expect(parseFrontmatter(malformed, path)).toBeNull();
    expect(await getMeeting(path)).toBeNull();
    expect(await getMeeting(path, { includeRestricted: true })).toBeNull();
    expect(
      (await listMeetings(tempDir, 10, { includeRestricted: true })).map(
        (meeting) => meeting.path
      )
    ).not.toContain(path);
    expect(
      (await searchMeetings(tempDir, "pricing", 10, {
        includeRestricted: true,
      })).map((meeting) => meeting.path)
    ).not.toContain(path);
  });

  it("fails closed before policy parsing when the sensitivity key is invalid UTF-8", async () => {
    const privateCanary = "INVALID-UTF8-RESTRICTED-CANARY";
    const normalCanary = "INVALID-UTF8-NORMAL-CANARY";
    const invalidPath = join(tempDir, "invalid-utf8.md");
    writeFileSync(
      invalidPath,
      restrictedMeetingWithInvalidUtf8Key(privateCanary)
    );
    writeMeeting(
      "normal.md",
      VALID_MEETING.replace(
        "Alex proposed monthly billing instead of annual.",
        normalCanary
      )
    );

    expect(await getMeeting(invalidPath)).toBeNull();
    expect(await getMeeting(invalidPath, { includeRestricted: true })).toBeNull();

    // One undecodable policy source denies the entire stable multi-source
    // authorization. It must not silently disappear and return the other
    // meeting as a partially authorized corpus.
    const outcomes = await Promise.allSettled([
      listMeetings(tempDir, 10),
      searchMeetings(tempDir, privateCanary, 10),
      listMeetings(tempDir, 10, { includeRestricted: true }),
    ]);
    expect(outcomes.every((outcome) => outcome.status === "rejected")).toBe(
      true
    );
    const serialized = JSON.stringify(outcomes);
    expect(serialized).not.toContain(privateCanary);
    expect(serialized).not.toContain(normalCanary);
    expect(serialized).not.toContain(invalidPath);
  });

  it("listMeetings includes restricted meetings with the explicit override", async () => {
    writeMeeting("valid.md", VALID_MEETING);
    writeMeeting("restricted.md", RESTRICTED_MEETING);
    const meetings = await listMeetings(tempDir, 10, { includeRestricted: true });
    const titles = meetings.map((m) => m.frontmatter.title);
    expect(titles).toContain("Board Pricing Strategy");
  });

  it("searchMeetings does not surface a restricted meeting that matches the query", async () => {
    writeMeeting("valid.md", VALID_MEETING);
    writeMeeting("restricted.md", RESTRICTED_MEETING);
    // Both meetings mention "pricing"; only the non-restricted one is returned.
    const def = await searchMeetings(tempDir, "pricing", 10);
    expect(def.map((m) => m.frontmatter.title)).not.toContain(
      "Board Pricing Strategy"
    );
    const overridden = await searchMeetings(tempDir, "pricing", 10, {
      includeRestricted: true,
    });
    expect(overridden.map((m) => m.frontmatter.title)).toContain(
      "Board Pricing Strategy"
    );
  });

  it("getMeeting returns a minimal stub for a restricted meeting fetched by path", async () => {
    const pathCanary = "RESTRICTED-PARENT-PATH-CANARY";
    const fileCanary = "RESTRICTED-FILENAME-CANARY";
    const privateParent = join(tempDir, pathCanary);
    mkdirSync(privateParent);
    const path = join(privateParent, `${fileCanary}.md`);
    writeFileSync(path, RESTRICTED_MEETING);
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    const stub = await getMeeting(path);
    expect(stub).not.toBeNull();
    expect(stub!.restricted_stub).toBe(true);
    if (!stub || !stub.restricted_stub) {
      throw new Error("restricted exact read must return the public path-free stub type");
    }
    expectTypeOf(stub).toEqualTypeOf<RestrictedMeetingStub>();
    expectTypeOf<Awaited<ReturnType<typeof getMeeting>>>().toEqualTypeOf<
      ExactMeetingResult | null
    >();
    // The stub keeps only title/date/type/sensitivity...
    expect(stub!.frontmatter.title).toBe("Board Pricing Strategy");
    expect(stub!.frontmatter.sensitivity).toBe("restricted");
    expect(stub!.frontmatter.date).not.toBe("");
    // ...and points at the override, never the content.
    expect(stub!.body).toContain("excluded by default");
    expect(stub!.body).toContain("includeRestricted: true");
    expect(stub!.body).not.toContain("Confidential board pricing discussion");
    expect(Object.keys(stub!.frontmatter).sort()).toEqual([
      "date",
      "sensitivity",
      "title",
      "type",
    ]);
    expect(Object.keys(stub!).sort()).toEqual([
      "body",
      "frontmatter",
      "restricted_stub",
    ]);
    const serialized = JSON.stringify(stub);
    expect(serialized).not.toContain(pathCanary);
    expect(serialized).not.toContain(fileCanary);
    expect(serialized).not.toContain(tempDir);
    expect(warning).toHaveBeenCalledTimes(1);
    expect(warning.mock.calls.flat().join(" ")).not.toContain(path);
    warning.mockRestore();
  });

  it("getMeeting returns the full restricted meeting with the explicit override", async () => {
    const path = writeMeeting("restricted.md", RESTRICTED_MEETING);
    const overridden = await getMeeting(path, { includeRestricted: true });
    expect(overridden?.restricted_stub).toBeUndefined();
    expect(overridden?.frontmatter.title).toBe("Board Pricing Strategy");
    if (!overridden || overridden.restricted_stub) {
      throw new Error("explicit override must return a full meeting");
    }
    expect(overridden.frontmatter.action_items.length).toBeGreaterThan(0);
    expect(overridden?.body).toContain("Confidential board pricing discussion");
  });

  it("getMeetingWithOverlays inherits the restricted stub without calling the CLI", async () => {
    const path = writeMeeting("restricted.md", RESTRICTED_MEETING);
    // minutesBin points nowhere, so any CLI attempt would fail; the stub is
    // returned before the overlay path runs at all.
    const out = await getMeetingWithOverlays(path, { minutesBin: "/nonexistent" });
    expect(out?.restricted_stub).toBe(true);
    expect(out?.body).toContain("excluded by default");
  });

  it("findOpenActions skips actions from restricted meetings by default", async () => {
    writeMeeting("valid.md", VALID_MEETING);
    writeMeeting("restricted.md", RESTRICTED_MEETING);
    const def = await findOpenActions(tempDir);
    expect(def.some((a) => a.item.task === "Draft confidential pricing memo")).toBe(
      false
    );
    const overridden = await findOpenActions(tempDir, undefined, {
      includeRestricted: true,
    });
    expect(
      overridden.some((a) => a.item.task === "Draft confidential pricing memo")
    ).toBe(true);
  });

  it("findDecisions skips decisions from restricted meetings by default", async () => {
    writeMeeting("valid.md", VALID_MEETING);
    writeMeeting("restricted.md", RESTRICTED_MEETING);
    const def = await findDecisions(tempDir, "pricing", 50);
    expect(
      def.some((d) => d.decision.text.includes("secret pricing floor"))
    ).toBe(false);
    const overridden = await findDecisions(tempDir, "pricing", 50, {
      includeRestricted: true,
    });
    expect(
      overridden.some((d) => d.decision.text.includes("secret pricing floor"))
    ).toBe(true);
  });

  it("getPersonProfile excludes restricted meetings from a person's history by default", async () => {
    writeMeeting("valid.md", VALID_MEETING);
    writeMeeting("restricted.md", RESTRICTED_MEETING);
    // "Alex K." attends both; the restricted one must not appear by default.
    const def = await getPersonProfile(tempDir, "Alex K.");
    expect(def.meetings.map((m) => m.title)).not.toContain(
      "Board Pricing Strategy"
    );
    const overridden = await getPersonProfile(tempDir, "Alex K.", {
      includeRestricted: true,
    });
    expect(overridden.meetings.map((m) => m.title)).toContain(
      "Board Pricing Strategy"
    );
  });
});
