---
name: minutes-graph
description: Cross-meeting entity graph — query who/what/when across all your meetings as structured data, with co-occurrence and cross-entity queries that text search can't answer. Use whenever the user says "show me everyone who mentioned X", "every time we talked about Y", "who knows about Z", "graph", "across all meetings", "entity search", "first time we talked about", "trend for X over time", "who's been mentioned alongside", or wants to query meetings as an index rather than full-text search. Builds a JSON entity index on first run (one-time slow), then answers queries instantly. Surface this skill for relationship intelligence, due diligence, or any "across all my history" question that text search alone can't answer.
user_invocable: true
---

# /minutes-graph

Cross-meeting entity graph that lets you query your meeting history as structured data — **people and topics** out of the box, with companies and products as an opt-in deep-extraction path.

Minutes already exposes `minutes people`, `minutes person`, and `minutes insights` for first-class entity queries. **Graph layers on top of those** to answer questions the CLI can't:

- "What's the co-occurrence between Sarah and pricing?"
- "First time the term 'X' appears in my history"
- "Frequency trend for topic Y over the last 6 months"
- "Who's been mentioned in the same meetings as Sarah?"
- "What topics came up when we talked about hiring?"

Defer to the existing CLI when it suffices. Use graph for the queries the CLI can't answer. Anything about companies or products requires opt-in deep extraction (see Phase 1).

## How it works

Graph has two modes: **build** (creates the index) and **query** (uses it).

### Phase 0: Determine the user's intent

If the user explicitly says "build" / "rebuild" / "refresh the graph" → Phase 1 (build mode).

Otherwise → Phase 2 (query mode). Query mode auto-builds the index if it doesn't exist, and incrementally refreshes it if new meetings exist since the last build.

**Detect company/product queries upfront.** If the user's question mentions a specific company name, product, brand, or anything that wouldn't be in standard meeting frontmatter (e.g., "Stripe", "Notion", "the deal with Acme"), tell them upfront — before any building happens — that this needs deep extraction:

> "That query is about a company/product, which isn't in standard meeting frontmatter. I'd need to deep-scan transcripts (~<estimate> seconds for <N> meetings) to answer. Continue, or rephrase to use topics/people that are already indexed?"

This avoids the worst flow: build → discover the data isn't there → ask user → rebuild.

### Phase 1: Build the index via the bundled helper script

The index lives at `~/.minutes/graph/index.json`. **Use the bundled `graph_build.py` script — do not try to walk meeting files or parse YAML frontmatter in-context.** The script is deterministic, fast, atomic, and handles all the edge cases (incremental rebuilds, garbage filtering, name disambiguation, augmentation from `minutes people --json`).

**Always warn before building**, even on the auto-trigger path. Users hate commands that hang silently more than they hate one-line confirmations:

> "Building entity graph from <N> meetings. The frontmatter-only path is fast (a few seconds for hundreds of meetings). Continue? (Ctrl+C to abort)"

To get the meeting count for the warning, use Glob on the meetings dir from `minutes paths`.

**Run the script:**

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/skills/minutes-graph/scripts/graph_build.py" --incremental
```

Defaults:
- `--meetings-dir` defaults to `output_dir` from `minutes paths`
- `--output` defaults to `~/.minutes/graph/index.json`
- `--incremental` skips the rebuild entirely if no meeting files have been modified since the last build (the common case after the first build)

The script prints a one-line summary JSON to stdout:

```json
{"status": "ok", "meeting_count": 142, "person_count": 12, "topic_count": 38, "output": "/Users/.../index.json"}
```

Or `{"status": "fresh", ...}` if `--incremental` found nothing to rebuild.

**What the script does, in order:**

1. Walks every meeting file in the meetings directory
2. Parses the YAML frontmatter using a small line-based extractor (no PyYAML dep)
3. Extracts: `date`, `attendees` (display names), `people` (wikilink slugs), `tags`, and `decisions[].topic` — all the entity-relevant fields the real meeting schema actually has
4. Filters out `none`/`null`/`~` topic values that show up in some malformed meetings
5. When `attendees` and `people` have the same length, zips them positionally to map each slug to its display name (gives `mat` → `Mat S.`, etc.)
6. Builds people-people, people-topic, and topic-topic co-occurrence within each meeting
7. Runs `minutes people --json` and merges its `top_topics`, `open_commitments`, `score`, `losing_touch` fields into the people entries it already built
8. Filters diarization noise (`unknown-speaker`, `speaker-3`, etc.) out of the final people index
9. Picks a canonical display name for each person using a "looks human" heuristic (capital letter + space wins; lowercase slug-style loses)
10. Atomically writes the index to JSON (temp file + rename)

**What's NOT in the default build**: companies and products. Some real meetings **do** have an `entities:` block in their frontmatter — the current schema looks like:

```yaml
entities:
  people:
    - slug: speaker-0
      label: Speaker 0
      aliases: [speaker 0]
  projects:
    - slug: codex-native-call-attribution
      label: Codex Native Call Attribution
```

But the schema is **inconsistent across the corpus** — many meetings have no `entities:` block, and when it's present its structure varies (some meetings have `people`, some add `projects`, there's no guarantee of `companies` or `products`). `graph_build.py` intentionally uses a narrower set of fields (`attendees`, `people` slugs, `tags`, `decisions[].topic`) that are more consistently populated across the full meeting corpus, so the default build is predictable.

If you want the entities block data in the graph, modify `graph_build.py` to also parse it — but be ready to handle variant schemas across meetings. For on-demand company/product queries that go beyond what frontmatter has, use the opt-in deep extraction path below.

Two paths from here:

1. **Default path (the script above)**: people, topics, dates. Fast, deterministic, runs on every install with zero deps.

2. **Opt-in deep extraction**: only if Phase 0 detected a company/product query and the user confirmed, run a one-time LLM pass over each meeting's transcript section to extract company and product names. Cache results in the index keyed by meeting filename. Every subsequent query reuses the cache. Deep extraction is **not** built into `graph_build.py` — it's a separate workflow Claude orchestrates by reading meetings, calling out to the LLM, and updating the index JSON manually.

**Index structure:**

```json
{
  "version": 1,
  "built_at": "2026-04-08T15:30:00Z",
  "meeting_count": 142,
  "people": {
    "case-wintermute": {
      "name": "Case W.",
      "aliases": ["Case", "Case W."],
      "meetings": ["2026-03-17-q2-pricing.md", "2026-03-22-followup.md"],
      "first_mention": "2025-11-04",
      "last_mention": "2026-03-22",
      "count": 12,
      "top_topics": ["pricing", "onboarding", "hiring"],
      "open_commitments": 3,
      "score": 8.4,
      "losing_touch": false,
      "co_occurs_with": {
        "people": {"sarah-chen": 8, "logan": 3},
        "topics": {"pricing": 6, "hiring": 4}
      }
    }
  },
  "topics": {
    "pricing": {
      "meetings": ["2026-03-17-...", "2026-03-22-..."],
      "first_mention": "2025-11-04",
      "last_mention": "2026-03-22",
      "count": 9,
      "co_occurs_with": {
        "people": {"sarah-chen": 6, "case-wintermute": 4},
        "topics": {"hiring": 3, "onboarding": 2}
      }
    }
  }
}
```

**Co-occurrence** is computed within each meeting: every pair of entities (people-people, people-topics, topics-topics) that appear in the same meeting increments their respective `co_occurs_with` counts. After walking all meetings, the index answers "what co-occurs with X most often?" with a single map lookup — no transcript re-reading.

If the user opted into deep extraction (path 2 above), add `"companies": {...}` and `"products": {...}` blocks at the same level as `people` and `topics`, with the same shape.

Write the index:
```bash
chmod 644 ~/.minutes/graph/index.json
```

The index is metadata — names, dates, counts, slugs. Not transcript text. 644 perms are fine. Users with unusually sensitive entity names can `chmod 600` themselves.

**Tell the user when it's done:**

> "Built graph from <N> meetings: <P> people, <T> topics. Index at `~/.minutes/graph/index.json`. Run any cross-meeting query and I'll answer instantly from the index."

### Phase 2: Query the index

**Index freshness check:**
- If the index doesn't exist, **always warn before building** (see Phase 1) — even on the auto-trigger path. Never build silently. Users hate hanging commands more than they hate one-line confirmations.
- If new meetings exist since `built_at` (any file with `mtime > built_at`), do an **incremental refresh** before answering. Incremental refreshes are fast (<1s for typical updates) and don't need a confirmation prompt.
- The 7-day "stale" rule from earlier drafts is removed — incremental refresh is so fast there's no reason to wait until the index gets stale.

Read `~/.minutes/graph/index.json` once at the start of the query response. Don't re-read the file for every sub-question — the JSON is one self-contained blob, so a single Read gives you everything you need to answer any number of follow-ups in the same turn.

**Common query types and how to answer them:**

| User asks | What to do |
|---|---|
| "Every time we talked about pricing" | Find "pricing" in `topics`. Return all `meetings`, most recent first, with dates from each meeting's frontmatter. |
| "First time we talked about X" | Look up `first_mention` for the entity. Return the date and the meeting file. |
| "Trend for X over the last 6 months" | Walk the entity's `meetings` list, group by month, return a count-by-month ASCII chart. |
| "Who's been mentioned alongside Sarah" | Look up the canonical slug for Sarah (search both `aliases` arrays), then read `co_occurs_with.people`. Return sorted by count. |
| "What topics came up when we talked about hiring" | Find "hiring" in `topics` and read its `co_occurs_with.topics` map directly — that's exactly the data you want, sorted by count. (Co-occurrence is precomputed during Phase 1, so this is a single map lookup, not a reverse scan.) |
| "Show me everyone who mentioned Stripe" | Stripe is a company, not in default frontmatter. Tell the user this needs deep extraction and ask before running it (see Phase 1 path 2). |
| "Most active people in the last 30 days" | **Defer to `minutes people`** — the CLI does this natively and better, with `score` and `losing_touch` flags graph would just be re-deriving. |
| "Who am I losing touch with" | **Defer to `minutes people --json`** — `losing_touch: true` is already a first-class field. Don't reinvent it. |

**Lookup rules:**
- People are keyed by **slug**, not display name. When the user types "Sarah", search the `aliases` array of every person to find a slug match. If multiple slugs match, ask which one.
- Topics are keyed by **lowercase string**. When the user queries "Pricing" or "PRICING", lowercase the query first. Do a substring match against topic keys; if multiple match, surface all of them with counts and let the user pick or accept the merged view.
- Topic merging at query time: case-insensitive substring only. **No stemming.** If "hire" and "hiring" should be merged, the user can ask for "hir" and graph will return both. Mechanical, predictable, no guessing about word forms.

**Output format:**

Always cite source meetings as filenames so the user can drill in. Use markdown tables for ranked results. For trends, use simple ASCII bar charts:

```
2025-10: ███         3
2025-11: ██████      6
2025-12: ████        4
2026-01: ████████    8
2026-02: ███████████ 11  ← peak
2026-03: █████       5
```

### Phase 3: Closing nudge

After answering, suggest the next natural query **if and only if** it's obviously useful:

- After "everyone who mentioned X" → "Want a `/minutes-brief` on any of them?"
- After "trend for X" → "The peak was <month>. Want me to dig into what was happening then?"
- After "first mention" → "Want me to run `minutes research \"<topic>\"` for a deep dive across the meetings?" (`research` is a CLI command, not a slash skill)

Don't always nudge. Only when the next move is genuinely useful — otherwise stay out of the way.

## Gotchas

- **Defer to existing CLI when it suffices.** `minutes people --json` already gives a relationship overview with `losing_touch`, `score`, `top_topics`, `open_commitments` per person. `minutes person <name>` gives a single profile. `minutes insights --participant <name>` (output is JSON by default — do not pass `--json`) gives structured decisions and commitments. If the user's question fits one of those natively, **just call the CLI** — graph is for the queries the CLI can't answer (cross-entity, co-occurrence, "show me X across everything"). Never reinvent.
- **The "fast path" really is fast because it uses real frontmatter fields.** `attendees`, `tags`, `people`, `decisions[].topic`, `action_items[].assignee` all exist in real meeting frontmatter. There is **no** `entities:` block — don't expect one and don't invent one. Companies and products are NOT in frontmatter; they're a separate opt-in deep-extraction path that the user has to consent to before each run.
- **Incremental rebuilds are the common path.** After the first build, only process files where `mtime > built_at`. Don't re-extract everything every time. Most rebuilds complete in <1s.
- **Always warn before building, even on the auto-trigger path.** Users hate commands that hang silently more than they hate one-line confirmations.
- **People are keyed by slug, not display name.** Real meetings have `attendees: [Case W., Mat S.]` (display) and `people: [[case-wintermute], [mat]]` (slugs). Slugs are stable across meetings; display names drift. Use slugs as the primary key, store display names in an `aliases` array.
- **Topic merging is mechanical.** Lowercase substring match at query time. No stemming, no synonym tables, no NLP. If the user wants "hire" and "hiring" merged, they query "hir" and get both. This is predictable; clever fuzzy matching is not.
- **Don't invent entities.** When the user opts into deep extraction, only extract entities the transcript actually references. No web lookups, no synthesis, no "this sounds like it could be a Stripe-adjacent product".
- **The index isn't a database.** It's a JSON file optimized for "load once, query in memory". If it grows past ~10MB, that's a signal to switch to SQLite — but for users under ~1000 meetings, JSON is plenty.
- **Privacy: graph is metadata, not transcripts.** The index contains names, slugs, topics, and counts — not transcript text. 644 perms are fine. Users with unusually sensitive entity names can `chmod 600` themselves.
- **Don't claim certainty about tone or sentiment.** Graph is structural — counts, co-occurrence, dates, trends. It is **not** a sentiment engine. Leave subjective claims about how someone feels to mirror or to the user themselves.
- **`minutes paths` is the source of truth for the meeting directory.** Don't hardcode `~/meetings` — read it from `minutes paths`. Users may sync to Obsidian/Logseq with a custom output directory.
- **When the answer is already in the CLI, say so.** If a user asks "who am I losing touch with?" — that's `minutes people --json` natively (the `losing_touch: true` field is built-in). Run it and surface the result, then let the user know graph wasn't needed. Trust earned.
- **Garbage in the people index is normal.** `minutes people --json` may return entries like "Unknown speaker" or "Speaker_3" or "Matt" (lowercase variant of "Mat") — these come from imperfect speaker diarization. When listing people in graph output, filter out obvious garbage (slugs that look like `unknown-speaker`, `speaker-N`, or duplicate variants of the user's own name).

