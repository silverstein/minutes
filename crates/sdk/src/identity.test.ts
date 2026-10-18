import { describe, expect, it } from "vitest";
import {
  meetingHasPersonParticipant,
  meetingMentionsPerson,
  personOwnsAction,
  resolvePersonIdentity,
  type PersonIdentityFrontmatter,
} from "./identity.js";

const meeting = (frontmatter: PersonIdentityFrontmatter = {}, body = "") => ({ frontmatter, body });

describe("exact person identity", () => {
  it("does not match Ann inside Joanna, planning, or transcript prose", () => {
    const source = meeting({
      attendees: ["Joanna"],
      action_items: [{ assignee: "Joanna", status: "open" }],
    }, "Ann explained the planning process.");
    expect(resolvePersonIdentity([source], "Ann")).toBeUndefined();
    expect(resolvePersonIdentity([source], "planning")).toBeUndefined();
    const annSource = meeting({ attendees: ["Ann"] });
    const ann = resolvePersonIdentity([source, annSource], " Ann ")!;
    expect(meetingHasPersonParticipant(source, ann)).toBe(false);
    expect(personOwnsAction(source, "Joanna", ann)).toBe(false);
    expect(meetingHasPersonParticipant(annSource, ann)).toBe(true);
  });

  it("rejects an empty selector even for an empty corpus", () => {
    for (const selector of ["", " ", "\n\t"]) {
      expect(() => resolvePersonIdentity([], selector)).toThrow("person selector is empty");
    }
  });

  it("resolves exact names, normalized whitespace, slugs and declared aliases", () => {
    const source = meeting({
      entities: { people: [{ slug: "anne-smith", label: "Anne Smith", aliases: ["Annie"] }] },
      attendees: ["Annie"],
      action_items: [{ assignee: "Anne Smith", status: "open" }],
    });
    for (const selector of ["Anne Smith", " ANNE   SMITH ", "anne-smith", "Annie"]) {
      const identity = resolvePersonIdentity([source], selector)!;
      expect(identity.canonical).toBe("anne-smith");
      expect(meetingHasPersonParticipant(source, identity)).toBe(true);
      expect(personOwnsAction(source, "Anne Smith", identity)).toBe(true);
      expect(personOwnsAction(source, "Ann", identity)).toBe(false);
    }
  });

  it("rejects shared aliases across the entire corpus regardless of ordering", () => {
    const anne = meeting({ entities: { people: [{ slug: "anne-smith", label: "Anne Smith", aliases: ["Ann"] }] } });
    const ann = meeting({ entities: { people: [{ slug: "ann-jones", label: "Ann Jones", aliases: ["Ann"] }] } });
    expect(() => resolvePersonIdentity([anne, ann], "Ann")).toThrow("ambiguous");
    expect(() => resolvePersonIdentity([ann, anne], "Ann")).toThrow("ambiguous");
    expect(resolvePersonIdentity([anne, ann], "Anne Smith")?.canonical).toBe("anne-smith");
  });

  it("does not assign ambiguous participant or owner aliases to a canonical person", () => {
    const source = meeting({
      entities: { people: [
        { slug: "alex-chen", label: "Alex Chen", aliases: ["Alex"] },
        { slug: "alex-jones", label: "Alex Jones", aliases: ["Alex"] },
      ] },
      attendees: ["Alex"],
      action_items: [{ assignee: "Alex", status: "open" }],
    });
    expect(() => resolvePersonIdentity([source], "Alex")).toThrow("ambiguous");
    const chen = resolvePersonIdentity([source], "alex-chen")!;
    expect(meetingMentionsPerson(source, chen)).toBe(true);
    expect(meetingHasPersonParticipant(source, chen)).toBe(false);
    expect(personOwnsAction(source, "Alex", chen)).toBe(false);
  });

  it("keeps mentioned people and open action owners separate from participants", () => {
    const source = meeting({
      people: ["Ann"],
      action_items: [{ assignee: "Ann", status: "open" }],
    });
    const ann = resolvePersonIdentity([source], "Ann")!;
    expect(meetingMentionsPerson(source, ann)).toBe(true);
    expect(meetingHasPersonParticipant(source, ann)).toBe(false);
    expect(personOwnsAction(source, "Ann", ann)).toBe(true);
  });

  it("recognizes open and stale intent owners, excluding closed commitments", () => {
    const source = meeting({ intents: [
      { kind: "commitment", who: "Ann", status: "open" },
      { kind: "action-item", who: "Anne", status: "stale" },
      { kind: "commitment", who: "Joanna", status: "closed" },
      { kind: "decision", who: "Alice", status: "open" },
    ] });
    expect(resolvePersonIdentity([source], "Ann")).toBeDefined();
    expect(resolvePersonIdentity([source], "Anne")).toBeDefined();
    expect(resolvePersonIdentity([source], "Joanna")).toBeUndefined();
    expect(resolvePersonIdentity([source], "Alice")).toBeUndefined();
  });

  it("only treats high-confidence speaker mappings as participation evidence", () => {
    const source = meeting({ speaker_map: [
      { name: "Ann", confidence: "high" },
      { name: "Joanna", confidence: "low" },
      { name: "Anne", confidence: "medium" },
    ] });
    const ann = resolvePersonIdentity([source], "Ann")!;
    expect(meetingHasPersonParticipant(source, ann)).toBe(true);
    expect(resolvePersonIdentity([source], "Joanna")).toBeUndefined();
    expect(resolvePersonIdentity([source], "Anne")).toBeUndefined();
  });

  it("parses attendee display names without treating emails or roles as names", () => {
    const source = meeting({ attendees_raw: "Ann <ann@example.com>, Joanna (Designer), none" });
    expect(meetingHasPersonParticipant(source, resolvePersonIdentity([source], "Ann")!)).toBe(true);
    expect(resolvePersonIdentity([source], "ann@example.com")).toBeUndefined();
    expect(resolvePersonIdentity([source], "Designer")).toBeUndefined();
    expect(resolvePersonIdentity([source], "none")).toBeUndefined();
  });

  it("preserves non-Latin exact identity and does not match empty slugs", () => {
    const source = meeting({ attendees: ["李雷", "韓梅"] });
    const identity = resolvePersonIdentity([source], "李雷")!;
    expect(identity.canonical).toBe("李雷");
    expect(personOwnsAction(source, "韓梅", identity)).toBe(false);
    expect(resolvePersonIdentity([source], "未知")).toBeUndefined();
  });
  it("keeps distinct mixed-script labels separate within and across meetings", () => {
    const first = meeting({ attendees: ["李明 Chen"] });
    const second = meeting({ attendees: ["王伟 Chen"],
      action_items: [{ assignee: "王伟 Chen", status: "open" }] });
    const person = resolvePersonIdentity([first, second], "李明 Chen")!;
    expect(person.canonical).toBe("李明 chen");
    expect(meetingMentionsPerson(second, person)).toBe(false);
    expect(personOwnsAction(second, "王伟 Chen", person)).toBe(false);
    expect(resolvePersonIdentity([first, second], "chen")).toBeUndefined();
    const both = meeting({ attendees: ["李明 Chen", "王伟 Chen"] });
    expect(personOwnsAction(both, "王伟 Chen", resolvePersonIdentity([both], "李明 Chen")!)).toBe(false);
  });

  it("does not merge labels whose generated ASCII slugs collide", () => {
    const source = meeting({ attendees: ["A-B", "A B", "José", "Jose"] });
    const hyphenated = resolvePersonIdentity([source], "A-B")!;
    expect(personOwnsAction(source, "A B", hyphenated)).toBe(false);
    const accented = resolvePersonIdentity([source], "José")!;
    expect(personOwnsAction(source, "Jose", accented)).toBe(false);
  });

});
