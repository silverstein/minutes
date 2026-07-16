---
name: minutes-release-notes
description: Draft user-facing Minutes release notes for a version from the commit range, recent GitHub releases, and the repository release checks. Use when the user asks to write, generate, prepare, revise, or review release notes or a changelog for a Minutes version.
user_invocable: true
---

# /minutes-release-notes

Draft release notes that explain why a Minutes release matters without making users decode the commit history. Produce a draft only. Do not create, edit, or publish a GitHub release unless the user explicitly asks.

## Inputs

Collect:

- the new version, such as `v0.22.0`
- the previous stable tag, or an explicit starting ref
- the target ref, normally `HEAD`
- the release channel, normally stable or preview
- any known breaking changes, migrations, compatibility notes, or contributor credits

Resolve missing refs from the repository instead of guessing:

```bash
git describe --tags --abbrev=0
git log <previous-tag>..HEAD --pretty='%s (%h)'
```

When `HEAD` is already tagged, resolve the previous tag from its parent so the range does not collapse to zero commits:

```bash
git describe --tags --abbrev=0 HEAD^
```

Confirm both refs with `git rev-parse --verify` before drafting. If the version or range is ambiguous, ask one short question.

## Steps

### 1. Learn the current release voice

Read two or three recent stable releases before writing:

```bash
gh release list --limit 5
gh release view <recent-tag>
```

Match the current heading hierarchy, amount of detail, install wording, contributor treatment, and overall tone. Prefer concrete user outcomes, honest limitations, and short technical explanations. Do not copy stale version-specific claims.

Never use em dashes in the draft. Rewrite with commas, colons, parentheses, or separate sentences. This is a repository release convention, not an optional style preference.

### 2. Build the change ledger

Read the full range and inspect touched files when a subject is unclear:

```bash
git log <previous-tag>..HEAD --pretty='%s (%h)'
git diff --stat <previous-tag>..HEAD
git show --stat <commit>
```

Classify every relevant commit by conventional prefix:

- `feat` -> **Features**
- `fix` -> **Fixes**
- `perf` -> **Performance**
- `docs`, `chore`, `build`, `ci`, `test`, and uncategorized maintenance -> **Docs / Chore rollup**

Treat merge commits and follow-up fixes as part of the user-visible change they complete. Deduplicate stacked or backported commits. Use file inspection to verify the affected surface: desktop, CLI, MCP and agent integrations, site, plugin, SDK, or shared engine.

Drop internal-only version bumps, lockfile syncs, generated-file refreshes, formatting, test-only changes, CI churn, and release bookkeeping unless they change installation, compatibility, reliability, security, or another user-visible behavior. Do not inflate the notes with one bullet per commit.

### 3. Turn the ledger into release prose

Lead with one or two sentences that state why the release exists. Convert the strongest Features and Performance items into outcome-led headline sections. Roll smaller items into **Fixes**. Include Docs / Chore items only when users must act on them or will notice the result.

For each claim:

- say what changed and who benefits
- distinguish defaults from opt-in or experimental behavior
- name affected platforms when behavior differs
- preserve important limitations and fallback behavior
- link issue or pull request numbers when the history supports them
- credit external contributors by verified GitHub handle

Do not infer a breaking change, migration, benchmark, security property, compatibility promise, or contributor from the subject line alone. Verify it in the diff, release procedure, or repository documentation.

### 4. Check release integrity

Run the repository version check after the release version has been applied:

```bash
node scripts/check_version_sync.mjs --release
```

If the check fails because the requested version has not been bumped yet, report that clearly. Do not change versions as part of drafting notes unless the user asked for release preparation too.

Search the range for configuration, storage, schema, feature-default, platform-support, and install-path changes. State either the required migration or that no migration is required. Never leave breaking-change status implicit.

## Output format

Follow the closest of the recent releases inspected in step 1. Use this current Minutes layout when those releases do not establish a more specific pattern:

```markdown
## Minutes vX.Y.Z

<One short paragraph explaining why this release matters.>

### <Feature or outcome headline>
<User-facing explanation, with bullets only when they improve scanning.>

### <Additional feature or performance headline, if needed>
<User-facing explanation.>

### Fixes
- <Grouped, concrete fix>

### Notes
<Breaking change, migration, compatibility, preview, or known-issue note. Say "No migration is required" when that is the verified result.>

## Install / update

The desktop app updates itself: open Minutes and it pulls vX.Y.Z on next launch, or grab the DMG from the assets below.

- **DMG**: download from the release assets below
- **CLI**: `brew install silverstein/tap/minutes` or `cargo install minutes-cli`
- **MCP**: `npx minutes-mcp` (or update the Claude Desktop extension)

## Claude Code plugin
<Include only when the plugin changed. Use the refresh commands and wording from a recent release.>

---
<Use the current release-preparation credit and contributor block only when recent releases include them and the credits are verified.>
```

Keep the install block wording and order exact unless the repository's current distribution paths changed. Omit empty feature sections, but never omit the install block or breaking-change and migration status.

## Checklist

Before returning the draft, verify:

- [ ] The previous tag, target ref, and new version are explicit.
- [ ] Every user-facing claim is supported by the range or repository documentation.
- [ ] Features, Fixes, Performance, and Docs / Chore commits were classified before rollup.
- [ ] Internal-only version bumps, lockfiles, generated syncs, and CI churn were dropped.
- [ ] The prose matches two or three recent releases and contains no em dash characters.
- [ ] Breaking changes, migrations, compatibility notes, preview status, and known issues are explicit.
- [ ] `node scripts/check_version_sync.mjs --release` passes, or its version-bump blocker is reported.
- [ ] The standard DMG, CLI, and MCP download block is present and current.
- [ ] Plugin update instructions appear only if the plugin changed.
- [ ] Contributor handles and issue or pull request references are verified.


