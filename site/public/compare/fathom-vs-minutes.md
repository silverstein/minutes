# Minutes vs Fathom

Last reviewed: 2026-10-07

Fathom offers hosted meeting recordings, summaries, and team integrations. Minutes keeps a local conversation archive for your existing assistant. Both expose MCP; the useful distinction is the workflow and storage behind that connection.

## Quick verdict

- Choose **Fathom** if you want hosted meeting notes, shared recordings, or CRM workflows.
- Choose **Minutes** if you want inspectable local records and a file-based archive your preferred assistant can search.

## At a glance

| Topic | Fathom | Minutes |
| --- | --- | --- |
| Workflow | Hosted meeting assistant with recordings and team integrations | Local conversation records across meetings, memos, and dictation |
| Capture | Meeting bot or bot-free Mac capture in beta | Device capture without a meeting bot; support depends on platform |
| Record storage | Hosted recordings and notes; retained until deleted | Markdown and YAML on your disk |
| AI processing | Hosted summaries and assistance | Local transcription; optional local or cloud AI |
| Agent access | Public API and first-party MCP server | MCP, CLI, SDK, and portable skills over authorized local records |
| Pricing | Free recording and transcription; paid individual and team plans | Free MIT software; optional provider usage is separate |

## Where Fathom helps

Fathom offers unlimited recording and transcription on its free tier.

Its paid plans add team and CRM workflows around shared meeting records.

## Where Minutes helps

Minutes writes the primary conversation record as Markdown with YAML metadata.

Its open-source desktop, CLI, SDK, and MCP surfaces work over the same authorized corpus. The desktop app supports macOS and Windows; Linux has a CLI.

## Workflow and data boundaries

Fathom can provide hosted meeting context through its API and MCP. Minutes exposes local files through its own tools. Test the retrieval and permission model you will actually use rather than treating MCP availability as a quality benchmark.

Minutes captures, transcribes, and stores conversation records locally. Engine availability depends on your platform, build, and installed models. If you choose a cloud summarizer or connect a cloud assistant, authorized meeting context can reach that provider. File sync and backups you configure are separate data boundaries.

## How to choose

Choose the workflow you will use every day. Try one conversation, inspect the saved record, and ask your assistant to retrieve a specific decision with its source.

Check provider settings, retention, sync, and assistant permissions separately. Local transcription alone does not establish where every later step runs.

## When Minutes may not fit

Minutes may require more setup if you want a managed team workspace, centralized administration, or ready-made CRM integrations.

A local archive does not establish regulatory compliance. Assess your configuration and obligations before recording sensitive conversations.

## How this was evaluated

Official product, pricing, security, and developer documentation was reviewed on October 7, 2026. This is a maintainer-written comparison, not a hands-on accuracy or reliability benchmark.

The Minutes side reflects its published documentation and source. Optional cloud AI and user-configured sync are separate from local capture and transcription.

## Sources

- [Fathom plans and capture](https://www.fathom.ai/pricing)
- [Fathom MCP](https://developers.fathom.ai/mcp-docs)
- [Fathom recording storage](https://help.fathom.video/en/articles/296448)
- [Minutes security and data flow](https://useminutes.app/security)
- [Minutes proof and limitations](https://useminutes.app/proof)
