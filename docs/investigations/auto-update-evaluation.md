# Auto-Update Evaluation

This document evaluates whether Minutes should add in-app update support for the
desktop app, and if so, how to do it without violating the project’s trust
model.

## Summary

Recommendation:

- **Do not enable auto-update yet**
- **Choose Tauri Updater over Sparkle when we do**
- **Ship manual update as the default path until a full updater rollout meets the gates below**

Why:

- the project now has signed + notarized macOS releases
- the project now has stable vs preview release-channel rules
- the project now has reproducible release-note generation
- but it still does **not** have:
  - a hosted update manifest strategy
  - runtime UI for update disclosure and channel selection
  - user-tested rollback behavior

So the right answer today is not “never update in-app.” It is “defer the
feature until the trust surface is ready.”

## Evaluated options

### Option 1: Tauri Updater

Source:

- [Tauri updater plugin docs](https://v2.tauri.app/plugin/updater/)
- [tauri-plugin-updater crate](https://docs.rs/crate/tauri-plugin-updater/latest)

What it gives us:

- native fit with the existing Tauri desktop app
- required cryptographic signatures for update artifacts
- static JSON or dynamic server support
- runtime-configurable endpoints, including per-channel routing
- same conceptual model across more than one platform if Minutes expands later

Important constraints from the official docs:

- update signing cannot be disabled
- updater artifacts must be created during bundling
- production endpoints are expected to use TLS
- a static JSON file can be hosted from a simple CDN or GitHub Releases
- release channels can be implemented with runtime-selected endpoints

Minutes fit:

- strong fit with our current architecture
- strong fit with the “explicit, auditable, boring” release discipline we just added
- lets us keep one updater mental model if the desktop app ever expands beyond macOS

### Option 2: Sparkle

Source:

- [Sparkle documentation](https://sparkle-project.org/documentation/)

What it gives us:

- mature macOS-only update framework
- appcast-based distribution
- delta update support
- strong long-term track record in native macOS apps

Important constraints from the official docs:

- requires Sparkle-specific appcast infrastructure
- expects update archive hosting plus appcast publishing
- uses its own release feed model and signing flow
- is macOS-specific

Minutes fit:

- viable, but not ideal for this repo
- would introduce a second update architecture beside Tauri’s own ecosystem
- pushes us toward a more Mac-native-but-less-repo-native stack
- increases maintenance surface for an open-source project that already has a
  working Tauri release pipeline

## Recommendation

When Minutes adds in-app updates, prefer **Tauri Updater**.

Why Tauri Updater wins for Minutes:

1. it matches the existing desktop stack
2. it already supports signed artifacts and static manifests
3. it supports release-channel routing without introducing a second update
   framework
4. it keeps the updater story closer to the repo’s cross-surface discipline
   instead of introducing a macOS-only side system

Why we should still defer:

1. the project does not yet have a hosted update manifest path
2. users do not yet have a visible channel-selection or update-disclosure UI
3. rollback behavior has been defined at the release level, but not exercised as
   an updater flow
4. auto-update in an open-source app changes the trust contract more than a
   normal release pipeline does

## Required rollout gates

Minutes should not ship in-app updating until all of these are true:

### Gate 1: Signed releases are routine

Already mostly satisfied:

- signed + notarized macOS workflow exists

Still needed:

- at least one successful real release through the workflow
- one maintainer-confirmed install from a workflow-produced artifact

### Gate 2: Channel semantics are visible in-product

Needed:

- a simple update settings surface
- clear wording for:
  - stable
  - preview
  - manual updates
- no hidden enrollment into preview

### Gate 3: Manual fallback stays first-class

Needed:

- the app must always expose a “download manually from GitHub Releases” path
- users must be able to ignore or defer in-app updates without losing control

### Gate 4: Rollback path is tested

Needed:

- at least one dry-run of a preview rollback plan
- confirmation that a bad updater manifest can be superseded without confusing
  stable users

## Suggested rollout order

1. keep manual updates only
2. implement **check for updates** without automatic download or install
3. expose stable vs preview channels in a user-visible setting
4. add download-and-install only after the first three steps feel boring

This keeps the first updater iteration informational instead of intrusive.

## Recommended product stance

If we implement updates later, the product stance should be:

- updater is **optional**
- stable is the default
- preview is explicitly opt-in
- manual install remains documented and available
- the app explains where updates come from and what channel the user is on

That is much more aligned with an open-source, privacy-sensitive desktop app
than “silently download whatever the latest build is.”

## Decision

Current decision: **defer implementation**

Preferred future path: **Tauri Updater with manual-update fallback and
channel-aware UI**

Rejected for now: **Sparkle**, because it adds a second updater stack without a
compelling Minutes-specific advantage over Tauri’s built-in updater path.
