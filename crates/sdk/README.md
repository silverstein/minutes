# minutes-sdk

Conversation memory for AI agents. Query meeting transcripts, decisions, action items, and people from any AI agent or application.

The "Mem0 for human conversations." Works with [Minutes](https://github.com/silverstein/minutes) meeting files or any markdown with YAML frontmatter.

## Install

```bash
npm install minutes-sdk
```

## Quick start

```typescript
import { listMeetings, searchMeetings, findOpenActions } from 'minutes-sdk';

// List recent meetings
const meetings = await listMeetings('~/meetings');
// → [{ frontmatter: { title, date, action_items, decisions, ... }, body, path }]

// Search policy-authorized meetings within the supported corpus bounds
const results = await searchMeetings('~/meetings', 'pricing strategy');

// Find open action items
const actions = await findOpenActions('~/meetings', 'alex');
// → [{ path: '...', item: { assignee: 'alex', task: '...', status: 'open' } }]
```

## Slow storage

Set `MINUTES_CORPUS_STORAGE_PROFILE=slow` in the SDK process environment to allow up to 240 seconds for corpus verification on seek-heavy storage. The default `standard` profile allows 60 seconds. File and memory limits, full verification, and worker termination remain enforced. Calls return as soon as verification and the operation finish; the setting does not impose a four-minute wait. Other profile values are rejected.

For MCP clients, allow at least 300 seconds per request if the host supports a configurable timeout. The server cannot extend the client's timeout, and a client timeout does not guarantee server cancellation. This setting does not affect native CLI search. See [configuration](https://github.com/silverstein/minutes/blob/main/docs/configuration.md#meeting-libraries-on-slow-storage) for examples and limitations. The numeric `MINUTES_CORPUS_AUTH_TIMEOUT_MS` variable is still test-harness-only.

## API

### `listMeetings(dir, limit?, options?)`

List meetings sorted by date (newest first). `limit` must be an integer from 1
through `SDK_MEETING_RESULT_MAX` (10,000). Restricted meetings are excluded by
default; pass `{ includeRestricted: true }` only for an explicit override.

```typescript
const meetings = await listMeetings('~/meetings', 50);
```

### `searchMeetings(dir, query, limit?, options?)`

Full-text search across titles and transcripts. It uses the same validated
1–10,000 meeting-result limit and restricted-meeting policy as `listMeetings`.

```typescript
const results = await searchMeetings('~/meetings', 'Q2 roadmap');
```

### `getMeeting(path, options?)`

Read one meeting inside `options.rootDir` (the configured meetings directory by
default). The result is `Promise<ExactMeetingResult | null>`: missing, unsafe,
or out-of-root paths return `null`. A restricted meeting returns a path-free
`restricted_stub` by default; `{ includeRestricted: true }` is the explicit,
logged override.

```typescript
const meeting = await getMeeting('~/meetings/2026-03-24-planning.md');
if (!meeting) {
  console.log('Meeting unavailable');
} else if (meeting.restricted_stub) {
  console.log(meeting.body); // exclusion notice; never the transcript
} else {
  console.log(meeting.frontmatter.decisions);
}
```

### `findOpenActions(dir, assignee?, options?)`

Find open action items, optionally filtered by assignee. Set `options.limit` to
an integer from 1 through `SDK_OPEN_ACTION_RESULT_MAX` (1,000); omitted limits
default to that hard cap. Restricted meetings are excluded unless
`options.includeRestricted` is explicitly set.

```typescript
const allOpen = await findOpenActions('~/meetings');
const mine = await findOpenActions('~/meetings', 'mat');
const firstTen = await findOpenActions('~/meetings', undefined, { limit: 10 });
```

### `getPersonProfile(dir, name, options?)`

Build a profile for someone across policy-authorized meetings within the supported corpus bounds — their meetings, open action items, and topics.
The three returned collections are independently bounded. Use `meetingLimit`,
`openActionLimit`, and `topicLimit`; each defaults to and may not exceed its
exported 1,000-item cap. `includeRestricted` defaults to false.

```typescript
const profile = await getPersonProfile('~/meetings', 'alex');
// → { name, meetings: [...], openActions: [...], topics: ['pricing', 'api'] }
```

## Availability and resource bounds

Multi-meeting reads are fail-closed snapshots, not partial best-effort scans.
They require local filesystem watcher support and successful sentinel delivery,
then reverify the canonical root and complete active-Markdown manifest before
returning. Watcher errors, missing events, unstable roots, or an unsupported
network/FUSE-style filesystem deny the whole call after bounded retries.

Each Markdown file is limited to 16 MiB and the total decoded corpus to 80 MiB.
Directory, entry, watcher, helper-process, and result counts are also bounded.
Exceeding any bound denies the whole multi-meeting operation; it never silently
returns a partial corpus. Every call performs a full baseline snapshot and a
final manifest verification. The helper-process pool is constant-bounded, but
the retained snapshot uses O(corpus bytes) memory within the hard corpus limit.

### `listVoiceMemos(dir, options?)`

List recent memos newest-first. `options.limit` is bounded to 1–1,000 and
`options.days` to 0–36,500. Restricted memos are excluded unless
`options.includeRestricted` is explicitly set.

### `findDecisions(dir, topic?, limit?, options?)`

List decisions newest-first. `limit` defaults to 50 and must be an integer from
1 through `SDK_DECISION_RESULT_MAX` (1,000). Restricted meetings are excluded
unless `options.includeRestricted` is explicitly set.

### Restricted-read options

Every agent-facing collection read excludes `sensitivity: restricted` sources
by default. Passing `{ includeRestricted: true }` is an explicit override and
writes a warning naming the surfaced count and read surface to stderr. For
exact-path `getMeeting` reads, `options.rootDir` selects the authoritative
active-corpus root; paths outside that root still return `null`.

### `parseFrontmatter(content, path)`

Parse a markdown string into a `MeetingFile`. Useful for custom integrations.

```typescript
import { parseFrontmatter } from 'minutes-sdk';

const meeting = parseFrontmatter(markdownString, '/path/to/file.md');
```

## Use with AI frameworks

### Vercel AI SDK tool

```typescript
import { tool } from 'ai';
import { z } from 'zod';
import { searchMeetings } from 'minutes-sdk';

const meetingSearch = tool({
  description: 'Search past meeting transcripts and decisions',
  parameters: z.object({ query: z.string() }),
  execute: async ({ query }) => {
    const results = await searchMeetings('~/meetings', query, 5);
    return results.map(m => ({
      title: m.frontmatter.title,
      date: m.frontmatter.date,
      decisions: m.frontmatter.decisions,
      actions: m.frontmatter.action_items,
    }));
  },
});
```

### LangChain tool

```typescript
import { DynamicTool } from '@langchain/core/tools';
import { searchMeetings } from 'minutes-sdk';

const meetingTool = new DynamicTool({
  name: 'search_meetings',
  description: 'Search meeting transcripts for decisions and context',
  func: async (query) => {
    const results = await searchMeetings('~/meetings', query, 5);
    return JSON.stringify(results.map(m => ({
      title: m.frontmatter.title,
      date: m.frontmatter.date,
      summary: m.body.slice(0, 500),
    })));
  },
});
```

## Types

```typescript
interface MeetingFile {
  frontmatter: Frontmatter;
  body: string;      // Full markdown body (transcript, summary, notes)
  path: string;      // Absolute file path
}

interface Frontmatter {
  title: string;
  type: string;      // "meeting" | "memo" | "dictation"
  date: string;      // ISO 8601
  duration: string;
  source?: string;   // "voice-memos" | "dictation" | undefined
  device?: string;   // "iPhone" etc (cross-device pipeline)
  tags: string[];
  attendees: string[];
  people: string[];
  action_items: ActionItem[];
  decisions: Decision[];
  intents: Intent[];  // Structured commitments, questions, decisions
}

interface ActionItem {
  assignee: string;
  task: string;
  due?: string;
  status: string;    // "open" | "done"
}

interface Decision {
  text: string;
  topic?: string;
}

interface Intent {
  kind: string;      // "commitment" | "decision" | "open-question"
  what: string;
  who?: string;
  status: string;
  by_date?: string;
}
```

## How it works

The SDK reads markdown files with YAML frontmatter produced by [Minutes](https://github.com/silverstein/minutes). No database, no server, no API key — just files on disk.

```
~/meetings/
├── 2026-03-24-q2-planning.md          ← meetings
├── 2026-03-24-client-call.md
└── memos/
    ├── 2026-03-24-pricing-idea.md     ← voice memos
    └── 2026-03-23-onboarding-thought.md
```

Each file has structured YAML frontmatter (title, date, attendees, action items, decisions, intents) and a markdown body (transcript, summary, notes). The SDK parses these and provides query functions.

## License

MIT
