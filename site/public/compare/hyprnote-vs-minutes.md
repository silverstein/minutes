# Minutes vs Anarlog (formerly Hyprnote)

Last reviewed: 2026-09-05

Anarlog, formerly Hyprnote, and Minutes both support local meeting workflows and external AI tools. Anarlog centers on an editable notepad backed by SQLite. Minutes centers on conversation records stored as Markdown with source and consent metadata. Choose by the workflow you will actually use.

| Area | Anarlog | Minutes |
|---|---|---|
| Best fit | An editable meeting notepad | A conversation corpus for your existing AI |
| License | MIT community app; commercial enterprise components | MIT |
| Storage | Local SQLite, recordings and attachments; Markdown export | Markdown with YAML frontmatter as the primary record |
| AI processing | Local or hosted transcription and intelligence providers | Local transcription; optional local or cloud AI |
| Agent access | Local CLI, MCP server, and agent skills | MCP, CLI, SDK, and portable agent skills |

## Workflow

Both projects now expose a CLI and MCP. Having MCP alone is not a reason to prefer Minutes. Evaluate setup, retrieval quality, and what your assistant is allowed to read.

In Minutes, try the sample pricing reversal through your own assistant, request source files, then repeat with a real conversation. A sample success does not establish recording reliability.

## Limitations

Minutes may add unnecessary setup if you only want polished notes inside one app.

Check platform-specific capture support before committing to either tool.

## Evidence

Reviewed September 5, 2026 using the official repositories and documentation. This is a maintainer-written comparison, not a hands-on benchmark.

Anarlog is the maintained open-source project formerly called Hyprnote. Char is a separate current product from its team.

## Sources

- [Anarlog repository and licensing](https://github.com/fastrepl/anarlog)
- [Anarlog documentation](https://docs.anarlog.so)
- [Minutes agent workflow](https://useminutes.app/for-agents)
- [Minutes proof and limitations](https://useminutes.app/proof)
