# Minutes Frontmatter Schema

> **This page is the interop contract.** Any tool, agent, or context graph that
> reads meeting files written by Minutes can rely on the schema below. Any tool
> that wants to produce Minutes-compatible output should emit the same shape.

Minutes writes every meeting, voice memo, and dictation session as a plain
markdown file with a YAML frontmatter block at the top. Agents (Claude Code,
Codex, Gemini CLI, Cursor, OpenCode, Mistral Vibe, and any MCP-compatible
client) read these files directly. The filesystem is the API.

- **File location:** `~/meetings/` by default. Configurable via
  `~/.config/minutes/config.toml`.
- **Encoding:** UTF-8, Unix line endings.
- **Permissions:** `0600` (meeting data is sensitive).
- **Naming:** `YYYY-MM-DD-<slug>.md`.

---

## Schema version

The current schema is **v1** with backward-compatible v2 additions. There is
no required `schema-version` field on existing meetings, and one will not be
added without a corresponding major release. Consumers should treat any
unknown field as passthrough and any missing optional field as `None`.

Breaking changes to any field marked **required** below would trigger a
schema version bump. Additive fields do not.

---

## Top-level fields

### Meta (required)

| Field | Type | Required | Notes |
|---|---|---|---|
| `title` | string | ✓ | Human-readable title. LLM-generated during ingest, editable. |
| `type` | enum | ✓ | One of: `meeting`, `memo`, `dictation`. |
| `date` | datetime | ✓ | ISO-8601 with timezone. Example: `2026-03-17T14:00:00-07:00`. |
| `duration` | string | ✓ | Human-friendly, e.g. `42m`, `1h 5m`. |

### Meta (optional)

| Field | Type | Notes |
|---|---|---|
| `source` | string | Provenance tag, e.g. `live-recording`, `voice-memo`, `upload`. |
| `status` | enum | One of: `complete`, `processing`, `failed`. |
| `tags` | list&lt;string&gt; | Keyword tags. No schema; free-form. |
| `device` | string | Audio device label at capture time. |
| `captured_at` | datetime | When capture started, if different from `date`. |
| `context` | string | Optional free-text notes the user added at record time. |
| `recorded_by` | string | User identity slug (matches `people[]` if the user is in the corpus). |
| `visibility` | enum | `private` (default) or `team`. Informational; Minutes does not enforce ACLs. |
| `calendar_event` | string | Calendar event id (Google Calendar, Outlook) when matched. |

### Participants

| Field | Type | Notes |
|---|---|---|
| `attendees` | list&lt;string&gt; | Display names as they appear at the meeting. Preferred field. |
| `people` | list&lt;string&gt; | Canonical slugs (e.g. `alex-kim`, `mat`). Matches `people/` folder entries in a knowledge graph. Zip positionally with `attendees` when both are present and same length. |
| `attendees_raw` | string | Legacy free-text attendee string (e.g. Granola imports). Optional; prefer `attendees`. |
| `entities` | object | Structured entity links. See [Entities](#entities) below. |
| `speaker_map` | list&lt;SpeakerAttribution&gt; | Diarizer speaker label → real person mapping. See [Speaker attribution](#speaker-attribution) below. |

### Structured extraction

These fields are the product. They are what agents query when a user asks
"what did I promise Sarah?" or "what got decided about pricing?".

| Field | Type | Notes |
|---|---|---|
| `action_items` | list&lt;[ActionItem](#actionitem)&gt; | Who owes what by when. |
| `decisions` | list&lt;[Decision](#decision)&gt; | What the group chose, grouped by topic. |
| `intents` | list&lt;[Intent](#intent)&gt; | Union type: action items, decisions, open questions, commitments. Used for richer structured extraction that doesn't fit `action_items`. |

---

## Nested types

### ActionItem

```yaml
action_items:
  - assignee: mat
    task: Send pricing doc
    due: Friday
    status: open
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `assignee` | string | ✓ | Canonical slug or display name. |
| `task` | string | ✓ | Free-text description. |
| `due` | string | | Free-text date ("Friday", "end of quarter", "2026-03-31"). No strict format. |
| `status` | string | ✓ | `open` or `done`. Extensible (`blocked`, `deferred`) but agents should handle unknown values gracefully. |

### Decision

```yaml
decisions:
  - text: Revert to annual-only billing across all segments
    topic: pricing
    authority: high
    supersedes: "2026-02-28 monthly billing decision"
```

| Field | Type | Required | Version | Notes |
|---|---|---|---|---|
| `text` | string | ✓ | v1 | The decision statement. |
| `topic` | string | | v1 | Topic bucket for consistency checks. Two decisions with the same normalized `topic` and different `text` flag as potential conflicts in `/minutes-lint`. |
| `authority` | enum | | **v2** | `high` / `medium` / `low`. Optional hint for weighting. A CEO commitment at a board meeting is high; a drive-by aside is low. Consumers can use this to rank conflicting decisions. |
| `supersedes` | string | | **v2** | Free-text reference to the earlier decision this one replaces. When set, the consistency report treats the topic conflict as a documented supersession rather than an unresolved contradiction. Example: `"2026-02-28 monthly billing decision"`. |

### Intent

Richer structured extraction type. Covers action items, decisions, open
questions, and commitments that don't fit the narrow ActionItem shape.

```yaml
intents:
  - kind: commitment
    what: Ship SSO nested-groups fix
    who: alex
    status: open
    by_date: 2026-04-03
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `kind` | enum | ✓ | `action-item`, `decision`, `open-question`, `commitment`. |
| `what` | string | ✓ | Free-text description. |
| `who` | string | | Assignee slug or name. |
| `status` | string | ✓ | `open`, `done`, or extensible values. |
| `by_date` | string | | Free-text date. |

### Entities

```yaml
entities:
  people:
    - slug: alex-kim
      label: Alex Kim
      aliases: [alex, ak]
  projects:
    - slug: sso-nested-groups
      label: SSO nested-groups support
```

| Field | Type | Notes |
|---|---|---|
| `people` | list&lt;EntityRef&gt; | Person entities with stable slugs for graph lookups. |
| `projects` | list&lt;EntityRef&gt; | Project/initiative entities. |

Each `EntityRef` has `slug` (required), `label` (required), and `aliases`
(optional list of alternate names).

### Speaker attribution

```yaml
speaker_map:
  - speaker_label: SPEAKER_0
    name: mat
    confidence: high
    source: manual
  - speaker_label: SPEAKER_1
    name: alex
    confidence: medium
    source: llm
```

Each entry has four fields:

| Field | Type | Required | Notes |
|---|---|---|---|
| `speaker_label` | string | ✓ | Diarizer-produced label, e.g. `SPEAKER_0`. |
| `name` | string | ✓ | The real person this speaker is. Matches a slug in `people[]` when possible. |
| `confidence` | enum | ✓ | `high` / `medium` / `low`. |
| `source` | enum | ✓ | `deterministic` / `llm` / `enrollment` / `manual`. Tells consumers how the attribution was derived. |

Produced by Minutes' diarization + attribution pipeline. Four confidence
levels (L0-L3):

- **L0** (deterministic): inferred from calendar attendee + user identity with
  no ambiguity. Highest trust.
- **L1** (LLM-suggested): proposed by an LLM from context. Capped at `medium`
  confidence by design, since wrong names are worse than anonymous.
- **L2** (voice-enrolled): matched against a stored voice profile in
  `~/.minutes/voices.db`. High when the match is strong.
- **L3** (user-confirmed): the user explicitly confirmed this attribution.
  Highest trust once set.

Only attributions at `high` confidence rewrite transcript speaker labels.
Lower-confidence attributions stay as `SPEAKER_N` tokens in the transcript
with the best-guess name in `speaker_map`.

---

## Example

A minimal but realistic meeting file:

```markdown
---
title: Q1 Pricing Review
type: meeting
date: 2026-03-17T14:00:00-07:00
duration: 42m
attendees: [Mat S., Alex K., Jordan M.]
people: [mat, alex, jordan]
tags: [pricing, gtm]
action_items:
  - assignee: jordan
    task: Draft pricing landing copy
    due: 2026-03-24
    status: open
decisions:
  - text: Test monthly billing for consultants only
    topic: pricing
    authority: high
speaker_map:
  - speaker_label: SPEAKER_0
    name: mat
    confidence: high
    source: manual
---

## Summary
- Pain: consultants won't commit to annual up front.
- Plan: monthly billing for next three consultant signups.
- Reassess end of Q2.

## Transcript
[SPEAKER_0 0:00] Let's talk pricing...
```

---

## Consuming Minutes

Every file is plain markdown. Read them however you want.

### Python

```python
from pathlib import Path
import yaml

def iter_meetings(root="~/meetings"):
    for p in Path(root).expanduser().rglob("*.md"):
        text = p.read_text()
        if not text.startswith("---\n"):
            continue
        _, fm_str, _ = text.split("---\n", 2)
        yield p, yaml.safe_load(fm_str)

for path, fm in iter_meetings():
    for d in fm.get("decisions", []):
        print(path.name, d["text"])
```

### TypeScript

```ts
import { readdir, readFile } from "node:fs/promises";
import { parse as parseYaml } from "yaml";

async function* iterMeetings(root = `${process.env.HOME}/meetings`) {
  for (const name of await readdir(root, { recursive: true })) {
    if (!name.endsWith(".md")) continue;
    const text = await readFile(`${root}/${name}`, "utf8");
    const m = text.match(/^---\n([\s\S]+?)\n---\n/);
    if (m) yield { name, frontmatter: parseYaml(m[1]) };
  }
}

for await (const { name, frontmatter } of iterMeetings()) {
  for (const d of frontmatter.decisions ?? []) {
    console.log(name, d.text);
  }
}
```

### Rust

Parse with `serde_yaml` (no Minutes-specific crate required):

```rust
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Deserialize)]
struct Frontmatter {
    title: String,
    #[serde(default)]
    decisions: Vec<Decision>,
}

#[derive(Deserialize)]
struct Decision {
    text: String,
    #[serde(default)]
    authority: Option<String>,
}

for entry in walkdir::WalkDir::new(Path::new(&std::env::var("HOME")?).join("meetings")) {
    let entry = entry?;
    if entry.path().extension().map(|e| e != "md").unwrap_or(true) {
        continue;
    }
    let text = fs::read_to_string(entry.path())?;
    let (_, fm, _) = text.splitn(3, "---\n").collect::<Vec<_>>().try_into().ok()
        .and_then(|a: [&str; 3]| Some((a[0], a[1], a[2])))
        .ok_or("missing frontmatter")?;
    let fm: Frontmatter = serde_yaml::from_str(fm)?;
    for d in &fm.decisions {
        println!("{} — {}", fm.title, d.text);
    }
}
```

Minutes itself uses the `minutes-reader` crate internally (pure Rust, no
audio deps). If you're building a tool inside the Minutes workspace, depend
on it directly; otherwise the serde_yaml approach is easier.

### Shell

```bash
# All open action items assigned to you in the last 90 days
grep -rl "assignee: mat" ~/meetings \
  | xargs -I{} awk '/status: open/{print FILENAME": "$0}' {}
```

### Reference adapters for third-party memory platforms

Working code that pipes Minutes output into other agent-memory platforms.
Each adapter reads from `~/.minutes/demo/` (populated by `npx minutes-mcp --demo`)
by default, so you can run them end-to-end in under a minute.

| Target | Location | What it demonstrates |
|---|---|---|
| [Mem0](https://mem0.ai) | [`examples/mem0/`](https://github.com/silverstein/minutes/tree/main/examples/mem0) | Meetings + decisions + action items as Mem0 memories under a single user/agent pair. |
| [Graphiti](https://github.com/getzep/graphiti) | [`examples/graphiti/`](https://github.com/silverstein/minutes/tree/main/examples/graphiti) | Meetings as temporal episodes; Graphiti extracts entities (people, topics) and builds a temporal fact graph. |

Want to add another? See
[`examples/README.md`](https://github.com/silverstein/minutes/tree/main/examples/README.md)
for the contributor contract.

---

## Stability contract

- **Required fields** (`title`, `type`, `date`, `duration`) will not be
  renamed or removed without a schema version bump and a clear migration
  path.
- **Optional fields** may be added over time. Consumers should gracefully
  handle unknown fields.
- **Enum values** (`type`, `kind`, `status`, `authority`, etc.) may gain new
  variants. Consumers should treat unknown variants as passthrough and not
  fail.
- **Deprecated fields** will remain readable for at least two minor versions
  after deprecation is announced in the changelog.

Questions, feature requests, or interop issues belong in
[GitHub Discussions](https://github.com/silverstein/minutes/discussions).

---

_Last updated: 2026-04-20 — corresponds to Minutes v0.13.2 plus frontmatter
v2 additions (`authority`, `supersedes` on `Decision`)._
