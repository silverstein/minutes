# Release Channels and Rollback Policy

This document defines how Minutes ships releases across desktop, CLI, and MCP
surfaces.

The goal is not maximum speed. The goal is trustworthy releases that users can
reason about.

## Channel definitions

Minutes currently has two public channels:

### Stable

Use stable for releases intended for broad usage.

- Tag format: `vX.Y.Z`
- Examples:
  - `v0.2.0`
  - `v1.0.3`
- Expectations:
  - signed + notarized macOS desktop artifact
  - Windows desktop artifact may also be attached; if it is unsigned or otherwise experimental, say so explicitly in the release notes
  - no known data-loss or install-blocking issues
  - release notes written for humans, not just commit history

### Preview

Use preview for early access and validation.

- Tag format: `vX.Y.Z-alpha.N`, `vX.Y.Z-beta.N`, or `vX.Y.Z-rc.N`
- Examples:
  - `v0.2.0-alpha.1`
  - `v0.2.0-beta.2`
  - `v0.2.0-rc.1`
- Expectations:
  - signed + notarized macOS desktop artifact still required
  - Windows desktop artifact may be attached for validation even before installer/signing polish is complete
  - may contain rough edges or incomplete distribution polish
  - release notes must clearly say what is experimental

Preview is opt-in by interpretation. Users should not have to guess whether a
release is experimental.

## GitHub release behavior

- Tags that contain a prerelease suffix (`-alpha`, `-beta`, `-rc`) are treated
  as preview releases.
- Plain `vX.Y.Z` tags are treated as stable releases.
- Minutes does not reuse or move release tags after publication.

If a release is wrong, cut a new tag. Do not silently replace an old one.

## Changelog quality bar

Every public release note must include these sections:

1. `What changed`
2. `Who should care`
3. `CLI / MCP / desktop impact`
4. `Breaking changes or migration notes`
5. `Known issues`

The release note does not need to be long, but it must be explicit.

Examples of acceptable cross-surface phrasing:

- `Desktop: adds signed Minutes.app with notarized dmg`
- `Desktop (Windows): adds experimental NSIS installer as minutes-desktop-windows-x64-setup.exe and raw fallback binary`
- `CLI: no command changes in this release`
- `MCP: new quick-thought mode available through start_recording(mode=...)`

## Promotion rules

Promotion from preview to stable should happen only when:

- the release has been used successfully on at least one maintainer machine
- no silent-loss or install-blocking bugs are known
- the release notes are complete
- the rollback story is understood before the tag is pushed

Stable should be boring. Preview is where we learn.

## Rollback policy

### Stable rollback

If a stable release ships with a serious regression:

1. do not retag the broken release
2. keep the bad release visible for auditability
3. publish a new patch version with the fix, for example `v0.2.1`
4. call out the regression and fix explicitly in the next release note

If Homebrew or another distribution surface points at the bad release, update
that surface to the fixed patch rather than mutating history.

### Preview rollback

If a preview release is bad:

1. mark the GitHub Release as superseded in its notes
2. either ship the next preview quickly or stop promoting that preview line
3. do not promote a known-bad preview build to stable

Preview exists so bad ideas fail safely before stable users absorb them.

## Surface-specific expectations

Minutes releases should talk about all three surface categories even when only
one changed:

- desktop app
- CLI
- MCP / agent integrations

That keeps users from having to infer whether a release is “just UI” or also
changes the command/tool contract.

## What this policy does not do

This policy does not automate release notes. That belongs to the reproducible
release notes work. It only defines the minimum bar that every release note has
to meet.
