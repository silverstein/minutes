/** Explicit identity evidence used by the reader's person-profile queries. */
export interface PersonIdentityFrontmatter {
  attendees?: readonly string[];
  attendees_raw?: string;
  people?: readonly string[];
  action_items?: readonly { assignee: string; status: string }[];
  intents?: readonly { kind: string; who?: string; status: string }[];
  speaker_map?: readonly { name: string; confidence: string }[];
  entities?: {
    people?: readonly { slug?: string; label: string; aliases?: readonly string[] }[];
  };
}

export interface PersonIdentityMeeting {
  frontmatter: PersonIdentityFrontmatter;
}

export interface ResolvedPersonIdentity {
  readonly canonical: string;
}

interface IdentityEvidence extends ResolvedPersonIdentity {
  selectors: Set<string>;
}

export function normalizePersonSelector(value?: string): string {
  return (value ?? "").trim().replace(/\s+/g, " ").toLowerCase();
}

export function personSelectorSlug(value?: string): string {
  return normalizePersonSelector(value)
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

// Labels and declared aliases are identity evidence. A generated ASCII slug is
// lossy (including mixed scripts), so it must never create or join identities.
function selectorKeys(value: string): string[] {
  const normalized = normalizePersonSelector(value);
  return normalized ? [normalized] : [];
}

function matches(identity: IdentityEvidence, value: string): boolean {
  return identity.selectors.has(normalizePersonSelector(value));
}

function parseRawAttendees(raw?: string): string[] {
  return (raw ?? "").split(",").flatMap((token) => {
    const value = token.trim();
    if (!value || value.toLowerCase() === "none") return [];
    const parenthesized = value.match(/^(.*?)\s*\([^)]*\)$/)?.[1];
    const angled = value.match(/^(.*?)\s*<[^>]*>$/)?.[1];
    const display = (parenthesized || angled || value).trim();
    return display ? [display] : [];
  });
}

function participants(frontmatter: PersonIdentityFrontmatter): string[] {
  return [
    ...(frontmatter.attendees ?? []),
    ...parseRawAttendees(frontmatter.attendees_raw),
    ...(frontmatter.speaker_map ?? [])
      .filter((speaker) => speaker.confidence === "high")
      .map((speaker) => speaker.name),
  ];
}

function meetingIdentities(meeting: PersonIdentityMeeting): IdentityEvidence[] {
  const frontmatter = meeting.frontmatter;
  const identities = new Map<string, IdentityEvidence>();
  const add = (canonical: string, names: readonly string[]) => {
    if (!canonical) return;
    const identity = identities.get(canonical) ?? { canonical, selectors: new Set<string>() };
    for (const name of names) {
      for (const key of selectorKeys(name)) identity.selectors.add(key);
    }
    identities.set(canonical, identity);
  };
  const addEntity = (label: string, id?: string, aliases: readonly string[] = []) => {
    if (!normalizePersonSelector(label)) return;
    const canonical = normalizePersonSelector(id)
      || normalizePersonSelector(label);
    add(canonical, [canonical, label, ...aliases]);
  };
  for (const entity of frontmatter.entities?.people ?? []) {
    addEntity(entity.label, entity.slug, entity.aliases);
  }
  const declared = [...identities.values()];
  const names = [
    ...participants(frontmatter),
    ...(frontmatter.people ?? []),
    ...(frontmatter.action_items ?? [])
      .filter((action) => action.status === "open" || action.status === "stale")
      .map((action) => action.assignee),
    ...(frontmatter.intents ?? [])
      .filter((intent) =>
        (intent.kind === "action-item" || intent.kind === "commitment")
        && (intent.status === "open" || intent.status === "stale"))
      .flatMap((intent) => intent.who ? [intent.who] : []),
  ];
  for (const name of names) {
    const normalized = normalizePersonSelector(name);
    if (!normalized || declared.some((identity) => matches(identity, name))) continue;
    add(normalized, [name]);
  }
  return [...identities.values()];
}

/**
 * Resolve against the entire authorized corpus before applying result limits.
 * A shared alias never silently chooses one of several canonical identities.
 * Transcript prose is deliberately not identity evidence.
 */
export function resolvePersonIdentity(
  meetings: readonly PersonIdentityMeeting[],
  selector: string,
): ResolvedPersonIdentity | undefined {
  if (!normalizePersonSelector(selector)) throw new Error("person selector is empty");
  let resolved: ResolvedPersonIdentity | undefined;
  for (const meeting of meetings) {
    for (const identity of meetingIdentities(meeting)) {
      if (!matches(identity, selector)) continue;
      if (resolved && resolved.canonical !== identity.canonical) {
        throw new Error("person selector is ambiguous across policy-authorized meetings");
      }
      resolved = { canonical: identity.canonical };
    }
  }
  return resolved;
}

export function meetingMentionsPerson(
  meeting: PersonIdentityMeeting,
  identity: ResolvedPersonIdentity,
): boolean {
  return meetingIdentities(meeting).some((candidate) => candidate.canonical === identity.canonical);
}

function nameUniquelyMatches(
  identities: readonly IdentityEvidence[],
  name: string,
  identity: ResolvedPersonIdentity,
): boolean {
  const candidates = identities.filter((candidate) => matches(candidate, name));
  return candidates.length === 1 && candidates[0].canonical === identity.canonical;
}

/** Mentioned people and action owners do not imply meeting participation. */
export function meetingHasPersonParticipant(
  meeting: PersonIdentityMeeting,
  identity: ResolvedPersonIdentity,
): boolean {
  const identities = meetingIdentities(meeting);
  return participants(meeting.frontmatter).some((name) => nameUniquelyMatches(identities, name, identity));
}

/** Only a locally unambiguous, exact owner name or declared alias can match. */
export function personOwnsAction(
  meeting: PersonIdentityMeeting,
  assignee: string,
  identity: ResolvedPersonIdentity,
): boolean {
  return nameUniquelyMatches(meetingIdentities(meeting), assignee, identity);
}
