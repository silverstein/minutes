# Pi agent support

Minutes supports Mario Zechner's `pi` coding agent as an opt-in local agent CLI.

## What is wired

- Desktop settings now recognize `pi` as a well-known `agent_command`.
- Desktop Recall can launch `assistant.agent = "pi"` as an interactive Pi session.
- `engine = "agent"` can run `pi` for summarization.
- Agent routing evals can be run with `--agent pi`.
- Pi can use the existing `.agents/skills/minutes/` skill mirror; Pi's package manager auto-discovers ancestor `.agents/skills` directories, so a separate `.pi/skills` tree would duplicate the same skill names.

## Recall assistant config

```toml
[assistant]
agent = "pi"
agent_args = []
```

For the interactive Recall panel, Minutes launches `pi` in the assistant
workspace and passes `agent_args` through unchanged, after dropping only
approval-bypass flags that belong to other agents.

Pi owns provider auth and model selection. Use Pi's interactive `/login` and
`/model` flows, or explicit Pi flags such as `--model <provider/model>`, to
choose the model. If a GitHub Copilot model such as `github-copilot/gpt-5.5`
fails with "Personal Access Tokens are not supported for this endpoint", refresh
Pi's GitHub Copilot login with `/login`; do not paste a GitHub PAT into Minutes.

## Summarization config

```toml
[summarization]
engine = "agent"
agent_command = "pi"
```

Minutes runs Pi with:

```bash
pi --no-session --no-tools --no-extensions --no-skills --no-prompt-templates --no-context-files -p @<private-prompt-file>
```

Configure provider/model defaults in Pi itself; Minutes does not currently pass
extra `[summarization]` CLI flags through to the pipeline invocation.

That invocation is intentionally narrow: no saved session, no tool access, no automatic context files, and transcript prompt content passed through a private temp file rather than the command line.

## Inflection Pi boundary

Inflection's Pi is a different thing from the `pi` coding-agent CLI. Inflection-3 Pi is tuned for emotional intelligence and customer-support style chat, so it may be useful later for opt-in tone coaching or reflection features. It should not be a default transcript processor because meeting transcripts often include personal data, and Inflection's developer terms currently tell API users not to send personal information or other regulated data.
