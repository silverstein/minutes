Maintainer follow-up: add `actionlint` as a required status check in branch protection — it cannot join ci_gate.needs (separate workflow).

PR-D completion also requires verifying that the `actionlint` check is required by branch protection:

```bash
test "$(gh api repos/silverstein/minutes/branches/main/protection \
  --jq '[.required_status_checks.contexts[], .required_status_checks.checks[].context] | any(. == "actionlint")')" = true
```

## PR-E baseline report

The initial `design/token-baseline.json` contains **528** distinct, line-agnostic
`(file, property, value)` violations on the current tree.

Top five burn-down candidates by violation count:

| Rank | File | Violations |
| ---: | --- | ---: |
| 1 | `tauri/src/styles/theme-tokens.css` | 179 |
| 2 | `tauri/src/index.html` | 102 |
| 3 | `tauri/src/dictation-overlay.html` | 53 |
| 4 | `tauri/src/copilot-hud.html` | 45 |
| 5 | `tauri/src/note.html` | 40 |

Counts are based on the checker's exact baseline identity, so repeated uses of
the same property/value pair in one file count once. The definition files remain
in the report when a custom-property value is not sanctioned by
`design/tokens.json`; sanctioned definition values themselves are not violations.
