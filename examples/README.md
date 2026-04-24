# Minutes Reference Adapters

Small working adapters that pipe Minutes markdown output into third-party
agent-memory platforms. Each one demonstrates that the
[Minutes frontmatter contract](../docs/frontmatter-schema.md) interoperates
with the rest of the agent-memory ecosystem at a baseline level. The filesystem
is the integration contract; these adapters are runnable proof sketches, not
production-grade integrations.

## Quickest path

```bash
# 1. Drop a fixture corpus on your disk
npx minutes-mcp --demo

# 2. Run one of the adapters against it (pick one, each in its own shell or subshell)
(cd examples/mem0     && pip install -r requirements.txt && python adapter.py --dry-run)
(cd examples/graphiti && pip install -r requirements.txt && python adapter.py --dry-run)
```

By default every adapter reads from `~/.minutes/demo/`. Point at your real
corpus with `--meetings-dir ~/meetings/`. Adapters also honor the
`MEETINGS_DIR` environment variable when no flag is supplied, so if your
Minutes install uses a custom path you don't have to repeat it.

## Adapters

| Platform | Path | What it demonstrates |
|---|---|---|
| [Mem0](https://mem0.ai) | `mem0/` | Meeting summaries, decisions, and action items landing as Mem0 memories under a single user/agent pair. Useful for drop-in conversational memory for an agent already using Mem0. |
| [Graphiti](https://github.com/getzep/graphiti) | `graphiti/` | Meetings as temporal episodes with structured JSON bodies. Graphiti's LLM extractor infers entities and facts from each episode; the adapter seeds the raw structure but doesn't pre-declare nodes. Useful when you want temporal reasoning over a corpus. |

## What a reference adapter is (and isn't)

- **Is:** small, honest, runnable. Shows the mapping from Minutes frontmatter
  fields to the target platform's primitives. Meant to be forked and adapted.
- **Isn't:** a supported SDK, a shipping product, or a commitment to maintain
  compatibility with future versions of Mem0 or Graphiti. Platform SDKs evolve;
  when they do, treat these adapters as starting points, not libraries.
- **Still missing from v1:** per-attendee identity mapping, duplicate-safe
  manifests, exact dependency pins, and CI dry-runs. Those are tracked as the
  reference-adapter v2 hardening path.

## Contributing a new adapter

Letta, Cognee, LangChain memory, Graphlit, Zep Cloud, and any other agent-memory
platform is fair game. A good adapter:

1. Has a short README explaining the mapping (meeting → what concept on the
   target platform) and the setup (API keys, install, minimum version).
2. Reads primarily from frontmatter. Body-level extraction (a Summary section,
   transcript lines) is fine as a fallback; if you find yourself needing a
   field the schema doesn't expose, open a discussion so we can add it
   cleanly rather than hack around it.
3. Defaults to `~/.minutes/demo/` so anyone with `npx minutes-mcp --demo` can
   run it end-to-end in seconds. Honors `MEETINGS_DIR` as a fallback.
4. Targets small — the shipped first-party adapters are ~160-180 lines. If
   yours is much larger, consider splitting out a library.

Open a PR with a new directory under `examples/`.
