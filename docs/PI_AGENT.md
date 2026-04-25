# Pi agent support

Minutes supports Mario Zechner's `pi` coding agent as an opt-in local agent CLI.

## What is wired

- Desktop settings now recognize `pi` as a well-known `agent_command`.
- `engine = "agent"` can run `pi` for summarization.
- Agent routing evals can be run with `--agent pi`.
- Pi can use the existing `.agents/skills/minutes/` skill mirror; Pi's package manager auto-discovers ancestor `.agents/skills` directories, so a separate `.pi/skills` tree would duplicate the same skill names.

## Summarization config

```toml
[summarization]
engine = "agent"
agent_command = "pi"
agent_args = "--provider openai --model gpt-4o-mini"
```

Minutes runs Pi with:

```bash
pi --no-session --no-tools --no-extensions --no-skills --no-prompt-templates --no-context-files -p @<private-prompt-file>
```

That invocation is intentionally narrow: no saved session, no tool access, no automatic context files, and transcript prompt content passed through a private temp file rather than the command line.

## Inflection Pi boundary

Inflection's Pi is a different thing from the `pi` coding-agent CLI. Inflection-3 Pi is tuned for emotional intelligence and customer-support style chat, so it may be useful later for opt-in tone coaching or reflection features. It should not be a default transcript processor because meeting transcripts often include personal data, and Inflection's developer terms currently tell API users not to send personal information or other regulated data.
