Maintainer follow-up: add `actionlint` as a required status check in branch protection — it cannot join ci_gate.needs (separate workflow).

PR-D completion also requires verifying that the `actionlint` check is required by branch protection:

```bash
test "$(gh api repos/silverstein/minutes/branches/main/protection \
  --jq '[.required_status_checks.contexts[], .required_status_checks.checks[].context] | any(. == "actionlint")')" = true
```
